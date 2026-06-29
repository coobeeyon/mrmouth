# Mr Mouth Speed And Evals

Concepts: Codex exec startup cost, session reuse, app-server, eval harness, lifecycle metrics
Key files: `src/agent.rs`, `src/streaming.rs`, `src/run.rs`, `src/do_cmd.rs`, `src/loop_cmd.rs`, `src/reviewer.rs`
Commands/config: `mrmouth do --json-events`, `mrmouth run --json-events`, `mrmouth loop --json-events`, `codex exec`, `codex exec resume`, `codex app-server`
Useful when: comparing Mr Mouth to Codex `/goal`, designing evals, profiling agent latency, or changing Codex invocation strategy

Mr Mouth currently pays a fresh Codex batch invocation for each agent role that uses
Codex. The generated scripts in `src/agent.rs` call `codex exec --json ...`, and
the direct current-container path in `src/streaming.rs` builds the same kind of
`codex exec` command. Epic and loop flows reuse a long-lived Docker session via
`src/run.rs::start_session` plus `execute_in_session`, but that reuse is at the
container/setup layer. It avoids repeating Docker clone/setup work, not Codex
thread startup or prompt/context rehydration.

Current Codex CLI capabilities worth evaluating before replacing the launcher:

- `codex exec resume` can continue a previous non-interactive session, including
  `--last` or a specific session id. This is a plausible low-disruption speed
  experiment for multi-stage flows, but it needs validation around task
  isolation, prompt accumulation, and whether logs/lifecycle summaries stay
  attributable to the right Litebrite item.
- `codex app-server` exposes programmatic thread/turn APIs over JSON-RPC. It is
  the deeper integration path for persistent threads, streamed events,
  approvals, and richer clients. The official manual currently recommends the
  Codex SDK for CI-style automation, while app-server is framed as a rich-client
  integration surface, so adopting it should be treated as an architectural
  change rather than a simple command swap.
- Codex `/goal` attaches a persistent objective to an active CLI thread. Mr
  Mouth's closest durable product advantage is not the in-thread goal itself,
  but externalized task state, claims, commits, reviewer/shipper roles, and
  machine-readable lifecycle events.

The eval design should separate correctness and latency:

- Correctness evals should run frozen fixture repositories with seeded Litebrite
  graphs, invoke `mrmouth do --json-events` or `mrmouth run --json-events`, and
  assert deterministic outcomes first: expected tests pass, required files
  changed, unrelated files unchanged, commits exist, `lb` state is correct, and
  lifecycle JSON ends in a stable `lifecycle_summary`.
- LLM judging is still useful for fitness-for-purpose review, but it should sit
  after deterministic checks and use the Litebrite item text plus diff as the
  judgment input.
- Speed evals should record cold Docker, warm Docker/session, current-container,
  agent first-event latency, agent wall time, reviewer time, summary time, and
  total wall clock. Existing `::mrmouth::timing` markers already cover several
  phases; missing high-value markers include Codex process spawn to first JSON
  event and per-role timing in reviewer/decider/summary/shipper paths.

The likely implementation sequence is:

1. Add a small eval/bench harness that consumes lifecycle JSON and logs existing
   timing markers, before changing the launcher.
2. Add missing timing events around Codex startup/first-event and role-level
   reviewer/summary/decider spans.
3. Run baseline evals across cold, warm, and current-container paths.
4. Prototype `codex exec resume` for bounded multi-turn flows.
5. Consider `codex app-server` only if resume cannot provide the needed speed
   or control.
