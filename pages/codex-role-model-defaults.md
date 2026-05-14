# Codex Role Model Defaults

Mrmouth has one agent selector (`agent = "claude"` or `agent = "codex"`) but
multiple configured model slots: runner `model` plus loop role models for
decider, summary, reviewer, and shipper.

The built-in agent default is Codex. `Config::default()` and
`AgentKind::default()` both choose `AgentKind::Codex`, so missing config files
and missing `agent` fields behave the same way. `--claude` still overrides the
configured/default agent for a command.

The built-in role model defaults are Claude aliases:

- `model = "opus"`
- `decider_model = "sonnet"`
- `summary_model = "haiku"`
- `reviewer_model = "sonnet"`
- `shipper_model = "sonnet"`

These aliases are valid for Claude Code, but Codex rejects them. A real failure
was seen after a successful `mrmouth do --codex` run in `my2048`: the runner
completed and pushed, then the reviewer container launched Codex with
`--model sonnet` and failed with a 400 invalid request because the ChatGPT
Codex account did not support `sonnet`.

The operational rule is that configured role models must be normalized for the
active agent before constructing an agent command. In Codex mode, the built-in
Claude aliases should become an empty model string so `AgentKind::Codex`
omits `--model` and lets Codex use its own configured/default model. Explicit
Codex model names such as `gpt-5.2` should be preserved.

This applies anywhere a loop role model is consumed, including:

- reviewer after `do`, `ready`, or `loop` runs
- decider inside `loop`
- shipper branch naming and merge/shipping work
- summary generation

The runner CLI model path already has an explicit override concept, so explicit
`--model` values remain caller-owned. The shared helper for configured values is
`Config::effective_model_for_agent`.

Because Codex is the default but the historical model default remains
`model = "opus"`, user-facing default summaries should display the effective
model. For Codex plus a built-in Claude alias, that means showing "agent
default" rather than the raw alias that will be omitted from the Codex command.
