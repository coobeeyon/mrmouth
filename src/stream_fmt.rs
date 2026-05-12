use serde_json::Value;
use std::collections::HashSet;
use std::io::{self, BufRead, Write};

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collapse large fenced code blocks (file dumps) to a single line.
fn collapse_code_blocks(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut in_block = false;
    let mut block_start: usize = 0;

    for (i, line) in lines.iter().enumerate() {
        if !in_block && line.starts_with("```") {
            in_block = true;
            block_start = i;
            continue;
        }
        if in_block && line.trim_end() == "```" {
            let block_len = i - block_start - 1;
            if block_len > 10 {
                out.push(format!("({block_len} lines omitted)"));
            } else {
                for line in &lines[block_start..=i] {
                    out.push(line.to_string());
                }
            }
            in_block = false;
            continue;
        }
        if !in_block {
            out.push(line.to_string());
        }
    }
    if in_block {
        for line in &lines[block_start..] {
            out.push(line.to_string());
        }
    }

    out.join("\n")
}

const TODO_TOOLS: &[&str] = &[
    "TodoRead",
    "TodoWrite",
    "TaskCreate",
    "TaskGet",
    "TaskUpdate",
    "TaskList",
];
const QUIET_TOOLS: &[&str] = &["Read"];

struct Formatter {
    suppressed_ids: HashSet<String>,
    is_tty: bool,
}

impl Formatter {
    fn new(is_tty: bool) -> Self {
        Self {
            suppressed_ids: HashSet::new(),
            is_tty,
        }
    }

    fn dim(&self) -> &str {
        if self.is_tty {
            DIM
        } else {
            ""
        }
    }
    fn bold(&self) -> &str {
        if self.is_tty {
            BOLD
        } else {
            ""
        }
    }
    fn cyan(&self) -> &str {
        if self.is_tty {
            CYAN
        } else {
            ""
        }
    }
    fn yellow(&self) -> &str {
        if self.is_tty {
            YELLOW
        } else {
            ""
        }
    }
    fn green(&self) -> &str {
        if self.is_tty {
            GREEN
        } else {
            ""
        }
    }
    fn red(&self) -> &str {
        if self.is_tty {
            RED
        } else {
            ""
        }
    }
    fn reset(&self) -> &str {
        if self.is_tty {
            RESET
        } else {
            ""
        }
    }

    fn format_event(&mut self, line: &str) -> Option<String> {
        let event: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Some(line.to_string()),
        };

        let event_type = event.get("type")?.as_str()?;

