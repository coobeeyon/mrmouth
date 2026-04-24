use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, IsTerminal, Write};
use std::process::{Child, Stdio};

use crate::logger::Logger;
use crate::stream_fmt::{self, StreamFormatter};
use crate::tui::TuiSender;

/// Output destination for streaming claude output.
pub enum StreamTarget {
    /// Route formatted output to a TUI pane.
    Tui(TuiSender),
    /// Print formatted output to stderr (non-TUI mode).
    Stderr,
    /// Print formatted output to stdout (e.g. standalone summary command).
    Stdout,
}

impl StreamTarget {
    /// Whether ANSI color codes should be emitted for this target.
    /// TUI always supports colors; terminal targets check is_terminal().
    pub fn supports_color(&self) -> bool {
        match self {
            StreamTarget::Tui(_) => true,
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

            // Check for the "result" event to capture its result field.
            // When --json-schema is used, the structured output is in
            // "structured_output" (a JSON object), not "result" (empty string).
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
        StreamTarget::Tui(sender) => sender.send_line(text),
        StreamTarget::Stderr => eprintln!("{text}"),
        StreamTarget::Stdout => println!("{text}"),
    }
}
