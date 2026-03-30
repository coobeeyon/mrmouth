use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::litebrite;
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

pub struct ItemInfo {
    pub title: String,
    pub item_type: String,
}

impl std::fmt::Display for ItemInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.item_type, self.title)
    }
}

pub struct DoOptions {
    pub item_id: String,
    pub timeout: u32,
    pub max_failures: u32,
    pub model: String,
}

pub fn execute(config: &Config, repo_root: &Path, opts: DoOptions, tui: Option<&TuiHandle>) -> Result<(), DoError> {
    let tui_tx = tui.map(|t| t.sender("AGENT SESSION"));

    // Create an epic-level logger so litebrite/subprocess output routes through
    // the TUI instead of falling back to eprintln.
    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let epic_logger = match tui {
        Some(t) => crate::logger::Logger::with_tui(&log_dir.join("epic.log"), t.sender("AGENT SESSION")),
        None => crate::logger::Logger::new(&log_dir.join("epic.log")),
    }.ok();

    // 1. Verify the item exists and determine its type
    let item_info = lb_show(repo_root, &opts.item_id)?;
    emit(&tui_tx, &format!("DO: {item_info}"));

    // 2. Create feature branch (if not already on one)
    let current_branch = git_current_branch(repo_root)?;
    let feature_branch = if current_branch == "main" || current_branch == "master" {
        let slug = make_slug(&item_info.title);
        let branch_name = format!("{}-{}", opts.item_id, slug);
        emit(&tui_tx, &format!("Creating feature branch: {branch_name}"));
        git_checkout_new_branch(repo_root, &branch_name)?;
        // Push the new branch so container can clone it
        emit(&tui_tx, "Pushing branch to remote...");
        let push_output = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "push", "-u", "origin", &branch_name])
            .output();
        match push_output {
            Ok(o) if o.status.success() => {}
            Ok(o) => emit(&tui_tx, &format!("WARNING: git push exited with code {} — container may fail to clone this branch", o.status.code().unwrap_or(-1))),
            Err(e) => emit(&tui_tx, &format!("WARNING: git push failed: {e} — container may fail to clone this branch")),
        }
        branch_name
    } else {
        emit(&tui_tx, &format!("Already on branch: {current_branch}"));
        current_branch
    };

    // 3. Dispatch based on item type
    let is_task = item_info.item_type == "task"
        || count_remaining_tasks(repo_root, &opts.item_id) == 0;

    if is_task {
        execute_task(config, repo_root, &opts, &feature_branch, tui, &tui_tx, epic_logger.as_ref())?;
    } else {
        execute_epic(config, repo_root, &opts, &feature_branch, tui, &tui_tx, epic_logger.as_ref())?;
    }

    // Final sync
    emit(&tui_tx, "DO COMPLETE");
    emit(&tui_tx, "Final push to remote...");
    sync_and_push(repo_root, &feature_branch, epic_logger.as_ref());
    emit(&tui_tx, &format!("Done. Merge branch '{feature_branch}' when ready."));

    Ok(())
}

