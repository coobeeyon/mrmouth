use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::do_cmd::{git_checkout_new_branch, sync_and_push};
use crate::litebrite;
use crate::prompt;
use crate::reviewer;
use crate::run::{self, RunOptions};
use crate::tui::{TuiHandle, TuiSender};

/// Route a message to the TUI pane if available, otherwise stderr.
fn emit(tui_tx: &Option<TuiSender>, msg: &str) {
    match tui_tx {
        Some(sender) => sender.send_line(msg),
        None => eprintln!("{msg}"),
    }
}

pub struct ReadyOptions {
    pub timeout: u32,
    pub max_failures: u32,
    pub model: String,
}

#[derive(Debug)]
pub enum ReadyError {
    Command(String),
    /// Aborted after N consecutive task failures. Carries the last failure's
    /// log path + exit code so the debrief can tail it.
    TooManyFailures {
        count: u32,
        last_reason: String,
        last_log: Option<PathBuf>,
        last_exit: Option<i32>,
    },
}

impl std::fmt::Display for ReadyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(msg) => write!(f, "{msg}"),
            Self::TooManyFailures { count, .. } => {
                write!(f, "aborted after {count} consecutive failures")
            }
        }
    }
}

impl std::error::Error for ReadyError {}

impl ReadyError {
    /// Build a debrief for printing after TUI teardown. TooManyFailures picks up
    /// the last failure's reason, log path, and exit code.
    pub fn debrief(&self) -> crate::debrief::FailureDebrief {
        match self {
            Self::TooManyFailures { count, last_reason, last_log, last_exit } => {
                let message = format!(
                    "aborted after {count} consecutive failures; last attempt: {last_reason}"
                );
                let mut d = crate::debrief::FailureDebrief::new(message);
                d.exit_code = *last_exit;
                d.log_path = last_log.clone();
                d
            }
            Self::Command(msg) => crate::debrief::FailureDebrief::new(msg.clone()),
        }
    }
}

