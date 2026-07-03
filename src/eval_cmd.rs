use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::debrief::FailureDebrief;
use crate::events::LifecycleSummary;

#[derive(Debug)]
pub struct EvalOptions {
    pub cwd: Option<PathBuf>,
    pub output: PathBuf,
    pub command: Vec<String>,
}

#[derive(Debug)]
pub enum EvalError {
    EmptyCommand,
    Io {
        context: String,
        source: std::io::Error,
    },
    Serialize(serde_json::Error),
    ChildFailed {
        code: Option<i32>,
        report_path: PathBuf,
    },
}

impl EvalError {
    pub fn debrief(self) -> FailureDebrief {
        match self {
            Self::EmptyCommand => FailureDebrief::new("eval command is empty".into()),
            Self::Io { context, source } => FailureDebrief::new(format!("{context}: {source}")),
            Self::Serialize(e) => FailureDebrief::new(format!("serializing eval report: {e}")),
            Self::ChildFailed { code, report_path } => {
                let mut debrief = FailureDebrief::new(format!(
                    "eval command failed; report written to {}",
                    report_path.display()
                ));
                debrief.exit_code = code;
                debrief
            }
        }
    }
}

pub fn execute(repo_root: &Path, opts: EvalOptions) -> Result<(), EvalError> {
    if opts.command.is_empty() {
        return Err(EvalError::EmptyCommand);
    }

    let cwd = opts.cwd.clone().unwrap_or_else(|| repo_root.to_path_buf());
    let report_path = resolve_path(repo_root, &opts.output);
    let started = Instant::now();
    let child_output = Command::new(&opts.command[0])
        .args(&opts.command[1..])
        .current_dir(&cwd)
        .output()
        .map_err(|e| EvalError::Io {
            context: format!("running eval command `{}`", opts.command.join(" ")),
            source: e,
        })?;
    let wall_ms = millis_u64(started.elapsed());

    let stdout = String::from_utf8_lossy(&child_output.stdout);
    let lifecycle = analyze_lifecycle_stdout(&stdout, &cwd);
    let report = EvalReport {
        command: opts.command,
        cwd: cwd.display().to_string(),
        success: child_output.status.success(),
        exit_code: child_output.status.code(),
        wall_ms,
        stdout_bytes: child_output.stdout.len() as u64,
        stderr_bytes: child_output.stderr.len() as u64,
        lifecycle,
    };

    write_report(&report_path, &report)?;
    println!("eval report: {}", report_path.display());

    if report.success {
        Ok(())
    } else {
        Err(EvalError::ChildFailed {
            code: report.exit_code,
            report_path,
        })
    }
}

fn write_report(path: &Path, report: &EvalReport) -> Result<(), EvalError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| EvalError::Io {
            context: format!("creating eval output directory {}", parent.display()),
            source: e,
        })?;
    }
    let json = serde_json::to_vec_pretty(report).map_err(EvalError::Serialize)?;
    fs::write(path, json).map_err(|e| EvalError::Io {
        context: format!("writing eval report {}", path.display()),
        source: e,
    })
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn millis_u64(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalReport {
    pub command: Vec<String>,
    pub cwd: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub wall_ms: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub lifecycle: LifecycleAnalysis,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleAnalysis {
    pub parsed_events: u64,
    pub ignored_lines: u64,
    pub event_counts: BTreeMap<String, u64>,
    pub final_summary: Option<LifecycleSummary>,
    pub timing_markers: Vec<TimingMarker>,
    pub token_usage: Option<TokenUsageSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingMarker {
    pub phase: String,
    pub elapsed_ms: u64,
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub source: String,
    pub turn_count: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub total_uncached_tokens: u64,
}

pub fn analyze_lifecycle_stdout(stdout: &str, cwd: &Path) -> LifecycleAnalysis {
    let mut analysis = LifecycleAnalysis::default();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            analysis.ignored_lines += 1;
            continue;
        };
        let Some(event_type) = value.get("type").and_then(|t| t.as_str()) else {
            analysis.ignored_lines += 1;
            continue;
        };

        analysis.parsed_events += 1;
        *analysis
            .event_counts
            .entry(event_type.to_string())
            .or_insert(0) += 1;

        if event_type == "lifecycle_summary" {
            if let Some(summary_value) = value.get("summary") {
                if let Ok(summary) =
                    serde_json::from_value::<LifecycleSummary>(summary_value.clone())
                {
                    analysis.final_summary = Some(summary);
                }
            }
        }
    }

    if let Some(summary) = &analysis.final_summary {
        if let Some(log_path) = &summary.log_path {
            let path = resolve_path(cwd, Path::new(log_path));
            analysis.timing_markers = read_timing_markers(&path);
        }
        if let Some(jsonl_path) = &summary.jsonl_path {
            let path = resolve_path(cwd, Path::new(jsonl_path));
            analysis.token_usage = read_token_usage(&path);
        }
    }

    analysis
}

pub fn read_token_usage(path: &Path) -> Option<TokenUsageSummary> {
    let content = fs::read_to_string(path).ok()?;
    let mut summary = TokenUsageSummary {
        source: path.display().to_string(),
        ..TokenUsageSummary::default()
    };

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("turn.completed") {
            continue;
        }
        let Some(usage) = value.get("usage") else {
            continue;
        };
        summary.turn_count += 1;
        summary.input_tokens += usage_u64(usage, "input_tokens");
        summary.cached_input_tokens += usage_u64(usage, "cached_input_tokens");
        summary.output_tokens += usage_u64(usage, "output_tokens");
        summary.reasoning_output_tokens += usage_u64(usage, "reasoning_output_tokens");
    }

    if summary.turn_count == 0 {
        return None;
    }

    summary.uncached_input_tokens = summary
        .input_tokens
        .saturating_sub(summary.cached_input_tokens);
    summary.total_tokens = summary.input_tokens + summary.output_tokens;
    summary.total_uncached_tokens = summary.uncached_input_tokens + summary.output_tokens;
    Some(summary)
}

