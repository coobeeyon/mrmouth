use std::fs::File;
use std::io::{BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::docker::{ContainerArgs, CopyFromContainerOutcome, DockerBuilder};
use crate::events::{EventSinkHandle, MrmouthEvent, ReviewerAction};
use crate::logger::Logger;
use crate::repo_layout::{BOOKKEEPING_CONTAINER_PATH, WORK_CONTAINER_PATH};
use crate::stream_fmt::{self, StreamFormatter};

pub struct ReviewerOptions {
    pub model: String,
    pub current_branch: String,
    /// If set, the reviewer scopes its review to only the changes in this
    /// commit range (before..after) instead of the entire branch.
    pub commit_range: Option<(String, String)>,
    /// The litebrite item or work area the run was intended to satisfy.
    pub review_target: Option<ReviewTarget>,
    /// Host path to a distinct code worktree. When set, the reviewer container
    /// mounts it at WORK_CONTAINER_PATH and reviews code there while keeping
    /// Litebrite/Trapperkeeper operations rooted in BOOKKEEPING_CONTAINER_PATH.
    pub worktree_path: Option<PathBuf>,
    pub event_sink: Option<EventSinkHandle>,
}

pub struct ReviewTarget {
    pub item_id: String,
    pub label: String,
}

/// Run a reviewer agent inside the project Docker container so it has access
/// to the project's build toolchain. Inspects changes on the current branch
/// vs SPEC.md, verifies the build and tests pass, and creates/closes litebrite
/// items for issues found. Non-fatal — errors are logged but don't stop the loop.
pub fn execute(
    config: &Config,
    repo_root: &Path,
    opts: &ReviewerOptions,
    logger: Option<&Logger>,
) -> Result<(), ReviewerError> {
    emit_event(
        &opts.event_sink,
        MrmouthEvent::ReviewerLifecycle {
            action: ReviewerAction::Starting,
            branch: opts.current_branch.clone(),
            commit_range: commit_range_label(&opts.commit_range),
        },
    );
    crate::logger::log(
        logger,
        &format!("CODE REVIEW  branch={}", opts.current_branch),
    );

    let effective_dockerfile =
        crate::docker::effective_dockerfile_content(repo_root, &config.dockerfile);

    let preamble = crate::prompt::SYSTEM_PREAMBLE;

    let scope_instructions = review_scope_instructions(
        &opts.current_branch,
        &opts.commit_range,
        opts.worktree_path.as_deref(),
    );
    let workspace_instructions = review_workspace_instructions(opts.worktree_path.as_deref());

    let purpose_instructions = review_purpose_instructions(opts.review_target.as_ref());

    let prompt = format!(
        "## System\n\n{preamble}\n\n\
        You are the **Reviewer**. Your job is to review code and file issues. You do NOT implement features, make architectural decisions, or decide whether the loop continues.\n\n\
        ## Instructions\n\n\
        {scope_instructions}\n\n\
        {workspace_instructions}\n\n\
        {purpose_instructions}\n\n\
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
        If you find issues (bugs, failure to satisfy the requested item, spec deviations, missing tests, build/test failures, code quality problems), \
        create litebrite items for them and attach them to the relevant current work context. \
        For a reviewed epic or feature, create child tasks with `lb create \"<title>\" -t task --parent <reviewed-id> -d \"<description>\"`. \
        For a reviewed task that has a parent, create sibling issue tasks under that same parent with `lb create \"<title>\" -t task --parent <parent-id> -d \"<description>\"`. \
        Only create a top-level issue when no relevant parent/work context exists.\n\n\
        If you see completed items that are still open, close them: lb close <id>\n\n\
        Be concise. Only flag real issues, not style nits.",
    );

    let escaped_prompt = prompt.replace('\'', "'\\''");
    let model = &opts.model;
    let agent = config.agent;
    let agent_name = agent.as_str();
    let agent_bin = agent.binary();
    let agent_restore_block = agent.restore_block();
    let agent_command = agent.shell_command(model, &escaped_prompt, None);

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

_mm_tool_init() {{
  local _out
  if _out=$("$@" 2>&1); then
    [ -n "$_out" ] && echo "$_out"
    return 0
  fi
  if echo "$_out" | grep -q "already initialized"; then
    return 0
  fi
  echo "$_out" >&2
  return 1
}}

_mm_commit_dockerfile_if_changed() {{
  if [ -n "$(git status --porcelain -- "$dockerfile_rel")" ]; then
    echo "::mrmouth::warning committing uncommitted Dockerfile self-update"
    git add -- "$dockerfile_rel" || return 0
    if git diff --cached --quiet -- "$dockerfile_rel"; then
      return 0
    fi
    git commit -m "Update mrmouth Dockerfile" -- "$dockerfile_rel" || true
  fi
}}

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"
dockerfile_rel="__DOCKERFILE_REL_PATH__"

# Clone repo
if [ ! -d "$work_dir/.git" ]; then
  if [ -n "$repo_url" ]; then
    git config --global --add safe.directory /host-repo
    echo "Cloning $repo_url (branch: $branch)..."
    git clone --branch "$branch" "$repo_url" "$work_dir"
  fi
fi
cd "$work_dir"
git config --global --add safe.directory "$work_dir"
work_repo_dir="${{MRMOUTH_WORK_REPO:-$work_dir}}"
if [ "$work_repo_dir" != "$work_dir" ] && [ -e "$work_repo_dir" ]; then
  git config --global --add safe.directory "$work_repo_dir"
fi

# Seed Dockerfile if absent (gives reviewer a file to read and modify)
dockerfile_path="$work_dir/$dockerfile_rel"
if [ ! -f "$dockerfile_path" ]; then
  mkdir -p "$(dirname "$dockerfile_path")"
  cat > "$dockerfile_path" << 'MRMOUTH_DOCKERFILE_EOF'
__DOCKERFILE_CONTENT__
MRMOUTH_DOCKERFILE_EOF
  echo "Seeded Dockerfile into workspace."
fi

# Initialize task tooling (only if matching branches exist)
if [ -d "$work_dir/.git" ]; then
  if git show-ref --quiet refs/heads/litebrite refs/remotes/origin/litebrite 2>/dev/null; then
    echo "Initializing litebrite..."
    _mm_tool_init lb init
    lb setup {agent_name} 2>/dev/null || true
    lb sync 2>/dev/null || true
  fi
  if git show-ref --quiet refs/heads/trapperkeeper refs/remotes/origin/trapperkeeper 2>/dev/null; then
    echo "Initializing trapperkeeper..."
    _mm_tool_init trk init
    trk setup {agent_name} 2>/dev/null || true
    trk sync 2>/dev/null || true
  fi
fi

{agent_restore_block}

# Run reviewer
echo "Starting code review..."
command -v {agent_bin} >/dev/null || {{ echo "::mrmouth::missing-tool tool={agent_bin} reason={agent_bin} binary not in image" >&2; exit 64; }}
{agent_command}
echo "Code review complete."

# Push state changes back so the host loop can sync them
if [ -d "$work_dir/.git" ]; then
  lb sync 2>/dev/null || true
  trk sync 2>/dev/null || true
  _mm_commit_dockerfile_if_changed
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
        None => (
            "file:///host-repo".to_string(),
            Some(repo_root.to_path_buf()),
        ),
    };

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let container_name = format!("review-{timestamp}");
    let volume = config.effective_volume(repo_root);
    let uses_real_remote = file_remote_path.is_none();

    // Create dedicated review log + jsonl files
    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let review_log_path = log_dir.join(format!("review-{timestamp}.log"));
    let review_jsonl_path = log_dir.join(format!("review-{timestamp}.jsonl"));

    let review_logger = match logger.and_then(|l| l.display_sink()) {
        Some(display) => Logger::with_display_handle(&review_log_path, display),
        None => Logger::new(&review_log_path),
    }
    .ok();

    let mut jsonl_writer: Option<BufWriter<File>> =
        File::create(&review_jsonl_path).ok().map(BufWriter::new);

    DockerBuilder::remove_container(&container_name);

    let docker = DockerBuilder::new(&config.image);
    emit_event(
        &opts.event_sink,
        MrmouthEvent::ReviewerLifecycle {
            action: ReviewerAction::Running,
            branch: opts.current_branch.clone(),
            commit_range: commit_range_label(&opts.commit_range),
        },
    );
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(|e| ReviewerError(format!("failed to build reviewer image: {e}")))?;

    let container_args = ContainerArgs {
        name: container_name.clone(),
        repo_url,
        branch: opts.current_branch.clone(),
        runner_script: tmp_path.to_path_buf(),
        volume,
        agent_home: config.agent.home_mount(),
        local: false,
        local_workspace_path: None,
        worktree_path: opts.worktree_path.clone(),
        file_remote_path,
        timeout_secs: None,
    };

    let mut handle = docker
        .run(&container_args)
        .map_err(|e| ReviewerError(format!("failed to start reviewer container: {e}")))?;

    let is_tty =
        logger.is_some_and(|l| l.display_supports_color()) || std::io::stdout().is_terminal();
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

    // If the reviewer committed and pushed a Dockerfile edit, fast-forward the
    // host checkout before extraction so we do not dirty the worktree with the
    // same content that is already in git.
    if uses_real_remote {
        pull_code_changes(repo_root, logger);
    }

    // Extract updated Dockerfile from container (reviewer may have modified it).
    // Failed reviewer runs can leave partial Dockerfile edits; do not copy those
    // back into the host checkout.
    if exit_code == 0 {
        let dockerfile_dest = repo_root.join(&config.dockerfile);
        let container_path = format!("/home/runner/workspace/{}", config.dockerfile);
        match DockerBuilder::copy_from_container_if_changed(
            &container_name,
            &container_path,
            &dockerfile_dest,
        ) {
            Ok(CopyFromContainerOutcome::Updated) => crate::logger::log(
                logger,
                "Extracted updated Dockerfile from reviewer container.",
            ),
            Ok(CopyFromContainerOutcome::Unchanged) => crate::logger::log(
                logger,
                "Reviewer Dockerfile matches host; leaving worktree unchanged.",
            ),
            Ok(CopyFromContainerOutcome::Missing) => {}
            Err(e) => crate::logger::log(
                logger,
                &format!("Warning: reviewer Dockerfile extraction failed: {e}"),
            ),
        }
    } else {
        crate::logger::log(
            logger,
            "Skipping reviewer Dockerfile extraction because the reviewer run failed.",
        );
    }

    DockerBuilder::remove_container(&container_name);

    if exit_code != 0 {
        crate::logger::log(
            logger,
            &format!("Reviewer container exited with code {exit_code}"),
        );
        emit_event(
            &opts.event_sink,
            MrmouthEvent::failure(
                "reviewer container exited",
                Some(exit_code),
                Some(format!("branch={}", opts.current_branch)),
            ),
        );
        return Err(ReviewerError(format!(
            "reviewer container exited with code {exit_code}"
        )));
    }

    crate::logger::log(logger, "Reviewer pass complete.");
    emit_event(
        &opts.event_sink,
        MrmouthEvent::ReviewerLifecycle {
            action: ReviewerAction::Finished,
            branch: opts.current_branch.clone(),
            commit_range: commit_range_label(&opts.commit_range),
        },
    );
    Ok(())
}