fn execute_task(
    config: &Config,
    repo_root: &Path,
    opts: &DoOptions,
    feature_branch: &str,
    tui: Option<&TuiHandle>,
    tui_tx: &Option<TuiSender>,
    logger: Option<&crate::logger::Logger>,
) -> Result<(), DoError> {
    emit(tui_tx, &format!("Running single task: {}", opts.item_id));

    let head_before = git_head(repo_root);

    let prompt = format!(
        "Work on task {}. Run `lb show {}` to understand the task, then `lb claim {}`. \
        Implement the changes, commit your code, then `lb close {}`, `lb sync`, and `git push`.",
        opts.item_id, opts.item_id, opts.item_id, opts.item_id
    );

    let run_opts = RunOptions {
        raw: false,
        model: opts.model.clone(),
        timeout: Some(opts.timeout),
        local: false,
        prompt_override: Some(prompt),
        branch: None,
    };

    let run_result = run::execute(config, repo_root, run_opts, tui);

    match run_result {
        Ok(ref run_logger) => {
            emit(tui_tx, "Task agent succeeded, syncing...");
            sync_and_push(repo_root, feature_branch, Some(run_logger));
        }
        Err(e) => {
            emit(tui_tx, &format!("Task agent failed: {e}"));
            return Err(DoError::Command(format!("task agent failed: {e}")));
        }
    }

    // Run reviewer on the diff if new commits were made
    let head_after = git_head(repo_root);
    let commit_range = match (&head_before, &head_after) {
        (Ok(before), Ok(after)) if before != after => Some((before.clone(), after.clone())),
        _ => None,
    };

    if commit_range.is_some() {
        emit(tui_tx, "Running reviewer on task changes...");
        if let Some(t) = tui { t.set_stage("Reviewer"); }
        let reviewer_opts = reviewer::ReviewerOptions {
            model: config.loop_config.reviewer_model.clone(),
            current_branch: feature_branch.to_string(),
            commit_range,
        };
        if let Err(e) = reviewer::execute(config, repo_root, &reviewer_opts, logger) {
            emit(tui_tx, &format!("Reviewer failed (non-fatal): {e}"));
        }
        litebrite::sync(repo_root, logger);
    } else {
        emit(tui_tx, "Reviewer skipped: no new commits.");
    }

    Ok(())
}

fn execute_epic(
    config: &Config,
    repo_root: &Path,
    opts: &DoOptions,
    feature_branch: &str,
    tui: Option<&TuiHandle>,
    tui_tx: &Option<TuiSender>,
    _logger: Option<&crate::logger::Logger>,
) -> Result<(), DoError> {
    let mut task_num: u32 = 0;
    let mut consecutive_failures: u32 = 0;

    loop {
        if tui.is_some_and(|t| t.is_cancelled()) {
            emit(tui_tx, "Epic cancelled by user.");
            break;
        }

        let remaining = count_remaining_tasks(repo_root, &opts.item_id);
        if remaining == 0 {
            emit(tui_tx, &format!("All tasks in {} complete.", opts.item_id));
            break;
        }

        task_num += 1;
        if let Some(t) = tui { t.set_run(Some(format!("Task {task_num}"))); }
        emit(tui_tx, &format!(
            "TASK {}  ({} remaining)  {}",
            task_num, remaining, chrono::Local::now().format("%H:%M:%S")
        ));

        let prompt = format!(
            "You are working on epic {}. \
            Run 'lb list --parent {}' to see tasks. Pick ONE open child task and complete it. \
            Do NOT work on tasks outside this epic. \
            Commit your changes, close the item, and push when done.",
            opts.item_id, opts.item_id
        );

        let run_opts = RunOptions {
            raw: false,
            model: opts.model.clone(),
            timeout: Some(opts.timeout),
            local: false,
            prompt_override: Some(prompt),
            branch: None,
        };

        let run_result = run::execute(config, repo_root, run_opts, tui);

        match run_result {
            Ok(ref run_logger) => {
                consecutive_failures = 0;
                emit(tui_tx, &format!("Task {task_num} succeeded, syncing..."));
                sync_and_push(repo_root, feature_branch, Some(run_logger));
            }
            Err(e) => {
                consecutive_failures += 1;
                emit(tui_tx, &format!("--- Task {task_num} failed: {e}"));

                if consecutive_failures >= opts.max_failures {
                    emit(tui_tx, &format!(
                        "ERROR: {} consecutive failures — aborting",
                        opts.max_failures
                    ));
                    return Err(DoError::TooManyFailures(opts.max_failures));
                }
            }
        }

        if let Ok(output) = Command::new("lb")
            .args(["list", "--parent", &opts.item_id])
            .current_dir(repo_root)
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                emit(tui_tx, line);
            }
        }
    }

    Ok(())
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

