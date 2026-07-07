# Event Rendering Architecture

Mr Mouth should separate orchestration from presentation by having core flows
emit structured lifecycle events. Renderers should consume those events for the
TUI, human terminal output, JSONL automation output, and logs.

The current implementation still passes `Option<&TuiHandle>` through command
modules and lets `Logger` own an optional `TuiSender`. That couples business
logic to the TUI. The desired direction is:

1. Define a small event model and sink abstraction.
2. Keep durable file logging separate from display rendering.
3. Drive the TUI through an event renderer.
4. Route run/do/ready/loop/reviewer/shipper lifecycle reporting through events.
5. Add lifecycle JSONL output as a distinct mode from raw inner-agent JSON.

`mrmouth run --raw` should continue to mean raw Claude/Codex stream output.
Structured lifecycle output should use a separate flag such as `--json-events`
or `--agent-json`, with final summary events that supervising tools can parse.

The Litebrite epic for this plan is `lb-40uv`.

## Core Event Surface

`src/events.rs` defines the first internal event vocabulary. `MrmouthEvent` is
a serde-tagged enum with `snake_case` event names and variants for display
messages, stage changes, run/task labels, task selection, branch lifecycle,
container lifecycle, run lifecycle, reviewer lifecycle, shipper lifecycle,
sync, finish, and failure. The types are intentionally mrmouth lifecycle
events, not passthrough Claude/Codex stream events.

The sink abstraction is `EventSink`, with `EventSinkHandle` as the cloneable
handle passed through orchestration code. `NoopEventSink` provides a default
do-nothing target, `FanoutEventSink` forwards one event to multiple sinks, and
`RecordingEventSink` supports focused tests for event-producing code. Callers
have not been migrated yet; later tasks should route existing `Logger`,
`TuiHandle`, reviewer, shipper, and command lifecycle output through this
surface.

## Review Notes

The `lb-40uv` branch routes the TUI through the same `EventSinkHandle` used for
lifecycle JSON, so `RunOptions.event_sink.is_some()` no longer means "machine
JSON mode". Stream rendering decisions must distinguish JSON lifecycle output
from a normal TUI event sink. Otherwise normal TUI runs skip formatted agent
stream display and only write the inner stream to log files.

The follow-up fix adds an explicit `RunOptions::json_events` flag and propagates
the CLI mode through `do`, `ready`, and `loop`. Stream routing now formats the
inner agent stream whenever `raw == false` and `json_events == false`, even when
a TUI lifecycle sink is present.

## Terminal Summary Ordering

`lifecycle_summary` is a terminal contract for the top-level command being
supervised. Nested runner calls inside `do`, `batch`, `ready`, and `loop` should
not emit their own terminal `run` summaries into the caller's lifecycle stream,
because those summaries can appear before reviewer, summary-generation,
session-teardown, final sync, or artifact preservation work completes.

`src/run.rs::RunOptions::emit_terminal_events` controls this boundary:
standalone `mrmouth run` sets it to `true`; orchestrators set it to `false` and
emit their own final summary after child work has settled. `loop` stores a
pending `LoopTerminal` during the decision loop, stops the long-lived session,
flushes the loop logger, attaches `logs/latest.log` and `logs/latest.jsonl` when
present, then emits the terminal event. This keeps live JSON consumers from
treating an intermediate runner state as the final state to preserve.
