# Mr Mouth Speed And Evals

Concepts: Codex exec startup cost, session reuse, app-server, eval harness, lifecycle metrics
Key files: `src/agent.rs`, `src/streaming.rs`, `src/run.rs`, `src/do_cmd.rs`, `src/loop_cmd.rs`, `src/reviewer.rs`, `evals/README.md`, `evals/codex_goal_harness.mjs`
Commands/config: `mrmouth eval -- <command>`, `mrmouth do --json-events`, `mrmouth run --json-events`, `mrmouth loop --json-events`, `codex exec`, `codex exec resume`, `codex app-server`
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
  total wall clock. Existing `::mrmouth::timing` markers cover runner/container
  and role-level wall time. A remaining high-value marker is Codex process spawn
  to first JSON event.

`mrmouth eval` is the first thin harness for speed/correctness experiments. It
wraps any child command, usually another Mr Mouth invocation with
`--json-events`, and writes a JSON report:

```sh
mrmouth eval --output logs/eval-result.json -- mrmouth run --json-events
```

Relative `--output` paths resolve from the current repository; `--cwd <path>`
sets the child command's working directory. The report captures command argv,
cwd, child success/exit code, wrapper wall time, stdout/stderr byte counts,
lifecycle event counts parsed from child stdout, the final
`lifecycle_summary`, and timing markers parsed from the summary's `log_path`.
The harness writes the report even when the child fails, then exits nonzero so
scripts still observe the failed run.

Role-level wall-clock markers are now emitted to orchestration logs for the
main non-runner AI roles:

- `decider-wall` around loop decider work, including the open-task
  short-circuit path
- `shipper-wall` around loop shipper execution
- `reviewer-wall` around reviewer execution in `do`, `ready`, and `loop`
- `summary-wall` around loop summary generation

These appear beside existing runner/container markers such as `docker-build`,
`session-setup`, `container-wall`, and `current-container-wall`, and use the
same `::mrmouth::timing phase=<name> elapsed_ms=<n>` format parsed by
`mrmouth eval`.

`evals/README.md` defines the first fixture-backed eval contract. Fixtures live
under `evals/fixtures/<name>/`, prepare generated input state in `repo/`, run
through a single `run.sh`, and verify deterministic outcomes through
`assert.sh`. Assertions should inspect the eval report, tests, changed files,
commits, Litebrite state, and required timing phases before any LLM judge is
used.

`evals/fixtures/smol-current-container/` is the first real fixture. It rebuilds
generated `repo/`, `reports/`, and `remotes/` directories from committed
`seed/` inputs, initializes local bare remotes, creates one Litebrite task, and
runs both Mr Mouth and Codex Goal mode against the same seeded task. The
bookkeeping repo setup includes `lb init`, `trk init`, `lb setup codex`, and
`trk setup codex`; the generated `CODEX_HOME` trusts the fixture project and the
two project-local SessionStart hooks (`lb prime` and `trk prime`). `run.sh` uses
`mrmouth do --json-events --current-container --worktree repo/worktree` through
`mrmouth eval`; `run_goal.sh` uses `evals/codex_goal_harness.mjs` to start
`codex app-server --stdio --enable goals`, create a thread, set a goal with
`thread/goal/set`, run one turn with `turn/start`, and assert the final goal
status is `complete`. The task changes one line in `message.txt`, runs
`./check.sh`, commits the worktree, and closes the task. In sandboxed fixture
runs after hook/trust setup, the Mr Mouth path completed successfully in about
47 seconds with a `current-container-wall` marker, and the Codex Goal app-server
path completed successfully in about 87-127 seconds with both project hooks
firing and `tokensUsed` around 49-64k. The fixture runner sets a writable ignored
`CODEX_HOME` under the generated bookkeeping repo because nested Codex can fail
when its default home or helper path is read-only. Goal prompt files are ignored
eval artifacts so they do not create cleanup work inside the measured task.

The Goal-mode harness intentionally uses app-server goal APIs instead of trying
to automate TUI keystrokes for `/goal`. This gives us a stable machine-readable
baseline for time and quality comparison while still exercising Codex's
persistent goal machinery: the report records wall clock, app-server event
counts, thread/turn ids, final goal object, and optional raw JSONL events.
Because the goal status is self-reported by Codex, fixture assertions should
also check same-turn evidence from the app-server event stream. The smol goal
assertion now requires command/diff evidence that the turn saw the original
`message.txt`, produced the target diff, ran `check.sh`, committed the worktree,
closed the Litebrite item, and observed the closed item state.

