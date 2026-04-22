use std::fs::{self, File};
use std::io::{BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
    if let Some(t) = tui { t.set_stage("Agent"); }
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
    let local = opts.local || has_local_only_tooling_branch(repo_root);
    if local && !opts.local {
        logger.log("Tooling branch exists locally but not on remote — using local mode.");
    }
    let effective_dockerfile = crate::docker::effective_dockerfile_content(repo_root, &config.dockerfile);
    if preflight_skipped() {
        logger.log("MRMOUTH_SKIP_PREFLIGHT=1 — skipping preflight checks.");
    } else {
        logger.log("Checking preflight conditions...");
    }
    preflight(repo_root, local, &effective_dockerfile).inspect_err(|_| { logger.flush(); })?;

    // 2. Resolve repo URL
    let (repo_url, file_remote_path) = if local {
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
    let runner_script = write_runner_script(repo_root, &opts.model, opts.prompt_override.as_deref(), &effective_dockerfile, &config.dockerfile, Some(&logger))?;

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
        runner_script: runner_script.to_path_buf(),
        volume,
        local,
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
    if !local {
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

    if !local && file_remote_path.is_none() {
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
        // Flush so classify_exit reads everything the runner script wrote.
        logger.flush();
        let reason = classify_exit(exit_code, &log_path);
        return Err(RunError::ContainerFailed { code: exit_code, reason, log_path: log_path.clone() });
    }

    Ok(logger)
}

/// True when MRMOUTH_SKIP_PREFLIGHT=1 is set — a user-facing escape hatch for
/// overriding all preflight checks (tooling coherence, origin reachability,
/// docker, credentials, uncommitted changes, SSH agent).
fn preflight_skipped() -> bool {
    std::env::var("MRMOUTH_SKIP_PREFLIGHT").ok().as_deref() == Some("1")
}

fn preflight(repo_root: &Path, local: bool, dockerfile_content: &str) -> Result<(), RunError> {
    if preflight_skipped() {
        return Ok(());
    }

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

    check_tooling_coherence(repo_root, dockerfile_content)?;

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

        if let Some(url) = git_remote_url(repo_root) {
            if is_ssh_remote(&url) {
                check_ssh_agent()?;
            }
            check_origin_reachable(repo_root)?;
        }
    }

    Ok(())
}

/// Pairs of (branch name, binary name) for task-tracking tools that the runner
/// script invokes conditionally based on branch existence. If a tooling branch
/// exists but the Dockerfile doesn't build the binary, the runner would hit
/// exit 127 deep inside the container — catch it on the host instead.
const TOOLING_PAIRS: &[(&str, &str)] = &[("litebrite", "lb"), ("trapperkeeper", "trk")];

/// For each tooling branch present (locally or on origin), verify the effective
/// Dockerfile references the tool by name (e.g. contains "trapperkeeper"). If
/// it doesn't, the runner script's `trk init` / `lb init` call will hit exit
/// 127 deep inside the container — surface it here instead.
fn check_tooling_coherence(repo_root: &Path, dockerfile_content: &str) -> Result<(), RunError> {
    for &(branch, binary) in TOOLING_PAIRS {
        if !tooling_branch_exists(repo_root, branch) {
            continue;
        }
        if !dockerfile_content.contains(branch) {
            return Err(RunError::Preflight(format!(
                "Your Dockerfile does not build {binary}, but a {branch} branch exists. \
                 Either add the build stage (see DEFAULT_DOCKERFILE) or delete the branch. \
                 Set MRMOUTH_SKIP_PREFLIGHT=1 to override."
            )));
        }
    }
    Ok(())
}

/// Returns true if `branch` exists locally or on origin.
fn tooling_branch_exists(repo_root: &Path, branch: &str) -> bool {
    for reference in [format!("refs/heads/{branch}"), format!("refs/remotes/origin/{branch}")] {
        let ok = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "show-ref", "--quiet", &reference])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return true;
        }
    }
    false
}

/// Wall-clock timeout outcome for a subprocess invocation.
enum TimeoutOutcome {
    Completed(std::process::Output),
    TimedOut,
}

