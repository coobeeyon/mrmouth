# Mr Mouth Evals

This directory is the home for fixture-backed evals. The first eval layer is
deliberately simple: a fixture is a small repository or workspace that can be
run under `mrmouth eval`, then checked with deterministic assertions.

## Fixture Contract

Each fixture should live under `evals/fixtures/<name>/` and include:

- `README.md` explaining the behavior being evaluated.
- `repo/` containing the generated input repository state, including any
  `.mrmouth/`, Litebrite, or Trapperkeeper setup the case requires.
- `seed/` when the generated `repo/` should be rebuilt for each run instead of
  committed directly.
- `run.sh` as the single entrypoint for the measured command.
- `run_goal.sh` when the fixture supports the Codex Goal-mode harness.
- `assert.sh` for deterministic checks after the run.
- `assert_goal.sh` for deterministic checks after `run_goal.sh`.

`run.sh` should write its report under `reports/` inside the fixture or under a
caller-provided path. Keep it explicit and scriptable:

```sh
../../target/debug/mrmouth eval \
  --cwd repo \
  --output ../reports/result.json \
  -- mrmouth do <item-id> --json-events
```

`assert.sh` should inspect the fixture repo and eval report without calling an
LLM. Good first checks are:

- the eval report exists and has `success: true`
- the final lifecycle summary has the expected command, status, and item id
- required tests pass in the fixture repo
- expected files changed and unrelated files did not
- expected commits exist
- Litebrite state is correct, including closed/completed items
- required timing phases are present in `lifecycle.timing_markers`

Use LLM judging only after deterministic checks pass, and feed it the Litebrite
item text plus the final diff. LLM judging should explain fitness for purpose;
it should not replace file, test, task-state, or timing assertions.

## Baseline Matrix

The first speed baseline should run the same fixture through these modes:

- cold Docker: no existing image/container cache assumptions
- warm Docker: image already built
- session reuse: epic or loop flow that reuses the session container
- current-container: `--current-container` with explicit `--worktree`

Compare report fields first: `wall_ms`, lifecycle event counts, final summary,
and timing phases such as `docker-build`, `session-setup`, `container-wall`,
`current-container-wall`, `reviewer-wall`, `decider-wall`, `summary-wall`, and
`shipper-wall`.

Token comparisons should use normalized report fields, not ad hoc log scraping.
For Mr Mouth runs, `lifecycle.token_usage` is parsed from the inner Codex JSONL
`turn.completed.usage` records and includes raw input, cached input, uncached
input, output, reasoning output, total, and uncached total. Raw input can be much
larger than the effective uncached total because each Codex tool-loop step
replays accumulated context and most stable prefixes may be cache hits.

For Codex Goal runs, `token_usage` preserves the final goal `tokensUsed`, the
latest `thread/tokenUsage/updated` payload, any normalized turn usage fields,
and comparable total fields. Prefer `total_uncached_tokens` or
`comparable_total_uncached_tokens` when both harnesses expose cache-aware
breakdowns; otherwise call out that the comparison is using each surface's best
available aggregate.

## Fixtures

- `fixtures/smol-current-container/` is a one-line text edit used to validate
  harness parity and setup costs.
- `fixtures/medium-python-cli/` is a small Python CLI implementation task. It
  exercises CSV parsing, JSON shape changes, tests, a worktree commit, and
  Litebrite closure while still fitting in a single turn for both harnesses.
- `fixtures/multi-do-epic-python/` is a four-leaf epic fixture. Its Mr Mouth
  path runs `mrmouth do` once per child and aggregates per-child reports, while
  its batch path runs one experimental `mrmouth batch` execution over the
  parent epic. Its Goal path asks one persistent goal turn to complete the
  parent epic and all children.
- `fixtures/hard-epic-python/` is a six-leaf support operations fixture for
  pushing batch and Goal harder on a larger package while preserving focused
  per-child commits and deterministic checks.
