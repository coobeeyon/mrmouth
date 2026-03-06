use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Default Dockerfile content used when no `.mrmouth/Dockerfile` exists.
pub const DEFAULT_DOCKERFILE: &str = r#"# Stage 1: Build litebrite (lb) — static musl binary, no glibc dependency
FROM rust:slim AS lb-builder
RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl
RUN cargo install --git https://github.com/coobeeyon/litebrite.git --target x86_64-unknown-linux-musl

# Stage 2: Runtime image — no Rust toolchain
FROM node:22

# Layer 1: System deps (changes ~never)
RUN apt-get update && apt-get install -y --no-install-recommends \
    unzip openssh-client sudo \
  && rm -rf /var/lib/apt/lists/*

# Layer 2: GitHub SSH known host (changes ~never)
RUN mkdir -p /root/.ssh && \
    ssh-keyscan github.com >> /root/.ssh/known_hosts

# Layer 3: Copy lb binary from builder
COPY --from=lb-builder /usr/local/cargo/bin/lb /usr/local/bin/lb

# Layer 4: Claude Code (changes occasionally)
RUN npm install -g @anthropic-ai/claude-code

# Layer 5: Non-root user matching host UID (for SSH agent socket access)
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

        eprintln!("Building runner container...");
        let status = Command::new("docker")
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
            .status()
            .map_err(|e| DockerError::Io("running docker build".into(), e))?;

        if !status.success() {
            return Err(DockerError::BuildFailed(status.code().unwrap_or(-1)));
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
        cmd.args(["--name", &args.name]);

        // Env file
        if args.env_file.exists() {
            cmd.args(["--env-file", &args.env_file.to_string_lossy()]);
        }

        // Env vars
        cmd.args(["-e", &format!("REPO_URL={}", args.repo_url)]);
        cmd.args(["-e", &format!("BRANCH={}", args.branch)]);

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
                    eprintln!("Timeout ({timeout_secs}s) reached — stopping container {container_name}...");
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
            name: args.name.clone(),
            watchdog_cancelled: cancelled,
        })
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
    pub env_file: std::path::PathBuf,
    pub repo_url: String,
    pub branch: String,
    pub runner_script: std::path::PathBuf,
    pub volume: String,
    pub local: bool,
    pub timeout_secs: Option<u64>,
}

pub struct ContainerHandle {
    pub child: Child,
    pub name: String,
    watchdog_cancelled: Arc<AtomicBool>,
}

impl ContainerHandle {
    /// Stream stdout line by line, calling `handler` for each line.
    /// Also captures stderr and prints it.
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

        // Spawn a thread to drain stderr
        let stderr_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    eprintln!("{line}");
                }
            }
        });

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.map_err(|e| DockerError::Io("reading container output".into(), e))?;
            handler(&line);
        }

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
