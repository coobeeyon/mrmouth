use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::agent::AgentKind;

const CONFIG_DIR: &str = ".mrmouth";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentKind,
    pub model: String,
    pub image: String,
    pub dockerfile: String,
    pub volume: Option<String>,
    pub log_dir: String,
    pub branch: Option<String>,
    #[serde(rename = "loop")]
    pub loop_config: LoopConfig,
    #[serde(rename = "epic")]
    pub do_config: DoConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct LoopConfig {
    pub delay: u32,
    pub max_runs: u32,
    pub decider_model: String,
    pub summary_model: String,
    pub reviewer_model: String,
    pub shipper_model: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DoConfig {
    pub timeout: u32,
    pub max_failures: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: AgentKind::Claude,
            model: "opus".into(),
            image: "mrmouth-runner".into(),
            dockerfile: ".mrmouth/Dockerfile".into(),
            volume: None,
            log_dir: "logs".into(),
            branch: None,
            loop_config: LoopConfig::default(),
            do_config: DoConfig::default(),
        }
    }
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            delay: 0,
            max_runs: 0,
            decider_model: "sonnet".into(),
            summary_model: "haiku".into(),
            reviewer_model: "sonnet".into(),
            shipper_model: "sonnet".into(),
        }
    }
}

impl Default for DoConfig {
    fn default() -> Self {
        Self {
            timeout: 15,
            max_failures: 3,
        }
    }
}

impl Config {
    /// Returns a model suitable for the configured agent.
    ///
    /// The built-in role defaults are Claude model aliases. When Codex is the
    /// active agent, omitting the model lets Codex use its configured default
    /// instead of passing an invalid Claude alias such as `sonnet`.
    pub fn effective_model_for_agent(&self, model: &str) -> String {
        if self.agent == AgentKind::Codex && is_claude_model_alias(model) {
            return String::new();
        }
        model.to_string()
    }

    /// Returns the effective volume name: user-configured value, or the
    /// provider default. Claude keeps the historical per-repo default; Codex
    /// uses a shared machine-wide default so one login works across repos.
    pub fn effective_volume(&self, repo_root: &Path) -> String {
        self.effective_volume_for_agent(repo_root, self.agent)
    }

    pub fn effective_volume_for_agent(&self, repo_root: &Path, agent: AgentKind) -> String {
        if let Some(ref v) = self.volume {
            return v.clone();
        }
        match agent {
            AgentKind::Claude => {
                let slug = repo_slug(repo_root);
                format!("{}-{slug}", agent.default_volume_prefix())
            }
            AgentKind::Codex => agent.default_volume_prefix().to_string(),
        }
    }

    /// Load config from `.mrmouth/config.toml` relative to `repo_root`.
    /// Returns defaults if the file doesn't exist.
    pub fn load(repo_root: &Path) -> Result<Self, ConfigError> {
        let config_path = repo_root.join(CONFIG_DIR).join(CONFIG_FILE);

        if !config_path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&config_path).map_err(|e| ConfigError::Read {
            path: config_path.clone(),
            source: e,
        })?;

        let config: Self = toml::from_str(&contents).map_err(|e| ConfigError::Parse {
            path: config_path,
            source: e,
        })?;

        Ok(config)
    }

    /// Find the repo root by searching upward for a `.git` directory.
    pub fn find_repo_root() -> Result<PathBuf, ConfigError> {
        let mut dir = std::env::current_dir().map_err(ConfigError::Cwd)?;
        loop {
            if dir.join(".git").exists() {
                return Ok(dir);
            }
            if !dir.pop() {
                return Err(ConfigError::NotARepo);
            }
        }
    }

    /// Find the repo root, falling back to cwd if no `.git` is found.
    /// Used in local/bootstrap mode where a git repo may not exist yet.
    pub fn find_repo_root_or_cwd() -> Result<PathBuf, ConfigError> {
        match Self::find_repo_root() {
            Ok(root) => Ok(root),
            Err(ConfigError::NotARepo) => std::env::current_dir().map_err(ConfigError::Cwd),
            Err(e) => Err(e),
        }
    }
}

