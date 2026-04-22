use std::path::Path;
use std::process::Command;

use crate::logger::Logger;

fn has_lb() -> bool {
    Command::new("which")
        .arg("lb")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run an lb subcommand, logging failures through the logger if provided.
fn run_lb(repo_root: &Path, args: &[&str], logger: Option<&Logger>) {
    let result = Command::new("lb")
        .args(args)
        .current_dir(repo_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match result {
        Ok(output) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if is_benign_lb_error(args, stderr) {
                return;
            }
            if !stderr.is_empty() {
                crate::logger::log(logger, &format!("[litebrite] `lb {}` failed (exit {}): {}", args.join(" "), output.status, stderr));
            } else {
                crate::logger::log(logger, &format!("[litebrite] `lb {}` failed (exit {})", args.join(" "), output.status));
            }
        }
        Err(e) => {
            crate::logger::log(logger, &format!("[litebrite] failed to run `lb {}`: {}", args.join(" "), e));
        }
        _ => {}
    }
}

/// Some lb failures are expected and not worth surfacing. Today the only case
/// is `lb init` complaining that the repo is already initialized — our
/// `.litebrite` dir guard misses initialization stored only in the litebrite
/// branch, so this fires on every run in existing repos.
fn is_benign_lb_error(args: &[&str], stderr: &str) -> bool {
    matches!(args.first(), Some(&"init")) && stderr.contains("already initialized")
}

/// Check whether the repo has a git remote named "origin".
fn has_git_remote(repo_root: &Path) -> bool {
    Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "remote", "get-url", "origin"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Full litebrite setup: init, setup claude, then sync.
/// Used before/after agent runs where the repo state may need initialization.
pub fn init_and_sync(repo_root: &Path, logger: Option<&Logger>) {
    if !has_lb() {
        return;
    }
    if !repo_root.join(".litebrite").exists() {
        run_lb(repo_root, &["init"], logger);
    }
    run_lb(repo_root, &["setup", "claude"], logger);
    if has_git_remote(repo_root) {
        run_lb(repo_root, &["sync"], logger);
    }
}

/// Sync-only: just run `lb sync` to exchange state with remote.
/// Used between loop iterations where init is already done.
pub fn sync(repo_root: &Path, logger: Option<&Logger>) {
    if !has_lb() {
        return;
    }
    if has_git_remote(repo_root) {
        run_lb(repo_root, &["sync"], logger);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benign_lb_init_already_initialized() {
        assert!(is_benign_lb_error(&["init"], "litebrite already initialized"));
        assert!(is_benign_lb_error(&["init"], "Error: already initialized in this repo"));
    }

    #[test]
    fn other_init_failures_are_not_benign() {
        assert!(!is_benign_lb_error(&["init"], "permission denied"));
        assert!(!is_benign_lb_error(&["init"], ""));
    }

    #[test]
    fn benign_only_applies_to_init() {
        assert!(!is_benign_lb_error(&["sync"], "already initialized"));
        assert!(!is_benign_lb_error(&["setup", "claude"], "already initialized"));
    }
}
