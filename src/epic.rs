use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::litebrite;
use crate::run::{self, RunOptions};

pub struct EpicOptions {
    pub epic_id: String,
    pub timeout: u32,
    pub max_failures: u32,
    pub model: String,
}

pub fn execute(config: &Config, repo_root: &Path, opts: EpicOptions) -> Result<(), EpicError> {
    // 1. Verify the epic exists
    let epic_info = lb_show(repo_root, &opts.epic_id)?;
    eprintln!("Epic: {epic_info}");

    // 2. Create feature branch (if not already on one)
    let current_branch = git_current_branch(repo_root)?;
    let feature_branch = if current_branch == "main" || current_branch == "master" {
        let slug = make_slug(&epic_info);
        let branch_name = format!("{}-{}", opts.epic_id, slug);
        eprintln!("Creating feature branch: {branch_name}");
        git_checkout_new_branch(repo_root, &branch_name)?;
        branch_name
    } else {
        eprintln!("Already on branch: {current_branch}");
        current_branch
    };

    // 3. Task loop
    let mut task_num: u32 = 0;
    let mut consecutive_failures: u32 = 0;

    loop {
        // Check remaining tasks
        let remaining = count_remaining_tasks(repo_root, &opts.epic_id);
        if remaining == 0 {
            eprintln!();
            eprintln!("All tasks in {} complete.", opts.epic_id);
            break;
        }

        task_num += 1;
        eprintln!();
        eprintln!(
            "=== Task {task_num} | {remaining} remaining | {} ===",
            chrono::Local::now().format("%H:%M:%S")
        );

        // Build epic-focused prompt
        let prompt = format!(
            "You are working on epic {}. \
            Run 'lb list --parent {}' to see tasks. Pick ONE open child task and complete it. \
            Do NOT work on tasks outside this epic. \
            Commit your changes, close the item, and push when done.",
            opts.epic_id, opts.epic_id
        );

        let run_opts = RunOptions {
            raw: false,
            model: opts.model.clone(),
            timeout: Some(opts.timeout),
            local: false,
            prompt_override: Some(prompt),
        };

        let run_result = run::execute(config, repo_root, run_opts);

        match run_result {
            Ok(()) => {
                consecutive_failures = 0;
                eprintln!("--- Task {task_num} succeeded, syncing...");
                sync_and_push(repo_root, &feature_branch);
            }
            Err(e) => {
                consecutive_failures += 1;
                eprintln!("--- Task {task_num} failed: {e}");

                if consecutive_failures >= opts.max_failures {
                    eprintln!();
                    eprintln!(
                        "ERROR: {} consecutive failures — aborting",
                        opts.max_failures
                    );
                    return Err(EpicError::TooManyFailures(opts.max_failures));
                }
            }
        }

        // Show current task state
        let _ = Command::new("lb")
            .args(["list", "--parent", &opts.epic_id])
            .current_dir(repo_root)
            .status();
    }

    // Final sync
    eprintln!();
    eprintln!("Final push to remote...");
    sync_and_push(repo_root, &feature_branch);
    eprintln!(
        "Done. Merge branch '{}' when ready.",
        feature_branch
    );

    Ok(())
}

fn lb_show(repo_root: &Path, epic_id: &str) -> Result<String, EpicError> {
    let output = Command::new("lb")
        .args(["show", epic_id])
        .current_dir(repo_root)
        .output()
        .map_err(|e| EpicError::Command(format!("failed to run lb show: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EpicError::EpicNotFound(format!(
            "Epic {epic_id} not found: {stderr}"
        )));
    }

    // Extract the title line (first line of lb show output)
    let stdout = String::from_utf8_lossy(&output.stdout);
    let title = stdout
        .lines()
        .find(|l| l.contains("Title:"))
        .map(|l| l.trim().trim_start_matches("Title:").trim().to_string())
        .unwrap_or_else(|| epic_id.to_string());

    Ok(title)
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

fn git_current_branch(repo_root: &Path) -> Result<String, EpicError> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "branch", "--show-current"])
        .output()
        .map_err(|e| EpicError::Command(format!("failed to get current branch: {e}")))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_checkout_new_branch(repo_root: &Path, branch: &str) -> Result<(), EpicError> {
    let status = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "checkout", "-b", branch])
        .status()
        .map_err(|e| EpicError::Command(format!("failed to create branch: {e}")))?;

    if !status.success() {
        return Err(EpicError::Command(format!(
            "git checkout -b {branch} failed"
        )));
    }

    Ok(())
}

fn sync_and_push(repo_root: &Path, branch: &str) {
    litebrite::sync(repo_root);

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
        .status();
}

fn make_slug(title: &str) -> String {
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
        trimmed[..50].trim_end_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug)]
pub enum EpicError {
    EpicNotFound(String),
    Command(String),
    TooManyFailures(u32),
}

impl std::fmt::Display for EpicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EpicNotFound(msg) => write!(f, "{msg}"),
            Self::Command(msg) => write!(f, "{msg}"),
            Self::TooManyFailures(n) => {
                write!(f, "aborted after {n} consecutive failures")
            }
        }
    }
}

impl std::error::Error for EpicError {}
