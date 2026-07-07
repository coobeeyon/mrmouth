use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use crate::repo_layout::{BOOKKEEPING_CONTAINER_PATH, WORK_CONTAINER_PATH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFromContainerOutcome {
    Updated,
    Unchanged,
    Missing,
}

/// Default Dockerfile content used when no `.mrmouth/Dockerfile` exists.
pub const DEFAULT_DOCKERFILE: &str = r#"# Stage 1: Build litebrite (lb) and trapperkeeper (trk) — static musl binaries
FROM rust:slim AS tools-builder
RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN MUSL_TARGET="$(uname -m)-unknown-linux-musl" && \
    rustup target add "$MUSL_TARGET" && \
    cargo install --git https://github.com/coobeeyon/litebrite.git --target "$MUSL_TARGET" && \
    cargo install --git https://github.com/coobeeyon/trapperkeeper.git --target "$MUSL_TARGET"

# Stage 2: Runtime image
FROM node:22

# Layer 1: System deps (changes ~never)
RUN apt-get update && apt-get install -y --no-install-recommends \
    unzip openssh-client sudo curl git-lfs ripgrep cargo \
  && git lfs install \
  && rm -rf /var/lib/apt/lists/*

# Layer 2: GitHub CLI (changes occasionally)
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg && \
    chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      | tee /etc/apt/sources.list.d/github-cli.list > /dev/null && \
    apt-get update && apt-get install -y --no-install-recommends gh && \
    rm -rf /var/lib/apt/lists/*

# Layer 3: GitHub SSH known host (changes ~never)
RUN mkdir -p /root/.ssh && \
    ssh-keyscan github.com >> /root/.ssh/known_hosts

# Layer 4: Copy tool binaries from builder
COPY --from=tools-builder /usr/local/cargo/bin/lb /usr/local/bin/lb
COPY --from=tools-builder /usr/local/cargo/bin/trk /usr/local/bin/trk

# Layer 5: Agent CLIs (changes occasionally)
RUN npm install -g @anthropic-ai/claude-code @openai/codex

# Layer 6: Non-root user matching host UID (for SSH agent socket access)
ARG HOST_UID=1000
ARG HOST_GID=1000
RUN userdel -r node 2>/dev/null || true && \
    groupadd -g ${HOST_GID} runner 2>/dev/null || true && \
    useradd -m -s /bin/bash -u ${HOST_UID} -g ${HOST_GID} runner && \
    echo "runner ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/runner && \
    cp -r /root/.ssh /home/runner/.ssh && \
    chown -R runner:runner /home/runner/.ssh
USER runner
ENV HOME=/home/runner
RUN git config --global user.name "agent-runner" && \
    git config --global user.email "agent-runner@local"

ENTRYPOINT ["bash"]
"#;

/// Returns the effective Dockerfile content: reads from `repo_root/dockerfile_path`
/// if it exists, otherwise returns DEFAULT_DOCKERFILE.
pub fn effective_dockerfile_content(repo_root: &Path, dockerfile_path: &str) -> String {
    let dockerfile = repo_root.join(dockerfile_path);
    if dockerfile.exists() {
        if let Ok(content) = std::fs::read_to_string(&dockerfile) {
            return content;
        }
    }
    DEFAULT_DOCKERFILE.to_string()
}

pub struct DockerBuilder {
    image_name: String,
}

impl DockerBuilder {
    pub fn new(image_name: &str) -> Self {
        Self {
            image_name: image_name.to_string(),
        }
    }

    /// Build the Docker image. Uses the configured Dockerfile path, falling back
    /// to a built-in default if it doesn't exist.
    pub fn build(&self, repo_root: &Path, dockerfile_path: &str) -> Result<(), DockerError> {
        let dockerfile = repo_root.join(dockerfile_path);

        // If no Dockerfile exists, write the default to a temp file
        let (actual_dockerfile, _tempfile) = if dockerfile.exists() {
            (dockerfile, None)
        } else {
            let tmp = tempfile::NamedTempFile::new()
                .map_err(|e| DockerError::Io("creating temp Dockerfile".into(), e))?;
            std::fs::write(tmp.path(), DEFAULT_DOCKERFILE)
                .map_err(|e| DockerError::Io("writing temp Dockerfile".into(), e))?;
            let path = tmp.path().to_path_buf();
            (path, Some(tmp))
        };

        let uid = get_uid();
        let gid = get_gid();

        let output = Command::new("docker")
            .args([
                "build",
                "-q",
                "-t",
                &self.image_name,
                "--build-arg",
                &format!("HOST_UID={uid}"),
                "--build-arg",
                &format!("HOST_GID={gid}"),
                "-f",
                &actual_dockerfile.to_string_lossy(),
                &repo_root.to_string_lossy(),
            ])
            .output()
            .map_err(|e| DockerError::Io("running docker build".into(), e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(DockerError::BuildFailed(
                output.status.code().unwrap_or(-1),
                stderr,
            ));
        }

        Ok(())
    }

    /// Create and ensure the persistent volume exists.
    pub fn ensure_volume(&self, volume_name: &str) -> Result<(), DockerError> {
        let _ = Command::new("docker")
            .args(["volume", "create", volume_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        // Fix ownership
        let uid = get_uid();
        let gid = get_gid();
        let _ = Command::new("docker")
            .args([
                "run",
                "--rm",
                "-v",
                &format!("{volume_name}:/data"),
                "alpine",
                "chown",
                &format!("{uid}:{gid}"),
                "/data",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        Ok(())
    }

    /// Start the container and return a handle for streaming output.
    pub fn run(&self, args: &ContainerArgs) -> Result<ContainerHandle, DockerError> {
        let mut cmd = self.run_command(args)?;
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| DockerError::Io("spawning docker run".into(), e))?;

        // Spawn a watchdog thread that stops the container after the timeout
        let cancelled = Arc::new(AtomicBool::new(false));
        if let Some(timeout_secs) = args.timeout_secs {
            let container_name = args.name.clone();
            let cancelled_clone = Arc::clone(&cancelled);
            std::thread::spawn(move || {
                // Sleep in 1-second increments so we can check for cancellation
                for _ in 0..timeout_secs {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    if cancelled_clone.load(Ordering::Relaxed) {
                        return;
                    }
                }
                if !cancelled_clone.load(Ordering::Relaxed) {
                    let _ = Command::new("docker")
                        .args(["stop", &container_name])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            });
        }

        Ok(ContainerHandle {
            child,
            watchdog_cancelled: cancelled,
        })
    }

    fn run_command(&self, args: &ContainerArgs) -> Result<Command, DockerError> {
        let mut cmd = Command::new("docker");
        cmd.arg("run");
        cmd.arg("--init");
        cmd.args(["--name", &args.name]);

        // Env vars
        cmd.args(["-e", &format!("REPO_URL={}", args.repo_url)]);
        cmd.args(["-e", &format!("BRANCH={}", args.branch)]);
        cmd.args([
            "-e",
            &format!("MRMOUTH_BOOKKEEPING_REPO={BOOKKEEPING_CONTAINER_PATH}"),
        ]);
        let work_repo = if args.worktree_path.is_some() {
            WORK_CONTAINER_PATH
        } else {
            BOOKKEEPING_CONTAINER_PATH
        };
        cmd.args(["-e", &format!("MRMOUTH_WORK_REPO={work_repo}")]);
        for var in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd.args(["-e", &format!("{var}={val}")]);
            }
        }

        // SSH agent
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            cmd.args(["-v", &format!("{sock}:/ssh-agent")]);
            cmd.args(["-e", "SSH_AUTH_SOCK=/ssh-agent"]);
        }

        // Mount runner script
        cmd.args([
            "-v",
            &format!("{}:/run.sh:ro", args.runner_script.to_string_lossy()),
        ]);

        // Persistent volume for agent memory/auth state
        cmd.args(["-v", &format!("{}:{}", args.volume, args.agent_home)]);

        // Local mode: bind-mount workspace
        if args.local {
            let cwd = match &args.local_workspace_path {
                Some(path) => path.clone(),
                None => {
                    std::env::current_dir().map_err(|e| DockerError::Io("getting cwd".into(), e))?
                }
            };
            cmd.args([
                "-v",
                &format!("{}:{BOOKKEEPING_CONTAINER_PATH}", cwd.to_string_lossy()),
            ]);
        }

        if let Some(ref path) = args.worktree_path {
            cmd.args([
                "-v",
                &format!("{}:{WORK_CONTAINER_PATH}", path.to_string_lossy()),
            ]);
            cmd.args(["-e", &format!("MRMOUTH_WORKTREE={WORK_CONTAINER_PATH}")]);
        }

        // File-remote mode: mount host repo as a git remote the container can clone from and push to
        if let Some(ref path) = args.file_remote_path {
            cmd.args(["-v", &format!("{}:/host-repo", path.to_string_lossy())]);
        }
        for mount in &args.local_remote_mounts {
            cmd.args([
                "-v",
                &format!(
                    "{}:{}",
                    mount.host_path.to_string_lossy(),
                    mount.container_path
                ),
            ]);
        }

        cmd.arg(&self.image_name);
        cmd.arg("/run.sh");

        Ok(cmd)
    }

    /// Copy a file from a stopped container to a local path (best-effort).
    /// Returns true if the copy succeeded.
    pub fn copy_from_container(
        container_name: &str,
        container_path: &str,
        local_path: &Path,
    ) -> bool {
        let status = Command::new("docker")
            .args([
                "cp",
                &format!("{container_name}:{container_path}"),
                &local_path.to_string_lossy(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        matches!(status, Ok(s) if s.success())
    }

    /// Copy a file from a stopped container only when its bytes differ from the
    /// existing local file. The local file is left untouched when content is
    /// identical, avoiding accidental worktree dirtiness before/after a pull.
    pub fn copy_from_container_if_changed(
        container_name: &str,
        container_path: &str,
        local_path: &Path,
    ) -> io::Result<CopyFromContainerOutcome> {
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_dir = match local_path.parent() {
            Some(parent) => tempfile::Builder::new()
                .prefix(".mrmouth-copy-")
                .tempdir_in(parent)?,
            None => tempfile::tempdir()?,
        };
        let temp_path = temp_dir.path().join("container-file");

        if !Self::copy_from_container(container_name, container_path, &temp_path) {
            return Ok(CopyFromContainerOutcome::Missing);
        }

        let container_bytes = fs::read(&temp_path)?;
        if fs::read(local_path).is_ok_and(|local_bytes| local_bytes == container_bytes) {
            return Ok(CopyFromContainerOutcome::Unchanged);
        }

        fs::rename(temp_path, local_path)?;
        Ok(CopyFromContainerOutcome::Updated)
    }

    /// Stop a running container by name (best-effort).
    pub fn stop_container(name: &str) {
        let _ = Command::new("docker")
            .args(["stop", "-t", "5", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Remove a container by name (best-effort).
    pub fn remove_container(name: &str) {
        let _ = Command::new("docker")
            .args(["rm", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Return the current image ID (sha256) for this builder's image, if it exists.
    /// Used to detect whether a rebuild produced a new image (Dockerfile change).
    pub fn image_id(&self) -> Option<String> {
        let output = Command::new("docker")
            .args(["images", "-q", "--no-trunc", &self.image_name])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    /// Start a long-lived detached container that stays alive until explicitly
    /// stopped. The container's default process is `tail -f /dev/null`, leaving
    /// the environment ready for `exec_script` to run individual task scripts.
    /// Mounts `scripts_dir` at `/mrmouth-scripts:ro` so the host can update
    /// scripts between tasks without restarting the container.
    pub fn start_session(&self, args: &SessionArgs) -> Result<(), DockerError> {
        let mut cmd = self.start_session_command(args)?;
        let output = cmd
            .output()
            .map_err(|e| DockerError::Io("running docker run -d".into(), e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(DockerError::SessionStartFailed(
                output.status.code().unwrap_or(-1),
                stderr,
            ));
        }

        Ok(())
    }

    fn start_session_command(&self, args: &SessionArgs) -> Result<Command, DockerError> {
        let mut cmd = Command::new("docker");
        cmd.arg("run");
        cmd.arg("-d");
        cmd.arg("--init");
        cmd.args(["--name", &args.name]);

        // Env vars that don't change per task — set once at session start.
        cmd.args(["-e", &format!("REPO_URL={}", args.repo_url)]);
        cmd.args([
            "-e",
            &format!("MRMOUTH_BOOKKEEPING_REPO={BOOKKEEPING_CONTAINER_PATH}"),
        ]);
        let work_repo = if args.worktree_path.is_some() {
            WORK_CONTAINER_PATH
        } else {
            BOOKKEEPING_CONTAINER_PATH
        };
        cmd.args(["-e", &format!("MRMOUTH_WORK_REPO={work_repo}")]);
        for var in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd.args(["-e", &format!("{var}={val}")]);
            }
        }

        // SSH agent
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            cmd.args(["-v", &format!("{sock}:/ssh-agent")]);
            cmd.args(["-e", "SSH_AUTH_SOCK=/ssh-agent"]);
        }

        // Scripts directory (bind-mount so host can rewrite task.sh between tasks).
        cmd.args([
            "-v",
            &format!("{}:/mrmouth-scripts:ro", args.scripts_dir.to_string_lossy()),
        ]);

        // Persistent volume for agent memory/auth state
        cmd.args(["-v", &format!("{}:{}", args.volume, args.agent_home)]);

        // Local mode: bind-mount workspace
        if args.local {
            let cwd = match &args.local_workspace_path {
                Some(path) => path.clone(),
                None => {
                    std::env::current_dir().map_err(|e| DockerError::Io("getting cwd".into(), e))?
                }
            };
            cmd.args([
                "-v",
                &format!("{}:{BOOKKEEPING_CONTAINER_PATH}", cwd.to_string_lossy()),
            ]);
        }

        if let Some(ref path) = args.worktree_path {
            cmd.args([
                "-v",
                &format!("{}:{WORK_CONTAINER_PATH}", path.to_string_lossy()),
            ]);
            cmd.args(["-e", &format!("MRMOUTH_WORKTREE={WORK_CONTAINER_PATH}")]);
        }

        // File-remote mode
        if let Some(ref path) = args.file_remote_path {
            cmd.args(["-v", &format!("{}:/host-repo", path.to_string_lossy())]);
        }
        for mount in &args.local_remote_mounts {
            cmd.args([
                "-v",
                &format!(
                    "{}:{}",
                    mount.host_path.to_string_lossy(),
                    mount.container_path
                ),
            ]);
        }

        cmd.arg(&self.image_name);
        cmd.args(["-c", "tail -f /dev/null"]);

        Ok(cmd)
    }

    /// Exec a script that lives at `/mrmouth-scripts/<script_name>` inside the
    /// running session container, streaming output via the returned handle.
    /// `env_vars` are set only for this exec.
    ///
    /// On `timeout_secs` deadline, sends SIGTERM to the host-side `docker exec`
    /// process (docker forwards it to the in-container process), then escalates
    /// to SIGKILL after a 5s grace period if the exec hasn't exited. The session
    /// container itself is left running so the caller can exec another script —
    /// critical for session-reuse in epic mode. A SIGKILL-on-host can orphan the
    /// container-side process, but that's a rare tail case acceptable in return
    /// for preserving the session.
    pub fn exec_script(
        container_name: &str,
        script_name: &str,
        env_vars: &[(String, String)],
        timeout_secs: Option<u64>,
    ) -> Result<ContainerHandle, DockerError> {
        let mut cmd = Command::new("docker");
        cmd.arg("exec");
        for (k, v) in env_vars {
            cmd.args(["-e", &format!("{k}={v}")]);
        }
        cmd.arg(container_name);
        cmd.arg("bash");
        cmd.arg(format!("/mrmouth-scripts/{script_name}"));

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| DockerError::Io("spawning docker exec".into(), e))?;

        let cancelled = Arc::new(AtomicBool::new(false));
        if let Some(timeout_secs) = timeout_secs {
            let cancelled_clone = Arc::clone(&cancelled);
            let pid = child.id();
            std::thread::spawn(move || {
                for _ in 0..timeout_secs {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    if cancelled_clone.load(Ordering::Relaxed) {
                        return;
                    }
                }
                #[cfg(unix)]
                {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
                // Grace period — SIGTERM may need a moment to propagate through
                // docker exec to the in-container process and for it to clean up.
                for _ in 0..5 {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    if cancelled_clone.load(Ordering::Relaxed) {
                        return;
                    }
                }
                #[cfg(unix)]
                {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                }
            });
        }

        Ok(ContainerHandle {
            child,
            watchdog_cancelled: cancelled,
        })
    }
}

pub struct ContainerArgs {
    pub name: String,
    pub repo_url: String,
    pub branch: String,
    pub runner_script: PathBuf,
    pub volume: String,
    pub agent_home: &'static str,
    pub local: bool,
    pub local_workspace_path: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub file_remote_path: Option<PathBuf>,
    pub local_remote_mounts: Vec<LocalRemoteMount>,
    pub timeout_secs: Option<u64>,
}

/// Arguments for a long-lived session container (detached, reused across tasks).
/// Unlike `ContainerArgs`, branch/timeout/script-path are per-task rather than
/// per-container, so they're passed to `exec_script` instead.
pub struct SessionArgs {
    pub name: String,
    pub repo_url: String,
    pub scripts_dir: PathBuf,
    pub volume: String,
    pub agent_home: &'static str,
    pub local: bool,
    pub local_workspace_path: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub file_remote_path: Option<PathBuf>,
    pub local_remote_mounts: Vec<LocalRemoteMount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRemoteMount {
    pub host_path: PathBuf,
    pub container_path: String,
    pub rewrite_urls: Vec<String>,
}

pub struct ContainerHandle {
    pub child: Child,
    watchdog_cancelled: Arc<AtomicBool>,
}

impl ContainerHandle {
    /// Stream stdout and stderr line by line, interleaved in arrival order,
    /// calling `handler` for each line. Both streams feed a single mpsc channel
    /// so errors appear in the TUI/log when they happen, not after stdout closes.
    pub fn stream_output<F>(&mut self, mut handler: F) -> Result<(), DockerError>
    where
        F: FnMut(&str),
    {
        let stdout = self.child.stdout.take().ok_or(DockerError::NoStdout)?;
        let stderr = self.child.stderr.take().ok_or(DockerError::NoStderr)?;

        let (tx, rx) = mpsc::channel::<String>();
        let tx_stdout = tx.clone();
        let tx_stderr = tx;

        let stdout_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if tx_stdout.send(line).is_err() {
                    break;
                }
            }
        });

        let stderr_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if tx_stderr.send(line).is_err() {
                    break;
                }
            }
        });

        // Drain the channel on the main thread. The handler is not Send,
        // so it must run here. The loop exits when both sender clones drop,
        // which happens when both reader threads finish.
        for line in rx {
            handler(&line);
        }

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();

        Ok(())
    }

    /// Wait for the container to exit and return its exit code.
    /// Cancels the timeout watchdog once the container exits.
    pub fn wait(&mut self) -> Result<i32, DockerError> {
        let status = self
            .child
            .wait()
            .map_err(|e| DockerError::Io("waiting for container".into(), e))?;
        self.watchdog_cancelled.store(true, Ordering::Relaxed);
        Ok(status.code().unwrap_or(-1))
    }
}

fn get_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        1000
    }
}

fn get_gid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getgid() }
    }
    #[cfg(not(unix))]
    {
        1000
    }
}

#[derive(Debug)]
pub enum DockerError {
    Io(String, std::io::Error),
    BuildFailed(i32, String),
    SessionStartFailed(i32, String),
    NoStdout,
    NoStderr,
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Self::BuildFailed(code, stderr) => {
                write!(f, "docker build failed (exit code {code})")?;
                let stderr = stderr.trim();
                if !stderr.is_empty() {
                    write!(f, "\n{stderr}")?;
                }
                Ok(())
            }
            Self::SessionStartFailed(code, stderr) => {
                write!(f, "docker run -d (session) failed (exit code {code})")?;
                let stderr = stderr.trim();
                if !stderr.is_empty() {
                    write!(f, "\n{stderr}")?;
                }
                Ok(())
            }
            Self::NoStdout => write!(f, "failed to capture container stdout"),
            Self::NoStderr => write!(f, "failed to capture container stderr"),
        }
    }
}

impl std::error::Error for DockerError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn run_command_mounts_worktree_without_replacing_default_workspace_clone() {
        let docker = DockerBuilder::new("mrmouth-test");
        let container_args = ContainerArgs {
            name: "run-test".to_string(),
            repo_url: "git@example.com:org/repo.git".to_string(),
            branch: "feature".to_string(),
            runner_script: PathBuf::from("/tmp/run.sh"),
            volume: "mrmouth-home".to_string(),
            agent_home: "/home/runner/.codex",
            local: false,
            local_workspace_path: None,
            worktree_path: Some(PathBuf::from("/host/service")),
            file_remote_path: None,
            local_remote_mounts: Vec::new(),
            timeout_secs: None,
        };

        let cmd = docker.run_command(&container_args).unwrap();
        let args = args(&cmd);

        assert!(args.contains(&"REPO_URL=git@example.com:org/repo.git".to_string()));
        assert!(args.contains(&"BRANCH=feature".to_string()));
        assert!(args.contains(&"/host/service:/home/runner/worktree".to_string()));
        assert!(args.contains(&"MRMOUTH_WORKTREE=/home/runner/worktree".to_string()));
        assert!(!args
            .iter()
            .any(|arg| arg.ends_with(":/home/runner/workspace")));
        assert_eq!(args.last().map(String::as_str), Some("/run.sh"));
    }

    #[test]
    fn session_command_mounts_worktree_without_replacing_default_workspace_clone() {
        let docker = DockerBuilder::new("mrmouth-test");
        let session_args = SessionArgs {
            name: "session-test".to_string(),
            repo_url: "git@example.com:org/repo.git".to_string(),
            scripts_dir: PathBuf::from("/tmp/scripts"),
            volume: "mrmouth-home".to_string(),
            agent_home: "/home/runner/.codex",
            local: false,
            local_workspace_path: None,
            worktree_path: Some(PathBuf::from("/host/service")),
            file_remote_path: None,
            local_remote_mounts: Vec::new(),
        };

        let cmd = docker.start_session_command(&session_args).unwrap();
        let args = args(&cmd);

        assert!(args.contains(&"REPO_URL=git@example.com:org/repo.git".to_string()));
        assert!(args.contains(&"/host/service:/home/runner/worktree".to_string()));
        assert!(args.contains(&"MRMOUTH_WORKTREE=/home/runner/worktree".to_string()));
        assert!(!args
            .iter()
            .any(|arg| arg.ends_with(":/home/runner/workspace")));
        assert_eq!(args.last().map(String::as_str), Some("tail -f /dev/null"));
    }

    #[test]
    fn local_mode_still_mounts_workspace_at_default_path() {
        let docker = DockerBuilder::new("mrmouth-test");
        let container_args = ContainerArgs {
            name: "run-test".to_string(),
            repo_url: String::new(),
            branch: "feature".to_string(),
            runner_script: PathBuf::from("/tmp/run.sh"),
            volume: "mrmouth-home".to_string(),
            agent_home: "/home/runner/.codex",
            local: true,
            local_workspace_path: Some(PathBuf::from("/host/tracking")),
            worktree_path: None,
            file_remote_path: None,
            local_remote_mounts: Vec::new(),
            timeout_secs: None,
        };

        let cmd = docker.run_command(&container_args).unwrap();
        let args = args(&cmd);

        assert!(args.contains(&"/host/tracking:/home/runner/workspace".to_string()));
        assert!(!args
            .iter()
            .any(|arg| arg.ends_with(":/home/runner/worktree")));
        assert!(!args.contains(&"MRMOUTH_WORKTREE=/home/runner/worktree".to_string()));
    }

    #[test]
    fn run_command_mounts_extra_local_remote_paths() {
        let docker = DockerBuilder::new("mrmouth-test");
        let container_args = ContainerArgs {
            name: "run-test".to_string(),
            repo_url: "file:///host-repo".to_string(),
            branch: "feature".to_string(),
            runner_script: PathBuf::from("/tmp/run.sh"),
            volume: "mrmouth-home".to_string(),
            agent_home: "/home/runner/.codex",
            local: false,
            local_workspace_path: None,
            worktree_path: Some(PathBuf::from("/host/service")),
            file_remote_path: Some(PathBuf::from("/host/tracking-remote")),
            local_remote_mounts: vec![LocalRemoteMount {
                host_path: PathBuf::from("/host/service-remote"),
                container_path: "/host-worktree-origin".to_string(),
                rewrite_urls: vec!["/tmp/service-remote".to_string()],
            }],
            timeout_secs: None,
        };

        let cmd = docker.run_command(&container_args).unwrap();
        let args = args(&cmd);

        assert!(args.contains(&"/host/tracking-remote:/host-repo".to_string()));
        assert!(args.contains(&"/host/service-remote:/host-worktree-origin".to_string()));
    }
}
