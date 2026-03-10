use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::litebrite;
use crate::logger::Logger;
use crate::reviewer;
use crate::run::{self, RunOptions};
use crate::shipper;
use crate::stream_fmt::StreamFormatter;
use crate::streaming::{self, StreamTarget};
use crate::summary;
use crate::tui::{TuiHandle, TuiSender};

pub struct LoopOptions {
    pub delay: u32,
    pub max_runs: u32,
    pub no_summary: bool,
    pub model: String,
}

/// Route a message to the TUI pane if available, otherwise stderr.
fn emit(tui_tx: &Option<TuiSender>, msg: &str) {
    match tui_tx {
        Some(sender) => sender.send_line(msg),
        None => eprintln!("{msg}"),
    }
}

pub fn execute(config: &Config, repo_root: &Path, opts: LoopOptions, tui: Option<&TuiHandle>) -> Result<(), LoopError> {
    let tui_tx = tui.map(|t| t.sender("LOOP"));

    // Cold-start: no git repo yet — init one and run in local (bind-mount) mode
    let bootstrap_mode = !repo_root.join(".git").exists();
    if bootstrap_mode {
        emit(&tui_tx, &make_banner("BOOTSTRAP"));
        emit(&tui_tx, &format!("No git repository found in {}. Running git init...", repo_root.display()));
        let status = Command::new("git")
            .arg("init")
            .current_dir(repo_root)
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to run git init: {e}")))?;
        if !status.success() {
            return Err(LoopError::Bootstrap("git init failed".into()));
        }
        let gitignore_path = repo_root.join(".gitignore");
        if !gitignore_path.exists() {
            let _ = std::fs::write(&gitignore_path, "logs/\n");
        } else if let Ok(contents) = std::fs::read_to_string(&gitignore_path) {
            if !contents.lines().any(|l| l.trim() == "logs/") {
                let mut updated = contents;
                if !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push_str("logs/\n");
                let _ = std::fs::write(&gitignore_path, updated);
            }
        }

        let add_status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_root)
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to stage files: {e}")))?;
        if add_status.success() {
            let has_staged = Command::new("git")
                .args(["diff", "--cached", "--quiet"])
                .current_dir(repo_root)
                .status()
                .map(|s| !s.success())
                .unwrap_or(false);
            if has_staged {
                let commit_status = Command::new("git")
                    .args(["commit", "-m", "Initial commit"])
                    .current_dir(repo_root)
                    .status()
                    .map_err(|e| LoopError::Bootstrap(format!("failed to commit initial files: {e}")))?;
                if !commit_status.success() {
                    return Err(LoopError::Bootstrap("initial commit failed".into()));
                }
            }
        }
    }

    // Capture parent branch before creating feature branch
    let parent_branch = git_current_branch(repo_root).unwrap_or_else(|_| "main".into());

    // Create feature branch (unless bootstrap mode — stay on main)
    let mut current_branch = if bootstrap_mode {
        parent_branch.clone()
    } else {
        emit(&tui_tx, &make_banner("BRANCH SETUP"));
        let branch_name = shipper::generate_branch_name(repo_root, &config.loop_config.shipper_model, None)
            .map_err(|e| LoopError::BranchCreation(format!("failed to generate branch name: {e}")))?;
        shipper::create_and_push_branch(repo_root, &branch_name, None)
            .map_err(|e| LoopError::BranchCreation(format!("failed to create branch: {e}")))?;
        branch_name
    };

    let max_label = if opts.max_runs == 0 {
        "unlimited".to_string()
    } else {
        opts.max_runs.to_string()
    };
    emit(&tui_tx, &format!("Agent loop: {}s between runs, max={}, Ctrl-C to stop", opts.delay, max_label));

    let mut run_number: u32 = 0;

    loop {
        run_number += 1;

        // Check if TUI user cancelled
        if tui.map_or(false, |t| t.is_cancelled()) {
            emit(&tui_tx, &make_banner("LOOP CANCELLED BY USER"));
            break;
        }

        if opts.max_runs > 0 && run_number > opts.max_runs {
            emit(&tui_tx, "");
            emit(&tui_tx, &make_banner(&format!("LOOP COMPLETE  {} runs", opts.max_runs)));
            break;
        }

        let run_opts = RunOptions {
            raw: false,
            model: opts.model.clone(),
            timeout: None,
            local: false,
            prompt_override: None,
            branch: Some(current_branch.clone()),
        };

        let head_before = git_head(repo_root);

        // run::execute prints its own ITERATION banner with branch + timestamp
        let run_result = run::execute(config, repo_root, run_opts, tui);
        let logger_opt: Option<Logger> = match run_result {
            Ok(logger) => Some(logger),
            Err(e) => {
                emit(&tui_tx, &format!("Run {run_number} failed: {e}"));
                None
            }
        };

        // Check if TUI user cancelled during the run
        if tui.map_or(false, |t| t.is_cancelled()) {
            emit(&tui_tx, &make_banner("LOOP CANCELLED BY USER"));
            break;
        }

        // Sync litebrite so reviewer and decider see fresh task state
        litebrite::sync(repo_root);

        // Only run reviewer if the agent actually committed something
        let head_after = git_head(repo_root);
        let agent_made_commits = head_before.is_ok()
            && head_after.is_ok()
            && head_before.unwrap() != head_after.unwrap();

        if agent_made_commits {
            let reviewer_logger = logger_opt.as_ref().map(|l| l.with_label("CODE REVIEW"));
            let reviewer_opts = reviewer::ReviewerOptions {
                model: config.loop_config.reviewer_model.clone(),
                current_branch: current_branch.clone(),
            };
            if let Err(e) = reviewer::execute(repo_root, &reviewer_opts, reviewer_logger.as_ref()) {
                crate::logger::log(logger_opt.as_ref(), &format!("Reviewer failed (non-fatal): {e}"));
            }
        } else {
            crate::logger::log(logger_opt.as_ref(), "Reviewer skipped: no new commits from this run.");
        }

        // Run summary and decider in parallel — they're independent
        let decider_model = config.loop_config.decider_model.clone();
        let decision = std::thread::scope(|s| {
            if !opts.no_summary {
                let log_file = format!("{}/latest.jsonl", config.log_dir);
                let summary_logger = logger_opt.as_ref().map(|l| l.with_label("SUMMARY"));
                s.spawn(move || {
                    if let Err(e) = summary::execute(config, repo_root, &log_file, summary_logger.as_ref()) {
                        crate::logger::log(summary_logger.as_ref(), &format!("Summary generation failed: {e}"));
                    }
                });
            }

            let decider_logger = logger_opt.as_ref().map(|l| l.with_label("DECISION"));
            let decider_handle = s.spawn(move || {
                should_continue(repo_root, &decider_model, decider_logger.as_ref())
            });

            decider_handle.join().expect("decider thread panicked")
        });

        match decision {
            Ok(Decision::Continue(reason)) => {
                crate::logger::log(logger_opt.as_ref(), &format!("Decider: continue — {reason}"));
            }
            Ok(Decision::Ship(reason)) => {
                crate::logger::log(logger_opt.as_ref(), &format!("Decider: ship — {reason}"));

                let ship_opts = shipper::ShipperOptions {
                    model: config.loop_config.shipper_model.clone(),
                    current_branch: current_branch.clone(),
                    parent_branch: parent_branch.clone(),
                };

                match shipper::execute(repo_root, &ship_opts, logger_opt.as_ref()) {
                    Ok(result) => {
                        crate::logger::log(logger_opt.as_ref(), &format!("Shipped! New branch: {}", result.new_branch));
                        current_branch = result.new_branch;
                    }
                    Err(e) => {
                        crate::logger::log(logger_opt.as_ref(), &format!("Ship failed (continuing on current branch): {e}"));
                    }
                }
            }
            Ok(Decision::Stop(reason)) => {
                crate::logger::log(logger_opt.as_ref(), &format!("Decider: stop — {reason}"));
                emit(&tui_tx, &make_banner(&format!("LOOP COMPLETE  {} runs", run_number)));
                break;
            }
            Err(e) => {
                crate::logger::log(logger_opt.as_ref(), &format!("Decider error (continuing anyway): {e}"));
            }
        }

        // Check if TUI user cancelled before sleeping
        if tui.map_or(false, |t| t.is_cancelled()) {
            emit(&tui_tx, &make_banner("LOOP CANCELLED BY USER"));
            break;
        }

        if opts.delay > 0 {
            emit(&tui_tx, &format!("Waiting {}s until next run...", opts.delay));
            std::thread::sleep(std::time::Duration::from_secs(opts.delay as u64));
        }
    }

    Ok(())
}

