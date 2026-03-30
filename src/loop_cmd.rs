use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::{Command, Stdio};

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
    // Use the same TUI pane name as run::execute so all output is visible
    // in one place — the user watches "AGENT SESSION" during runs and should
    // see post-run activity (reviewer, decider, etc.) on the same pane.
    let tui_tx = tui.map(|t| t.sender("AGENT SESSION"));

    // Create a loop-level logger so that sub-calls (shipper, reviewer, decider)
    // route output through the TUI instead of falling back to eprintln.
    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let loop_logger = match tui {
        Some(t) => Logger::with_tui(&log_dir.join("loop.log"), t.sender("AGENT SESSION")),
        None => Logger::new(&log_dir.join("loop.log")),
    }.ok();

    // Cold-start: no git repo yet — init one and run in local (bind-mount) mode
    let bootstrap_mode = !repo_root.join(".git").exists();
    if bootstrap_mode {
        emit(&tui_tx, "BOOTSTRAP");
        emit(&tui_tx, &format!("No git repository found in {}. Running git init...", repo_root.display()));
        let init_output = Command::new("git")
            .arg("init")
            .current_dir(repo_root)
            .output()
            .map_err(|e| LoopError::Bootstrap(format!("failed to run git init: {e}")))?;
        if !init_output.status.success() {
            let stderr = String::from_utf8_lossy(&init_output.stderr);
            emit(&tui_tx, &format!("git init failed: {}", stderr.trim()));
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

        // Seed default Dockerfile so the decider has a base to add layers to
        let dockerfile_path = repo_root.join(&config.dockerfile);
        if !dockerfile_path.exists() {
            if let Some(parent) = dockerfile_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dockerfile_path, crate::docker::DEFAULT_DOCKERFILE);
        }

        let add_status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to stage files: {e}")))?;
        if add_status.success() {
            let has_staged = Command::new("git")
                .args(["diff", "--cached", "--quiet"])
                .current_dir(repo_root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| !s.success())
                .unwrap_or(false);
            if has_staged {
                let commit_output = Command::new("git")
                    .args(["commit", "-m", "Initial commit"])
                    .current_dir(repo_root)
                    .output()
                    .map_err(|e| LoopError::Bootstrap(format!("failed to commit initial files: {e}")))?;
                if !commit_output.status.success() {
                    let stderr = String::from_utf8_lossy(&commit_output.stderr);
                    emit(&tui_tx, &format!("Initial commit failed: {}", stderr.trim()));
                    return Err(LoopError::Bootstrap("initial commit failed".into()));
                }
            }
        }
    }

    // Pre-initialized repo with zero commits: seed one so branches can be
    // created and cloned.  This covers the case where the user ran `git init`
    // (and possibly `lb init`) before invoking mrmouth.
    let has_commits = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(repo_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_commits {
        emit(&tui_tx, "No commits found — creating seed commit");
        let gitignore_path = repo_root.join(".gitignore");
        if !gitignore_path.exists() {
            let _ = std::fs::write(&gitignore_path, "logs/\n");
        }
        let dockerfile_path = repo_root.join(&config.dockerfile);
        if !dockerfile_path.exists() {
            if let Some(parent) = dockerfile_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dockerfile_path, crate::docker::DEFAULT_DOCKERFILE);
        }
        let _ = Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let seed_status = Command::new("git")
            .args(["commit", "--allow-empty", "-m", "Initial commit"])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to create seed commit: {e}")))?;
        if !seed_status.success() {
            return Err(LoopError::Bootstrap("seed commit failed".into()));
        }
    }

    // Capture parent branch before creating feature branch
    let parent_branch = git_current_branch(repo_root).unwrap_or_else(|_| "main".into());

    // Create feature branch (unless bootstrap mode — stay on main)
    let mut current_branch = if bootstrap_mode {
        parent_branch.clone()
    } else {
        emit(&tui_tx, "BRANCH SETUP");
        let branch_name = shipper::generate_branch_name(repo_root, &config.loop_config.shipper_model, loop_logger.as_ref())
            .map_err(|e| LoopError::BranchCreation(format!("failed to generate branch name: {e}")))?;
        shipper::create_and_push_branch(repo_root, &branch_name, loop_logger.as_ref())
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
        if tui.is_some_and(|t| t.is_cancelled()) {
            emit(&tui_tx, "LOOP CANCELLED BY USER");
            break;
        }

        if opts.max_runs > 0 && run_number > opts.max_runs {
            emit(&tui_tx, "");
            emit(&tui_tx, &format!("LOOP COMPLETE  {} runs", opts.max_runs));
            break;
        }

        // --- Decider (runs first; uses loop_logger since no run logger exists yet) ---
        if let Some(t) = tui { t.set_stage("Deciding"); }
        let decider_model = config.loop_config.decider_model.clone();
        let decision = should_continue(repo_root, &decider_model, loop_logger.as_ref(), &log_dir);

        match decision {
            Ok(Decision::Continue(reason)) => {
                crate::logger::log(loop_logger.as_ref(), &format!("Decider: continue — {reason}"));
            }
            Ok(Decision::Ship(reason)) => {
                crate::logger::log(loop_logger.as_ref(), &format!("Decider: ship — {reason}"));

                if let Some(t) = tui { t.set_stage("Shipper"); }
                let ship_opts = shipper::ShipperOptions {
                    model: config.loop_config.shipper_model.clone(),
                    current_branch: current_branch.clone(),
                    parent_branch: parent_branch.clone(),
                };

                match shipper::execute(config, repo_root, &ship_opts, loop_logger.as_ref()) {
                    Ok(result) => {
                        crate::logger::log(loop_logger.as_ref(), &format!("Shipped! New branch: {}", result.new_branch));
                        current_branch = result.new_branch;
                        continue;
                    }
                    Err(e) => {
                        crate::logger::log(loop_logger.as_ref(), &format!("Ship failed (continuing on current branch): {e}"));
                    }
                }
            }
            Ok(Decision::Stop(reason)) => {
                crate::logger::log(loop_logger.as_ref(), &format!("Decider: stop — {reason}"));
                let completed = run_number - 1;
                emit(&tui_tx, &format!("LOOP COMPLETE  {completed} runs"));
                break;
            }
            Err(e) => {
                crate::logger::log(loop_logger.as_ref(), &format!("Decider error (continuing anyway): {e}"));
            }
        }

        // Check if TUI user cancelled after decider
        if tui.is_some_and(|t| t.is_cancelled()) {
            emit(&tui_tx, "LOOP CANCELLED BY USER");
            break;
        }

        // --- Runner ---
        let run_opts = RunOptions {
            raw: false,
            model: opts.model.clone(),
            timeout: None,
            local: false,
            prompt_override: None,
            branch: Some(current_branch.clone()),
        };

        let head_before = git_head(repo_root);

        if let Some(t) = tui { t.set_run(Some(format!("Run {run_number}"))); }
        let run_result = run::execute(config, repo_root, run_opts, tui);
        let run_logger: Option<Logger> = match run_result {
            Ok(logger) => Some(logger),
            Err(e) => {
                emit(&tui_tx, &format!("Run {run_number} failed: {e}"));
                None
            }
        };
        // Use the run's logger if available, otherwise fall back to the loop logger.
        let logger_opt: Option<&Logger> = run_logger.as_ref().or(loop_logger.as_ref());

        // Check if TUI user cancelled during the run
        if tui.is_some_and(|t| t.is_cancelled()) {
            emit(&tui_tx, "LOOP CANCELLED BY USER");
            break;
        }

        // Sync litebrite so reviewer sees fresh task state
        litebrite::sync(repo_root, logger_opt);

        // --- Reviewer (only if the agent actually committed something) ---
        let head_after = git_head(repo_root);
        let commit_range = match (&head_before, &head_after) {
            (Ok(before), Ok(after)) if before != after => Some((before.clone(), after.clone())),
            _ => None,
        };

        if commit_range.is_some() {
            if let Some(t) = tui { t.set_stage("Reviewer"); }
            let reviewer_opts = reviewer::ReviewerOptions {
                model: config.loop_config.reviewer_model.clone(),
                current_branch: current_branch.clone(),
                commit_range,
            };
            if let Err(e) = reviewer::execute(config, repo_root, &reviewer_opts, logger_opt) {
                crate::logger::log(logger_opt, &format!("Reviewer failed (non-fatal): {e}"));
            }
            // Sync lb state pushed by reviewer container back to host
            litebrite::sync(repo_root, logger_opt);
        } else {
            crate::logger::log(logger_opt, "Reviewer skipped: no new commits from this run.");
        }

        // --- Summary (runs after reviewer) ---
        if !opts.no_summary {
            let log_file = format!("{}/latest.jsonl", config.log_dir);
            if let Err(e) = summary::execute(config, repo_root, &log_file, logger_opt) {
                crate::logger::log(logger_opt, &format!("Summary generation failed: {e}"));
            }
        }

        // Check if TUI user cancelled before sleeping
        if tui.is_some_and(|t| t.is_cancelled()) {
            emit(&tui_tx, "LOOP CANCELLED BY USER");
            break;
        }

        if opts.delay > 0 {
            emit(&tui_tx, &format!("Waiting {}s until next run...", opts.delay));
            std::thread::sleep(std::time::Duration::from_secs(opts.delay as u64));
        }
    }

    Ok(())
}

