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
