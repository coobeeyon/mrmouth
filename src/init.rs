use std::fs;
use std::io::Write;
use std::path::Path;

use crate::config::Config;
use crate::docker::DEFAULT_DOCKERFILE;
use crate::prompt::DEFAULT_PROMPT;

const DEFAULT_CONFIG: &str = r#"# Mr Mouth configuration
# See: https://github.com/coobeeyon/mrmouth

# Model for agent runs (default: opus)
# model = "opus"

# Docker image name (default: mrmouth-runner)
# image = "mrmouth-runner"

# Dockerfile path relative to repo root (default: .mrmouth/Dockerfile)
# dockerfile = ".mrmouth/Dockerfile"

# Persistent Docker volume name for Claude memory (default: mrmouth-claude-home)
# volume = "mrmouth-claude-home"

# Log directory relative to repo root (default: logs)
# log_dir = "logs"

# Credentials env file (default: .env)
# env_file = ".env"

# Branch to work on (default: current branch)
# branch = "main"

# [loop]
# delay = 0
# max_runs = 0
# decider_model = "sonnet"
# summary_model = "haiku"

# [epic]
# timeout = 15
# max_failures = 3
"#;

const GITIGNORE_ENTRIES: &[&str] = &[
    "# Mr Mouth",
    "logs/",
    ".env",
];

pub fn execute() -> Result<(), InitError> {
    let repo_root = Config::find_repo_root().map_err(|e| InitError::Config(e.to_string()))?;
    execute_in(&repo_root)
}

fn execute_in(repo_root: &Path) -> Result<(), InitError> {
    let mrmouth_dir = repo_root.join(".mrmouth");

    // Create .mrmouth/ directory
    fs::create_dir_all(&mrmouth_dir)
        .map_err(|e| InitError::Io("creating .mrmouth directory".into(), e))?;

    // Write config.toml (skip if exists)
    let config_path = mrmouth_dir.join("config.toml");
    write_if_missing(&config_path, DEFAULT_CONFIG, "config.toml")?;

    // Write Dockerfile (skip if exists)
    let dockerfile_path = mrmouth_dir.join("Dockerfile");
    write_if_missing(&dockerfile_path, DEFAULT_DOCKERFILE, "Dockerfile")?;

    // Write prompt.md (skip if exists)
    let prompt_path = mrmouth_dir.join("prompt.md");
    write_if_missing(&prompt_path, DEFAULT_PROMPT, "prompt.md")?;

    // Update .gitignore
    update_gitignore(&repo_root)?;

    // Print next steps
    eprintln!();
    eprintln!("Initialized .mrmouth/ in {}", repo_root.display());
    eprintln!();
    eprintln!("Created:");
    eprintln!("  .mrmouth/config.toml   — configuration (all defaults, edit as needed)");
    eprintln!("  .mrmouth/Dockerfile    — agent container image");
    eprintln!("  .mrmouth/prompt.md     — agent prompt (customize for your project)");
    eprintln!();
    eprintln!("Next steps:");
    eprintln!("  1. Add your API key to .env:  echo 'ANTHROPIC_API_KEY=sk-...' > .env");
    eprintln!("  2. Run an agent session:      mrmouth run");
    eprintln!();

    Ok(())
}

fn write_if_missing(path: &Path, content: &str, label: &str) -> Result<(), InitError> {
    if path.exists() {
        eprintln!("  skip: .mrmouth/{label} (already exists)");
        return Ok(());
    }
    fs::write(path, content)
        .map_err(|e| InitError::Io(format!("writing .mrmouth/{label}"), e))?;
    Ok(())
}

fn update_gitignore(repo_root: &Path) -> Result<(), InitError> {
    let gitignore_path = repo_root.join(".gitignore");
    let existing = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path)
            .map_err(|e| InitError::Io("reading .gitignore".into(), e))?
    } else {
        String::new()
    };

    let mut to_add: Vec<&str> = Vec::new();
    for entry in GITIGNORE_ENTRIES {
        if !existing.lines().any(|line| line.trim() == *entry) {
            to_add.push(entry);
        }
    }

    if to_add.is_empty() {
        return Ok(());
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .map_err(|e| InitError::Io("opening .gitignore".into(), e))?;

    // Add a newline separator if the file doesn't end with one
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file).map_err(|e| InitError::Io("writing .gitignore".into(), e))?;
    }

    for entry in &to_add {
        writeln!(file, "{entry}").map_err(|e| InitError::Io("writing .gitignore".into(), e))?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum InitError {
    Config(String),
    Io(String, std::io::Error),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "{msg}"),
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
        }
    }
}

impl std::error::Error for InitError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_repo(tmp: &Path) {
        // Create a fake git repo
        fs::create_dir_all(tmp.join(".git")).unwrap();
    }

    #[test]
    fn init_creates_all_files() {
        let tmp = tempfile::tempdir().unwrap();
        setup_repo(tmp.path());

        let result = execute_in(tmp.path());

        assert!(result.is_ok());
        assert!(tmp.path().join(".mrmouth/config.toml").exists());
        assert!(tmp.path().join(".mrmouth/Dockerfile").exists());
        assert!(tmp.path().join(".mrmouth/prompt.md").exists());

        // Check gitignore was created with entries
        let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("logs/"));
        assert!(gitignore.contains(".env"));
    }

    #[test]
    fn init_skips_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        setup_repo(tmp.path());
        let mrmouth_dir = tmp.path().join(".mrmouth");
        fs::create_dir_all(&mrmouth_dir).unwrap();
        fs::write(mrmouth_dir.join("config.toml"), "model = \"haiku\"\n").unwrap();

        let result = execute_in(tmp.path());

        assert!(result.is_ok());
        // config.toml should NOT be overwritten
        let config = fs::read_to_string(mrmouth_dir.join("config.toml")).unwrap();
        assert_eq!(config, "model = \"haiku\"\n");
        // But Dockerfile and prompt.md should be created
        assert!(mrmouth_dir.join("Dockerfile").exists());
        assert!(mrmouth_dir.join("prompt.md").exists());
    }

    #[test]
    fn init_appends_to_existing_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        setup_repo(tmp.path());
        fs::write(tmp.path().join(".gitignore"), "node_modules/\n").unwrap();

        let result = execute_in(tmp.path());

        assert!(result.is_ok());
        let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(gitignore.starts_with("node_modules/\n"));
        assert!(gitignore.contains("logs/"));
        assert!(gitignore.contains(".env"));
    }

    #[test]
    fn init_does_not_duplicate_gitignore_entries() {
        let tmp = tempfile::tempdir().unwrap();
        setup_repo(tmp.path());
        fs::write(tmp.path().join(".gitignore"), "logs/\n.env\n").unwrap();

        let result = execute_in(tmp.path());

        assert!(result.is_ok());
        let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        // Should only have one occurrence of each
        assert_eq!(gitignore.matches("logs/").count(), 1);
        assert_eq!(gitignore.matches(".env").count(), 1);
    }
}
