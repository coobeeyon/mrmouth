use std::fs::{self, File};
use std::io::{BufWriter, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config::Config;
use crate::docker::{ContainerArgs, DockerBuilder};
use crate::litebrite;
use crate::logger::Logger;
use crate::prompt;
use crate::stream_fmt::{self, StreamFormatter};
use crate::tui::TuiHandle;

/// Guard that sets an AtomicBool to true on drop, ensuring the cancel watcher
/// thread is signaled even if the function returns early via `?`.
struct DoneGuard(Arc<AtomicBool>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub struct RunOptions {
    pub raw: bool,
    pub model: String,
    pub timeout: Option<u32>,
    pub local: bool,
    pub prompt_override: Option<String>,
    pub branch: Option<String>,
}

/// Execute one agent run. Returns the Logger so callers can continue writing to the same
/// log file for subsequent stages (reviewer, decider, summary, etc.).
pub fn execute(config: &Config, repo_root: &Path, opts: RunOptions, tui: Option<&TuiHandle>) -> Result<Logger, RunError> {
    if let Some(t) = tui { t.set_status("Agent"); }
    // 0. Set up logging first so every stage is captured
    let log_dir = repo_root.join(&config.log_dir);
    fs::create_dir_all(&log_dir)
        .map_err(|e| RunError::Io("creating log directory".into(), e))?;
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let log_filename = format!("run-{timestamp}.log");
    let log_path = log_dir.join(&log_filename);
    let logger = match tui {
        Some(t) => Logger::with_tui(&log_path, t.sender("AGENT SESSION"))
            .map_err(|e| RunError::Io("creating log file".into(), e))?,
        None => Logger::new(&log_path)
            .map_err(|e| RunError::Io("creating log file".into(), e))?,
    };

    // Resolve branch early so we can include it in the opening banner
    let branch = opts
        .branch
        .clone()
        .or_else(|| config.branch.clone())
        .unwrap_or_else(|| git_current_branch(repo_root).unwrap_or_else(|_| "main".into()));

    logger.log(&format!("AGENT RUN  branch={branch}  {timestamp}"));

    // 1. Preflight checks
    logger.log("Checking preflight conditions...");
    preflight(repo_root, opts.local).inspect_err(|_| { logger.flush(); })?;

    // 2. Resolve repo URL
    let (repo_url, file_remote_path) = if opts.local {
        (String::new(), None)
    } else {
        match git_remote_url(repo_root) {
            Some(url) => (url, None),
            None => {
                configure_file_remote(repo_root)?;
                ("file:///host-repo".to_string(), Some(repo_root.to_path_buf()))
            }
        }
    };

    // 3. Sync litebrite (best-effort)
    logger.log("Syncing litebrite...");
    litebrite::init_and_sync(repo_root, Some(&logger));

    // 4. Write runner entrypoint script
    let runner_script = write_runner_script(repo_root, &opts.model, opts.prompt_override.as_deref(), Some(&logger))?;

    // 5. Build Docker image
    logger.log("Docker build starting...");
    let docker = DockerBuilder::new(&config.image);
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(RunError::Docker)?;

    // 6. Ensure persistent volume
    let volume = config.effective_volume(repo_root);
    docker
        .ensure_volume(&volume)
        .map_err(RunError::Docker)?;

    // 7. Set up JSONL log alongside the text log
    let jsonl_filename = format!("run-{timestamp}.jsonl");
    let jsonl_path = log_dir.join(&jsonl_filename);

    let container_name = format!("run-{timestamp}");

    // Remove stale container
    DockerBuilder::remove_container(&container_name);

    // 8. Start container
    logger.log(&format!("AGENT SESSION  container={container_name}"));
    logger.log(&format!("Branch: {branch}"));

    let container_args = ContainerArgs {
        name: container_name.clone(),
        repo_url,
        branch: branch.clone(),
        runner_script: runner_script.path().to_path_buf(),
        volume,
        local: opts.local,
        file_remote_path: file_remote_path.clone(),
        timeout_secs: opts.timeout.map(|m| m as u64 * 60),
    };

    let mut handle = docker.run(&container_args).map_err(RunError::Docker)?;

    // 8b. Spawn a watcher that stops the container if the TUI user cancels (q / Ctrl+C)
    let watcher_done = Arc::new(AtomicBool::new(false));
    // DoneGuard ensures watcher_done is set even if we return early via `?`,
    // preventing the cancel watcher thread from spinning indefinitely.
    let _done_guard = DoneGuard(Arc::clone(&watcher_done));
    let _cancel_watcher = if let Some(t) = tui {
        let flag = t.cancelled_flag();
        let done = Arc::clone(&watcher_done);
        let name = container_name.clone();
        Some(std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if flag.load(Ordering::Relaxed) {
                DockerBuilder::stop_container(&name);
            }
        }))
    } else {
        None
    };

    // 9. Stream output — raw JSONL to .jsonl file, formatted text to terminal + .log file
    let jsonl_file = File::create(&jsonl_path)
        .map_err(|e| RunError::Io("creating jsonl file".into(), e))?;
    let mut jsonl_writer = BufWriter::new(jsonl_file);
    let is_tty = logger.has_tui() || std::io::stdout().is_terminal();

    if opts.raw {
        let stdout = std::io::stdout();
        handle
            .stream_output(|line| {
                let _ = writeln!(stdout.lock(), "{line}");
                let _ = writeln!(jsonl_writer, "{line}");
                logger.log_file_only(line);
            })
            .map_err(RunError::Docker)?;
    } else {
        let mut formatter = StreamFormatter::new(is_tty);
        handle
            .stream_output(|line| {
                let _ = writeln!(jsonl_writer, "{line}");
                if let Some(formatted) = stream_fmt::format_line(&mut formatter, line) {
                    logger.display(&formatted);
                    logger.log_file_only(&formatted);
                }
            })
            .map_err(RunError::Docker)?;
    }

    let _ = jsonl_writer.flush();

    // 10. Wait for container exit
    let exit_code = handle.wait().map_err(RunError::Docker)?;
    watcher_done.store(true, Ordering::Relaxed);
    logger.log(&format!("Container {container_name} finished (exit code {exit_code})."));

    // 11. Update symlinks atomically (latest.jsonl and latest.log)
    let latest_jsonl = log_dir.join("latest.jsonl");
    let latest_log = log_dir.join("latest.log");
    #[cfg(unix)]
    {
        atomic_symlink(&jsonl_filename, &latest_jsonl);
        atomic_symlink(&log_filename, &latest_log);
    }

    // 12. Extract updated Dockerfile from container (agent may have modified it)
    if !opts.local {
        let dockerfile_dest = repo_root.join(&config.dockerfile);
        let container_path = format!("/home/runner/workspace/{}", config.dockerfile);
        if DockerBuilder::copy_from_container(&container_name, &container_path, &dockerfile_dest) {
            logger.log("Extracted updated Dockerfile from container.");
        }
    }

    // 13. Clean up container
    DockerBuilder::remove_container(&container_name);

    // 14. Post-run sync
    logger.log("Post-run sync...");

    if !opts.local && file_remote_path.is_none() {
        logger.log("Pulling code changes from remote...");
        let pull_output = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "pull", "--ff-only"])
            .output();
        match pull_output {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                if stderr.contains("Already up to date") || stderr.is_empty() {
                    logger.log("No new commits to pull.");
                } else {
                    logger.log(&format!("Warning: git pull failed: {}", stderr.trim()));
                }
            }
            Err(e) => logger.log(&format!("Warning: git pull failed: {e}")),
        }
    }

    litebrite::init_and_sync(repo_root, Some(&logger));
    logger.log(&format!("Done. Log saved: {}", log_path.display()));

    if exit_code != 0 {
        return Err(RunError::ContainerFailed(exit_code));
    }

    Ok(logger)
}

