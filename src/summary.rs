use std::path::Path;

use crate::config::Config;
use crate::logger::Logger;
use crate::stream_fmt::StreamFormatter;
use crate::streaming::{self, StreamTarget};

pub fn execute(config: &Config, repo_root: &Path, log_file: &str, logger: Option<&Logger>) -> Result<(), SummaryError> {
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
        SummaryError(format!("failed to create summaries directory: {e}"))
    })?;
    let summary_file = summary_dir.join(format!("{log_name}.md"));

    let prompt = format!(
        "Read the log file at {} and print a concise markdown summary to stdout covering:\n\
        - What tasks were worked on\n\
        - What was accomplished (files created/modified, commits)\n\
        - Whether the run succeeded or failed (and why)\n\
        - Any errors or notable events\n\
        \n\
        Do NOT write any files. Output the summary to stdout only.",
        log_path.display(),
    );

    crate::logger::log(logger, "SUMMARY");

    let mut cmd = streaming::claude_stream_cmd(
        repo_root,
        &config.loop_config.summary_model,
        "Read",
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| SummaryError(format!("failed to run claude CLI: {e}")))?;

    streaming::send_prompt(&mut child, &prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.clone()),
        None => StreamTarget::Stdout,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let (result, exit_code) = streaming::run_streaming_claude(child, &mut formatter, logger, &target, &mut None)
        .map_err(|e| SummaryError(format!("streaming error: {e}")))?;

    if exit_code != 0 {
        return Err(SummaryError(format!(
            "claude CLI exited with code {exit_code}"
        )));
    }

    if !result.trim().is_empty() {
        std::fs::write(&summary_file, &result).map_err(|e| {
            SummaryError(format!("failed to write summary file: {e}"))
        })?;
        crate::logger::log(logger, &format!("Summary saved: {}", summary_file.display()));
    } else {
        crate::logger::log(logger, "Summary agent produced no output; skipping file write.");
    }
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

impl SummaryError {
    pub fn debrief(&self) -> crate::debrief::FailureDebrief {
        crate::debrief::FailureDebrief::new(self.0.clone())
    }
}
