# Log

## [2026-05-12] documented | Litebrite agent contract
Captured the litebrite command semantics that matter for mrmouth and supervising agents: `lb prime` as the intended AI context surface, the ready/show/claim/work/commit/close/sync protocol, claim atomicity, ready discovery semantics, close-with-open-children behavior, and implications for a future mrmouth operator skill.

## [2026-05-12] clarified | Brite hierarchy model
Recorded that mrmouth-prepared brites should primarily form a nested parent/child work breakdown. Parents roll up child completion rather than depending on children via `blocks`; explicit `blocks` dependencies should model sequencing between siblings.

## [2026-05-12] generalized | Brite authoring as work graph design
Captured that the desired Codex capability is broader than mrmouth operation: author nested litebrite work graphs from user goals, with parent brites for outcomes, leaf brites as ramp-in handoffs, and sibling `blocks` for true ordering constraints.

## [2026-05-12] planned | Event rendering separation
Created Litebrite epic `lb-40uv` for separating mrmouth lifecycle events from TUI rendering and recorded the architectural intent: core flows emit events, renderers handle TUI/human/JSON/log presentation, and lifecycle JSON remains distinct from raw inner-agent JSON.

## [2026-05-12] fixed | Dockerfile extraction after pull
Recorded the lifecycle rule for container-edited Dockerfiles: remote-backed runs should pull first, then extract via a temporary file and leave the host worktree untouched when the post-pull Dockerfile already matches the container content. This prevents self-produced, already-pushed Dockerfile edits from blocking `git pull --ff-only`.

## [2026-05-12] implemented | Core mrmouth event surface
Recorded the new `src/events.rs` API for lifecycle event emission: serde-tagged `MrmouthEvent` variants, cloneable `EventSinkHandle`, no-op/fanout sinks, and a recording sink for focused tests. This is the foundation for later TUI, human, and JSONL renderers.

## [2026-05-12] reviewed | Event sink output-mode coupling
Reviewed the completed `lb-40uv` branch. `cargo test` passed, but normal TUI mode now creates an event sink, so stream-rendering branches that use `opts.event_sink.is_some()` incorrectly suppress formatted agent output in the TUI. Future fixes should carry an explicit lifecycle JSON/output-mode flag instead of inferring it from event-sink presence.

## [2026-05-12] fixed | TUI stream routing with lifecycle event sinks
Recorded the fix for the `lb-40uv` output-mode regression: `RunOptions` now carries explicit `json_events` state, and run stream routing uses that state instead of treating any event sink as machine JSON mode. Plain TUI mode keeps formatted agent output while lifecycle events still drive TUI status.

## [2026-05-12] fixed | Agent-aware credential preflight
Recorded the credential preflight rule: Claude mode requires `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`, while Codex mode must not be gated by Claude credentials because Codex may authenticate via env vars or persisted Docker-volume device auth.

## [2026-05-13] implemented | Mr Mouth prime command
Recorded `mrmouth prime` as the AI-facing context surface for supervising agents: it prints effective defaults, command semantics, lifecycle JSON guidance, and the recommended bounded delegation protocol without starting Docker, TUI, or mutating litebrite state.

## [2026-05-13] updated | Codex setup command
Recorded `mrmouth setup codex` as the primary Codex device-auth setup command for the persisted Docker home volume, with `mrmouth codex-login` retained as a legacy alias.

## [2026-05-13] corrected | Codex setup command
Corrected the meaning of `mrmouth setup codex`: it follows the Trapperkeeper hook setup pattern by enabling Codex hooks, adding a `SessionStart` hook for `mrmouth prime`, and allowing the `mrmouth` command prefix. Codex auth remains `mrmouth codex-login`.

## [2026-05-13] debugged | Codex reviewer model failure
Recorded the root cause of a `my2048` reviewer failure after `mrmouth do --codex`: the runner used a Codex-safe model path, but reviewer/loop role models still passed default Claude aliases such as `sonnet`. Codex role-model consumers should use `Config::effective_model_for_agent` so built-in Claude aliases become an omitted model in Codex mode.
