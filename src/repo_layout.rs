use std::path::{Path, PathBuf};

use crate::config::Config;

pub const BOOKKEEPING_CONTAINER_PATH: &str = "/home/runner/workspace";
pub const WORK_CONTAINER_PATH: &str = "/home/runner/worktree";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLayout {
    pub bookkeeping_repo: PathBuf,
    pub work_repo: PathBuf,
}

impl RepoLayout {
    pub fn resolve(
        config: &Config,
        bookkeeping_repo: &Path,
        cli_work_repo: Option<&Path>,
    ) -> Result<Self, RepoLayoutError> {
        let bookkeeping_repo = canonicalize_existing("bookkeeping repo", bookkeeping_repo)?;
        let work_repo = match cli_work_repo {
            Some(path) => {
                let base = std::env::current_dir().map_err(RepoLayoutError::CurrentDir)?;
                resolve_existing_path("work repo", &base, path)?
            }
            None => match config.work_repo.as_deref() {
                Some(path) => resolve_existing_path("work_repo", &bookkeeping_repo, path)?,
                None => bookkeeping_repo.clone(),
            },
        };

        Ok(Self {
            bookkeeping_repo,
            work_repo,
        })
    }

    pub fn is_split(&self) -> bool {
        self.bookkeeping_repo != self.work_repo
    }

    pub fn docker_work_mount(&self) -> Option<PathBuf> {
        self.is_split().then(|| self.work_repo.clone())
    }

    pub fn agent_work_path(&self, current_container: bool) -> String {
        if current_container {
            self.work_repo.display().to_string()
        } else if self.is_split() {
            WORK_CONTAINER_PATH.to_string()
        } else {
            BOOKKEEPING_CONTAINER_PATH.to_string()
        }
    }

    pub fn prompt_block(&self, current_container: bool) -> Option<String> {
        if !self.is_split() {
            return None;
        }

        if current_container {
            Some(format!(
                "The bookkeeping repo for Litebrite/Trapperkeeper state is `{}`; run `lb` and `trk` commands there. \
                The work repo for code changes is `{}`. \
                Make code changes, code commits, and code pushes in the work repo; make only task-state commits in the bookkeeping repo when needed. ",
                self.bookkeeping_repo.display(),
                self.work_repo.display()
            ))
        } else {
            Some(format!(
                "The bookkeeping repo for Litebrite/Trapperkeeper state is `{BOOKKEEPING_CONTAINER_PATH}`; run `lb` and `trk` commands there. \
                The work repo for code changes is bind-mounted at `{WORK_CONTAINER_PATH}` from host path `{}`. \
                Make code changes, code commits, and code pushes in `{WORK_CONTAINER_PATH}`; make only task-state commits in `{BOOKKEEPING_CONTAINER_PATH}` when needed. ",
                self.work_repo.display()
            ))
        }
    }

    pub fn prepend_prompt_block(&self, prompt: String, current_container: bool) -> String {
        match self.prompt_block(current_container) {
            Some(block) => format!("## Repository Layout\n\n{block}\n\n{prompt}"),
            None => prompt,
        }
    }
}

fn resolve_existing_path(
    label: &'static str,
    base: &Path,
    path: &Path,
) -> Result<PathBuf, RepoLayoutError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    canonicalize_existing(label, &absolute)
}

fn canonicalize_existing(label: &'static str, path: &Path) -> Result<PathBuf, RepoLayoutError> {
    if !path.exists() {
        return Err(RepoLayoutError::Missing {
            label,
            path: path.to_path_buf(),
        });
    }
    std::fs::canonicalize(path).map_err(|source| RepoLayoutError::Canonicalize {
        label,
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug)]
pub enum RepoLayoutError {
    CurrentDir(std::io::Error),
    Missing {
        label: &'static str,
        path: PathBuf,
    },
    Canonicalize {
        label: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for RepoLayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentDir(e) => write!(f, "failed to get current directory: {e}"),
            Self::Missing { label, path } => {
                write!(f, "{label} path does not exist: {}", path.display())
            }
            Self::Canonicalize {
                label,
                path,
                source,
            } => write!(
                f,
                "failed to resolve {label} path {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RepoLayoutError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_work_repo_to_bookkeeping_repo() {
        let dir = tempfile::tempdir().unwrap();
        let layout = RepoLayout::resolve(&Config::default(), dir.path(), None).unwrap();

        assert!(!layout.is_split());
        assert_eq!(layout.docker_work_mount(), None);
        assert_eq!(layout.agent_work_path(false), BOOKKEEPING_CONTAINER_PATH);
    }

    #[test]
    fn config_work_repo_is_relative_to_bookkeeping_repo() {
        let dir = tempfile::tempdir().unwrap();
        let service = dir.path().join("service");
        std::fs::create_dir(&service).unwrap();
        let config = Config {
            work_repo: Some(PathBuf::from("service")),
            ..Config::default()
        };

        let layout = RepoLayout::resolve(&config, dir.path(), None).unwrap();

        assert!(layout.is_split());
        assert_eq!(layout.work_repo, std::fs::canonicalize(&service).unwrap());
        assert_eq!(
            layout.docker_work_mount().as_deref(),
            Some(layout.work_repo.as_path())
        );
        assert!(layout
            .prompt_block(false)
            .unwrap()
            .contains("/home/runner/worktree"));
    }

    #[test]
    fn cli_work_repo_overrides_config() {
        let dir = tempfile::tempdir().unwrap();
        let configured = dir.path().join("configured");
        let cli = dir.path().join("cli");
        std::fs::create_dir(&configured).unwrap();
        std::fs::create_dir(&cli).unwrap();
        let config = Config {
            work_repo: Some(PathBuf::from("configured")),
            ..Config::default()
        };

        let layout = RepoLayout::resolve(&config, dir.path(), Some(&cli)).unwrap();

        assert_eq!(layout.work_repo, std::fs::canonicalize(&cli).unwrap());
    }

    #[test]
    fn same_resolved_paths_are_not_split() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            work_repo: Some(PathBuf::from(".")),
            ..Config::default()
        };

        let layout = RepoLayout::resolve(&config, dir.path(), None).unwrap();

        assert!(!layout.is_split());
        assert_eq!(layout.docker_work_mount(), None);
    }
}
