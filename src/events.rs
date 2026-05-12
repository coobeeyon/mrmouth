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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_tagged_event_names() {
        let event = MrmouthEvent::StageChanged {
            stage: "Agent".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();

        assert_eq!(json, r#"{"type":"stage_changed","stage":"Agent"}"#);
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
}