fn lb_show(repo_root: &Path, item_id: &str) -> Result<ItemInfo, DoError> {
    let output = Command::new("lb")
        .args(["show", item_id])
        .current_dir(repo_root)
        .output()
        .map_err(|e| DoError::Command(format!("failed to run lb show: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DoError::ItemNotFound(format!(
            "Item {item_id} not found: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let title = stdout
        .lines()
        .find(|l| l.contains("Title:"))
        .map(|l| l.trim().trim_start_matches("Title:").trim().to_string())
        .unwrap_or_else(|| item_id.to_string());

    let item_type = stdout
        .lines()
        .find(|l| l.contains("Type:"))
        .map(|l| l.trim().trim_start_matches("Type:").trim().to_string())
        .unwrap_or_else(|| "task".to_string());

    Ok(ItemInfo { title, item_type })
}

fn count_remaining_tasks(repo_root: &Path, epic_id: &str) -> u32 {
    let output = Command::new("lb")
        .args(["list", "--parent", epic_id, "-s", "open"])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // Count non-empty, non-header lines
            stdout
                .lines()
                .filter(|l| {
                    let trimmed = l.trim();
                    !trimmed.is_empty()
                        && !trimmed.starts_with("ID")
                        && !trimmed.starts_with("---")
                })
                .count() as u32
        }
        _ => 0,
    }
}

pub(crate) fn git_current_branch(repo_root: &Path) -> Result<String, DoError> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "branch", "--show-current"])
        .output()
        .map_err(|e| DoError::Command(format!("failed to get current branch: {e}")))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn git_checkout_new_branch(repo_root: &Path, branch: &str) -> Result<(), DoError> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "checkout", "-b", branch])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| DoError::Command(format!("failed to create branch: {e}")))?;

    if !output.success() {
        return Err(DoError::Command(format!(
            "git checkout -b {branch} failed"
        )));
    }

    Ok(())
}

pub(crate) fn sync_and_push(repo_root: &Path, branch: &str, logger: Option<&crate::logger::Logger>) {
    litebrite::sync(repo_root, logger);

    // Push to remote
    let _ = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "push",
            "-u",
            "origin",
            branch,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(crate) fn make_slug(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse multiple dashes, trim leading/trailing dashes
    let mut result = String::new();
    let mut prev_dash = true; // start true to skip leading dashes
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }

    // Trim trailing dash and limit length
    let trimmed = result.trim_end_matches('-');
    if trimmed.len() > 50 {
        // Use char boundary to avoid panic on multibyte UTF-8
        let mut end = 50;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        trimmed[..end].trim_end_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug)]
pub enum DoError {
    ItemNotFound(String),
    Command(String),
    TooManyFailures(u32),
}

impl std::fmt::Display for DoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ItemNotFound(msg) => write!(f, "{msg}"),
            Self::Command(msg) => write!(f, "{msg}"),
            Self::TooManyFailures(n) => {
                write!(f, "aborted after {n} consecutive failures")
            }
        }
    }
}

impl std::error::Error for DoError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basic() {
        assert_eq!(make_slug("Add user authentication"), "add-user-authentication");
    }

    #[test]
    fn slug_special_chars() {
        assert_eq!(make_slug("Fix bug #123 (critical)"), "fix-bug-123-critical");
    }

    #[test]
    fn slug_leading_trailing_dashes() {
        assert_eq!(make_slug("  --hello world--  "), "hello-world");
    }

    #[test]
    fn slug_consecutive_dashes() {
        assert_eq!(make_slug("foo---bar___baz"), "foo-bar-baz");
    }

    #[test]
    fn slug_truncates_long_titles() {
        let long_title = "a".repeat(100);
        let slug = make_slug(&long_title);
        assert!(slug.len() <= 50);
    }

    #[test]
    fn slug_empty_input() {
        assert_eq!(make_slug(""), "");
    }

    #[test]
    fn slug_all_special_chars() {
        assert_eq!(make_slug("!@#$%^&*()"), "");
    }

    #[test]
    fn slug_multibyte_utf8_truncation() {
        // Title with accented chars that produce multibyte UTF-8
        let title = "é".repeat(60); // each é is 2 bytes
        let slug = make_slug(&title);
        assert!(slug.len() <= 50);
        // Verify it's valid UTF-8 (would panic on bad boundary)
        let _ = slug.chars().count();
    }
}
