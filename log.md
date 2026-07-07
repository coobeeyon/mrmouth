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

## [2026-06-29] implemented | Role-level eval timing markers

Recorded the role timing markers now emitted to orchestration logs: `decider-wall`, `shipper-wall`, `reviewer-wall`, and `summary-wall`, all using the existing `::mrmouth::timing phase=<name> elapsed_ms=<n>` format consumed by `mrmouth eval`.

## [2026-06-29] documented | Minimal eval fixture contract

Recorded `evals/README.md` as the fixture-backed eval contract: fixture directories contain `repo/`, `run.sh`, and `assert.sh`; assertions should verify eval reports, tests, changed files, commits, Litebrite state, and timing phases before optional LLM judging.

## [2026-06-29] implemented | Smol current-container eval fixture

Recorded `evals/fixtures/smol-current-container/` as the first real fixture-backed eval. It rebuilds generated repo/report/remote directories from committed seeds, runs `mrmouth do --json-events --current-container --worktree` through `mrmouth eval`, asserts the report/task/file/commit/timing outcomes, and completed successfully in about 59 seconds for a one-line change. Successful `do` summaries now include latest log/jsonl paths so eval timing extraction works on success.

## [2026-07-01] implemented | Codex Goal eval harness

Recorded `evals/codex_goal_harness.mjs` and the smol fixture `run_goal.sh`/`assert_goal.sh` path. The harness drives `codex app-server --stdio --enable goals`, sets a persistent goal with `thread/goal/set`, starts one turn, writes a JSON report with wall time, thread/turn ids, final goal state, and app-server event counts, and asserts deterministic fixture outcomes. Successful smol runs completed in about 97-139 seconds with final goal status `complete`; the shared fixture setup also kept the Mr Mouth path passing at about 57 seconds.

## [2026-07-01] corrected | Codex Goal fixture setup parity

Recorded the missing setup discovered in the smol Goal harness: the generated repo must include `trk init`, `lb setup codex`, `trk setup codex`, project trust, and trusted hook state for the `lb prime` and `trk prime` SessionStart hooks. The fixture now commits the Codex hook/rule files, pushes Litebrite and Trapperkeeper branches, trusts the generated project in `CODEX_HOME`, and ignores goal prompt files as eval artifacts. Trusted-hook Goal runs showed both hooks firing and completed in about 87-127 seconds; the Mr Mouth path remained passing at about 47 seconds.

## [2026-07-02] implemented | Medium Python CLI eval fixture

Recorded `evals/fixtures/medium-python-cli/` as the next deterministic fixture for comparing Mr Mouth and Codex Goal mode. The fixture rebuilds seeded bookkeeping/worktree repos, initializes the same Litebrite/Trapperkeeper/Codex hook trust setup as the smol fixture, and asks the agent to implement a Python sales summary CLI from `SPEC.md`. Clean runs passed deterministic assertions: Mr Mouth completed in 170,520 ms, and Codex Goal completed in 157,463 ms with final goal status `complete`, both producing a single-file worktree implementation commit and closing the Litebrite item. Seed worktrees now ignore Python bytecode so local compile/test checks cannot contaminate generated initial commits.

## [2026-07-03] normalized | Eval token accounting

Recorded the cache-aware interpretation of medium fixture token usage. Mr Mouth and Codex Goal both showed 300k-400k raw token totals because repeated internal tool-loop context is counted each model step, but most repeated input was cached. Matched medium reruns showed 71,942 uncached input plus output tokens for Mr Mouth and 69,887 for Goal. Eval reports now expose normalized token summaries: Mr Mouth parses `lifecycle.token_usage` from inner Codex JSONL `turn.completed.usage`, and the Codex Goal harness records final goal tokens, `thread/tokenUsage/updated` samples, normalized nested app-server usage, and comparable aggregate fields.

## [2026-07-03] implemented | Multi-do epic eval fixture

Recorded `evals/fixtures/multi-do-epic-python/` as the first multi-leaf eval fixture. The fixture generates a parent epic with four ordered child tasks for an inventory package and compares repeated `mrmouth do` leaf execution against one Codex Goal objective over the whole epic. Clean runs passed deterministic assertions: Mr Mouth multi-do completed in 550,945 ms with 261,521 uncached input plus output tokens across four Codex turns, while Codex Goal completed in 225,378 ms with 86,095 uncached input plus output tokens in one app-server turn. Both produced four focused implementation commits and closed the expected Litebrite items.

## [2026-07-03] implemented | Experimental mrmouth batch command

Recorded `mrmouth batch` as an experimental current-container path for completing multiple ready children of a parent Litebrite item in one Codex runner execution. The first `multi-do-epic-python` batch run passed deterministic assertions, closed four children plus the parent epic, produced four focused implementation commits, and completed in 242,558 ms with 111,557 uncached input plus output tokens. This is still behind the Goal baseline, but it removes most of the repeated `mrmouth do` overhead while preserving external task boundaries.

## [2026-07-03] implemented | Hard support operations eval fixture

