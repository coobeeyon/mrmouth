use serde::Deserialize;
use std::path::{Path, PathBuf};

const CONFIG_DIR: &str = ".mrmouth";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: String,
    pub image: String,
    pub dockerfile: String,
    pub volume: String,
    pub log_dir: String,
    pub env_file: String,
    pub branch: Option<String>,
    #[serde(rename = "loop")]
    pub loop_config: LoopConfig,
    pub epic: EpicConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct LoopConfig {
    pub delay: u32,
    pub max_runs: u32,
    pub decider_model: String,
    pub summary_model: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EpicConfig {
    pub timeout: u32,
    pub max_failures: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "opus".into(),
            image: "mrmouth-runner".into(),
            dockerfile: ".mrmouth/Dockerfile".into(),
            volume: "mrmouth-claude-home".into(),
            log_dir: "logs".into(),
            env_file: ".env".into(),
            branch: None,
            loop_config: LoopConfig::default(),
            epic: EpicConfig::default(),
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
        }
    }
}

impl Default for EpicConfig {
    fn default() -> Self {
        Self {
            timeout: 15,
            max_failures: 3,
        }
    }
}

impl Config {
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

        let config: Self =
            toml::from_str(&contents).map_err(|e| ConfigError::Parse { path: config_path, source: e })?;

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

    /// Resolve the config directory path.
    pub fn config_dir(repo_root: &Path) -> PathBuf {
        repo_root.join(CONFIG_DIR)
    }
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
            Self::Cwd(e) => write!(f, "failed to get current directory: {}", e),
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
        assert_eq!(config.model, "opus");
        assert_eq!(config.image, "mrmouth-runner");
        assert_eq!(config.loop_config.decider_model, "sonnet");
        assert_eq!(config.epic.timeout, 15);
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
model = "haiku"
image = "my-image"
dockerfile = "custom/Dockerfile"
volume = "my-vol"
log_dir = "my-logs"
env_file = ".env.prod"
branch = "dev"

[loop]
delay = 5
max_runs = 10
decider_model = "opus"
summary_model = "sonnet"

[epic]
timeout = 30
max_failures = 5
"#,
        )
        .unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.model, "haiku");
        assert_eq!(config.image, "my-image");
        assert_eq!(config.dockerfile, "custom/Dockerfile");
        assert_eq!(config.volume, "my-vol");
        assert_eq!(config.log_dir, "my-logs");
        assert_eq!(config.env_file, ".env.prod");
        assert_eq!(config.branch.as_deref(), Some("dev"));
        assert_eq!(config.loop_config.delay, 5);
        assert_eq!(config.loop_config.max_runs, 10);
        assert_eq!(config.loop_config.decider_model, "opus");
        assert_eq!(config.loop_config.summary_model, "sonnet");
        assert_eq!(config.epic.timeout, 30);
        assert_eq!(config.epic.max_failures, 5);
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
