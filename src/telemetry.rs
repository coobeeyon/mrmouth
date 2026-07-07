use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTelemetry {
    pub timing_markers: Vec<TimingMarker>,
    pub token_usage: Option<TokenUsageSummary>,
}

impl RunTelemetry {
    pub fn is_empty(&self) -> bool {
        self.timing_markers.is_empty() && self.token_usage.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingMarker {
    pub phase: String,
    pub elapsed_ms: u64,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenUsageStatus {
    Completed,
    Partial,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub source: String,
    pub status: TokenUsageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caveat: Option<String>,
    pub turn_count: u64,
    pub partial_event_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_type: Option<String>,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub total_uncached_tokens: u64,
}

impl TokenUsageSummary {
    fn new(source: &Path, status: TokenUsageStatus) -> Self {
        Self {
            source: source.display().to_string(),
            status,
            caveat: None,
            turn_count: 0,
            partial_event_count: 0,
            last_event_type: None,
            input_tokens: 0,
            cached_input_tokens: 0,
            uncached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            total_uncached_tokens: 0,
        }
    }

    fn add_usage(&mut self, usage: NormalizedUsage) {
        self.input_tokens += usage.input_tokens;
        self.cached_input_tokens += usage.cached_input_tokens;
        self.output_tokens += usage.output_tokens;
        self.reasoning_output_tokens += usage.reasoning_output_tokens;
        self.recompute_totals();
    }

    fn set_usage(&mut self, usage: NormalizedUsage) {
        self.input_tokens = usage.input_tokens;
        self.cached_input_tokens = usage.cached_input_tokens;
        self.output_tokens = usage.output_tokens;
        self.reasoning_output_tokens = usage.reasoning_output_tokens;
        self.recompute_totals();
    }

    fn recompute_totals(&mut self) {
        self.uncached_input_tokens = self.input_tokens.saturating_sub(self.cached_input_tokens);
        self.total_tokens = self.input_tokens + self.output_tokens;
        self.total_uncached_tokens = self.uncached_input_tokens + self.output_tokens;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NormalizedUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
}

impl NormalizedUsage {
    fn total_observed(self) -> u64 {
        self.input_tokens
            + self.cached_input_tokens
            + self.output_tokens
            + self.reasoning_output_tokens
    }
}

pub fn read_run_telemetry(log_path: Option<&Path>, jsonl_path: Option<&Path>) -> RunTelemetry {
    RunTelemetry {
        timing_markers: log_path.map(read_timing_markers).unwrap_or_default(),
        token_usage: jsonl_path.and_then(read_token_usage),
    }
}

pub fn read_token_usage(path: &Path) -> Option<TokenUsageSummary> {
    let content = fs::read_to_string(path).ok()?;
    let mut completed = TokenUsageSummary::new(path, TokenUsageStatus::Completed);
    let mut partial = TokenUsageSummary::new(path, TokenUsageStatus::Partial);
    let mut last_partial: Option<NormalizedUsage> = None;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let event_type = value
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("unknown")
            .to_string();

        if event_type == "turn.completed" {
            if let Some(usage) = value.get("usage").and_then(normalize_usage) {
                completed.turn_count += 1;
                completed.add_usage(usage);
            }
            continue;
        }

        if let Some(usage) = find_usage(&value) {
            partial.partial_event_count += 1;
            partial.last_event_type = Some(event_type);
            last_partial = Some(usage);
        }
    }

    if completed.turn_count > 0 {
        completed.partial_event_count = partial.partial_event_count;
        completed.last_event_type = partial.last_event_type;
        return Some(completed);
    }

    if let Some(usage) = last_partial {
        partial.set_usage(usage);
        partial.caveat = Some(format!(
            "no turn.completed usage found; using last-seen partial usage from {}",
            partial
                .last_event_type
                .as_deref()
                .unwrap_or("unknown event")
        ));
        return Some(partial);
    }

    let mut missing = TokenUsageSummary::new(path, TokenUsageStatus::Missing);
    missing.caveat = Some("no turn.completed or partial token usage events found".to_string());
    Some(missing)
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

fn find_usage(value: &serde_json::Value) -> Option<NormalizedUsage> {
    if let Some(usage) = normalize_usage(value) {
        return Some(usage);
    }

    match value {
        serde_json::Value::Array(values) => values.iter().filter_map(find_usage).last(),
        serde_json::Value::Object(map) => map.values().filter_map(find_usage).last(),
        _ => None,
    }
}

fn normalize_usage(value: &serde_json::Value) -> Option<NormalizedUsage> {
    if let Some(nested) = value.get("usage").and_then(normalize_usage) {
        return Some(nested);
    }
    if let Some(nested) = value
        .get("tokenUsage")
        .and_then(|v| v.get("total").or(Some(v)))
        .and_then(normalize_usage)
    {
        return Some(nested);
    }
    if let Some(nested) = value.get("total").and_then(normalize_usage) {
        return Some(nested);
    }

    let usage = NormalizedUsage {
        input_tokens: usage_u64_any(value, &["input_tokens", "inputTokens"]),
        cached_input_tokens: usage_u64_any(
            value,
            &[
                "cached_input_tokens",
                "cachedInputTokens",
                "cached_tokens",
                "cachedTokens",
            ],
        ),
        output_tokens: usage_u64_any(value, &["output_tokens", "outputTokens"]),
        reasoning_output_tokens: usage_u64_any(
            value,
            &["reasoning_output_tokens", "reasoningOutputTokens"],
        ),
    };

    (usage.total_observed() > 0).then_some(usage)
}

fn usage_u64_any(value: &serde_json::Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_u64()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                status: TokenUsageStatus::Completed,
                caveat: None,
                turn_count: 1,
                partial_event_count: 0,
                last_event_type: None,
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

    #[test]
    fn token_usage_reader_reports_partial_when_completion_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("run.jsonl");
        fs::write(
            &jsonl,
            [
                json!({
                    "type": "thread/tokenUsage/updated",
                    "tokenUsage": {
                        "total": {
                            "inputTokens": 1200,
                            "cachedInputTokens": 900,
                            "outputTokens": 80,
                            "reasoningOutputTokens": 20
                        }
                    }
                })
                .to_string(),
                json!({
                    "type": "agent_message",
                    "message": "done"
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();

        let usage = read_token_usage(&jsonl).unwrap();

        assert_eq!(usage.status, TokenUsageStatus::Partial);
        assert_eq!(usage.turn_count, 0);
        assert_eq!(usage.partial_event_count, 1);
        assert_eq!(
            usage.last_event_type.as_deref(),
            Some("thread/tokenUsage/updated")
        );
        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.cached_input_tokens, 900);
        assert_eq!(usage.total_uncached_tokens, 380);
        assert!(usage
            .caveat
            .as_deref()
            .unwrap()
            .contains("no turn.completed"));
    }

    #[test]
    fn token_usage_reader_reports_missing_when_no_usage_exists() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("run.jsonl");
        fs::write(
            &jsonl,
            json!({"type": "agent_message", "message": "done"}).to_string(),
        )
        .unwrap();

        let usage = read_token_usage(&jsonl).unwrap();

        assert_eq!(usage.status, TokenUsageStatus::Missing);
        assert_eq!(usage.turn_count, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(usage
            .caveat
            .as_deref()
            .unwrap()
            .contains("no turn.completed"));
    }
}