/// Run `lb ready` and return the first item ID, or None if no items are ready.
fn lb_ready(repo_root: &Path) -> Option<String> {
    let output = Command::new("lb")
        .args(["ready"])
        .current_dir(repo_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse first non-header, non-separator line and extract the ID (first column)
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("ID") || trimmed.starts_with("---") {
            continue;
        }
        // ID is the first whitespace-delimited token
        if let Some(id) = trimmed.split_whitespace().next() {
            return Some(id.to_string());
        }
    }

    None
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

pub fn execute(config: &Config, repo_root: &Path, opts: ReadyOptions, tui: Option<&TuiHandle>) -> Result<(), ReadyError> {
    let tui_tx = tui.map(|t| t.sender("AGENT SESSION"));

    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let logger = match tui {
        Some(t) => crate::logger::Logger::with_tui(&log_dir.join("ready.log"), t.sender("AGENT SESSION")),
        None => crate::logger::Logger::new(&log_dir.join("ready.log")),
    }.ok();

    // 1. Create feature branch ready-{timestamp}
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let branch_name = format!("ready-{timestamp}");
    emit(&tui_tx, &format!("Creating branch: {branch_name}"));
    git_checkout_new_branch(repo_root, &branch_name)
        .map_err(|e| ReadyError::Command(format!("failed to create branch: {e}")))?;

    // 2. Loop: pick items from lb ready
    let mut consecutive_failures: u32 = 0;
    let mut task_num: u32 = 0;

    loop {
        if tui.is_some_and(|t| t.is_cancelled()) {
            emit(&tui_tx, "Ready loop cancelled by user.");
            break;
        }

        let item_id = match lb_ready(repo_root) {
            Some(id) => id,
            None => {
                emit(&tui_tx, "No ready items found. Done.");
                break;
            }
        };

        task_num += 1;
        if let Some(t) = tui { t.set_run(Some(format!("Task {task_num}"))); }
        emit(&tui_tx, &format!(
            "TASK {task_num}: {item_id}  {}",
            chrono::Local::now().format("%H:%M:%S")
        ));

        let head_before = git_head(repo_root);

        let base_prompt = prompt::load_prompt(repo_root, logger.as_ref());
        let prompt = format!(
            "## Scope\n\n\
            Work on task {item_id}. Run `lb show {item_id}` to understand the task, then `lb claim {item_id}`. \
            Implement the changes, commit your code, then `lb close {item_id}`, `lb sync`, and `git push`.\n\n\
            {base_prompt}"
        );

        let run_opts = RunOptions {
            raw: false,
            model: opts.model.clone(),
            timeout: Some(opts.timeout),
            local: false,
            prompt_override: Some(prompt),
            branch: None,
            event_sink: None,
        };

        let run_result = run::execute(config, repo_root, run_opts, tui);

        match run_result {
            Ok(ref run_logger) => {
                consecutive_failures = 0;
                emit(&tui_tx, &format!("Task {task_num} ({item_id}) succeeded, syncing..."));
                sync_and_push(repo_root, &branch_name, Some(run_logger));
            }
            Err(e) => {
                consecutive_failures += 1;
                emit(&tui_tx, &format!("Task {task_num} ({item_id}) failed: {e}"));

                if consecutive_failures >= opts.max_failures {
                    emit(&tui_tx, &format!(
                        "ERROR: {} consecutive failures — aborting",
                        opts.max_failures
                    ));
                    return Err(ReadyError::TooManyFailures {
                        count: opts.max_failures,
                        last_reason: e.short_reason(),
                        last_log: e.log_path().map(|p| p.to_path_buf()),
                        last_exit: e.exit_code(),
                    });
                }
                continue;
            }
        }

        // Run reviewer if new commits were made
        let head_after = git_head(repo_root);
        let commit_range = match (&head_before, &head_after) {
            (Ok(before), Ok(after)) if before != after => Some((before.clone(), after.clone())),
            _ => None,
        };

        if commit_range.is_some() {
            emit(&tui_tx, "Running reviewer...");
            if let Some(t) = tui { t.set_stage("Reviewer"); }
            let reviewer_opts = reviewer::ReviewerOptions {
                model: config.loop_config.reviewer_model.clone(),
                current_branch: branch_name.clone(),
                commit_range,
            };
            if let Err(e) = reviewer::execute(config, repo_root, &reviewer_opts, logger.as_ref()) {
                emit(&tui_tx, &format!("Reviewer failed (non-fatal): {e}"));
            }
            litebrite::sync(repo_root, logger.as_ref());
        }

        sync_and_push(repo_root, &branch_name, logger.as_ref());
    }

    // 3. Final sync
    emit(&tui_tx, "READY COMPLETE — final sync...");
    sync_and_push(repo_root, &branch_name, logger.as_ref());
    emit(&tui_tx, &format!("Done. Merge branch '{branch_name}' when ready."));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_many_failures_debrief_carries_last_log_and_exit() {
        let err = ReadyError::TooManyFailures {
            count: 3,
            last_reason: "exit 127 — missing tool 'trk'".into(),
            last_log: Some(PathBuf::from("/logs/run-z.log")),
            last_exit: Some(127),
        };
        let d = err.debrief();
        assert!(d.message.contains("aborted after 3"), "got: {}", d.message);
        assert!(d.message.contains("missing tool 'trk'"), "got: {}", d.message);
        assert_eq!(d.exit_code, Some(127));
        assert_eq!(d.log_path.as_deref(), Some(Path::new("/logs/run-z.log")));
    }

    #[test]
    fn too_many_failures_debrief_without_log() {
        let err = ReadyError::TooManyFailures {
            count: 2,
            last_reason: "preflight check failed: Docker is not available".into(),
            last_log: None,
            last_exit: None,
        };
        let d = err.debrief();
        assert!(d.message.contains("Docker is not available"), "got: {}", d.message);
        assert!(d.log_path.is_none());
        assert!(d.exit_code.is_none());
    }

    #[test]
    fn command_debrief_is_plain_message() {
        let err = ReadyError::Command("failed to create branch: boom".into());
        let d = err.debrief();
        assert_eq!(d.message, "failed to create branch: boom");
        assert!(d.log_path.is_none());
        assert!(d.exit_code.is_none());
    }
}
