use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::litebrite;
use crate::reviewer;
use crate::run::{self, RunOptions};
use crate::shipper;
use crate::summary;

pub struct LoopOptions {
    pub delay: u32,
    pub max_runs: u32,
    pub no_summary: bool,
    pub model: String,
}

pub fn execute(config: &Config, repo_root: &Path, opts: LoopOptions) -> Result<(), LoopError> {
    // Cold-start: no git repo yet — init one and run in local (bind-mount) mode
    let bootstrap_mode = !repo_root.join(".git").exists();
    if bootstrap_mode {
        eprintln!("No git repository found in {}. Running git init...", repo_root.display());
        let status = Command::new("git")
            .arg("init")
            .current_dir(repo_root)
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to run git init: {e}")))?;
        if !status.success() {
            return Err(LoopError::Bootstrap("git init failed".into()));
        }
        // Commit any existing files (e.g. SPEC.md) so the repo has at least one commit
        // and the file:// remote clone inside the container can succeed.
        let add_status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_root)
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to stage files: {e}")))?;
        if add_status.success() {
            // Only commit if there's something staged
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
        let branch_name = shipper::generate_branch_name(repo_root, &config.loop_config.shipper_model)
            .map_err(|e| LoopError::BranchCreation(format!("failed to generate branch name: {e}")))?;
        shipper::create_and_push_branch(repo_root, &branch_name)
            .map_err(|e| LoopError::BranchCreation(format!("failed to create branch: {e}")))?;
        branch_name
    };

    let max_label = if opts.max_runs == 0 {
        "unlimited".to_string()
    } else {
        opts.max_runs.to_string()
    };
    eprintln!("=== Agent loop ({}s between runs, max={}, Ctrl-C to stop) ===", opts.delay, max_label);

    let mut run_number: u32 = 0;

    loop {
        run_number += 1;

        if opts.max_runs > 0 && run_number > opts.max_runs {
            eprintln!();
            eprintln!("=== Loop complete: reached max runs ({}) ===", opts.max_runs);
            break;
        }

        eprintln!();
        eprintln!("--- Run {run_number} starting at {} ---", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));

        let run_opts = RunOptions {
            raw: false,
            model: opts.model.clone(),
            timeout: None,
            local: false,
            prompt_override: None,
            branch: Some(current_branch.clone()),
        };

        let head_before = git_head(repo_root);

        let run_result = run::execute(config, repo_root, run_opts);
        if let Err(e) = &run_result {
            eprintln!("Run {run_number} failed: {e}");
        }

        // Sync litebrite so reviewer and decider see fresh task state
        litebrite::sync(repo_root);

        // Only run reviewer if the agent actually committed something
        let head_after = git_head(repo_root);
        let agent_made_commits = head_before.is_ok()
            && head_after.is_ok()
            && head_before.unwrap() != head_after.unwrap();
        if agent_made_commits {
            let reviewer_opts = reviewer::ReviewerOptions {
                model: config.loop_config.reviewer_model.clone(),
                current_branch: current_branch.clone(),
            };
            if let Err(e) = reviewer::execute(repo_root, &reviewer_opts) {
                eprintln!("Reviewer failed (non-fatal): {e}");
            }
        } else {
            eprintln!("Reviewer skipped: no new commits from this run.");
        }

        // Run summary and decider in parallel — they're independent
        let decider_model = config.loop_config.decider_model.clone();
        let decision = std::thread::scope(|s| {
            if !opts.no_summary {
                let log_file = format!("{}/latest.jsonl", config.log_dir);
                s.spawn(move || {
                    if let Err(e) = summary::execute(config, repo_root, &log_file) {
                        eprintln!("Summary generation failed: {e}");
                    }
                });
            }

            let decider_handle = s.spawn(|| {
                should_continue(repo_root, &decider_model)
            });

            decider_handle.join().expect("decider thread panicked")
        });

        match decision {
            Ok(Decision::Continue(reason)) => {
                eprintln!("Decider: {reason}");
            }
            Ok(Decision::Ship(reason)) => {
                eprintln!("Decider (ship): {reason}");

                let ship_opts = shipper::ShipperOptions {
                    model: config.loop_config.shipper_model.clone(),
                    current_branch: current_branch.clone(),
                    parent_branch: parent_branch.clone(),
                };

                match shipper::execute(repo_root, &ship_opts) {
                    Ok(result) => {
                        eprintln!("Shipped! New branch: {}", result.new_branch);
                        current_branch = result.new_branch;
                    }
                    Err(e) => {
                        eprintln!("Ship failed (continuing on current branch): {e}");
                    }
                }
            }
            Ok(Decision::Stop(reason)) => {
                eprintln!("Decider: {reason}");
                eprintln!();
                eprintln!("=== Loop complete after {run_number} runs ===");
                break;
            }
            Err(e) => {
                eprintln!("Decider error (continuing anyway): {e}");
            }
        }

        if opts.delay > 0 {
            eprintln!();
            eprintln!("--- Waiting {}s until next run ---", opts.delay);
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

fn should_continue(repo_root: &Path, decider_model: &str) -> Result<Decision, LoopError> {
    let schema = r#"{"type":"object","properties":{"action":{"type":"string","enum":["continue","ship","stop"],"description":"continue = keep working, ship = merge current branch and start new one, stop = all done"},"reason":{"type":"string","description":"Brief explanation of the decision"}},"required":["action","reason"]}"#;

    let prompt = "You are deciding what an AI agent loop should do next. \
        The project is specified in SPEC.md. You can see in the lites what has been done \
        and what remains to do, and you can compare this to the SPEC.md (which may have changed) \
        in order to make your decision.\n\n\
        Actions:\n\
        - \"continue\": there is more work to do on the current feature branch\n\
        - \"ship\": the current batch of work is complete and ready to merge; start a new branch for remaining work\n\
        - \"stop\": all work is done, no more runs needed";

    let mut child = Command::new("claude")
        .args([
            "-p",
            "--no-session-persistence",
            "--model", decider_model,
            "--allowedTools", "Read,Bash(git *),Bash(lb *)",
            "--output-format", "json",
            "--json-schema", schema,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .current_dir(repo_root)
        .spawn()
        .map_err(|e| LoopError::Decider(format!("failed to run claude CLI: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| LoopError::Decider(format!("failed to wait for claude CLI: {e}")))?;

    if !output.status.success() {
        return Err(LoopError::Decider(format!(
            "claude CLI exited with code {}",
            output.status.code().unwrap_or(-1)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse the JSON output to extract structured_output
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| LoopError::Decider(format!("failed to parse decider output: {e}")))?;

    let structured = &parsed["structured_output"];
    let action = structured["action"].as_str().unwrap_or("continue");
    let reason = structured["reason"].as_str().unwrap_or("no reason given").to_string();

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
