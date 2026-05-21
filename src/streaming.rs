use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, IsTerminal, Write};
use std::process::{Child, Stdio};

use crate::agent::AgentKind;
use crate::logger::{DisplaySinkHandle, Logger};
use crate::stream_fmt::{self, StreamFormatter};

/// Output destination for streaming claude output.
pub enum StreamTarget {
    /// Route formatted output to a caller-provided display sink.
    Display(DisplaySinkHandle),
    /// Print formatted output to stderr (non-TUI mode).
    Stderr,
    /// Print formatted output to stdout (e.g. standalone summary command).
    Stdout,
}

impl StreamTarget {
    /// Whether ANSI color codes should be emitted for this target.
    /// Custom sinks report their own capability; terminal targets check is_terminal().
    pub fn supports_color(&self) -> bool {
        match self {
            StreamTarget::Display(sink) => sink.supports_color(),
            StreamTarget::Stderr => std::io::stderr().is_terminal(),
            StreamTarget::Stdout => std::io::stdout().is_terminal(),
        }
    }
}

/// Run a claude CLI child process that uses `--output-format stream-json`,
/// formatting its stdout through the stream formatter and routing to the
/// appropriate display target. Returns the `result` field from the final
/// "result" event (for structured output parsing by callers).
///
/// This function:
/// 1. Tees child stderr via `logger.tee_stderr()` (or drains to eprintln)
/// 2. Reads stdout JSONL line by line
/// 3. Routes each line through `stream_fmt::format_line()`
/// 4. Displays via StreamTarget (TUI pane or stderr)
/// 5. Logs formatted output to the log file
/// 6. Collects the final "result" event's result field
/// 7. Waits for child exit
pub fn run_streaming_claude(
    mut child: Child,
    formatter: &mut StreamFormatter,
    logger: Option<&Logger>,
    target: &StreamTarget,
    jsonl_writer: &mut Option<BufWriter<File>>,
) -> Result<(String, i32), std::io::Error> {
    // Tee stderr
    let tee_handle = child.stderr.take().map(|stderr| {
        if let Some(l) = logger {
            l.tee_stderr(stderr)
        } else {
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("{line}");
                }
            })
        }
    });

    // Read stdout JSONL
    let mut result_text = String::new();
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Write raw JSONL to dedicated log file when available
            if let Some(w) = jsonl_writer.as_mut() {
                let _ = writeln!(w, "{trimmed}");
            }

            // Capture final output from Claude stream-json and Codex exec
            // JSONL. Claude uses `type=result`; Codex has used message-style
            // final events across versions, so keep the extraction tolerant.
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if event.get("type").and_then(|v| v.as_str()) == Some("result") {
                    if let Some(so) = event.get("structured_output") {
                        if so.is_object() || so.is_array() {
                            result_text = serde_json::to_string(so).unwrap_or_default();
                        } else if let Some(s) = so.as_str() {
                            result_text = s.to_string();
                        }
                    } else if let Some(r) = event.get("result").and_then(|v| v.as_str()) {
                        result_text = r.to_string();
                    }
                } else if let Some(text) = extract_codex_result_text(&event) {
                    result_text = text;
                }
            }

            // Format and display
            if let Some(formatted) = stream_fmt::format_line(formatter, trimmed) {
                display(&formatted, target);
                if let Some(l) = logger {
                    l.log_file_only(&formatted);
                }
            }
        }
    }

    // Flush JSONL writer
    if let Some(w) = jsonl_writer.as_mut() {
        let _ = w.flush();
    }

    // Wait for stderr drain
    if let Some(h) = tee_handle {
        let _ = h.join();
    }

    // Wait for child exit
    let status = child.wait()?;
    let exit_code = status.code().unwrap_or(-1);

    Ok((result_text, exit_code))
}

