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

## [2026-05-14] implemented | Reviewer fitness for purpose context
Recorded the reviewer rule that code review should judge the diff against the Litebrite item that triggered the run. `mrmouth do` and `mrmouth ready` now pass a review target so the prompt directs reviewers to `lb show <item-id>` and check fitness for purpose; `mrmouth loop` falls back to inferring purpose from Litebrite state, commits, and diff.

## [2026-05-14] fixed | Reviewer Dockerfile extraction ordering
Recorded the reviewer-side Dockerfile extraction bug: reviewer containers could commit and push a Dockerfile edit, but the host copied the container Dockerfile before pulling that pushed commit, leaving an uncommitted host copy. Reviewer extraction now pulls first for real remotes, matching runner extraction.

## [2026-05-14] tightened | Reviewer issue parentage
Recorded that reviewer-created Litebrite issues should stay attached to the current work context: children under a reviewed epic/feature, siblings under a reviewed task's parent, and top-level only when no relevant parent exists.

## [2026-05-14] changed | Codex default agent
Recorded that Codex is now the built-in default agent in both `Config::default()` and `AgentKind::default()`. Prime displays the effective model as "agent default" when Codex omits a historical Claude alias such as `opus`.

## [2026-05-18] implemented | mrmouth do local worktree mode
Recorded `mrmouth do --worktree <path>` semantics: the tracking repo still lives at `/home/runner/workspace`, the resolved host worktree is mounted at `/home/runner/worktree`, and `MRMOUTH_WORKTREE` points there. The implementation spans `src/do_cmd.rs`, `src/run.rs`, and `src/docker.rs`; coverage now includes prompt text, path resolution, argument propagation, and Docker mount construction.
## [2026-06-04] added | Split bookkeeping/work repo model for fake monorepos

Recorded the `work_repo`/`--worktree` layout model: mrmouth now resolves bookkeeping and work repos for runs, mounts distinct work repos at `/home/runner/worktree`, exposes `MRMOUTH_BOOKKEEPING_REPO` and `MRMOUTH_WORK_REPO`, and prompts agents to separate task-state commands from code edits.

## [2026-06-04] fixed | Dockerfile self-update poisoning

Recorded the Dockerfile self-update guard: runner, session-task, and reviewer wrappers auto-commit a dirty `.mrmouth/Dockerfile` after successful runs, while host extraction is skipped after failed runs so partial container edits do not dirty the host checkout.

## [2026-06-05] documented | CI check commands and Clippy exceptions

Recorded the local Rust CI-equivalent gates and the rationale for narrow Clippy lint exceptions on orchestration boundary helpers and the lifecycle event enum.

## [2026-06-29] documented | Mr Mouth speed and eval direction

Recorded the current Codex invocation model, the distinction between Docker session reuse and Codex thread reuse, relevant Codex `exec resume`/`app-server` options, and a proposed eval split between deterministic correctness checks and speed benchmarks.

## [2026-06-29] implemented | Initial mrmouth eval harness

Recorded `mrmouth eval -- <command>` as the first eval harness: it wraps a child command, parses lifecycle JSON from stdout, extracts timing markers from the final summary log, writes a JSON report even on child failure, and exits nonzero when the child fails.
