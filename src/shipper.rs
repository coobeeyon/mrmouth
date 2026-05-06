use std::fs::File;
use std::io::{BufWriter, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::docker::{ContainerArgs, DockerBuilder};
use crate::logger::Logger;
use crate::stream_fmt::{self, StreamFormatter};
use crate::streaming::{self, StreamTarget};

pub struct ShipperOptions {
    pub model: String,
    pub current_branch: String,
    pub parent_branch: String,
}

/// Run the shipper agent: check readiness and merge branch into parent.
pub fn execute(
    config: &Config,
    repo_root: &Path,
    opts: &ShipperOptions,
    logger: Option<&Logger>,
) -> Result<(), ShipperError> {
    crate::logger::log(
        logger,
        &format!(
            "SHIPPING  {} -> {}",
            opts.current_branch, opts.parent_branch
        ),
    );

    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);

    // 1. Check readiness
    check_ready(
        config,
        repo_root,
        &opts.current_branch,
        &opts.model,
        logger,
        &log_dir,
    )?;

    // 2. Merge current branch into parent
    merge_branch(repo_root, &opts.current_branch, &opts.parent_branch, logger)?;

    Ok(())
}

fn check_ready(
    config: &Config,
    repo_root: &Path,
    current_branch: &str,
    model: &str,
    logger: Option<&Logger>,
    log_dir: &Path,
) -> Result<(), ShipperError> {
    let schema = r#"{"type":"object","properties":{"status":{"type":"string","enum":["READY","BLOCKED"]},"reason":{"type":"string"}},"required":["status","reason"]}"#;

    let preamble = crate::prompt::SYSTEM_PREAMBLE;
    let prompt = format!(
        "## System\n\n{preamble}\n\n\
        You are the **Shipper** (readiness check). Your only job is to verify the branch is ready to merge.\n\n\
        ## Instructions\n\n\
        Check if branch '{current_branch}' is ready to ship. \
        Check: (1) run 'lb list -s open' to confirm no open blocking tasks exist, \
        (2) discover the project's build and test commands by examining the project \
        structure (Makefile, package.json, Cargo.toml, etc.) and run them to verify \
        everything compiles and all tests pass. \
        Return READY only if both checks pass. Return BLOCKED if any tasks are open \
        or any build/test fails, with a clear reason."
    );

    let escaped_prompt = prompt.replace('\'', "'\\''");
    let escaped_schema = schema.replace('\'', "'\\''");
    let agent = config.agent;
    let agent_name = agent.as_str();
    let agent_bin = agent.binary();
    let agent_restore_block = agent.restore_block();
    let agent_command = agent.shell_command(model, &escaped_prompt, Some(&escaped_schema));

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

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"

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

# Run readiness check
echo "Starting readiness check..."
command -v {agent_bin} >/dev/null || {{ echo "::mrmouth::missing-tool tool={agent_bin} reason={agent_bin} binary not in image" >&2; exit 64; }}
{agent_command}
echo "Readiness check complete."

# Push state changes back so the host loop can sync them
if [ -d "$work_dir/.git" ]; then
  lb sync 2>/dev/null || true
  trk sync 2>/dev/null || true
  git push 2>/dev/null || true
fi
"#
    );

    let mut tmp = tempfile::NamedTempFile::new()
        .map_err(|e| ShipperError(format!("failed to create readiness script: {e}")))?;
    tmp.write_all(script.as_bytes())
        .map_err(|e| ShipperError(format!("failed to write readiness script: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(tmp.path(), perms)
            .map_err(|e| ShipperError(format!("failed to set script permissions: {e}")))?;
    }

    let (repo_url, file_remote_path) = match git_remote_url(repo_root) {
        Some(url) => (url, None),
        None => (
            "file:///host-repo".to_string(),
            Some(repo_root.to_path_buf()),
        ),
    };

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let container_name = format!("readiness-{timestamp}");
    let volume = config.effective_volume(repo_root);

    // Create dedicated ship log + jsonl files
    let ship_log_path = log_dir.join(format!("ship-{timestamp}.log"));
    let ship_jsonl_path = log_dir.join(format!("ship-{timestamp}.jsonl"));

    let ship_logger = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => Logger::with_tui(&ship_log_path, tui.clone()),
        None => Logger::new(&ship_log_path),
    }
    .ok();

    let mut jsonl_writer: Option<BufWriter<File>> =
        File::create(&ship_jsonl_path).ok().map(BufWriter::new);

    DockerBuilder::remove_container(&container_name);

    let docker = DockerBuilder::new(&config.image);
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(|e| ShipperError(format!("failed to build shipper image: {e}")))?;

    let container_args = ContainerArgs {
        name: container_name.clone(),
        repo_url,
        branch: current_branch.to_string(),
        runner_script: tmp.path().to_path_buf(),
        volume,
        agent_home: config.agent.home_mount(),
        local: false,
        file_remote_path,
        timeout_secs: None,
    };

    let mut handle = docker
        .run(&container_args)
        .map_err(|e| ShipperError(format!("failed to start readiness container: {e}")))?;

    let is_tty = logger.is_some_and(|l| l.has_tui()) || std::io::stdout().is_terminal();
    let mut formatter = StreamFormatter::new(is_tty);
    let mut result_text = String::new();

    handle
        .stream_output(|line| {
            // Write raw JSONL to dedicated file
            if let Some(w) = jsonl_writer.as_mut() {
                let _ = writeln!(w, "{line}");
            }

            if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
                if event.get("type").and_then(|v| v.as_str()) == Some("result") {
                    // structured_output (JSON object) takes priority over result (string)
                    // when --json-schema is used
                    if let Some(so) = event.get("structured_output") {
                        if so.is_object() || so.is_array() {
                            result_text = serde_json::to_string(so).unwrap_or_default();
                        } else if let Some(s) = so.as_str() {
                            result_text = s.to_string();
                        }
                    } else if let Some(r) = event.get("result").and_then(|v| v.as_str()) {
                        result_text = r.to_string();
                    }
                }
            }
            if let Some(formatted) = stream_fmt::format_line(&mut formatter, line) {
                // Display to TUI/stderr
                match logger {
                    Some(l) => l.display(&formatted),
                    None => eprintln!("{formatted}"),
                }
                // Write formatted text to dedicated ship log
                if let Some(sl) = ship_logger.as_ref() {
                    sl.log_file_only(&formatted);
                }
            }
        })
        .map_err(|e| ShipperError(format!("streaming error: {e}")))?;

    // Flush dedicated JSONL writer
    if let Some(w) = jsonl_writer.as_mut() {
        let _ = w.flush();
    }

    let exit_code = handle
        .wait()
        .map_err(|e| ShipperError(format!("container wait failed: {e}")))?;

    DockerBuilder::remove_container(&container_name);

    if exit_code != 0 {
        return Err(ShipperError(format!(
            "readiness check container exited with code {exit_code}"
        )));
    }

    let parsed: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log(
                logger,
                &format!("WARNING: readiness check returned invalid JSON: {e}"),
            );
            return Err(ShipperError(format!(
                "readiness check returned invalid JSON: {e}"
            )));
        }
    };
    let status = parsed["status"].as_str().unwrap_or("BLOCKED");
    let reason = parsed["reason"].as_str().unwrap_or("no reason given");

    if status == "BLOCKED" {
        return Err(ShipperError(format!("branch not ready to ship: {reason}")));
    }

    crate::logger::log(logger, &format!("Readiness check passed: {reason}"));
    Ok(())
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