fn extract_codex_result_text(event: &serde_json::Value) -> Option<String> {
    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let looks_final = event_type.contains("message")
        || event_type.contains("response")
        || event_type.contains("final")
        || event_type == "agent_message";
    if !looks_final {
        return None;
    }

    for key in [
        "message",
        "content",
        "text",
        "last_message",
        "final_response",
    ] {
        if let Some(s) = event.get(key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
    }

    if let Some(item) = event.get("item") {
        for key in ["message", "content", "text"] {
            if let Some(s) = item.get(key).and_then(|v| v.as_str()) {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }

    None
}

/// Build a provider-specific non-interactive agent command.
pub fn agent_stream_cmd(
    agent: AgentKind,
    repo_root: &std::path::Path,
    model: &str,
    allowed_tools: &str,
) -> std::process::Command {
    match agent {
        AgentKind::Claude => claude_stream_cmd(repo_root, model, allowed_tools),
        AgentKind::Codex => codex_stream_cmd(repo_root, model, None),
    }
}

/// Build a provider-specific command for a full coding runner. This is used by
/// current-container mode, where there is no wrapper script to add runner flags.
pub fn agent_runner_stream_cmd(
    agent: AgentKind,
    repo_root: &std::path::Path,
    model: &str,
) -> std::process::Command {
    match agent {
        AgentKind::Claude => claude_runner_stream_cmd(repo_root, model),
        AgentKind::Codex => codex_stream_cmd(repo_root, model, None),
    }
}

/// Build a provider-specific non-interactive agent command with structured
/// output when the provider supports a schema flag.
pub fn agent_stream_cmd_with_schema(
    agent: AgentKind,
    repo_root: &std::path::Path,
    model: &str,
    allowed_tools: &str,
    schema: &str,
) -> std::process::Command {
    match agent {
        AgentKind::Claude => claude_stream_cmd_with_schema(repo_root, model, allowed_tools, schema),
        AgentKind::Codex => codex_stream_cmd(repo_root, model, Some(schema)),
    }
}

/// Build a claude CLI Command with `--output-format stream-json` and standard
/// piping (stdin piped, stdout piped, stderr piped).
pub fn claude_stream_cmd(
    repo_root: &std::path::Path,
    model: &str,
    allowed_tools: &str,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("claude");
    cmd.args([
        "-p",
        "--verbose",
        "--no-session-persistence",
        "--model",
        model,
        "--effort",
        "xhigh",
        "--allowedTools",
        allowed_tools,
        "--output-format",
        "stream-json",
    ])
    .env("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .current_dir(repo_root);
    cmd
}

fn claude_runner_stream_cmd(repo_root: &std::path::Path, model: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("claude");
    cmd.args([
        "-p",
        "--dangerously-skip-permissions",
        "--verbose",
        "--no-session-persistence",
        "--model",
        model,
        "--effort",
        "xhigh",
        "--disallowedTools",
        "EnterPlanMode,ExitPlanMode",
        "--output-format",
        "stream-json",
    ])
    .env("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .current_dir(repo_root);
    cmd
}

fn codex_stream_cmd(
    repo_root: &std::path::Path,
    model: &str,
    schema: Option<&str>,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("codex");
    cmd.args([
        "exec",
        "--json",
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        "-C",
        &repo_root.to_string_lossy(),
    ]);
    if !model.trim().is_empty() {
        cmd.args(["--model", model]);
    }
    if let Some(schema) = schema {
        if let Some(path) = write_schema_tempfile(schema) {
            cmd.args(["--output-schema", &path.to_string_lossy()]);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(repo_root);
    cmd
}

fn write_schema_tempfile(schema: &str) -> Option<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "mrmouth-schema-{}-{}.json",
        std::process::id(),
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
    ));
    std::fs::write(&path, schema).ok()?;
    Some(path)
}

/// Build a claude CLI Command with `--output-format stream-json` plus
/// `--json-schema` for structured output.
pub fn claude_stream_cmd_with_schema(
    repo_root: &std::path::Path,
    model: &str,
    allowed_tools: &str,
    schema: &str,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("claude");
    cmd.args([
        "-p",
        "--verbose",
        "--no-session-persistence",
        "--model",
        model,
        "--effort",
        "xhigh",
        "--allowedTools",
        allowed_tools,
        "--output-format",
        "stream-json",
        "--json-schema",
        schema,
    ])
    .env("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .current_dir(repo_root);
    cmd
}

/// Send a prompt to the child's stdin and drop the handle.
pub fn send_prompt(child: &mut Child, prompt: &str) {
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }
}

fn display(text: &str, target: &StreamTarget) {
    match target {
        StreamTarget::Display(sink) => sink.display_line(text),
        StreamTarget::Stderr => eprintln!("{text}"),
        StreamTarget::Stdout => println!("{text}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_agent_command_uses_exec_json() {
        let cmd = agent_stream_cmd(
            AgentKind::Codex,
            std::path::Path::new("/tmp/workspace"),
            "gpt-5.2",
            "Read",
        );
        assert_eq!(cmd.get_program().to_string_lossy(), "codex");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"exec".to_string()));
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-5.2".to_string()));
    }

    #[test]
    fn codex_agent_command_omits_empty_model() {
        let cmd = agent_stream_cmd(
            AgentKind::Codex,
            std::path::Path::new("/tmp/workspace"),
            "",
            "Read",
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(!args.contains(&"--model".to_string()));
    }

    #[test]
    fn codex_result_extractor_accepts_message_events() {
        let event = serde_json::json!({
            "type": "agent_message",
            "message": "{\"action\":\"continue\",\"reason\":\"work remains\"}"
        });
        assert_eq!(
            extract_codex_result_text(&event).as_deref(),
            Some("{\"action\":\"continue\",\"reason\":\"work remains\"}")
        );
    }

    #[test]
    fn claude_runner_command_skips_permissions_and_plan_mode() {
        let cmd = agent_runner_stream_cmd(
            AgentKind::Claude,
            std::path::Path::new("/tmp/workspace"),
            "opus",
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--disallowedTools".to_string()));
        assert!(args.contains(&"EnterPlanMode,ExitPlanMode".to_string()));
    }
}