`evals/fixtures/medium-python-cli/` is the first larger deterministic fixture.
It rebuilds generated bookkeeping/worktree repos from seed state, initializes
the same Litebrite, Trapperkeeper, Codex hook/rule, project trust, and trusted
hook setup as the smol fixture, then asks the agent to implement a Python sales
summary CLI. The task requires reading `SPEC.md`, fixing CSV discount/refund
semantics, producing the expected JSON shape, running `./check.sh`, committing
the worktree, and closing the Litebrite item. Fixture seed worktrees should
ignore Python bytecode (`__pycache__/`, `*.pyc`) so syntax checks or test runs do
not poison the generated initial commits.

The first clean medium runs both passed deterministic assertions. Mr Mouth
`run.sh` completed in 170,520 ms with a `current-container-wall` marker and one
worktree commit touching `sales_report.py`. Codex Goal `run_goal.sh` completed
in 157,463 ms, used one turn, recorded final goal status `complete`, reported
54,958 tokens used, fired both SessionStart hooks, ran 42 captured commands, and
committed the same single-file implementation shape. This is a more plausible
comparison point than the smol fixture because the task work is large enough to
start absorbing fixed harness/setup overhead, though it still fits within one
context and one turn.

Token accounting needs cache-aware normalization before comparing Mr Mouth to
Codex Goal. A medium Mr Mouth rerun reported one Codex `turn.completed` event
with 366,320 raw input tokens, 300,672 cached input tokens, 6,294 output tokens,
and 1,195 reasoning output tokens. The raw input total is cumulative across the
internal Codex tool loop, where each model step sees accumulated context; the
uncached input plus output was 71,942 tokens. A matched Goal rerun exposed
app-server `tokenUsage.total` with 411,357 raw input tokens, 347,520 cached input
tokens, 6,050 output tokens, and 1,132 reasoning output tokens; its uncached
input plus output was 69,887 tokens. The apparent 300k-400k raw-token totals are
therefore mostly cache hits, and the apples-to-apples medium comparison is much
closer: about 72k uncached tokens for Mr Mouth versus about 70k for Goal. Eval
reports now add normalized token fields: Mr Mouth parses
`lifecycle.token_usage` from inner Codex JSONL `turn.completed.usage`, and the
Goal harness preserves final goal tokens, `thread/tokenUsage/updated` samples,
normalized nested app-server usage, and comparable aggregate fields.

`evals/fixtures/multi-do-epic-python/` is the first multi-leaf fixture. It
generates a parent Litebrite epic with four ordered child tasks for a small
Python inventory package: catalog loading, stock movement application, reorder
planning, and CLI reporting. The Mr Mouth harness intentionally stays on
`mrmouth do`: it runs one `mrmouth eval -- mrmouth do <leaf>` per child in
dependency order, then writes an aggregate `reports/result.json` with summed
child wall time and token usage. The Goal harness gives one persistent app-server
goal the parent epic and all child ids, then asserts that all children and the
epic are closed. Both assertions protect against test/data edits and require the
final `./check.sh` suite to pass.

The first clean multi-leaf runs both passed deterministic assertions and each
produced four focused implementation commits in the code worktree. Mr Mouth
multi-do completed in 550,945 ms, with summed child wall 550,896 ms and
261,521 uncached input plus output tokens across four Codex turns. Child
uncached totals were about 76k, 75k, 56k, and 54k. Codex Goal completed the same
epic in 225,378 ms with one app-server turn and 86,095 uncached input plus output
tokens. This is the first fixture where the repeated `codex exec`/task-boundary
cost becomes obvious while still avoiding `mrmouth loop`, reviewer, shipper, or
decider roles.

Successful `mrmouth do` lifecycle summaries now attach `logs/latest.log` and
`logs/latest.jsonl` when present. This is what lets `mrmouth eval` parse timing
markers from successful `do` runs; before that change, only failed `do` runs
carried log paths in the terminal summary.

The likely implementation sequence is:

1. Add missing timing events around Codex startup/first-event spans.
2. Run baseline evals across cold, warm, and current-container paths.
3. Prototype `codex exec resume` for bounded multi-turn flows.
4. Consider `codex app-server` only if resume cannot provide the needed speed
   or control.