fn merge_branch(
    repo_root: &Path,
    current_branch: &str,
    parent_branch: &str,
    logger: Option<&Logger>,
) -> Result<(), ShipperError> {
    let has_github_remote = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("github.com"))
        .unwrap_or(false);

    if has_github_remote {
        crate::logger::log(
            logger,
            &format!("Creating and merging PR: {current_branch} -> {parent_branch}"),
        );

        let pr_create = Command::new("gh")
            .args([
                "pr",
                "create",
                "--base",
                parent_branch,
                "--head",
                current_branch,
                "--title",
                &format!("Merge {current_branch}"),
                "--body",
                "Auto-merged by mrmouth shipper.",
            ])
            .current_dir(repo_root)
            .output()
            .map_err(|e| ShipperError(format!("failed to create PR: {e}")))?;

        if !pr_create.status.success() {
            let stderr = String::from_utf8_lossy(&pr_create.stderr);
            if !stderr.contains("already exists") {
                return Err(ShipperError(format!("gh pr create failed: {stderr}")));
            }
        }

        let pr_merge = Command::new("gh")
            .args(["pr", "merge", current_branch, "--merge", "--delete-branch"])
            .current_dir(repo_root)
            .output()
            .map_err(|e| ShipperError(format!("failed to merge PR: {e}")))?;

        if !pr_merge.status.success() {
            return Err(ShipperError("gh pr merge failed".into()));
        }

        let _ = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "checkout",
                parent_branch,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "pull", "--ff-only"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else {
        crate::logger::log(
            logger,
            &format!("Merging {current_branch} into {parent_branch} (no-ff)..."),
        );

        let checkout = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "checkout",
                parent_branch,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| ShipperError(format!("failed to checkout {parent_branch}: {e}")))?;
        if !checkout.success() {
            return Err(ShipperError(format!("failed to checkout {parent_branch}")));
        }

        let merge = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "merge",
                "--no-ff",
                current_branch,
                "-m",
                &format!("Merge branch '{current_branch}'"),
            ])
            .output()
            .map_err(|e| ShipperError(format!("merge failed: {e}")))?;
        if !merge.status.success() {
            return Err(ShipperError(format!("merge of {current_branch} failed")));
        }

        let _ = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "branch",
                "-d",
                current_branch,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    Ok(())
}