/// Run `cmd` with a wall-clock timeout, capturing stdout/stderr. On timeout,
/// the child is SIGKILL'd and reaped before returning `TimedOut`.
///
/// Note: this uses a `try_wait` polling loop, so the child's output must fit
/// inside the pipe buffer before it exits. Suitable for small-output commands
/// (git ls-remote HEAD, git rev-parse, etc.), not for streaming workloads.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<TimeoutOutcome> {
    use std::io::Read;
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let start = std::time::Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut s) = child.stdout.take() { let _ = s.read_to_end(&mut stdout); }
                if let Some(mut s) = child.stderr.take() { let _ = s.read_to_end(&mut stderr); }
                return Ok(TimeoutOutcome::Completed(std::process::Output { status, stdout, stderr }));
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(TimeoutOutcome::TimedOut);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Probe origin with `git ls-remote --exit-code origin HEAD`. On unreachable
/// origin (DNS, VPN, SSH key, network), the container would otherwise fail
/// opaquely inside `git clone`; this catches it before docker build.
fn check_origin_reachable(repo_root: &Path) -> Result<(), RunError> {
    let mut cmd = Command::new("git");
    cmd.args(["-C", &repo_root.to_string_lossy(), "ls-remote", "--exit-code", "origin", "HEAD"]);

    let outcome = run_with_timeout(cmd, Duration::from_secs(8))
        .map_err(|e| RunError::Io("running git ls-remote".into(), e))?;

    match outcome {
        TimeoutOutcome::Completed(out) if out.status.success() => Ok(()),
        TimeoutOutcome::Completed(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                format!(
                    "cannot reach origin (git ls-remote exit {})",
                    out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
                )
            } else {
                format!("cannot reach origin: {stderr}")
            };
            Err(RunError::Preflight(msg))
        }
        TimeoutOutcome::TimedOut => Err(RunError::Preflight(
            "cannot reach origin: git ls-remote did not respond within 8s (DNS, VPN, or SSH-agent issue?)".into(),
        )),
    }
}

/// True for SSH-form git remotes (git@host:... or ssh://...).
fn is_ssh_remote(url: &str) -> bool {
    url.starts_with("ssh://") || url.starts_with("git@")
}