fn emit_event(sink: &Option<EventSinkHandle>, event: MrmouthEvent) {
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

fn commit_range_label(commit_range: &Option<(String, String)>) -> Option<String> {
    commit_range
        .as_ref()
        .map(|(before, after)| format!("{before}..{after}"))
}

fn review_code_container_path(worktree_path: Option<&Path>) -> &'static str {
    if worktree_path.is_some() {
        WORK_CONTAINER_PATH
    } else {
        BOOKKEEPING_CONTAINER_PATH
    }
}

fn review_scope_instructions(
    current_branch: &str,
    commit_range: &Option<(String, String)>,
    worktree_path: Option<&Path>,
) -> String {
    let code_repo = review_code_container_path(worktree_path);
    match commit_range {
        Some((before, after)) => format!(
            "Review ONLY the changes between commits {before}..{after} on branch '{current_branch}'. \
            Run `git -C {code_repo} diff {before}..{after}` to see what changed - do NOT review code outside this range. \
            Run code build and test commands in `{code_repo}`."
        ),
        None => format!(
            "Review the changes on branch '{current_branch}' against the project spec (SPEC.md). \
            Use `git -C {code_repo} diff` and `git -C {code_repo} log` to understand what changed. \
            Run code build and test commands in `{code_repo}`."
        ),
    }
}

