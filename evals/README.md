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

## Fixtures

- `fixtures/smol-current-container/` is a one-line text edit used to validate
  harness parity and setup costs.
- `fixtures/medium-python-cli/` is a small Python CLI implementation task. It
  exercises CSV parsing, JSON shape changes, tests, a worktree commit, and
  Litebrite closure while still fitting in a single turn for both harnesses.