        match event_type {
            "system" => self.format_system(&event),
            "assistant" => self.format_assistant(&event),
            "user" => self.format_user(&event),
            "result" => self.format_result(&event),
            "thread.started" => self.format_codex_thread_started(&event),
            "turn.started" => Some(format!("{}[turn]{} started", self.cyan(), self.reset())),
            "turn.completed" => self.format_codex_turn_completed(&event),
            "item.started" => self.format_codex_item_started(&event),
            "item.completed" => self.format_codex_item_completed(&event),
            _ => None,
        }
    }

    fn format_codex_thread_started(&self, event: &Value) -> Option<String> {
        let thread_id = event
            .get("thread_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        Some(format!(
            "{}{}[codex]{}\n  thread={thread_id}",
            self.bold(),
            self.cyan(),
            self.reset()
        ))
    }

    fn format_system(&self, event: &Value) -> Option<String> {
        let subtype = event.get("subtype")?.as_str()?;
        if subtype == "init" {
            let model = event
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Some(format!(
                "{}{}[init]{}\n  model={model}",
                self.bold(),
                self.cyan(),
                self.reset()
            ))
        } else {
            None
        }
    }

    fn format_assistant(&mut self, event: &Value) -> Option<String> {
        let content = event.get("message")?.get("content")?.as_array()?;
        let mut parts: Vec<String> = Vec::new();

        for block in content {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");

            match block_type {
                "tool_use" => {
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(Value::Null);

                    if TODO_TOOLS.contains(&name) {
                        self.suppressed_ids.insert(id.to_string());
                        continue;
                    }
                    if QUIET_TOOLS.contains(&name) {
                        self.suppressed_ids.insert(id.to_string());
                    }

                    let formatted = match name {
                        "Bash" => {
                            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
                            format!("{}[{name}]{}\n{}", self.yellow(), self.reset(), indent(cmd))
                        }
                        "Read" | "Edit" | "Write" => {
                            let path = input
                                .get("file_path")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            format!("{}[{name}]{} {path}", self.yellow(), self.reset())
                        }
                        "Grep" => {
                            let pattern =
                                input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                            format!("{}[{name}]{}\n  /{pattern}/", self.yellow(), self.reset())
                        }
                        "Glob" => {
                            let pattern =
                                input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                            format!("{}[{name}]{}\n  {pattern}", self.yellow(), self.reset())
                        }
                        "Agent" => {
                            let desc = input
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let agent_type = input
                                .get("subagent_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            format!(
                                "{}[{name}]{} {desc} ({agent_type})",
                                self.yellow(),
                                self.reset()
                            )
                        }
                        _ => {
                            format!("{}[{name}]{}\n  {}", self.yellow(), self.reset(), input)
                        }
                    };
                    parts.push(formatted);
                }
                "thinking" => {
                    let thinking = block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                    if !thinking.is_empty() {
                        parts.push(format!(
                            "{}[think]{}\n{}{}{}",
                            self.dim(),
                            self.reset(),
                            self.dim(),
                            indent(thinking),
                            self.reset()
                        ));
                    }
                }
                "text" => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    parts.push(format!(
                        "{}[text]{}\n{}",
                        self.green(),
                        self.reset(),
                        indent(text)
                    ));
                }
                _ => {}
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    fn format_user(&self, event: &Value) -> Option<String> {
        let content = event.get("message")?.get("content")?.as_array()?;

        for block in content {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if block_type != "tool_result" {
                continue;
            }

            let tool_use_id = block
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if self.suppressed_ids.contains(tool_use_id) {
                continue;
            }

            let raw = block.get("content");
            let result_text = match raw {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(arr)) => arr
                    .iter()
                    .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(other) => other.to_string(),
                None => String::new(),
            };

            let collapsed = collapse_code_blocks(&result_text);
            let is_error = block
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if is_error {
                return Some(format!(
                    "{}[error]{}\n{}",
                    self.red(),
                    self.reset(),
                    indent(&collapsed)
                ));
            }
            if !collapsed.is_empty() {
                return Some(format!(
                    "{}[result]{}\n{}{}{}",
                    self.dim(),
                    self.reset(),
                    self.dim(),
                    indent(&collapsed),
                    self.reset()
                ));
            }
        }
        None
    }

    fn format_result(&self, event: &Value) -> Option<String> {
        let result = event
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("(no summary)");
        let cost = event.get("total_cost_usd").and_then(|v| v.as_f64());
        let turns = event.get("num_turns").and_then(|v| v.as_u64());

        let cost_str = cost
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "?".to_string());
        let turns_str = turns
            .map(|t| t.to_string())
            .unwrap_or_else(|| "?".to_string());

        let summary = if result.is_empty() {
            "(no summary)"
        } else {
            result
        };

        Some(format!(
            "\n{}{}[done]{}\n  turns={turns_str} cost={cost_str}\n{}",
            self.bold(),
            self.green(),
            self.reset(),
            indent(summary)
        ))
    }

    fn format_codex_turn_completed(&self, event: &Value) -> Option<String> {
        let usage = event.get("usage")?;
        let input = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let reasoning = usage
            .get("reasoning_output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Some(format!(
            "{}{}[done]{}\n  input_tokens={input} output_tokens={output} reasoning_tokens={reasoning}",
            self.bold(),
            self.green(),
            self.reset()
        ))
    }

    fn format_codex_item_started(&self, event: &Value) -> Option<String> {
        let item = event.get("item")?;
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "command_execution" => {
                let command = item.get("command").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!(
                    "{}[Bash]{}\n{}",
                    self.yellow(),
                    self.reset(),
                    indent(command)
                ))
            }
            "file_change" => Some(format!(
                "{}[Edit]{} {}",
                self.yellow(),
                self.reset(),
                format_codex_file_changes(item)
            )),
            _ => None,
        }
    }

    fn format_codex_item_completed(&self, event: &Value) -> Option<String> {
        let item = event.get("item")?;
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "agent_message" => {
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if text.is_empty() {
                    None
                } else {
                    Some(format!(
                        "{}[text]{}\n{}",
                        self.green(),
                        self.reset(),
                        indent(text)
                    ))
                }
            }
            "command_execution" => {
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let exit_code = item.get("exit_code").and_then(|v| v.as_i64());
                let output = item
                    .get("aggregated_output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let label = if status == "failed" || exit_code.is_some_and(|code| code != 0) {
                    format!("{}[error]{}", self.red(), self.reset())
                } else {
                    format!("{}[result]{}", self.dim(), self.reset())
                };
                let exit = exit_code
                    .map(|code| format!(" exit={code}"))
                    .unwrap_or_default();
                if output.is_empty() {
                    Some(format!("{label}{exit}"))
                } else {
                    Some(format!(
                        "{label}{exit}\n{}{}{}",
                        self.dim(),
                        indent(&collapse_code_blocks(output)),
                        self.reset()
                    ))
                }
            }
            "file_change" => Some(format!(
                "{}[Edit]{} {}",
                self.yellow(),
                self.reset(),
                format_codex_file_changes(item)
            )),
            _ => None,
        }
    }
}