fn review_workspace_instructions(worktree_path: Option<&Path>) -> String {
    match worktree_path {
        Some(host_path) => format!(
            "Repository layout: Litebrite and Trapperkeeper state live in `{BOOKKEEPING_CONTAINER_PATH}`. \
            The code worktree to review is `{WORK_CONTAINER_PATH}`, mounted from host path `{}`. \
            Run `lb` and `trk` commands from `{BOOKKEEPING_CONTAINER_PATH}`; run git diff/log and build/test commands from `{WORK_CONTAINER_PATH}`. \
            Do not treat bookkeeping-only changes as the code implementation diff.",
            host_path.display()
        ),
        None => format!(
            "Repository layout: the code checkout and task tracking repo are both `{BOOKKEEPING_CONTAINER_PATH}`. \
            Run git, build/test, `lb`, and `trk` commands there."
        ),
    }
}

fn review_purpose_instructions(review_target: Option<&ReviewTarget>) -> String {
    match review_target {
        Some(target) => format!(
            "The run was intended to satisfy this litebrite item: {} ({}). \
            Run `lb show {}` and treat that item title, description, parent/child context, \
            and acceptance details as the primary purpose of the change. Also read relevant \
            parent items when the item has a parent. \
            Review the diff for fitness for purpose: does the specific change accomplish \
            what this item asked for, without closing the item prematurely or leaving required \
            behavior/tests/documentation unfinished? \
            If you file review issues, keep them attached to this work: for a reviewed epic or \
            feature, create child tasks under {}; for a reviewed task with a parent, create \
            sibling tasks under that parent.",
            target.item_id, target.label, target.item_id, target.item_id
        ),
        None => "No single litebrite item was provided for this review. Use lb list, git log, \
            commit messages, and the diff to infer the intended work, then review the change \
            for fitness for that purpose as well as against SPEC.md."
            .to_string(),
    }
}