pub fn generate_branch_name(
    config: &Config,
    repo_root: &Path,
    model: &str,
    logger: Option<&Logger>,
) -> Result<String, ShipperError> {
    let schema = r#"{"type":"object","properties":{"name":{"type":"string","description":"2-4 word kebab-case slug for the branch"}},"required":["name"]}"#;

    let prompt = "Read SPEC.md and the current litebrite task state (lb ready). \
        Generate a short 2-4 word kebab-case name describing the next batch of work. \
        Examples: review-ship-flow, docker-caching, test-coverage. \
        Just the slug, no 'feat-' prefix.";

    let mut cmd = streaming::agent_stream_cmd_with_schema(
        config.agent,
        repo_root,
        model,
        "Read,Bash(lb *)",
        schema,
    );

    let mut child = cmd.spawn().map_err(|e| {
        ShipperError(format!(
            "failed to generate branch name with {}: {e}",
            config.agent.as_str()
        ))
    })?;

    streaming::send_prompt(&mut child, prompt);

    let target = match logger.and_then(|l| l.tui_sender()) {
        Some(tui) => StreamTarget::Tui(tui.clone()),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let (result_text, exit_code) =
        streaming::run_streaming_claude(child, &mut formatter, logger, &target, &mut None)
            .map_err(|e| ShipperError(format!("branch name generation failed: {e}")))?;

    if exit_code != 0 {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        return Ok(format!("feat-{ts}"));
    }

    let parsed: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log(logger, &format!("WARNING: branch name generation returned invalid JSON (using timestamp fallback): {e}"));
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            return Ok(format!("feat-{ts}"));
        }
    };
    let slug = parsed["name"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_lowercase()
        .replace(' ', "-");

    if slug.is_empty() {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        Ok(format!("feat-{ts}"))
    } else {
        Ok(format!("feat-{slug}"))
    }
}

pub fn create_and_push_branch(
    repo_root: &Path,
    branch_name: &str,
    logger: Option<&Logger>,
) -> Result<(), ShipperError> {
    let branch_exists = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch_name}"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let checkout_args: &[&str] = if branch_exists {
        &["checkout", branch_name]
    } else {
        &["checkout", "-b", branch_name]
    };

    let status = Command::new("git")
        .arg("-C")
        .arg(&*repo_root.to_string_lossy())
        .args(checkout_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| ShipperError(format!("failed to create branch {branch_name}: {e}")))?;

    if !status.success() {
        return Err(ShipperError(format!(
            "git checkout -b {branch_name} failed"
        )));
    }

    let has_remote = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_remote {
        let push = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "push",
                "-u",
                "origin",
                branch_name,
            ])
            .output()
            .map_err(|e| ShipperError(format!("failed to push branch: {e}")))?;

        if !push.status.success() {
            crate::logger::log(
                logger,
                &format!("Warning: failed to push branch {branch_name} to origin"),
            );
        }
    }

    crate::logger::log(logger, &format!("Created branch: {branch_name}"));
    Ok(())
}

#[derive(Debug)]
pub struct ShipperError(String);

impl std::fmt::Display for ShipperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ShipperError {}
