use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct ReviewerOptions {
    pub model: String,
    pub current_branch: String,
}

/// Run a reviewer agent that inspects changes on the current branch vs SPEC.md.
/// Creates litebrite items for issues found and closes done items.
/// Non-fatal — errors are logged but don't stop the loop.
pub fn execute(repo_root: &Path, opts: &ReviewerOptions) -> Result<(), ReviewerError> {
    eprintln!("Running reviewer on branch {}...", opts.current_branch);

    let prompt = format!(
        "You are a code reviewer for this project. Review the changes on branch '{}' \
        against the project spec (SPEC.md). Use git diff and git log to understand what changed. \
        Use lb commands to inspect task state.\n\n\
        If you find issues (bugs, spec deviations, missing tests, code quality problems), \
        create litebrite items for them: lb create \"<title>\" -d \"<description>\"\n\n\
        If you see completed items that are still open, close them: lb close <id>\n\n\
        Be concise. Only flag real issues, not style nits.",
        opts.current_branch
    );

    let mut child = Command::new("claude")
        .args([
            "-p",
            "--no-session-persistence",
            "--model", &opts.model,
            "--allowedTools", "Read,Bash(git diff *),Bash(git log *),Bash(lb *)",
            "--output-format", "json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .current_dir(repo_root)
        .spawn()
        .map_err(|e| ReviewerError(format!("failed to run claude CLI: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }

    let status = child
        .wait()
        .map_err(|e| ReviewerError(format!("failed to wait for claude CLI: {e}")))?;

    if !status.success() {
        return Err(ReviewerError(format!(
            "claude CLI exited with code {}",
            status.code().unwrap_or(-1)
        )));
    }

    eprintln!("Reviewer pass complete.");
    Ok(())
}

#[derive(Debug)]
pub struct ReviewerError(String);

impl std::fmt::Display for ReviewerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ReviewerError {}
