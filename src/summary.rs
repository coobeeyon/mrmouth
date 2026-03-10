use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::logger::Logger;

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

    crate::logger::banner(logger, "SUMMARY");

    let mut child = Command::new("claude")
        .args([
            "-p",
            "--no-session-persistence",
            "--model",
            &config.loop_config.summary_model,
            "--allowedTools",
            "Read,Write",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(repo_root)
        .spawn()
        .map_err(|e| SummaryError(format!("failed to run claude CLI: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }

    // Tee stdout (summary text) and stderr to terminal + log file
    let stdout_handle = child.stdout.take().map(|stdout| {
        if let Some(l) = logger {
            l.tee_stdout(stdout)
        } else {
            std::thread::spawn(move || {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    println!("{line}");
                }
            })
        }
    });

    let stderr_handle = child.stderr.take().map(|stderr| {
        if let Some(l) = logger {
            l.tee_stderr(stderr)
        } else {
            std::thread::spawn(move || {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("{line}");
                }
            })
        }
    });

    let status = child
        .wait()
        .map_err(|e| SummaryError(format!("failed to wait for claude CLI: {e}")))?;

    if let Some(h) = stdout_handle { let _ = h.join(); }
    if let Some(h) = stderr_handle { let _ = h.join(); }

    if !status.success() {
        return Err(SummaryError(format!(
            "claude CLI exited with code {}",
            status.code().unwrap_or(-1)
        )));
    }

    crate::logger::log(logger, &format!("Summary saved: {}", summary_file.display()));
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