/// Require SSH_AUTH_SOCK to be set and the socket path to exist on disk —
/// without it, the container has no way to authenticate git@ clones/pushes
/// and will either hang on a password prompt or fail with a permission error.
fn check_ssh_agent() -> Result<(), RunError> {
    let sock = std::env::var("SSH_AUTH_SOCK").ok().filter(|s| !s.is_empty());
    match sock {
        None => Err(RunError::Preflight(
            "origin is SSH (git@...) but SSH_AUTH_SOCK is not set — start an ssh-agent and ssh-add your key.".into(),
        )),
        Some(path) if !Path::new(&path).exists() => Err(RunError::Preflight(format!(
            "origin is SSH (git@...) but SSH_AUTH_SOCK={path} does not exist — start an ssh-agent and ssh-add your key."
        ))),
        Some(_) => Ok(()),
    }
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

/// Returns true if a litebrite or trapperkeeper branch exists locally but not on the remote.
fn has_local_only_tooling_branch(repo_root: &Path) -> bool {
    for branch in &["litebrite", "trapperkeeper"] {
        let local_exists = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "show-ref", "--quiet", &format!("refs/heads/{branch}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());

        if !local_exists {
            continue;
        }

        let remote_exists = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "show-ref", "--quiet", &format!("refs/remotes/origin/{branch}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());

        if !remote_exists {
            return true;
        }
    }
    false
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
    dockerfile_content: &str,
    dockerfile_path: &str,
    logger: Option<&Logger>,
) -> Result<tempfile::TempPath, RunError> {
    let prompt_text = match prompt_override {
        Some(p) => p.to_string(),
        None => prompt::load_prompt(repo_root, logger),
    };
    let escaped_prompt = prompt_text.replace('\'', "'\\''");

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
set -o errtrace
trap 'rc=$?; echo "::mrmouth::script-error rc=$rc line=$LINENO cmd=$BASH_COMMAND" >&2' ERR

# --- Tool versions (cheap, always-on diagnostic) ---
echo "::mrmouth::versions"
git --version || true
lb --version 2>/dev/null || echo "lb: not installed"
trk --version 2>/dev/null || echo "trk: not installed"
claude --version 2>/dev/null || echo "claude: not installed"
echo "::mrmouth::versions-end"

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"

# --- Clone repo (skip if workspace already mounted) ---
if [ ! -d "$work_dir/.git" ]; then
  if [ -n "$repo_url" ]; then
    # Mark the host-repo volume as safe so git clone can read it
    git config --global --add safe.directory /host-repo
    echo "Cloning $repo_url (branch: $branch)..."
    git clone --branch "$branch" "$repo_url" "$work_dir"
  else
    echo "No repo URL and no .git — starting fresh in $work_dir"
    mkdir -p "$work_dir"
  fi
fi
cd "$work_dir"
git config --global --add safe.directory "$work_dir"

# --- Seed Dockerfile if absent (gives agent a file to read and modify) ---
dockerfile_path="$work_dir/__DOCKERFILE_REL_PATH__"
if [ ! -f "$dockerfile_path" ]; then
  mkdir -p "$(dirname "$dockerfile_path")"
  cat > "$dockerfile_path" << 'MRMOUTH_DOCKERFILE_EOF'
__DOCKERFILE_CONTENT__
MRMOUTH_DOCKERFILE_EOF
  echo "Seeded Dockerfile into workspace."
fi

# --- Initialize task tooling (requires git repo with matching branches) ---
if [ -d "$work_dir/.git" ]; then
  if git show-ref --quiet refs/heads/litebrite refs/remotes/origin/litebrite 2>/dev/null; then
    command -v lb >/dev/null || {{ echo "::mrmouth::missing-tool tool=lb reason=litebrite branch exists but binary not in image" >&2; exit 64; }}
    echo "Initializing litebrite..."
    lb init
    lb setup claude 2>/dev/null || true
    lb sync 2>/dev/null || true
  fi
  if git show-ref --quiet refs/heads/trapperkeeper refs/remotes/origin/trapperkeeper 2>/dev/null; then
    command -v trk >/dev/null || {{ echo "::mrmouth::missing-tool tool=trk reason=trapperkeeper branch exists but binary not in image" >&2; exit 64; }}
    echo "Initializing trapperkeeper..."
    trk init
    trk setup claude 2>/dev/null || true
    trk sync 2>/dev/null || true
  fi
fi

# --- Restore .claude.json from persisted backup if missing ---
claude_config="$HOME/.claude.json"
if [ ! -f "$claude_config" ] && [ -d "$HOME/.claude/backups" ]; then
  latest_backup=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1 || true)
  if [ -n "$latest_backup" ]; then
    cp "$latest_backup" "$claude_config"
    echo "Restored .claude.json from backup: $(basename "$latest_backup")"
  fi
fi

# --- Run agent ---
command -v claude >/dev/null || {{ echo "::mrmouth::missing-tool tool=claude reason=claude binary not in image" >&2; exit 64; }}
echo "Starting agent run..."
CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1 claude -p --dangerously-skip-permissions --verbose --output-format stream-json --disallowedTools EnterPlanMode,ExitPlanMode --model {model} --effort max '{escaped_prompt}'

echo "Agent run complete."

# --- Belt-and-suspenders: force sync/push even if agent forgot ---
if [ -d "$work_dir/.git" ]; then
  echo "Post-agent cleanup: forcing sync and push..."
  lb sync 2>/dev/null || true
  trk sync 2>/dev/null || true
  git push 2>/dev/null || true
fi
"#
    );

    let script = script
        .replace("__DOCKERFILE_CONTENT__", dockerfile_content)
        .replace("__DOCKERFILE_REL_PATH__", dockerfile_path);

    let tmp = tempfile::NamedTempFile::new()
        .map_err(|e| RunError::Io("creating runner script".into(), e))?;

    // Close the write fd before mounting into Docker. Linux returns ETXTBSY when
    // execve() targets an inode that any process holds open for writing.
    // `into_temp_path()` closes the fd but keeps the deletion-on-drop guard.
    let tmp_path = tmp.into_temp_path();
    std::fs::write(&tmp_path, script.as_bytes())
        .map_err(|e| RunError::Io("writing runner script".into(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms)
            .map_err(|e| RunError::Io("setting runner script permissions".into(), e))?;
    }

    Ok(tmp_path)
}

#[derive(Debug)]
pub enum RunError {
    Preflight(String),
    Docker(crate::docker::DockerError),
    Io(String, std::io::Error),
    ContainerFailed { code: i32, reason: String, log_path: PathBuf },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preflight(msg) => write!(f, "preflight check failed: {msg}"),
            Self::Docker(e) => write!(f, "docker error: {e}"),
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Self::ContainerFailed { code, reason, .. } => {
                write!(f, "container exited with code {code}: {reason}")
            }
        }
    }
}

