use std::fs::{self, File};
use std::io::{BufWriter, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::docker::{ContainerArgs, DockerBuilder};
use crate::litebrite;
use crate::prompt;
use crate::stream_fmt::{self, StreamFormatter};

pub struct RunOptions {
    pub raw: bool,
    pub model: String,
    pub timeout: Option<u32>,
    pub local: bool,
    pub prompt_override: Option<String>,
}

pub fn execute(config: &Config, repo_root: &Path, opts: RunOptions) -> Result<(), RunError> {
    // 1. Preflight checks
    preflight(repo_root, opts.local)?;

    // 2. Resolve repo URL and branch
    let (repo_url, file_remote_path) = if opts.local {
        (git_remote_url(repo_root).unwrap_or_default(), None)
    } else {
        match git_remote_url(repo_root) {
            Some(url) => (url, None),
            None => {
                // No remote configured — mount the host repo into the container and
                // use it as a file:// remote. The container clones from it, the agent
                // commits and pushes back, and updateInstead keeps the host tree in sync.
                configure_file_remote(repo_root)?;
                ("file:///host-repo".to_string(), Some(repo_root.to_path_buf()))
            }
        }
    };
    let branch = config
        .branch
        .clone()
        .unwrap_or_else(|| git_current_branch(repo_root).unwrap_or_else(|_| "main".into()));

    // 3. Sync litebrite (best-effort)
    litebrite::init_and_sync(repo_root);

    // 4. Write the runner entrypoint script to a temp file
    let runner_script = write_runner_script(repo_root, &opts.model, opts.prompt_override.as_deref())?;

    // 5. Build Docker image
    let docker = DockerBuilder::new(&config.image);
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(RunError::Docker)?;

    // 6. Ensure persistent volume
    docker
        .ensure_volume(&config.volume)
        .map_err(RunError::Docker)?;

    // 7. Set up logging
    let log_dir = repo_root.join(&config.log_dir);
    fs::create_dir_all(&log_dir)
        .map_err(|e| RunError::Io("creating log directory".into(), e))?;
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let log_filename = format!("run-{timestamp}.jsonl");
    let log_path = log_dir.join(&log_filename);

    let container_name = format!("run-{timestamp}");
    eprintln!("Running agent on branch {branch}...");
    eprintln!("Container name: {container_name}");

    // Remove stale container
    DockerBuilder::remove_container(&container_name);

    // 8. Start container
    let container_args = ContainerArgs {
        name: container_name.clone(),
        repo_url,
        branch: branch.clone(),
        runner_script: runner_script.path().to_path_buf(),
        volume: config.volume.clone(),
        local: opts.local,
        file_remote_path: file_remote_path.clone(),
        timeout_secs: opts.timeout.map(|m| m as u64 * 60),
    };

    let mut handle = docker.run(&container_args).map_err(RunError::Docker)?;

    // 9. Stream output
    let log_file = File::create(&log_path)
        .map_err(|e| RunError::Io("creating log file".into(), e))?;
    let mut log_writer = BufWriter::new(log_file);
    let is_tty = std::io::stdout().is_terminal();

    if opts.raw {
        handle
            .stream_output(|line| {
                println!("{line}");
                let _ = writeln!(log_writer, "{line}");
            })
            .map_err(RunError::Docker)?;
    } else {
        let mut formatter = StreamFormatter::new(is_tty);
        handle
            .stream_output(|line| {
                // Always log raw JSONL
                let _ = writeln!(log_writer, "{line}");
                // Format for display
                if let Some(formatted) = stream_fmt::format_line(&mut formatter, line) {
                    println!("{formatted}");
                }
            })
            .map_err(RunError::Docker)?;
    }

    // Flush log
    let _ = log_writer.flush();

    // 10. Wait for container exit
    let exit_code = handle.wait().map_err(RunError::Docker)?;

    eprintln!();
    eprintln!("Container {container_name} finished (exit code {exit_code}).");

    // 11. Update latest symlink
    let latest_link = log_dir.join("latest.jsonl");
    let _ = fs::remove_file(&latest_link);
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(&log_filename, &latest_link);
    }

    // 12. Clean up container
    DockerBuilder::remove_container(&container_name);

    // 13. Pull changes (unless local mode or file-remote — updateInstead already synced the tree)
    if !opts.local && file_remote_path.is_none() {
        eprintln!("Pulling code changes from remote...");
        let pull_status = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "pull", "--ff-only"])
            .status();
        match pull_status {
            Ok(s) if s.success() => {}
            _ => eprintln!("No new commits to pull."),
        }
    }

    // 14. Sync litebrite again (pick up any changes)
    litebrite::init_and_sync(repo_root);

    eprintln!("Done. Log saved: {}", log_path.display());

    if exit_code != 0 {
        return Err(RunError::ContainerFailed(exit_code));
    }

    Ok(())
}

fn preflight(repo_root: &Path, local: bool) -> Result<(), RunError> {
    // Check for Docker
    let docker_check = Command::new("docker").arg("info").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    match docker_check {
        Ok(s) if s.success() => {}
        _ => return Err(RunError::Preflight("Docker is not available. Is Docker running?".into())),
    }

    // Check for credentials
    let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let has_oauth = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok();
    if !has_api_key && !has_oauth {
        return Err(RunError::Preflight(
            "No credentials found. Set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN in your environment.".into(),
        ));
    }

    // Check for clean working tree (skip in local mode — the whole point is to work on local state)
    if !local {
        let diff_status = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "diff", "--quiet"])
            .status()
            .map_err(|e| RunError::Io("checking git diff".into(), e))?;
        let cached_status = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "diff", "--cached", "--quiet"])
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

/// Returns the `origin` remote URL, or `None` if no remote is configured.
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

/// Configure the host repo to accept pushes to its checked-out branch by
/// updating the working tree in place (git's "updateInstead" policy).
fn configure_file_remote(repo_root: &Path) -> Result<(), RunError> {
    let status = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "config", "receive.denyCurrentBranch", "updateInstead"])
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

/// Write the runner entrypoint script that runs inside the container.
/// Returns a NamedTempFile that stays alive for the duration of the run.
fn write_runner_script(
    repo_root: &Path,
    model: &str,
    prompt_override: Option<&str>,
) -> Result<tempfile::NamedTempFile, RunError> {
    let prompt_text = match prompt_override {
        Some(p) => p.to_string(),
        None => prompt::load_prompt(repo_root),
    };
    // Escape single quotes for shell embedding
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

    // Make executable
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn runner_script_contains_model() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "sonnet", None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("--model sonnet"));
    }

    #[test]
    fn runner_script_contains_lb_sync() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        // Verify lb sync is called after init (not just at the end)
        let init_pos = content.find("lb init").unwrap();
        let sync_pos = content.find("lb sync 2>/dev/null || true").unwrap();
        assert!(sync_pos > init_pos, "lb sync should come after lb init");
    }

    #[test]
    fn runner_script_uses_prompt_override() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", Some("custom prompt here")).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("custom prompt here"));
    }

    #[test]
    fn runner_script_escapes_single_quotes_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", Some("don't break")).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        // Single quotes should be escaped for shell embedding
        assert!(content.contains(r"don'\''t break"));
    }

    #[test]
    fn runner_script_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None).unwrap();
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
        let tmp = write_runner_script(dir.path(), "opus", None).unwrap();
        let mut content = String::new();
        File::open(tmp.path()).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
    }
}
