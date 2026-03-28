use std::fs::File;
use std::io::{BufWriter, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

use crate::config::Config;
use crate::docker::{ContainerArgs, DockerBuilder};
use crate::logger::Logger;
use crate::stream_fmt::{self, StreamFormatter};

pub struct ReviewerOptions {
    pub model: String,
    pub current_branch: String,
}

/// Run a reviewer agent inside the project Docker container so it has access
/// to the project's build toolchain. Inspects changes on the current branch
/// vs SPEC.md, verifies the build and tests pass, and creates/closes litebrite
/// items for issues found. Non-fatal — errors are logged but don't stop the loop.
pub fn execute(config: &Config, repo_root: &Path, opts: &ReviewerOptions, logger: Option<&Logger>) -> Result<(), ReviewerError> {
    crate::logger::log(logger, &format!("CODE REVIEW  branch={}", opts.current_branch));

    let effective_dockerfile = crate::docker::effective_dockerfile_content(repo_root, &config.dockerfile);

    let preamble = crate::prompt::SYSTEM_PREAMBLE;
    let prompt = format!(
        "## System\n\n{preamble}\n\n\
        You are the **Reviewer**. Your job is to review code and file issues. You do NOT implement features, make architectural decisions, or decide whether the loop continues.\n\n\
        ## Instructions\n\n\
        Review the changes on branch '{}' \
        against the project spec (SPEC.md). Use git diff and git log to understand what changed. \
        Use lb commands to inspect task state.\n\n\
        First, verify the project builds and all tests pass. Discover the correct build/test \
        commands by examining the project structure (Makefile, package.json, Cargo.toml, etc.) \
        and run them. A build failure or test failure is a blocking issue that must be filed.\n\n\
        If a build fails because a required tool is missing from the container \
        (e.g., 'cargo: command not found', 'python3: not found'), this is a Dockerfile issue. \
        Fix it by editing `.mrmouth/Dockerfile` to install the missing toolchain \
        (add a RUN layer before the USER runner line), then commit and push. \
        Do NOT create a litebrite task for missing-tool issues — fix the Dockerfile directly.\n\n\
        Context: You are one step in an automated loop with multiple checks and balances. \
        If you find real issues, another agent will fix them and you will review again. \
        This means you must not miss genuine problems — but you also must not invent them. \
        A clean review is a valid and useful outcome. If the code looks good, say so and stop. \
        Do not manufacture issues to justify your existence.\n\n\
        If you find issues (bugs, spec deviations, missing tests, build/test failures, code quality problems), \
        create litebrite items for them: lb create \"<title>\" -d \"<description>\"\n\n\
        If you see completed items that are still open, close them: lb close <id>\n\n\
        Be concise. Only flag real issues, not style nits.",
        opts.current_branch
    );

    let escaped_prompt = prompt.replace('\'', "'\\''");
    let model = &opts.model;

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"

# Clone repo
if [ ! -d "$work_dir/.git" ]; then
  if [ -n "$repo_url" ]; then
    echo "Cloning $repo_url (branch: $branch)..."
    git clone --branch "$branch" "$repo_url" "$work_dir"
  fi
fi
cd "$work_dir"
git config --global --add safe.directory "$work_dir"

# Seed Dockerfile if absent (gives reviewer a file to read and modify)
dockerfile_path="$work_dir/__DOCKERFILE_REL_PATH__"
if [ ! -f "$dockerfile_path" ]; then
  mkdir -p "$(dirname "$dockerfile_path")"
  cat > "$dockerfile_path" << 'MRMOUTH_DOCKERFILE_EOF'
__DOCKERFILE_CONTENT__
MRMOUTH_DOCKERFILE_EOF
  echo "Seeded Dockerfile into workspace."
fi

# Initialize litebrite
if [ -d "$work_dir/.git" ]; then
  echo "Initializing litebrite..."
  lb init
  lb setup claude 2>/dev/null || true
  lb sync 2>/dev/null || true
fi

# Restore .claude.json from persisted backup if missing
claude_config="$HOME/.claude.json"
if [ ! -f "$claude_config" ] && [ -d "$HOME/.claude/backups" ]; then
  latest_backup=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1)
  if [ -n "$latest_backup" ]; then
    cp "$latest_backup" "$claude_config"
    echo "Restored .claude.json from backup."
  fi
fi

# Run reviewer
echo "Starting code review..."
claude -p --dangerously-skip-permissions --verbose --output-format stream-json --model {model} '{escaped_prompt}'
echo "Code review complete."

# Push lb state changes back so the host loop can sync them
if [ -d "$work_dir/.git" ]; then
  lb sync 2>/dev/null || true
  git push 2>/dev/null || true
fi
"#
    );

    let script = script
        .replace("__DOCKERFILE_CONTENT__", &effective_dockerfile)
        .replace("__DOCKERFILE_REL_PATH__", &config.dockerfile);

    let tmp = tempfile::NamedTempFile::new()
        .map_err(|e| ReviewerError(format!("failed to create reviewer script: {e}")))?;

    // Close the write fd before mounting into Docker. Linux returns ETXTBSY when
    // execve() targets an inode that any process holds open for writing.
    // `into_temp_path()` closes the fd but keeps the deletion-on-drop guard.
    let tmp_path = tmp.into_temp_path();
    std::fs::write(&tmp_path, script.as_bytes())
        .map_err(|e| ReviewerError(format!("failed to write reviewer script: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms)
            .map_err(|e| ReviewerError(format!("failed to set script permissions: {e}")))?;
    }

    let (repo_url, file_remote_path) = match git_remote_url(repo_root) {
        Some(url) => (url, None),
        None => ("file:///host-repo".to_string(), Some(repo_root.to_path_buf())),
    };

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let container_name = format!("review-{timestamp}");
    let volume = config.effective_volume(repo_root);

    // Create dedicated review log + jsonl files
    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let review_log_path = log_dir.join(format!("review-{timestamp}.log"));
    let review_jsonl_path = log_dir.join(format!("review-{timestamp}.jsonl"));

    let review_logger = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => Logger::with_tui(&review_log_path, tui.clone()),
        None => Logger::new(&review_log_path),
    }.ok();

    let mut jsonl_writer: Option<BufWriter<File>> = File::create(&review_jsonl_path)
        .ok()
        .map(BufWriter::new);

    DockerBuilder::remove_container(&container_name);

    let docker = DockerBuilder::new(&config.image);
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(|e| ReviewerError(format!("failed to build reviewer image: {e}")))?;

    let container_args = ContainerArgs {
        name: container_name.clone(),
        repo_url,
        branch: opts.current_branch.clone(),
        runner_script: tmp_path.to_path_buf(),
        volume,
        local: false,
        file_remote_path,
        timeout_secs: None,
    };

    let mut handle = docker
        .run(&container_args)
        .map_err(|e| ReviewerError(format!("failed to start reviewer container: {e}")))?;

    let is_tty = logger.is_some_and(|l| l.has_tui()) || std::io::stdout().is_terminal();
    let mut formatter = StreamFormatter::new(is_tty);

    handle
        .stream_output(|line| {
            // Write raw JSONL to dedicated file
            if let Some(w) = jsonl_writer.as_mut() {
                let _ = writeln!(w, "{line}");
            }

            if let Some(formatted) = stream_fmt::format_line(&mut formatter, line) {
                // Display to TUI/stderr
                match logger {
                    Some(l) => l.display(&formatted),
                    None => eprintln!("{formatted}"),
                }
                // Write formatted text to dedicated review log
                if let Some(rl) = review_logger.as_ref() {
                    rl.log_file_only(&formatted);
                }
            }
        })
        .map_err(|e| ReviewerError(format!("streaming error: {e}")))?;

    // Flush dedicated JSONL writer
    if let Some(w) = jsonl_writer.as_mut() {
        let _ = w.flush();
    }

    let exit_code = handle
        .wait()
        .map_err(|e| ReviewerError(format!("container wait failed: {e}")))?;

    // Extract updated Dockerfile from container (reviewer may have modified it)
    let dockerfile_dest = repo_root.join(&config.dockerfile);
    let container_path = format!("/home/runner/workspace/{}", config.dockerfile);
    if DockerBuilder::copy_from_container(&container_name, &container_path, &dockerfile_dest) {
        crate::logger::log(logger, "Extracted updated Dockerfile from reviewer container.");
    }

    DockerBuilder::remove_container(&container_name);

    if exit_code != 0 {
        crate::logger::log(logger, &format!("Reviewer container exited with code {exit_code}"));
        return Err(ReviewerError(format!("reviewer container exited with code {exit_code}")));
    }

    crate::logger::log(logger, "Reviewer pass complete.");
    Ok(())
}

fn git_remote_url(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

#[derive(Debug)]
pub struct ReviewerError(String);

impl std::fmt::Display for ReviewerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ReviewerError {}