impl std::error::Error for RunError {}

impl RunError {
    /// Log file path, if this error produced one. Only ContainerFailed carries
    /// a log path today — other variants fail before or during container start.
    pub fn log_path(&self) -> Option<&Path> {
        match self {
            Self::ContainerFailed { log_path, .. } => Some(log_path),
            _ => None,
        }
    }

    /// Short one-line reason suitable for attempt-summary rendering.
    /// For container exits, prefixes with 'exit <code>'; otherwise reuses Display.
    pub fn short_reason(&self) -> String {
        match self {
            Self::ContainerFailed { code, reason, .. } => format!("exit {code} — {reason}"),
            other => other.to_string(),
        }
    }
}

/// Classify a non-zero container exit code into a human-readable cause.
/// Scans the tail of the log for `::mrmouth::` markers (emitted by the runner
/// script) and well-known error fragments before falling back to exit-code
/// heuristics.
pub fn classify_exit(code: i32, log_path: &Path) -> String {
    let tail = read_log_tail(log_path, 8192);
    classify_exit_with_tail(code, &tail)
}

fn classify_exit_with_tail(code: i32, tail: &str) -> String {
    const MISSING_TOOL: &str = "::mrmouth::missing-tool ";
    const SCRIPT_ERROR: &str = "::mrmouth::script-error ";

    // Scan from the bottom up — the most recent marker wins.
    for line in tail.lines().rev() {
        if let Some(idx) = line.find(MISSING_TOOL) {
            let rest = &line[idx + MISSING_TOOL.len()..];
            let tool = parse_marker_field(rest, "tool=");
            let reason = parse_marker_tail(rest, "reason=");
            return match (tool, reason) {
                (Some(t), Some(r)) => format!("missing tool '{t}' — {r}"),
                (Some(t), None) => format!("missing tool '{t}'"),
                _ => "missing required tool in container".into(),
            };
        }
        if let Some(idx) = line.find(SCRIPT_ERROR) {
            let rest = &line[idx + SCRIPT_ERROR.len()..];
            let line_no = parse_marker_field(rest, "line=");
            let cmd = parse_marker_tail(rest, "cmd=");
            return match (line_no, cmd) {
                (Some(l), Some(c)) => format!("runner script error at line {l}: {c}"),
                (Some(l), None) => format!("runner script error at line {l}"),
                (None, Some(c)) => format!("runner script error: {c}"),
                _ => "runner script error".into(),
            };
        }
    }

    // Pattern-based heuristics — the runner script may not have caught it,
    // but the log still has tell-tale fragments.
    if let Some(line) = tail.lines().rev().find(|l| l.contains("command not found")) {
        let snippet = line.trim();
        return format!("missing command in container — {snippet}");
    }
    if tail.contains("Killed") || tail.contains("OOMKilled") {
        return "container killed (likely OOM)".into();
    }
    if tail.contains("context deadline exceeded") {
        return "timed out (context deadline exceeded)".into();
    }

    // Code-based fallback.
    match code {
        64 => "missing required tool in container (runner script guard fired — see log)".into(),
        124 => "timed out (timeout wrapper)".into(),
        126 => "permission denied / not executable".into(),
        127 => "missing command in container (check your Dockerfile includes all expected tools)".into(),
        137 => "container killed (likely OOM)".into(),
        143 => "timed out (SIGTERM)".into(),
        _ => format!("unrecognised exit code {code}"),
    }
}

