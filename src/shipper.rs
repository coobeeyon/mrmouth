use std::path::Path;
use std::process::Command;

use crate::logger::Logger;
use crate::stream_fmt::StreamFormatter;
use crate::streaming::{self, StreamTarget};

pub struct ShipperOptions {
    pub model: String,
    pub current_branch: String,
    pub parent_branch: String,
}

pub struct ShipResult {
    pub new_branch: String,
}

/// Run the shipper agent: check readiness, merge branch, create next feature branch.
pub fn execute(repo_root: &Path, opts: &ShipperOptions, logger: Option<&Logger>) -> Result<ShipResult, ShipperError> {
    crate::logger::banner(logger, &format!("SHIPPING  {} -> {}", opts.current_branch, opts.parent_branch));

    // 1. Check readiness
    check_ready(repo_root, &opts.current_branch, logger)?;

    // 2. Merge current branch into parent
    merge_branch(repo_root, &opts.current_branch, &opts.parent_branch, logger)?;

    // 3. Generate and create next feature branch
    let branch_name = generate_branch_name(repo_root, &opts.model, logger)?;
    create_and_push_branch(repo_root, &branch_name, logger)?;

    Ok(ShipResult { new_branch: branch_name })
}

fn check_ready(repo_root: &Path, current_branch: &str, logger: Option<&Logger>) -> Result<(), ShipperError> {
    let schema = r#"{"type":"object","properties":{"status":{"type":"string","enum":["READY","BLOCKED"]},"reason":{"type":"string"}},"required":["status","reason"]}"#;

    let prompt = format!(
        "You are checking if branch '{}' is ready to ship. \
        Check: (1) run 'lb list -s open' to see if there are open tasks that block shipping, \
        (2) check if there are any obvious test failures. \
        Return READY if the branch can be merged, BLOCKED if not.",
        current_branch
    );

    let mut cmd = streaming::claude_stream_cmd_with_schema(
        repo_root,
        "haiku",
        "Bash(lb *),Bash(cargo test *)",
        schema,
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| ShipperError(format!("failed to run readiness check: {e}")))?;

    streaming::send_prompt(&mut child, &prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.with_label("READINESS CHECK")),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let (result_text, exit_code) = streaming::run_streaming_claude(child, &mut formatter, logger, &target)
        .map_err(|e| ShipperError(format!("readiness check failed: {e}")))?;

    if exit_code != 0 {
        return Err(ShipperError(format!(
            "readiness check exited with code {exit_code}"
        )));
    }

    let parsed: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log(logger, &format!("WARNING: readiness check returned invalid JSON: {e}"));
            return Err(ShipperError(format!("readiness check returned invalid JSON: {e}")));
        }
    };
    let status = parsed["status"].as_str().unwrap_or("BLOCKED");
    let reason = parsed["reason"].as_str().unwrap_or("no reason given");

    if status == "BLOCKED" {
        return Err(ShipperError(format!("branch not ready to ship: {reason}")));
    }

    crate::logger::log(logger, &format!("Readiness check passed: {reason}"));
    Ok(())
}