fn preflight(repo_root: &Path, local: bool) -> Result<(), RunError> {
    let docker_check = Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match docker_check {
        Ok(s) if s.success() => {}
        _ => return Err(RunError::Preflight("Docker is not available. Is Docker running?".into())),
    }

    let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let has_oauth = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok();
    if !has_api_key && !has_oauth {
        return Err(RunError::Preflight(
            "No credentials found. Set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN.".into(),
        ));
    }

    if !local {
        let diff_status = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "diff", "--quiet"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| RunError::Io("checking git diff".into(), e))?;
        let cached_status = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "diff", "--cached", "--quiet"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| RunError::Io("checking git diff --cached".into(), e))?;

        if !diff_status.success() || !cached_status.success() {
            return Err(RunError::Preflight(
                "Working tree has uncommitted changes. Commit or stash first.".into(),
            ));
        }
    }

    Ok(())
}

fn git_remote_url(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn configure_file_remote(repo_root: &Path) -> Result<(), RunError> {
    let status = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "config", "receive.denyCurrentBranch", "updateInstead"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| RunError::Io("configuring git receive policy".into(), e))?;
    if !status.success() {
        return Err(RunError::Preflight("failed to set receive.denyCurrentBranch = updateInstead".into()));
    }
    Ok(())
}

