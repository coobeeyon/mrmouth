# Medium Python CLI Fixture

This fixture is a medium-sized deterministic eval. The code worktree contains a
small Python CLI that summarizes order CSV data. The seed implementation is
intentionally incomplete: it handles simple revenue totals but does not yet
implement discounts, refunds, category summaries, date ranges, or the exact JSON
shape required by the tests.

Run from this directory:

```sh
./run.sh
./assert.sh
./run_goal.sh
./assert_goal.sh
```

`prepare.sh` rebuilds generated `repo/`, `remotes/`, and `reports/`
directories from `seed/` on each invocation. It initializes Litebrite,
Trapperkeeper, Codex hook/rule setup, generated project trust, and trusted
`lb prime`/`trk prime` hooks before either harness starts.

`run.sh` measures Mr Mouth through `mrmouth eval` and `mrmouth do`.
`run_goal.sh` measures Codex Goal mode through the app-server harness.

The fixture requires `lb`, `trk`, `git`, `python3`, and the configured agent CLI
on `PATH`.
