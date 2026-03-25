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

/// Full litebrite setup: init, setup claude, then sync.
/// Used before/after agent runs where the repo state may need initialization.
pub fn init_and_sync(repo_root: &Path, logger: Option<&Logger>) {
    if !has_lb() {
        return;
    }
    run_lb(repo_root, &["init"], logger);
    run_lb(repo_root, &["setup", "claude"], logger);
    run_lb(repo_root, &["sync"], logger);
}

/// Sync-only: just run `lb sync` to exchange state with remote.
/// Used between loop iterations where init is already done.
pub fn sync(repo_root: &Path, logger: Option<&Logger>) {
    if !has_lb() {
        return;
    }
    run_lb(repo_root, &["sync"], logger);
}
