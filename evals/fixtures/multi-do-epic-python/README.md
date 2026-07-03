# Multi-Do Epic Python Fixture

This fixture compares repeated `mrmouth do` leaf execution with one Codex Goal
run over a small Litebrite epic.

The generated bookkeeping repo contains one epic and four ordered child tasks:

1. catalog loading
2. stock movement application
3. reorder planning
4. CLI reporting

Run from this directory:

```sh
./run.sh
./assert.sh
./run_goal.sh
./assert_goal.sh
```

`run.sh` rebuilds the fixture, runs `mrmouth do` once for each leaf task, and
writes per-leaf reports plus `reports/result.json` with summed wall and token
totals. `run_goal.sh` gives Codex Goal one objective to complete the epic and
all children.

The fixture requires `lb`, `trk`, `git`, `python3`, `node`, and the configured
agent CLI on `PATH`.