/// Read up to `n_bytes` from the end of `log_path` as a string. Returns "" on
/// any I/O error so callers can keep classifying with what they have.
fn read_log_tail(log_path: &Path, n_bytes: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(log_path) else { return String::new(); };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(n_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity(n_bytes);
    let _ = file.take(n_bytes as u64).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Extract a single-token value for `key=` from a marker payload.
/// Returns the substring up to the next whitespace.
fn parse_marker_field(payload: &str, key: &str) -> Option<String> {
    let pos = payload.find(key)?;
    let value_start = pos + key.len();
    let rest = &payload[value_start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Extract a multi-word value for `key=` that runs to the end of the payload.
/// Used for free-form fields like `reason=...` and `cmd=...`.
fn parse_marker_tail(payload: &str, key: &str) -> Option<String> {
    let pos = payload.find(key)?;
    let value_start = pos + key.len();
    Some(payload[value_start..].trim().to_string())
}

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
    use crate::docker;
    use std::io::Read as _;

    const TEST_DOCKERFILE: &str = docker::DEFAULT_DOCKERFILE;
    const TEST_DOCKERFILE_PATH: &str = ".mrmouth/Dockerfile";

    #[test]
    fn runner_script_contains_model() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "sonnet", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("--model sonnet"));
    }

    #[test]
    fn runner_script_contains_lb_sync() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        let init_pos = content.find("lb init").unwrap();
        let sync_pos = content.find("lb sync 2>/dev/null || true").unwrap();
        assert!(sync_pos > init_pos, "lb sync should come after lb init");
    }

    #[test]
    fn runner_script_uses_prompt_override() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", Some("custom prompt here"), TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("custom prompt here"));
    }

    #[test]
    fn runner_script_escapes_single_quotes_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", Some("don't break"), TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains(r"don'\''t break"));
    }

    #[test]
    fn runner_script_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&tmp).unwrap().permissions();
            assert_eq!(perms.mode() & 0o755, 0o755);
        }
    }

    #[test]
    fn runner_script_has_shebang() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn runner_script_seeds_dockerfile() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile = "FROM node:22\nRUN echo hello\nARG HOST_UID=${HOST_UID}";
        let tmp = write_runner_script(dir.path(), "opus", None, dockerfile, ".mrmouth/Dockerfile", None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("MRMOUTH_DOCKERFILE_EOF"));
        assert!(content.contains("FROM node:22"));
        assert!(content.contains("ARG HOST_UID=${HOST_UID}"), "Docker ARG syntax must be preserved literally");
        assert!(content.contains(".mrmouth/Dockerfile"));
    }

    #[test]
    fn runner_script_has_err_trap() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("set -o errtrace"));
        assert!(content.contains("::mrmouth::script-error"));
        assert!(content.contains("$LINENO"));
        assert!(content.contains("$BASH_COMMAND"));
        assert!(content.contains("' ERR"));
    }

    #[test]
    fn runner_script_echoes_tool_versions() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();

        let begin = content.find("::mrmouth::versions").expect("missing versions begin marker");
        let end = content.find("::mrmouth::versions-end").expect("missing versions end marker");
        assert!(begin < end, "versions begin must precede end");

        let block = &content[begin..end];
        assert!(block.contains("git --version"));
        assert!(block.contains("lb --version"));
        assert!(block.contains("trk --version"));
        assert!(block.contains("claude --version"));
        assert!(block.contains("lb: not installed"));
        assert!(block.contains("trk: not installed"));
        assert!(block.contains("claude: not installed"));

        // Versions must print before any tool is actually invoked for setup.
        let lb_init = content.find("lb init").expect("missing lb init");
        assert!(end < lb_init, "version echo must come before tool setup");
    }

    #[test]
    fn runner_script_guards_lb_binary() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        let guard_pos = content.find("command -v lb >/dev/null").expect("missing lb guard");
        let init_pos = content.find("lb init").expect("missing lb init");
        assert!(guard_pos < init_pos, "lb guard must precede lb init");
        assert!(content.contains("::mrmouth::missing-tool tool=lb"));
        assert!(content.contains("exit 64"));
    }

    #[test]
    fn runner_script_guards_trk_binary() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        let guard_pos = content.find("command -v trk >/dev/null").expect("missing trk guard");
        let init_pos = content.find("trk init").expect("missing trk init");
        assert!(guard_pos < init_pos, "trk guard must precede trk init");
        assert!(content.contains("::mrmouth::missing-tool tool=trk"));
    }

    #[test]
    fn runner_script_guards_claude_binary() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(dir.path(), "opus", None, TEST_DOCKERFILE, TEST_DOCKERFILE_PATH, None).unwrap();
        let mut content = String::new();
        File::open(&tmp).unwrap().read_to_string(&mut content).unwrap();
        let guard_pos = content.find("command -v claude >/dev/null").expect("missing claude guard");
        let claude_pos = content.find("claude -p --dangerously-skip-permissions").expect("missing claude invocation");
        assert!(guard_pos < claude_pos, "claude guard must precede claude invocation");
        assert!(content.contains("::mrmouth::missing-tool tool=claude"));
    }

    #[test]
    fn classify_recognises_missing_tool_marker() {
        let tail = "Initializing trapperkeeper...\n\
                    ::mrmouth::missing-tool tool=trk reason=trapperkeeper branch exists but binary not in image\n";
        let msg = classify_exit_with_tail(64, tail);
        assert!(msg.contains("missing tool 'trk'"), "got: {msg}");
        assert!(msg.contains("trapperkeeper branch exists but binary not in image"), "got: {msg}");
    }

    #[test]
    fn classify_recognises_script_error_marker() {
        let tail = "::mrmouth::script-error rc=1 line=42 cmd=git clone foo bar\n";
        let msg = classify_exit_with_tail(1, tail);
        assert!(msg.contains("line 42"), "got: {msg}");
        assert!(msg.contains("git clone foo bar"), "got: {msg}");
    }

    #[test]
    fn classify_marker_takes_precedence_over_code() {
        // Even with code 127, an explicit marker should win.
        let tail = "::mrmouth::missing-tool tool=lb reason=litebrite branch exists but binary not in image\n";
        let msg = classify_exit_with_tail(127, tail);
        assert!(msg.contains("missing tool 'lb'"), "got: {msg}");
    }

    #[test]
    fn classify_most_recent_marker_wins() {
        // If two markers appear, the one nearer the bottom (most recent) wins.
        let tail = "::mrmouth::missing-tool tool=lb reason=lb missing\n\
                    ::mrmouth::missing-tool tool=trk reason=trk missing\n";
        let msg = classify_exit_with_tail(64, tail);
        assert!(msg.contains("'trk'"), "got: {msg}");
        assert!(!msg.contains("'lb'"), "got: {msg}");
    }

    #[test]
    fn classify_falls_back_to_command_not_found_pattern() {
        let tail = "Initializing litebrite...\n\
                    /run.sh: line 30: lb: command not found\n";
        let msg = classify_exit_with_tail(127, tail);
        assert!(msg.contains("missing command in container"), "got: {msg}");
        assert!(msg.contains("command not found"), "got: {msg}");
    }

    #[test]
    fn classify_falls_back_to_exit_code() {
        assert!(classify_exit_with_tail(127, "").contains("missing command"));
        assert!(classify_exit_with_tail(137, "").contains("OOM"));
        assert!(classify_exit_with_tail(143, "").contains("SIGTERM"));
        assert!(classify_exit_with_tail(124, "").contains("timeout"));
        assert!(classify_exit_with_tail(126, "").contains("permission"));
        assert!(classify_exit_with_tail(64, "").contains("runner script guard"));
        assert!(classify_exit_with_tail(99, "").contains("99"));
    }

    #[test]
    fn classify_handles_unknown_code_with_no_log() {
        let msg = classify_exit_with_tail(42, "");
        assert!(msg.contains("42"));
    }

    #[test]
    fn classify_marker_without_fields_degrades_gracefully() {
        // Marker matched but with no recognised key=value fields.
        let tail = "::mrmouth::missing-tool unknown=junk\n";
        let msg = classify_exit_with_tail(64, tail);
        assert_eq!(msg, "missing required tool in container");
    }

    #[test]
    fn read_log_tail_returns_empty_for_missing_file() {
        let path = std::path::PathBuf::from("/nonexistent/path/to/log");
        assert_eq!(read_log_tail(&path, 1024), "");
    }

    #[test]
    fn read_log_tail_returns_last_n_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");
        std::fs::write(&path, "0123456789abcdef").unwrap();
        let tail = read_log_tail(&path, 5);
        assert_eq!(tail, "bcdef");
    }

    #[test]
    fn ssh_remote_detection() {
        assert!(is_ssh_remote("git@github.com:org/repo.git"));
        assert!(is_ssh_remote("ssh://git@github.com/org/repo.git"));
        assert!(!is_ssh_remote("https://github.com/org/repo.git"));
        assert!(!is_ssh_remote("file:///host-repo"));
        assert!(!is_ssh_remote(""));
    }

    #[test]
    fn check_ssh_agent_missing_env_errs() {
        // Serialize with other env-mutating tests to avoid flakiness under parallel cargo test.
        let _guard = env_lock();
        let prev = std::env::var_os("SSH_AUTH_SOCK");
        std::env::remove_var("SSH_AUTH_SOCK");
        let result = check_ssh_agent();
        if let Some(v) = prev { std::env::set_var("SSH_AUTH_SOCK", v); }
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("SSH_AUTH_SOCK is not set"));
    }

    #[test]
    fn check_ssh_agent_nonexistent_socket_errs() {
        let _guard = env_lock();
        let prev = std::env::var_os("SSH_AUTH_SOCK");
        std::env::set_var("SSH_AUTH_SOCK", "/tmp/definitely-not-a-real-socket-xyz-mrmouth");
        let result = check_ssh_agent();
        match prev {
            Some(v) => std::env::set_var("SSH_AUTH_SOCK", v),
            None => std::env::remove_var("SSH_AUTH_SOCK"),
        }
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not exist"), "got: {msg}");
    }

    #[test]
    fn check_ssh_agent_existing_socket_ok() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let fake_sock = dir.path().join("agent.sock");
        std::fs::write(&fake_sock, "").unwrap();
        let prev = std::env::var_os("SSH_AUTH_SOCK");
        std::env::set_var("SSH_AUTH_SOCK", &fake_sock);
        let result = check_ssh_agent();
        match prev {
            Some(v) => std::env::set_var("SSH_AUTH_SOCK", v),
            None => std::env::remove_var("SSH_AUTH_SOCK"),
        }
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn run_with_timeout_completes_fast_command() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf out; printf err >&2; exit 0"]);
        let out = match run_with_timeout(cmd, Duration::from_secs(5)).unwrap() {
            TimeoutOutcome::Completed(o) => o,
            TimeoutOutcome::TimedOut => panic!("fast command unexpectedly timed out"),
        };
        assert!(out.status.success());
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
    }

    #[test]
    #[cfg(unix)]
    fn run_with_timeout_kills_slow_command() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let start = std::time::Instant::now();
        let outcome = run_with_timeout(cmd, Duration::from_millis(300)).unwrap();
        let elapsed = start.elapsed();
        assert!(matches!(outcome, TimeoutOutcome::TimedOut));
        // Should return well before the 30s sleep completes.
        assert!(elapsed < Duration::from_secs(5), "run_with_timeout blocked for {elapsed:?}");
    }

    #[test]
    #[cfg(unix)]
    fn run_with_timeout_reports_nonzero_exit() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo boom >&2; exit 42"]);
        let out = match run_with_timeout(cmd, Duration::from_secs(5)).unwrap() {
            TimeoutOutcome::Completed(o) => o,
            TimeoutOutcome::TimedOut => panic!("unexpected timeout"),
        };
        assert_eq!(out.status.code(), Some(42));
        assert!(String::from_utf8_lossy(&out.stderr).contains("boom"));
    }

    #[cfg(unix)]
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-C", &dir.to_string_lossy()])
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git run");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    #[cfg(unix)]
    fn check_origin_reachable_succeeds_for_local_remote() {
        let origin_dir = tempfile::tempdir().unwrap();
        git(origin_dir.path(), &["init", "-q"]);
        std::fs::write(origin_dir.path().join("README"), "hi").unwrap();
        git(origin_dir.path(), &["add", "README"]);
        git(origin_dir.path(), &["commit", "-q", "-m", "init"]);

        let work_dir = tempfile::tempdir().unwrap();
        git(work_dir.path(), &["init", "-q"]);
        git(work_dir.path(), &["remote", "add", "origin", &origin_dir.path().to_string_lossy()]);

        check_origin_reachable(work_dir.path()).expect("local origin should be reachable");
    }

    #[test]
    #[cfg(unix)]
    fn check_origin_reachable_fails_for_missing_remote() {
        let work_dir = tempfile::tempdir().unwrap();
        git(work_dir.path(), &["init", "-q"]);
        git(work_dir.path(), &["remote", "add", "origin", "/nonexistent/path/mrmouth-y5z9"]);

        let err = check_origin_reachable(work_dir.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("cannot reach origin"), "got: {msg}");
    }

    #[cfg(unix)]
    fn make_repo_with_branch(branch: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("f"), "x").unwrap();
        git(dir.path(), &["add", "f"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        if let Some(b) = branch {
            git(dir.path(), &["branch", b]);
        }
        dir
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_ok_when_no_branches() {
        let dir = make_repo_with_branch(None);
        check_tooling_coherence(dir.path(), "FROM node:22\n").expect("no tooling branches → pass");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_ok_when_dockerfile_mentions_branch() {
        let dir = make_repo_with_branch(Some("trapperkeeper"));
        let dockerfile = "FROM rust\nRUN cargo install --git https://example.com/trapperkeeper.git\n";
        check_tooling_coherence(dir.path(), dockerfile).expect("dockerfile mentions trapperkeeper → pass");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_fails_when_dockerfile_missing_trk() {
        let dir = make_repo_with_branch(Some("trapperkeeper"));
        let err = check_tooling_coherence(dir.path(), "FROM node:22\n").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not build trk"), "got: {msg}");
        assert!(msg.contains("trapperkeeper branch exists"), "got: {msg}");
        assert!(msg.contains("MRMOUTH_SKIP_PREFLIGHT"), "got: {msg}");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_fails_when_dockerfile_missing_lb() {
        let dir = make_repo_with_branch(Some("litebrite"));
        let err = check_tooling_coherence(dir.path(), "FROM node:22\n").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not build lb"), "got: {msg}");
        assert!(msg.contains("litebrite branch exists"), "got: {msg}");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_passes_with_default_dockerfile() {
        // Sanity check: DEFAULT_DOCKERFILE contains both tooling words.
        let dir = make_repo_with_branch(Some("litebrite"));
        git(dir.path(), &["branch", "trapperkeeper"]);
        check_tooling_coherence(dir.path(), docker::DEFAULT_DOCKERFILE)
            .expect("default Dockerfile must satisfy coherence for both tools");
    }

    #[test]
    fn preflight_skipped_reads_env_as_one() {
        let _guard = env_lock();
        let prev = std::env::var_os("MRMOUTH_SKIP_PREFLIGHT");

        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "1");
        let skip_one = preflight_skipped();
        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "0");
        let skip_zero = preflight_skipped();
        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "true");
        let skip_true = preflight_skipped();
        std::env::remove_var("MRMOUTH_SKIP_PREFLIGHT");
        let skip_unset = preflight_skipped();

        match prev {
            Some(v) => std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", v),
            None => std::env::remove_var("MRMOUTH_SKIP_PREFLIGHT"),
        }

        assert!(skip_one, "=1 should skip");
        assert!(!skip_zero, "=0 should not skip");
        assert!(!skip_true, "=true should not skip (strict match on 1)");
        assert!(!skip_unset, "unset should not skip");
    }

    #[test]
    #[cfg(unix)]
    fn preflight_short_circuits_when_skip_env_set() {
        let _guard = env_lock();
        let dir = make_repo_with_branch(Some("trapperkeeper"));

        let prev_skip = std::env::var_os("MRMOUTH_SKIP_PREFLIGHT");
        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "1");
        // Dockerfile lacks trk, but env skip must bypass that (and all other checks).
        let result = preflight(dir.path(), true, "FROM scratch\n");
        match prev_skip {
            Some(v) => std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", v),
            None => std::env::remove_var("MRMOUTH_SKIP_PREFLIGHT"),
        }
        assert!(result.is_ok(), "skip env should bypass all preflight failures");
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }
}