fn git_current_branch(repo_root: &Path) -> Result<String, RunError> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "branch", "--show-current"])
        .output()
        .map_err(|e| RunError::Io("getting current branch".into(), e))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_runner_script(
    repo_root: &Path,
    model: &str,
    prompt_override: Option<&str>,
    logger: Option<&Logger>,
) -> Result<tempfile::NamedTempFile, RunError> {
    let prompt_text = match prompt_override {
        Some(p) => p.to_string(),
        None => prompt::load_prompt(repo_root, logger),
    };
    let escaped_prompt = prompt_text.replace('\'', "'\\''");

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"

# --- Clone repo (skip if workspace already mounted) ---
if [ ! -d "$work_dir/.git" ]; then
  if [ -n "$repo_url" ]; then
    echo "Cloning $repo_url (branch: $branch)..."
    git clone --branch "$branch" "$repo_url" "$work_dir"
  else
    echo "No repo URL and no .git — starting fresh in $work_dir"
    mkdir -p "$work_dir"
  fi
fi
cd "$work_dir"
git config --global --add safe.directory "$work_dir"

# --- Initialize litebrite (requires git repo) ---
if [ -d "$work_dir/.git" ]; then
  echo "Initializing litebrite..."
  lb init
  lb setup claude 2>/dev/null || true
  lb sync 2>/dev/null || true
fi

# --- Restore .claude.json from persisted backup if missing ---
claude_config="$HOME/.claude.json"
if [ ! -f "$claude_config" ] && [ -d "$HOME/.claude/backups" ]; then
  latest_backup=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1)
  if [ -n "$latest_backup" ]; then
    cp "$latest_backup" "$claude_config"
    echo "Restored .claude.json from backup: $(basename "$latest_backup")"
  fi
fi

# --- Run agent ---
echo "Starting agent run..."
claude -p --dangerously-skip-permissions --verbose --output-format stream-json --model {model} '{escaped_prompt}'

echo "Agent run complete."

# --- Belt-and-suspenders: force sync/push even if agent forgot ---
if [ -d "$work_dir/.git" ]; then
  echo "Post-agent cleanup: forcing lb sync and git push..."
  lb sync 2>/dev/null || true
  git push 2>/dev/null || true
fi
"#
    );

    let mut tmp = tempfile::NamedTempFile::new()
        .map_err(|e| RunError::Io("creating runner script".into(), e))?;
    tmp.write_all(script.as_bytes())
        .map_err(|e| RunError::Io("writing runner script".into(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(tmp.path(), perms)
            .map_err(|e| RunError::Io("setting runner script permissions".into(), e))?;
    }

    Ok(tmp)
}

#[derive(Debug)]
pub enum RunError {
    Preflight(String),
    Docker(crate::docker::DockerError),
    Io(String, std::io::Error),
    ContainerFailed(i32),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preflight(msg) => write!(f, "preflight check failed: {msg}"),
            Self::Docker(e) => write!(f, "docker error: {e}"),
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Self::ContainerFailed(code) => write!(f, "container exited with code {code}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Atomically replace a symlink by creating a temp link and renaming over the target.
#[cfg(unix)]
fn atomic_symlink(target: &str, link_path: &std::path::PathBuf) {
    use std::os::unix::fs as unix_fs;
    let tmp = link_path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    if unix_fs::symlink(target, &tmp).is_ok() && fs::rename(&tmp, link_path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn runner_script_contains_model() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "sonnet", None, None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("--model sonnet"));
    }

    #[test]
    fn runner_script_contains_lb_sync() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        let init_pos = content.find("lb init").unwrap();
        let sync_pos = content.find("lb sync 2>/dev/null || true").unwrap();
        assert!(sync_pos > init_pos, "lb sync should come after lb init");
    }

    #[test]
    fn runner_script_uses_prompt_override() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", Some("custom prompt here"), None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("custom prompt here"));
    }

    #[test]
    fn runner_script_escapes_single_quotes_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", Some("don't break"), None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains(r"don'\''t break"));
    }

    #[test]
    fn runner_script_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, None).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(tmp.path()).unwrap().permissions();
            assert_eq!(perms.mode() & 0o755, 0o755);
        }
    }

    #[test]
    fn runner_script_has_shebang() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
    }
}