fn usage_u64(usage: &serde_json::Value, key: &str) -> u64 {
    usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}

pub fn read_timing_markers(path: &Path) -> Vec<TimingMarker> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| parse_timing_marker(line, path))
        .collect()
}

fn parse_timing_marker(line: &str, source: &Path) -> Option<TimingMarker> {
    let prefix = "::mrmouth::timing phase=";
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let (phase, elapsed) = rest.split_once(" elapsed_ms=")?;
    let elapsed_digits: String = elapsed.chars().take_while(|c| c.is_ascii_digit()).collect();
    let elapsed_ms = elapsed_digits.parse().ok()?;
    Some(TimingMarker {
        phase: phase.to_string(),
        elapsed_ms,
        source: source.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycle_analysis_counts_events_and_summary() {
        let stdout = [
            json!({"type": "stage_changed", "stage": "Agent"}).to_string(),
            "not json".to_string(),
            json!({
                "type": "lifecycle_summary",
                "summary": {
                    "status": "success",
                    "command": "run",
                    "log_path": "logs/run-1.log",
                    "jsonl_path": "logs/run-1.jsonl"
                }
            })
            .to_string(),
        ]
        .join("\n");

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        fs::write(
            dir.path().join("logs/run-1.log"),
            "::mrmouth::timing phase=container-wall elapsed_ms=123\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("logs/run-1.jsonl"),
            [
                json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 60,
                        "output_tokens": 15,
                        "reasoning_output_tokens": 5
                    }
                })
                .to_string(),
                json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 50,
                        "cached_input_tokens": 20,
                        "output_tokens": 10,
                        "reasoning_output_tokens": 2
                    }
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();

        let analysis = analyze_lifecycle_stdout(&stdout, dir.path());

        assert_eq!(analysis.parsed_events, 2);
        assert_eq!(analysis.ignored_lines, 1);
        assert_eq!(analysis.event_counts.get("stage_changed"), Some(&1));
        assert_eq!(analysis.event_counts.get("lifecycle_summary"), Some(&1));
        assert_eq!(
            analysis.final_summary.as_ref().map(|s| s.command.as_str()),
            Some("run")
        );
        assert_eq!(
            analysis.timing_markers,
            vec![TimingMarker {
                phase: "container-wall".to_string(),
                elapsed_ms: 123,
                source: dir.path().join("logs/run-1.log").display().to_string(),
            }]
        );
        assert_eq!(
            analysis.token_usage,
            Some(TokenUsageSummary {
                source: dir.path().join("logs/run-1.jsonl").display().to_string(),
                turn_count: 2,
                input_tokens: 150,
                cached_input_tokens: 80,
                uncached_input_tokens: 70,
                output_tokens: 25,
                reasoning_output_tokens: 7,
                total_tokens: 175,
                total_uncached_tokens: 95,
            })
        );
    }

    #[test]
    fn timing_parser_handles_missing_and_trailing_text() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("run.log");
        fs::write(
            &log,
            "before\n::mrmouth::timing phase=docker-build elapsed_ms=42 extra\nbad\n",
        )
        .unwrap();

        assert_eq!(
            read_timing_markers(&log),
            vec![TimingMarker {
                phase: "docker-build".to_string(),
                elapsed_ms: 42,
                source: log.display().to_string(),
            }]
        );
    }

    #[test]
    fn token_usage_reader_sums_turn_completed_usage() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("run.jsonl");
        fs::write(
            &jsonl,
            [
                "not json".to_string(),
                json!({"type": "item.completed"}).to_string(),
                json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 369499,
                        "cached_input_tokens": 308224,
                        "output_tokens": 5754,
                        "reasoning_output_tokens": 1037
                    }
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();

        assert_eq!(
            read_token_usage(&jsonl),
            Some(TokenUsageSummary {
                source: jsonl.display().to_string(),
                turn_count: 1,
                input_tokens: 369499,
                cached_input_tokens: 308224,
                uncached_input_tokens: 61275,
                output_tokens: 5754,
                reasoning_output_tokens: 1037,
                total_tokens: 375253,
                total_uncached_tokens: 67029,
            })
        );
    }
}