enum Decision {
    Continue(String),
    Ship(String),
    Stop(String),
}

fn should_continue(repo_root: &Path, decider_model: &str, logger: Option<&Logger>, log_dir: &Path) -> Result<Decision, LoopError> {
    crate::logger::log(logger, "DECISION");

    // Create dedicated decider log + jsonl files
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let decider_log_path = log_dir.join(format!("decider-{timestamp}.log"));
    let decider_jsonl_path = log_dir.join(format!("decider-{timestamp}.jsonl"));

    let decider_logger = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => Logger::with_tui(&decider_log_path, tui.clone()),
        None => Logger::new(&decider_log_path),
    }.ok();

    let mut jsonl_writer: Option<BufWriter<File>> = File::create(&decider_jsonl_path)
        .ok()
        .map(BufWriter::new);

    let schema = r#"{"type":"object","properties":{"action":{"type":"string","enum":["continue","ship","stop"],"description":"continue = more work to do, ship = all done and ready to merge, stop = nothing to merge"},"reason":{"type":"string","description":"Brief explanation of the decision"}},"required":["action","reason"]}"#;

    let prompt = format!("## System\n\n{}\n\n\
        You are the **Decider**. Your job is to assess project state and return a decision.\n\n\
        ## Boundary\n\n\
        You do NOT implement features, claim tasks, or make code changes. \
        You MAY create litebrite items, edit the Dockerfile, and read any file.\n\n\
        ## Instructions\n\n\
        1. Run `lb list` to check for open litebrite items.\n\
        2. Check whether the open items include **leaf tasks** (type=task with no children) that the runner can implement.\n\
           - Run `lb list --tree` to see the hierarchy.\n\
           - If leaf tasks exist, return **continue**.\n\
           - If the only open items are **epics or features with no child tasks**, decompose them: \
             create concrete child tasks with `lb create \"<title>\" -t task --parent <epic-id> -d \"<description>\"`. \
             Also read `.mrmouth/Dockerfile` and SPEC.md — if the spec requires a toolchain \
             (e.g. Rust, Go, Python) that is not installed in the Dockerfile, add the necessary \
             `RUN` commands **before** the `USER runner` line so the runner has a working compiler/interpreter. \
             Then return **continue**.\n\
        3. If NO open items exist, read SPEC.md and compare it against the current implementation.\n\
           - If there are deficiencies or missing features, create litebrite tasks for them \
             (and optionally edit `.mrmouth/Dockerfile` if tooling changes are needed), then return **continue**.\n\
           - If the implementation fully satisfies the spec, check if the current branch has commits \
             ahead of main: `git rev-list --count HEAD --not main`. If > 0, return **ship**. \
             If 0, return **stop** (nothing to merge).\n\n\
        **Important:** If you edit any files (e.g. `.mrmouth/Dockerfile`), you MUST commit and push \
        before returning your decision: `git add -A && git commit -m \"<message>\" && git push`.\n\n\
        **Ship** means: all litebrite items are closed and the implementation matches the spec. \
        It merges the current branch and stops.\n\n\
        Actions:\n\
        - \"continue\": there is work remaining for the runner\n\
        - \"ship\": all work is complete — merge the branch\n\
        - \"stop\": nothing was done, nothing to merge",
        crate::prompt::SYSTEM_PREAMBLE);

    let mut cmd = streaming::claude_stream_cmd_with_schema(
        repo_root,
        decider_model,
        "Read,Edit,Write,Bash(git *),Bash(lb *)",
        schema,
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| LoopError::Decider(format!("failed to run claude CLI: {e}")))?;

    streaming::send_prompt(&mut child, &prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.clone()),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let effective_logger = decider_logger.as_ref().or(logger);
    let (result_text, exit_code) = streaming::run_streaming_claude(child, &mut formatter, effective_logger, &target, &mut jsonl_writer)
        .map_err(|e| LoopError::Decider(format!("streaming error: {e}")))?;

    if exit_code != 0 {
        return Err(LoopError::Decider(format!(
            "claude CLI exited with code {exit_code}"
        )));
    }

    // Parse the structured result from the stream-json result event
    let parsed: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log(effective_logger, &format!("WARNING: decider returned invalid JSON (defaulting to 'continue'): {e}"));
            crate::logger::log(effective_logger, &format!("  raw output: {result_text}"));
            return Ok(Decision::Continue("JSON parse failure — defaulting to continue".into()));
        }
    };
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