fn merge_branch(
    repo_root: &Path,
    current_branch: &str,
    parent_branch: &str,
    logger: Option<&Logger>,
) -> Result<(), ShipperError> {
    let has_github_remote = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("github.com"))
        .unwrap_or(false);

    if has_github_remote {
        crate::logger::log(logger, &format!("Creating and merging PR: {current_branch} -> {parent_branch}"));

        let pr_create = Command::new("gh")
            .args([
                "pr", "create",
                "--base", parent_branch,
                "--head", current_branch,
                "--title", &format!("Merge {current_branch}"),
                "--body", "Auto-merged by mrmouth shipper.",
            ])
            .current_dir(repo_root)
            .output()
            .map_err(|e| ShipperError(format!("failed to create PR: {e}")))?;

        if !pr_create.status.success() {
            let stderr = String::from_utf8_lossy(&pr_create.stderr);
            if !stderr.contains("already exists") {
                return Err(ShipperError(format!("gh pr create failed: {stderr}")));
            }
        }

        let pr_merge = Command::new("gh")
            .args(["pr", "merge", current_branch, "--merge", "--delete-branch"])
            .current_dir(repo_root)
            .status()
            .map_err(|e| ShipperError(format!("failed to merge PR: {e}")))?;

        if !pr_merge.success() {
            return Err(ShipperError("gh pr merge failed".into()));
        }

        let _ = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "checkout", parent_branch])
            .status();
        let _ = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "pull", "--ff-only"])
            .status();
    } else {
        crate::logger::log(logger, &format!("Merging {current_branch} into {parent_branch} (no-ff)..."));

        let checkout = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "checkout", parent_branch])
            .status()
            .map_err(|e| ShipperError(format!("failed to checkout {parent_branch}: {e}")))?;
        if !checkout.success() {
            return Err(ShipperError(format!("failed to checkout {parent_branch}")));
        }

        let merge = Command::new("git")
            .args([
                "-C", &repo_root.to_string_lossy(),
                "merge", "--no-ff", current_branch,
                "-m", &format!("Merge branch '{current_branch}'"),
            ])
            .status()
            .map_err(|e| ShipperError(format!("merge failed: {e}")))?;
        if !merge.success() {
            return Err(ShipperError(format!("merge of {current_branch} failed")));
        }

        let _ = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "branch", "-d", current_branch])
            .status();
    }

    Ok(())
}

pub fn generate_branch_name(repo_root: &Path, model: &str, logger: Option<&Logger>) -> Result<String, ShipperError> {
    let schema = r#"{"type":"object","properties":{"name":{"type":"string","description":"2-4 word kebab-case slug for the branch"}},"required":["name"]}"#;

    let prompt = "Read SPEC.md and the current litebrite task state (lb ready). \
        Generate a short 2-4 word kebab-case name describing the next batch of work. \
        Examples: review-ship-flow, docker-caching, test-coverage. \
        Just the slug, no 'feat-' prefix.";

    let mut cmd = streaming::claude_stream_cmd_with_schema(
        repo_root,
        model,
        "Read,Bash(lb *)",
        schema,
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| ShipperError(format!("failed to generate branch name: {e}")))?;

    streaming::send_prompt(&mut child, prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.with_label("BRANCH NAME")),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let (result_text, exit_code) = streaming::run_streaming_claude(child, &mut formatter, logger, &target)
        .map_err(|e| ShipperError(format!("branch name generation failed: {e}")))?;

    if exit_code != 0 {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        return Ok(format!("feat-{ts}"));
    }

    let parsed: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log(logger, &format!("WARNING: branch name generation returned invalid JSON (using timestamp fallback): {e}"));
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            return Ok(format!("feat-{ts}"));
        }
    };
    let slug = parsed["name"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_lowercase()
        .replace(' ', "-");

    if slug.is_empty() {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        Ok(format!("feat-{ts}"))
    } else {
        Ok(format!("feat-{slug}"))
    }
}

pub fn create_and_push_branch(repo_root: &Path, branch_name: &str, logger: Option<&Logger>) -> Result<(), ShipperError> {
    let branch_exists = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch_name}"),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let checkout_args: &[&str] = if branch_exists {
        &["checkout", branch_name]
    } else {
        &["checkout", "-b", branch_name]
    };

    let status = Command::new("git")
        .arg("-C")
        .arg(&*repo_root.to_string_lossy())
        .args(checkout_args)
        .status()
        .map_err(|e| ShipperError(format!("failed to create branch {branch_name}: {e}")))?;

    if !status.success() {
        return Err(ShipperError(format!("git checkout -b {branch_name} failed")));
    }

    let has_remote = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_remote {
        let push = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "push", "-u", "origin", branch_name])
            .status()
            .map_err(|e| ShipperError(format!("failed to push branch: {e}")))?;

        if !push.success() {
            crate::logger::log(logger, &format!("Warning: failed to push branch {branch_name} to origin"));
        }
    }

    crate::logger::log(logger, &format!("Created branch: {branch_name}"));
    Ok(())
}

#[derive(Debug)]
pub struct ShipperError(String);

impl std::fmt::Display for ShipperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ShipperError {}
