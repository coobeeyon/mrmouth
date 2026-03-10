use std::path::Path;

use crate::logger::Logger;
use crate::stream_fmt::StreamFormatter;
use crate::streaming::{self, StreamTarget};

pub struct ReviewerOptions {
    pub model: String,
    pub current_branch: String,
}

/// Run a reviewer agent that inspects changes on the current branch vs SPEC.md.
/// Creates litebrite items for issues found and closes done items.
/// Non-fatal — errors are logged but don't stop the loop.
pub fn execute(repo_root: &Path, opts: &ReviewerOptions, logger: Option<&Logger>) -> Result<(), ReviewerError> {
    crate::logger::banner(logger, &format!("CODE REVIEW  branch={}", opts.current_branch));

    let prompt = format!(
        "You are a code reviewer for this project. Review the changes on branch '{}' \
        against the project spec (SPEC.md). Use git diff and git log to understand what changed. \
        Use lb commands to inspect task state.\n\n\
        Context: You are one step in an automated loop with multiple checks and balances. \
        If you find real issues, another agent will fix them and you will review again. \
        This means you must not miss genuine problems — but you also must not invent them. \
        A clean review is a valid and useful outcome. If the code looks good, say so and stop. \
        Do not manufacture issues to justify your existence.\n\n\
        If you find issues (bugs, spec deviations, missing tests, code quality problems), \
        create litebrite items for them: lb create \"<title>\" -d \"<description>\"\n\n\
        If you see completed items that are still open, close them: lb close <id>\n\n\
        Be concise. Only flag real issues, not style nits.",
        opts.current_branch
    );

    let mut cmd = streaming::claude_stream_cmd(
        repo_root,
        &opts.model,
        "Read,Bash(git diff *),Bash(git log *),Bash(lb *)",
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| ReviewerError(format!("failed to run claude CLI: {e}")))?;

    streaming::send_prompt(&mut child, &prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.with_label("CODE REVIEW")),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let (_result, exit_code) = streaming::run_streaming_claude(child, &mut formatter, logger, &target)
        .map_err(|e| ReviewerError(format!("streaming error: {e}")))?;

    if exit_code != 0 {
        return Err(ReviewerError(format!(
            "claude CLI exited with code {exit_code}"
        )));
    }

    crate::logger::log(logger, "Reviewer pass complete.");
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