fn format_codex_file_changes(item: &Value) -> String {
    let Some(changes) = item.get("changes").and_then(|v| v.as_array()) else {
        return "files changed".to_string();
    };

    changes
        .iter()
        .filter_map(|change| {
            let path = change.get("path").and_then(|v| v.as_str())?;
            let kind = change
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("change");
            Some(format!("{kind} {path}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format Claude Code stream-json from stdin and write to stdout.
pub fn run_stream_formatter(is_tty: bool) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut formatter = Formatter::new(is_tty);

    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(formatted) = formatter.format_event(trimmed) {
            writeln!(out, "{formatted}")?;
        }
    }

    Ok(())
}

/// Format a single JSONL line. Returns None if the event should be suppressed.
/// This is the main public API for use by `mrmouth run` when processing container output.
pub fn format_line(formatter: &mut StreamFormatter, line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    formatter.0.format_event(trimmed)
}

/// Public wrapper around the internal Formatter for use by other modules.
pub struct StreamFormatter(Formatter);

impl StreamFormatter {
    pub fn new(is_tty: bool) -> Self {
        Self(Formatter::new(is_tty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(line: &str) -> Option<String> {
        let mut f = Formatter::new(false);
        f.format_event(line)
    }

    #[test]
    fn test_non_json_passthrough() {
        assert_eq!(fmt("not json"), Some("not json".to_string()));
    }

    #[test]
    fn test_system_init() {
        let line = r#"{"type":"system","subtype":"init","model":"claude-opus-4-6"}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[init]"));
        assert!(out.contains("model=claude-opus-4-6"));
    }

    #[test]
    fn test_assistant_text() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[text]"));
        assert!(out.contains("hello world"));
    }

    #[test]
    fn test_assistant_bash() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Bash]"));
        assert!(out.contains("ls -la"));
    }

    #[test]
    fn test_assistant_read() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/foo/bar.rs"}}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Read]"));
        assert!(out.contains("/foo/bar.rs"));
    }

    #[test]
    fn test_todo_tools_suppressed() {
        let mut f = Formatter::new(false);
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{}}]}}"#;
        assert!(f.format_event(line).is_none());
        assert!(f.suppressed_ids.contains("t1"));
    }

    #[test]
    fn test_user_tool_result() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"some output"}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[result]"));
        assert!(out.contains("  some output"));
    }

    #[test]
    fn test_user_error_result() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"bad stuff","is_error":true}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[error]"));
        assert!(out.contains("bad stuff"));
    }

    #[test]
    fn test_suppressed_tool_result() {
        let mut f = Formatter::new(false);
        // First, suppress via a Read tool_use
        let tool_use = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/x"}}]}}"#;
        f.format_event(tool_use);
        assert!(f.suppressed_ids.contains("r1"));

        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"r1","content":"file contents"}]}}"#;
        assert!(f.format_event(result).is_none());
    }

    #[test]
    fn test_result_event() {
        let line = r#"{"type":"result","result":"All done","total_cost_usd":0.1234,"num_turns":5}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[done]"));
        assert!(out.contains("turns=5"));
        assert!(out.contains("$0.1234"));
        assert!(out.contains("All done"));
    }

    #[test]
    fn test_collapse_code_blocks_small() {
        let input = "before\n```rust\nfn main() {}\n```\nafter";
        let result = collapse_code_blocks(input);
        assert!(result.contains("```rust"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn test_collapse_code_blocks_large() {
        let mut lines = vec!["before".to_string(), "```".to_string()];
        for i in 0..15 {
            lines.push(format!("line {i}"));
        }
        lines.push("```".to_string());
        lines.push("after".to_string());
        let input = lines.join("\n");
        let result = collapse_code_blocks(&input);
        assert!(result.contains("(15 lines omitted)"));
        assert!(!result.contains("line 0"));
    }

    #[test]
    fn test_thinking_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"let me think..."}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[think]"));
        assert!(out.contains("let me think..."));
    }

    #[test]
    fn test_agent_tool() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a1","name":"Agent","input":{"description":"search codebase","subagent_type":"Explore"}}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Agent]"));
        assert!(out.contains("search codebase"));
        assert!(out.contains("Explore"));
    }

    #[test]
    fn test_grep_tool() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"fn main"}}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Grep]"));
        assert!(out.contains("/fn main/"));
    }

    #[test]
    fn test_glob_tool() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"g1","name":"Glob","input":{"pattern":"**/*.rs"}}]}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Glob]"));
        assert!(out.contains("**/*.rs"));
    }

    #[test]
    fn test_codex_agent_message() {
        let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"I will inspect files."}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[text]"));
        assert!(out.contains("I will inspect files."));
    }

    #[test]
    fn test_codex_command_started() {
        let line = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'lb list'","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Bash]"));
        assert!(out.contains("lb list"));
    }

    #[test]
    fn test_codex_command_completed() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'lb list'","aggregated_output":"ID TYPE\nlb-123 task\n","exit_code":0,"status":"completed"}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[result]"));
        assert!(out.contains("ID TYPE"));
        assert!(out.contains("lb-123 task"));
    }

    #[test]
    fn test_codex_command_failed() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"rg --files","aggregated_output":"rg: command not found\n","exit_code":127,"status":"failed"}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[error]"));
        assert!(out.contains("exit=127"));
        assert!(out.contains("rg: command not found"));
    }

    #[test]
    fn test_codex_file_change() {
        let line = r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"/repo/src/main.rs","kind":"update"}],"status":"completed"}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[Edit]"));
        assert!(out.contains("update /repo/src/main.rs"));
    }

    #[test]
    fn test_codex_turn_completed() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":20,"reasoning_output_tokens":5}}"#;
        let out = fmt(line).unwrap();
        assert!(out.contains("[done]"));
        assert!(out.contains("input_tokens=10"));
        assert!(out.contains("output_tokens=20"));
        assert!(out.contains("reasoning_tokens=5"));
    }
}
