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
                crate::logger::log(
                    logger,
                    &format!(
                        "[litebrite] `lb {}` failed (exit {}): {}",
                        args.join(" "),
                        output.status,
                        stderr
                    ),
                );
            } else {
                crate::logger::log(
                    logger,
                    &format!(
                        "[litebrite] `lb {}` failed (exit {})",
                        args.join(" "),
                        output.status
                    ),
                );
            }
        }
        Err(e) => {
            crate::logger::log(
                logger,
                &format!("[litebrite] failed to run `lb {}`: {}", args.join(" "), e),
            );
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

/// Check whether a local git branch exists.
fn has_local_branch(repo_root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "show-ref",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check whether the repo has a git remote named "origin".
fn has_git_remote(repo_root: &Path) -> bool {
    Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
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
    if !has_local_branch(repo_root, "litebrite") {
        run_lb(repo_root, &["init"], logger);
    }
    run_lb(repo_root, &["setup", "claude"], logger);
    if has_git_remote(repo_root) {
        run_lb(repo_root, &["sync"], logger);
    }
}

/// Count open tasks in the litebrite tracker. Returns None when `lb` is not
/// installed or the command fails — caller should fall back to the LLM path.
/// Used by the decider short-circuit: when open leaf tasks exist, the runner
/// has work to do and no LLM judgement is required.
pub fn open_task_count(repo_root: &Path) -> Option<usize> {
    if !has_lb() {
        return None;
    }
    let output = Command::new("lb")
        .args(["list", "-s", "open", "-t", "task"])
        .current_dir(repo_root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(count_lb_rows(&stdout))
}

fn count_lb_rows(stdout: &str) -> usize {
    stdout.lines().filter(|l| l.starts_with("lb-")).count()
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
    use std::fs;
    use std::process::Command;

    #[test]
    fn benign_lb_init_already_initialized() {
        assert!(is_benign_lb_error(
            &["init"],
            "litebrite already initialized"
        ));
        assert!(is_benign_lb_error(
            &["init"],
            "Error: already initialized in this repo"
        ));
    }

    #[test]
    fn other_init_failures_are_not_benign() {
        assert!(!is_benign_lb_error(&["init"], "permission denied"));
        assert!(!is_benign_lb_error(&["init"], ""));
    }

    #[test]
    fn benign_only_applies_to_init() {
        assert!(!is_benign_lb_error(&["sync"], "already initialized"));
        assert!(!is_benign_lb_error(
            &["setup", "claude"],
            "already initialized"
        ));
    }

    #[test]
    fn count_lb_rows_skips_header_and_separator() {
        let out = "ID         TYPE     STATUS         PRI  TITLE\n\
                   ------------------------------------------------------------\n\
                   lb-6zk8    task open (claimed) P2   Skip decider LLM call when open brites exist\n\
                   lb-abcd    task open           P1   Another task\n";
        assert_eq!(count_lb_rows(out), 2);
    }

    #[test]
    fn count_lb_rows_zero_when_no_matches() {
        let out = "ID         TYPE     STATUS         PRI  TITLE\n\
                   ------------------------------------------------------------\n";
        assert_eq!(count_lb_rows(out), 0);
        assert_eq!(count_lb_rows(""), 0);
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_succeeds(repo: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    fn initialized_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        fs::write(dir.path().join("README.md"), "test\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "Initial commit",
            ],
        );
        dir
    }

    #[test]
    fn local_branch_check_detects_litebrite_without_worktree_dir() {
        let repo = initialized_repo();
        git(repo.path(), &["branch", "litebrite"]);

        assert!(has_local_branch(repo.path(), "litebrite"));
        assert!(!repo.path().join(".litebrite").exists());
    }

    #[test]
    fn git_remote_check_allows_sync_before_litebrite_remote_ref_exists() {
        let repo = initialized_repo();
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "--bare"]);
        git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );

        assert!(has_git_remote(repo.path()));
        assert!(!git_succeeds(
            repo.path(),
            &["show-ref", "--quiet", "refs/remotes/origin/litebrite"]
        ));
    }
}
