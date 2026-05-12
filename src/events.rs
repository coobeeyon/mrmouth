use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// A lifecycle event emitted by mrmouth orchestration code.
///
/// This is intentionally separate from inner-agent stream JSON. These events
/// describe mrmouth's own lifecycle so later renderers can drive the TUI,
/// human logs, and machine-readable JSONL from one stable surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MrmouthEvent {
    /// A displayable line that would historically be sent to Logger or stderr.
    Message {
        level: MessageLevel,
        text: String,
        target: MessageTarget,
    },
    /// The high-level phase currently visible to users.
    StageChanged { stage: String },
    /// A label associated with an agent run, such as branch, model, or session.
    RunLabel { name: String, value: String },
    /// A label associated with a litebrite item.
    TaskLabel {
        item_id: String,
        name: String,
        value: String,
    },
    /// A runner selected a litebrite item to work on.
    TaskSelected {
        item_id: String,
        title: String,
        parent_id: Option<String>,
    },
    /// Feature branch setup, push, merge, or cleanup activity.
    BranchLifecycle {
        action: BranchAction,
        branch: String,
        parent_branch: Option<String>,
    },
    /// Docker container setup, execution, completion, or cleanup activity.
    ContainerLifecycle {
        action: ContainerAction,
        name: String,
        image_id: Option<String>,
        exit_code: Option<i32>,
    },
    /// Agent-run lifecycle independent of any one container log line.
    RunLifecycle {
        action: RunAction,
        run_id: Option<String>,
        branch: Option<String>,
    },
    /// Reviewer agent lifecycle.
    ReviewerLifecycle {
        action: ReviewerAction,
        branch: String,
        commit_range: Option<String>,
    },
    /// Shipper or readiness-check lifecycle.
    ShipperLifecycle {
        action: ShipperAction,
        current_branch: String,
        parent_branch: Option<String>,
    },
    /// External state synchronization such as litebrite, trapperkeeper, or git.
    Sync {
        action: SyncAction,
        tool: SyncTool,
        detail: Option<String>,
    },
    /// A command or role completed.
    Finished {
        status: FinishStatus,
        summary: Option<String>,
    },
    /// A command or role failed.
    Failure {
        message: String,
        code: Option<i32>,
        detail: Option<String>,
    },
}