fn pull_code_changes(repo_root: &Path, logger: Option<&Logger>) {
    crate::logger::log(logger, "Pulling reviewer code changes from remote...");
    let pull_output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "pull", "--ff-only"])
        .output();
    match pull_output {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            if stderr.contains("Already up to date") || stderr.is_empty() {
                crate::logger::log(logger, "No reviewer code changes to pull.");
            } else {
                crate::logger::log(
                    logger,
                    &format!("Warning: reviewer git pull failed: {}", stderr.trim()),
                );
            }
        }
        Err(e) => crate::logger::log(logger, &format!("Warning: reviewer git pull failed: {e}")),
    }
}

fn git_remote_url(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_purpose_instructions_use_target_item_as_primary_purpose() {
        let instructions = review_purpose_instructions(Some(&ReviewTarget {
            item_id: "lb-1234".to_string(),
            label: "[task] Add review context".to_string(),
        }));

        assert!(instructions.contains("lb show lb-1234"));
        assert!(instructions.contains("primary purpose"));
        assert!(instructions.contains("fitness for purpose"));
        assert!(instructions.contains("create child tasks under lb-1234"));
        assert!(instructions.contains("sibling tasks under that parent"));
    }

    #[test]
    fn review_purpose_instructions_infer_purpose_without_target() {
        let instructions = review_purpose_instructions(None);

        assert!(instructions.contains("No single litebrite item"));
        assert!(instructions.contains("infer the intended work"));
        assert!(instructions.contains("against SPEC.md"));
    }

    #[test]
    fn split_worktree_scope_points_git_diff_at_code_checkout() {
        let range = Some(("abc123".to_string(), "def456".to_string()));
        let instructions =
            review_scope_instructions("feature", &range, Some(Path::new("/host/service")));

        assert!(instructions.contains("git -C /home/runner/worktree diff abc123..def456"));
        assert!(
            instructions.contains("Run code build and test commands in `/home/runner/worktree`")
        );
        assert!(!instructions.contains("git diff abc123..def456"));
    }

    #[test]
    fn split_worktree_instructions_keep_litebrite_in_bookkeeping_repo() {
        let instructions = review_workspace_instructions(Some(Path::new("/host/service")));

        assert!(instructions
            .contains("Litebrite and Trapperkeeper state live in `/home/runner/workspace`"));
        assert!(instructions.contains("code worktree to review is `/home/runner/worktree`"));
        assert!(instructions.contains("Run `lb` and `trk` commands from `/home/runner/workspace`"));
        assert!(instructions
            .contains("run git diff/log and build/test commands from `/home/runner/worktree`"));
    }

    #[test]
    fn same_repo_scope_uses_workspace_checkout() {
        let range = Some(("abc123".to_string(), "def456".to_string()));
        let instructions = review_scope_instructions("feature", &range, None);

        assert!(instructions.contains("git -C /home/runner/workspace diff abc123..def456"));
        assert!(
            instructions.contains("Run code build and test commands in `/home/runner/workspace`")
        );
    }
}