Recorded `evals/fixtures/hard-epic-python/` as a six-leaf support operations fixture for pushing batch and Goal beyond the four-leaf inventory case. Clean runs passed deterministic assertions and produced six focused commits in both harnesses. Batch completed in 335,010 ms with 122,006 uncached input plus output tokens; Goal completed in 254,391 ms with 97,801 uncached input plus output tokens. Quality looked tied on deterministic checks, clean worktrees, closed tasks, and commit shape, while Goal retained a time/token edge.

## [2026-07-03] implemented | Long fulfillment eval fixture

Recorded `evals/fixtures/long-epic-python/` as a ten-leaf fulfillment operations fixture for a harder batch-vs-Goal comparison. Clean runs passed deterministic assertions in both harnesses. Batch completed in 529,397 ms with 179,688 uncached input plus output tokens and ten focused implementation commits; Goal completed in 247,101 ms with 84,566 comparable uncached tokens and one combined implementation commit. The quality distinction is now visible in commit/task audit shape, while Goal has a much larger time/token edge.

## [2026-07-04] added | Biolife Rust game eval fixture

Recorded `evals/fixtures/biolife-rust/` as the first architecture/physics-heavy eval fixture. It generates a Rust workspace with `biolife_core` for offline simulation and `biolife_app` as a thin CLI boundary, plus ten ordered Litebrite leaves for chromosome-driven graph growth, energy, combat/defense, propulsion, viscous-fluid integration, deterministic world ticks, offline API, and frontend wiring. The generated initial worktree compiles and fails at the intended first TODO, giving agents a realistic simulation/game task rather than another text-processing pipeline.

## [2026-07-06] measured | Biolife Rust eval results

Recorded the first clean Biolife eval comparison. Codex Goal completed in 359,959 ms with 123,707 comparable uncached tokens and one combined implementation commit; `mrmouth batch` completed in 670,820 ms with 233,213 uncached input plus output tokens and ten focused implementation commits. Both passed `cargo test --workspace`, left tests untouched, closed all Litebrite items, and left generated worktrees clean. Goal still did not compact; the max per-step live input was about 64,911 tokens against a reported 258,400-token context window.

## [2026-07-06] tightened | Biolife GUI eval requirements

Updated the Biolife fixture to require a separable frontend with an inspectable HTML/SVG GUI export instead of a CLI-only adapter. The generated task and tests now require `biolife_app --ticks N --light L --drag D --gui out.html`, parameter controls, run/reset controls, organism and segment inspectors, color-coded segment visualization, and simulation state sourced from `biolife_core`.

## [2026-07-06] tightened | Biolife live GUI requirements

Corrected the Biolife GUI requirement from inspectable export to live browser visualization. The fixture now requires the frontend to animate a backend-produced snapshot timeline in real time, expose play/pause/reset/scrub and parameter controls, update inspectors during playback, and keep simulation rules out of frontend JavaScript.

## [2026-07-06] corrected | Biolife native Rust app requirement

Corrected the Biolife frontend target again: the desired product is a native graphical Rust app, not HTML/SVG/browser output. The fixture now requires `biolife_app` to launch a Rust windowed app by default, keep a `--headless` mode for automated tests, expose realtime controls and inspectors in the native UI, and keep simulation rules in `biolife_core`.

## [2026-07-07] fixed | Split worktree reviewer scope

Recorded that reviewer commit ranges now come from the resolved code work repo for `do`, `ready`, and `loop`. Reviewer Docker containers still use `/home/runner/workspace` for Litebrite/Trapperkeeper state, but mount split code repos at `/home/runner/worktree` and prompt reviewers to run git diff/log plus build/test commands there.

## [2026-07-07] updated | Terminal lifecycle summary ordering

Recorded that orchestrated `do`/`batch`/`ready`/`loop` runner calls suppress nested `run` terminal summaries via `RunOptions::emit_terminal_events`, and that `loop` defers its final summary until after session teardown and log flush.

## [2026-07-07] fixed | Runner local remote and push handling

Recorded Docker runner conventions for host-local git origins: local bookkeeping origins are mounted at `/host-repo`, split worktree origins are mounted with in-container `url.*.insteadOf` rewrites, no-origin file-remotes still skip host pull, separate local origins do not skip host pull, and cleanup push failures now emit `::mrmouth::push-error` so lifecycle summaries surface them as failures.

## [2026-07-07] implemented | Partial eval telemetry caveats

Recorded that `src/telemetry.rs` centralizes timing and token usage parsing. Eval reports now distinguish `completed`, `partial`, and `missing` token usage, attach structured caveats when `turn.completed` is absent, and lifecycle summaries for runner/orchestrator commands attach log paths, JSONL paths, and parsed telemetry when available.

## [2026-07-07] updated | Runner context hygiene prompt

Recorded that `src/prompt.rs` now carries default runner guidance to avoid treating generated files, build outputs, logs, preserved eval artifacts, and agent/plugin caches as source context. The prompt names `.codex-home/`, `.tmp/plugins/`, `logs/`, `target/`, `preserved/`, and generated eval fixture output directories, and tests lock those examples into the embedded default prompt.