fn make_banner(label: &str) -> String {
    const WIDTH: usize = 80;
    let border = "#".repeat(WIDTH);
    let empty = format!("##{}##", " ".repeat(WIDTH - 4));
    let text = format!("##  {:<width$}##", label, width = WIDTH - 6);
    format!("{border}\n{empty}\n{text}\n{empty}\n{border}")
}

enum Decision {
    Continue(String),
    Ship(String),
    Stop(String),
}

fn should_continue(repo_root: &Path, decider_model: &str, logger: Option<&Logger>) -> Result<Decision, LoopError> {
    crate::logger::banner(logger, "DECISION");

    let schema = r#"{"type":"object","properties":{"action":{"type":"string","enum":["continue","ship","stop"],"description":"continue = keep working, ship = merge current branch and start new one, stop = all done"},"reason":{"type":"string","description":"Brief explanation of the decision"}},"required":["action","reason"]}"#;

    let prompt = "You are deciding what an AI agent loop should do next. \
        The project is specified in SPEC.md. You can see in the lites what has been done \
        and what remains to do, and you can compare this to the SPEC.md (which may have changed) \
        in order to make your decision.\n\n\
        Actions:\n\
        - \"continue\": there is more work to do on the current feature branch\n\
        - \"ship\": the current batch of work is complete and ready to merge; start a new branch for remaining work\n\
        - \"stop\": all work is done, no more runs needed";

    let mut cmd = streaming::claude_stream_cmd_with_schema(
        repo_root,
        decider_model,
        "Read,Bash(git *),Bash(lb *)",
        schema,
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| LoopError::Decider(format!("failed to run claude CLI: {e}")))?;

    streaming::send_prompt(&mut child, prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.with_label("DECISION")),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let (result_text, exit_code) = streaming::run_streaming_claude(child, &mut formatter, logger, &target)
        .map_err(|e| LoopError::Decider(format!("streaming error: {e}")))?;

    if exit_code != 0 {
        return Err(LoopError::Decider(format!(
            "claude CLI exited with code {exit_code}"
        )));
    }

    // Parse the structured result from the stream-json result event
    let parsed: serde_json::Value = serde_json::from_str(&result_text)
        .unwrap_or_default();
    let action = parsed["action"].as_str().unwrap_or("continue");
    let reason = parsed["reason"].as_str().unwrap_or("no reason given").to_string();

    match action {
        "ship" => Ok(Decision::Ship(reason)),
        "stop" => Ok(Decision::Stop(reason)),
        _ => Ok(Decision::Continue(reason)),
    }
}

fn git_head(repo_root: &Path) -> Result<String, ()> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .map_err(|_| ())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(())
    }
}

fn git_current_branch(repo_root: &Path) -> Result<String, LoopError> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "branch", "--show-current"])
        .output()
        .map_err(|e| LoopError::BranchCreation(format!("failed to get current branch: {e}")))?;

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        Ok("main".into())
    } else {
        Ok(branch)
    }
}

#[derive(Debug)]
pub enum LoopError {
    Bootstrap(String),
    Decider(String),
    BranchCreation(String),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bootstrap(msg) => write!(f, "bootstrap error: {msg}"),
            Self::Decider(msg) => write!(f, "decider error: {msg}"),
            Self::BranchCreation(msg) => write!(f, "branch creation error: {msg}"),
        }
    }
}

impl std::error::Error for LoopError {}
