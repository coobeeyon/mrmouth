use std::path::Path;
use std::process::{Command, Stdio};

use crate::agent::AgentKind;
use crate::config::Config;
use crate::docker::{DockerBuilder, DockerError};

pub fn execute(config: &Config, repo_root: &Path) -> Result<(), CodexLoginError> {
    let docker = DockerBuilder::new(&config.image);

    eprintln!("Building mrmouth runner image...");
    docker.build(repo_root, &config.dockerfile)?;

    let volume = config.effective_volume_for_agent(repo_root, AgentKind::Codex);
    docker.ensure_volume(&volume)?;

    eprintln!("Starting Codex device auth in Docker.");
    eprintln!("Follow the URL/code printed by Codex, then return here when login completes.");

    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-it",
            "--init",
            "-v",
            &format!("{volume}:{}", AgentKind::Codex.home_mount()),
            &config.image,
            "-lc",
            "codex login --device-auth && codex login status",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CodexLoginError::Io("running docker codex login".into(), e))?;

    if !status.success() {
        return Err(CodexLoginError::Failed(status.code().unwrap_or(-1)));
    }

    eprintln!("Codex login saved in Docker volume: {volume}");
    Ok(())
}

#[derive(Debug)]
pub enum CodexLoginError {
    Docker(DockerError),
    Io(String, std::io::Error),
    Failed(i32),
}

impl std::fmt::Display for CodexLoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Docker(e) => write!(f, "docker error: {e}"),
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Self::Failed(code) => write!(f, "codex login container exited with code {code}"),
        }
    }
}

impl std::error::Error for CodexLoginError {}

impl From<DockerError> for CodexLoginError {
    fn from(value: DockerError) -> Self {
        Self::Docker(value)
    }
}

impl CodexLoginError {
    pub fn debrief(&self) -> crate::debrief::FailureDebrief {
        crate::debrief::FailureDebrief::new(self.to_string())
    }
}