fn repo_slug(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn is_claude_model_alias(model: &str) -> bool {
    matches!(model.trim(), "opus" | "sonnet" | "haiku")
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    Cwd(std::io::Error),
    NotARepo,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read {}: {}", path.display(), source)
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse {}: {}", path.display(), source)
            }
            Self::Cwd(e) => write!(f, "failed to get current directory: {e}"),
            Self::NotARepo => write!(f, "not inside a git repository"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn defaults_when_no_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.agent, AgentKind::Claude);
        assert_eq!(config.model, "opus");
        assert_eq!(config.image, "mrmouth-runner");
        assert_eq!(config.loop_config.decider_model, "sonnet");
        assert_eq!(config.do_config.timeout, 15);
    }

    #[test]
    fn partial_config_uses_defaults_for_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join(".mrmouth");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join("config.toml"), "model = \"sonnet\"\n").unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.model, "sonnet");
        assert_eq!(config.image, "mrmouth-runner"); // default
        assert_eq!(config.log_dir, "logs"); // default
    }

    #[test]
    fn full_config_parses() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join(".mrmouth");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.toml"),
            r#"
agent = "codex"
model = "haiku"
image = "my-image"
dockerfile = "custom/Dockerfile"
volume = "my-vol"
log_dir = "my-logs"
branch = "dev"

[loop]
delay = 5
max_runs = 10
decider_model = "opus"
summary_model = "sonnet"
reviewer_model = "haiku"
shipper_model = "opus"

[epic]
timeout = 30
max_failures = 5
"#,
        )
        .unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.agent, AgentKind::Codex);
        assert_eq!(config.model, "haiku");
        assert_eq!(config.image, "my-image");
        assert_eq!(config.dockerfile, "custom/Dockerfile");
        assert_eq!(config.volume.as_deref(), Some("my-vol"));
        assert_eq!(config.log_dir, "my-logs");
        assert_eq!(config.branch.as_deref(), Some("dev"));
        assert_eq!(config.loop_config.delay, 5);
        assert_eq!(config.loop_config.max_runs, 10);
        assert_eq!(config.loop_config.decider_model, "opus");
        assert_eq!(config.loop_config.summary_model, "sonnet");
        assert_eq!(config.loop_config.reviewer_model, "haiku");
        assert_eq!(config.loop_config.shipper_model, "opus");
        assert_eq!(config.do_config.timeout, 30);
        assert_eq!(config.do_config.max_failures, 5);
    }

    #[test]
    fn effective_volume_uses_repo_name_when_not_configured() {
        let config = Config::default();
        let path = std::path::Path::new("/some/path/myproject");
        assert_eq!(
            config.effective_volume(path),
            "mrmouth-claude-home-myproject"
        );
    }

    #[test]
    fn effective_volume_sanitizes_special_chars() {
        let config = Config::default();
        let path = std::path::Path::new("/some/path/my.project_v2");
        assert_eq!(
            config.effective_volume(path),
            "mrmouth-claude-home-my-project-v2"
        );
    }

    #[test]
    fn effective_volume_uses_shared_codex_default() {
        let config = Config {
            agent: AgentKind::Codex,
            ..Config::default()
        };
        let path = std::path::Path::new("/some/path/myproject");
        assert_eq!(config.effective_volume(path), "mrmouth-codex-home");
    }

    #[test]
    fn effective_volume_uses_configured_value() {
        let config = Config {
            volume: Some("custom-vol".into()),
            ..Config::default()
        };
        let path = std::path::Path::new("/some/path/myproject");
        assert_eq!(config.effective_volume(path), "custom-vol");
    }

    #[test]
    fn effective_model_omits_claude_aliases_for_codex() {
        let config = Config {
            agent: AgentKind::Codex,
            ..Config::default()
        };

        assert_eq!(config.effective_model_for_agent("sonnet"), "");
        assert_eq!(config.effective_model_for_agent("haiku"), "");
        assert_eq!(config.effective_model_for_agent("opus"), "");
    }

    #[test]
    fn effective_model_preserves_codex_models_for_codex() {
        let config = Config {
            agent: AgentKind::Codex,
            ..Config::default()
        };

        assert_eq!(config.effective_model_for_agent("gpt-5.2"), "gpt-5.2");
    }

    #[test]
    fn effective_model_preserves_claude_aliases_for_claude() {
        let config = Config::default();

        assert_eq!(config.effective_model_for_agent("sonnet"), "sonnet");
    }

    #[test]
    fn invalid_toml_returns_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join(".mrmouth");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join("config.toml"), "not valid [[[toml").unwrap();

        let err = Config::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
    }
}