impl MrmouthEvent {
    /// Stable snake_case event name used by serialized lifecycle output.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::Message { .. } => "message",
            Self::StageChanged { .. } => "stage_changed",
            Self::RunLabel { .. } => "run_label",
            Self::TaskLabel { .. } => "task_label",
            Self::TaskSelected { .. } => "task_selected",
            Self::BranchLifecycle { .. } => "branch_lifecycle",
            Self::ContainerLifecycle { .. } => "container_lifecycle",
            Self::RunLifecycle { .. } => "run_lifecycle",
            Self::ReviewerLifecycle { .. } => "reviewer_lifecycle",
            Self::ShipperLifecycle { .. } => "shipper_lifecycle",
            Self::Sync { .. } => "sync",
            Self::Finished { .. } => "finished",
            Self::Failure { .. } => "failure",
        }
    }

    pub fn finished(status: FinishStatus, summary: Option<impl Into<String>>) -> Self {
        Self::Finished {
            status,
            summary: summary.map(Into::into),
        }
    }

    pub fn failure(
        message: impl Into<String>,
        code: Option<i32>,
        detail: Option<impl Into<String>>,
    ) -> Self {
        Self::Failure {
            message: message.into(),
            code,
            detail: detail.map(Into::into),
        }
    }

    pub fn to_json_value(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::to_value(self)
    }

    pub fn to_json_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTarget {
    Agent,
    Decider,
    Reviewer,
    Shipper,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchAction {
    Creating,
    Created,
    Pushing,
    Pushed,
    Merging,
    Merged,
    Switching,
    Switched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerAction {
    BuildingImage,
    ImageBuilt,
    Starting,
    Started,
    Streaming,
    Exited,
    Stopping,
    Removed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAction {
    Starting,
    Preflight,
    BuildingImage,
    Syncing,
    RunningAgent,
    PullingChanges,
    ExtractingDockerfile,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerAction {
    Starting,
    Running,
    Finished,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipperAction {
    CheckingReadiness,
    Ready,
    Blocked,
    Merging,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    Starting,
    Finished,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTool {
    Git,
    Litebrite,
    Trapperkeeper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishStatus {
    Success,
    Failed,
    Cancelled,
}

/// Consumer of mrmouth lifecycle events.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &MrmouthEvent);
}

/// Cloneable event sink reference that can be passed through orchestration code.
#[derive(Clone)]
pub struct EventSinkHandle {
    sink: Arc<dyn EventSink>,
}

impl EventSinkHandle {
    pub fn new<S>(sink: S) -> Self
    where
        S: EventSink + 'static,
    {
        Self {
            sink: Arc::new(sink),
        }
    }

    pub fn noop() -> Self {
        Self::new(NoopEventSink)
    }

    pub fn fanout(sinks: Vec<EventSinkHandle>) -> Self {
        Self::new(FanoutEventSink::new(sinks))
    }

    pub fn emit(&self, event: MrmouthEvent) {
        self.sink.emit(&event);
    }

    pub fn emit_ref(&self, event: &MrmouthEvent) {
        self.sink.emit(event);
    }
}

/// Sink that intentionally ignores every event.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: &MrmouthEvent) {}
}

/// Sink that forwards each event to multiple downstream sinks.
#[derive(Clone)]
pub struct FanoutEventSink {
    sinks: Vec<EventSinkHandle>,
}

impl FanoutEventSink {
    pub fn new(sinks: Vec<EventSinkHandle>) -> Self {
        Self { sinks }
    }
}

impl EventSink for FanoutEventSink {
    fn emit(&self, event: &MrmouthEvent) {
        for sink in &self.sinks {
            sink.emit_ref(event);
        }
    }
}

/// In-memory sink intended for focused tests of event-producing code.
#[derive(Clone, Default)]
pub struct RecordingEventSink {
    events: Arc<Mutex<Vec<MrmouthEvent>>>,
}

impl RecordingEventSink {
    pub fn events(&self) -> Vec<MrmouthEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl EventSink for RecordingEventSink {
    fn emit(&self, event: &MrmouthEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
    }
}

/// Sink that writes one mrmouth lifecycle event per JSONL line.
///
/// Serialization or write failures are reported to stderr so lifecycle JSON on
/// stdout remains reserved for successfully encoded event objects.
pub struct JsonlEventSink<W: Write + Send> {
    writer: Arc<Mutex<W>>,
}

impl JsonlEventSink<io::Stdout> {
    pub fn stdout() -> Self {
        Self::new(io::stdout())
    }
}

impl<W: Write + Send> JsonlEventSink<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
        }
    }
}

impl<W: Write + Send> EventSink for JsonlEventSink<W> {
    fn emit(&self, event: &MrmouthEvent) {
        let line = match event.to_json_line() {
            Ok(line) => line,
            Err(e) => {
                eprintln!("warning: failed to serialize lifecycle event: {e}");
                return;
            }
        };
        match self.writer.lock() {
            Ok(mut writer) => {
                if let Err(e) = writeln!(writer, "{line}") {
                    eprintln!("warning: failed to write lifecycle event: {e}");
                }
                let _ = writer.flush();
            }
            Err(_) => eprintln!("warning: lifecycle event writer lock poisoned"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_type_names_match_serialized_tags() {
        let events = vec![
            MrmouthEvent::Message {
                level: MessageLevel::Info,
                text: "hello".to_string(),
                target: MessageTarget::Agent,
            },
            MrmouthEvent::StageChanged {
                stage: "Agent".to_string(),
            },
            MrmouthEvent::RunLabel {
                name: "branch".to_string(),
                value: "feature".to_string(),
            },
            MrmouthEvent::TaskLabel {
                item_id: "lb-1234".to_string(),
                name: "status".to_string(),
                value: "open".to_string(),
            },
            MrmouthEvent::TaskSelected {
                item_id: "lb-1234".to_string(),
                title: "Do work".to_string(),
                parent_id: Some("lb-epic".to_string()),
            },
            MrmouthEvent::BranchLifecycle {
                action: BranchAction::Created,
                branch: "feature".to_string(),
                parent_branch: Some("main".to_string()),
            },
            MrmouthEvent::ContainerLifecycle {
                action: ContainerAction::Exited,
                name: "mrmouth-runner".to_string(),
                image_id: Some("sha256:abc".to_string()),
                exit_code: Some(0),
            },
            MrmouthEvent::RunLifecycle {
                action: RunAction::RunningAgent,
                run_id: Some("run-1".to_string()),
                branch: Some("feature".to_string()),
            },
            MrmouthEvent::ReviewerLifecycle {
                action: ReviewerAction::Finished,
                branch: "feature".to_string(),
                commit_range: Some("HEAD~1..HEAD".to_string()),
            },
            MrmouthEvent::ShipperLifecycle {
                action: ShipperAction::Ready,
                current_branch: "feature".to_string(),
                parent_branch: Some("main".to_string()),
            },
            MrmouthEvent::Sync {
                action: SyncAction::Starting,
                tool: SyncTool::Litebrite,
                detail: None,
            },
            MrmouthEvent::finished(FinishStatus::Success, Some("done")),
            MrmouthEvent::failure("boom", Some(2), Some("stderr")),
        ];

        let names: Vec<_> = events.iter().map(MrmouthEvent::event_type).collect();

        assert_eq!(
            names,
            vec![
                "message",
                "stage_changed",
                "run_label",
                "task_label",
                "task_selected",
                "branch_lifecycle",
                "container_lifecycle",
                "run_lifecycle",
                "reviewer_lifecycle",
                "shipper_lifecycle",
                "sync",
                "finished",
                "failure",
            ]
        );

        for event in events {
            assert_eq!(event.to_json_value().unwrap()["type"], event.event_type());
        }
    }

    #[test]
    fn writes_event_as_single_json_line() {
        let event = MrmouthEvent::StageChanged {
            stage: "Agent".to_string(),
        };

        let line = event.to_json_line().unwrap();

        assert_eq!(line, r#"{"type":"stage_changed","stage":"Agent"}"#);
        assert!(!line.contains('\n'));
    }

    #[test]
    fn constructs_finished_payload() {
        let event = MrmouthEvent::finished(FinishStatus::Success, Some("changes pushed"));

        assert_eq!(
            event.to_json_value().unwrap(),
            json!({
                "type": "finished",
                "status": "success",
                "summary": "changes pushed",
            })
        );
    }

    #[test]
    fn constructs_failure_payload() {
        let event = MrmouthEvent::failure("cargo check failed", Some(101), None::<String>);

        assert_eq!(
            event.to_json_value().unwrap(),
            json!({
                "type": "failure",
                "message": "cargo check failed",
                "code": 101,
                "detail": null,
            })
        );
    }

    #[test]
    fn fanout_forwards_events_to_each_sink() {
        let first = RecordingEventSink::default();
        let second = RecordingEventSink::default();
        let first_handle = EventSinkHandle::new(first.clone());
        let second_handle = EventSinkHandle::new(second.clone());
        let fanout = EventSinkHandle::fanout(vec![first_handle, second_handle]);
        let event = MrmouthEvent::Sync {
            action: SyncAction::Starting,
            tool: SyncTool::Litebrite,
            detail: None,
        };

        fanout.emit(event.clone());

        assert_eq!(first.events(), vec![event.clone()]);
        assert_eq!(second.events(), vec![event]);
    }

    #[test]
    fn jsonl_sink_writes_one_event_per_line() {
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = JsonlEventSink::new(SharedTestWriter(output.clone()));

        sink.emit(&MrmouthEvent::StageChanged {
            stage: "Agent".to_string(),
        });
        sink.emit(&MrmouthEvent::RunLabel {
            name: "branch".to_string(),
            value: "feature".to_string(),
        });

        let bytes = output.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "{\"type\":\"stage_changed\",\"stage\":\"Agent\"}\n{\"type\":\"run_label\",\"name\":\"branch\",\"value\":\"feature\"}\n"
        );
    }

    struct SharedTestWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedTestWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
