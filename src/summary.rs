use std::path::Path;
use std::process::Command;

use crate::config::Config;

pub fn execute(config: &Config, repo_root: &Path, log_file: &str) -> Result<(), SummaryError> {
    let log_path = if Path::new(log_file).is_absolute() {
        std::path::PathBuf::from(log_file)
    } else {
        repo_root.join(log_file)
    };

    // Resolve symlinks (e.g. latest.jsonl -> run-20260306-120000.jsonl)
    let log_path = match std::fs::read_link(&log_path) {
        Ok(target) => {
            if target.is_absolute() {
                target
            } else {
                log_path.parent().unwrap_or(repo_root).join(target)
            }
        }
        Err(_) => log_path,
    };

    if !log_path.exists() {
        return Err(SummaryError(format!(
            "log file not found: {}",
            log_path.display()
        )));
    }

    let log_name = log_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let log_dir = repo_root.join(&config.log_dir);
    let summary_dir = log_dir.join("summaries");
    std::fs::create_dir_all(&summary_dir).map_err(|e| {
        SummaryError(format!(
            "failed to create summaries directory: {e}"
        ))
    })?;
    let summary_file = summary_dir.join(format!("{log_name}.md"));

    let prompt = format!(
        "Read the log file at {}. Write a concise markdown summary to {} covering:\n\
        - What tasks were worked on\n\
        - What was accomplished (files created/modified, commits)\n\
        - Whether the run succeeded or failed (and why)\n\
        - Any errors or notable events\n\
        \n\
        Also print the summary to stdout.",
        log_path.display(),
        summary_file.display()
    );

    eprintln!("Generating summary...");
    let status = Command::new("claude")
        .args([
            "-p",
            "--model",
            &config.loop_config.summary_model,
            "--allowedTools",
            "Read, Write",
        ])
        .arg(&prompt)
        .current_dir(repo_root)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| SummaryError(format!("failed to run claude CLI: {e}")))?;

    if !status.success() {
        return Err(SummaryError(format!(
            "claude CLI exited with code {}",
            status.code().unwrap_or(-1)
        )));
    }

    eprintln!("Summary saved: {}", summary_file.display());
    Ok(())
}

#[derive(Debug)]
pub struct SummaryError(String);

impl std::fmt::Display for SummaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SummaryError {}
