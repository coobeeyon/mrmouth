use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Default Dockerfile content used when no `.mrmouth/Dockerfile` exists.
pub const DEFAULT_DOCKERFILE: &str = r#"# Stage 1: Build litebrite (lb) — static musl binary, no glibc dependency
FROM rust:slim AS lb-builder
RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN MUSL_TARGET="$(uname -m)-unknown-linux-musl" && \
    rustup target add "$MUSL_TARGET" && \
    cargo install --git https://github.com/coobeeyon/litebrite.git --target "$MUSL_TARGET"

# Stage 2: Runtime image — no Rust toolchain
FROM node:22

# Layer 1: System deps (changes ~never)
RUN apt-get update && apt-get install -y --no-install-recommends \
    unzip openssh-client sudo curl git-lfs \
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

# Layer 4: Copy lb binary from builder
COPY --from=lb-builder /usr/local/cargo/bin/lb /usr/local/bin/lb

# Layer 5: Claude Code (changes occasionally)
RUN npm install -g @anthropic-ai/claude-code

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
    pub fn build(
        &self,
        repo_root: &Path,
        dockerfile_path: &str,
    ) -> Result<(), DockerError> {
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
            return Err(DockerError::BuildFailed(output.status.code().unwrap_or(-1)));
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
        let mut cmd = Command::new("docker");
        cmd.arg("run");
        cmd.arg("--init");
        cmd.args(["--name", &args.name]);

        // Env vars
        cmd.args(["-e", &format!("REPO_URL={}", args.repo_url)]);
        cmd.args(["-e", &format!("BRANCH={}", args.branch)]);
        for var in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
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
        cmd.args(["-v", &format!("{}:/run.sh:ro", args.runner_script.to_string_lossy())]);

        // Persistent volume for Claude memory
        cmd.args(["-v", &format!("{}:/home/runner/.claude", args.volume)]);

        // Local mode: bind-mount workspace
        if args.local {
            let cwd = std::env::current_dir()
                .map_err(|e| DockerError::Io("getting cwd".into(), e))?;
            cmd.args(["-v", &format!("{}:/home/runner/workspace", cwd.to_string_lossy())]);
        }

        // File-remote mode: mount host repo as a git remote the container can clone from and push to
        if let Some(ref path) = args.file_remote_path {
            cmd.args(["-v", &format!("{}:/host-repo", path.to_string_lossy())]);
        }

        cmd.arg(&self.image_name);
        cmd.arg("/run.sh");

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

    /// Copy a file from a stopped container to a local path (best-effort).
    /// Returns true if the copy succeeded.
    pub fn copy_from_container(container_name: &str, container_path: &str, local_path: &Path) -> bool {
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
}

pub struct ContainerArgs {
    pub name: String,
    pub repo_url: String,
    pub branch: String,
    pub runner_script: std::path::PathBuf,
    pub volume: String,
    pub local: bool,
    pub file_remote_path: Option<std::path::PathBuf>,
    pub timeout_secs: Option<u64>,
}

pub struct ContainerHandle {
    pub child: Child,
    watchdog_cancelled: Arc<AtomicBool>,
}

impl ContainerHandle {
    /// Stream stdout line by line, calling `handler` for each line.
    /// Container stderr is buffered and then passed through the same handler
    /// after stdout closes, so it reaches the TUI/logger rather than bypassing it.
    pub fn stream_output<F>(&mut self, mut handler: F) -> Result<(), DockerError>
    where
        F: FnMut(&str),
    {
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or(DockerError::NoStdout)?;
        let stderr = self
            .child
            .stderr
            .take()
            .ok_or(DockerError::NoStderr)?;

        // Buffer stderr on a background thread (handler is not Send)
        let stderr_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf_clone = Arc::clone(&stderr_buf);
        let stderr_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut v) = stderr_buf_clone.lock() {
                    v.push(line);
                }
            }
        });

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.map_err(|e| DockerError::Io("reading container output".into(), e))?;
            handler(&line);
        }

        let _ = stderr_handle.join();

        // Route buffered stderr through the same handler so it appears in the TUI/log
        if let Ok(lines) = stderr_buf.lock() {
            for line in lines.iter() {
                handler(line);
            }
        }

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
    BuildFailed(i32),
    NoStdout,
    NoStderr,
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Self::BuildFailed(code) => write!(f, "docker build failed (exit code {code})"),
            Self::NoStdout => write!(f, "failed to capture container stdout"),
            Self::NoStderr => write!(f, "failed to capture container stderr"),
        }
    }
}

impl std::error::Error for DockerError {}
