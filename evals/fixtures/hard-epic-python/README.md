# Hard Epic Python Fixture

This fixture is a harder multi-leaf eval for comparing `mrmouth batch` with
Codex Goal. It generates one parent epic with six ordered child tasks for a
small support operations reporting package:

1. ticket loading
2. SLA classification
3. queue routing
4. queue metrics
5. customer risk scoring
6. CLI reporting

Run from this directory:

```sh
./run_batch.sh
./assert_batch.sh
./run_goal.sh
./assert_goal.sh
```

`run.sh` is an alias for `run_batch.sh` so the fixture has a default measured
entrypoint. The fixture requires `lb`, `trk`, `git`, `python3`, `node`, and the
configured agent CLI on `PATH`.
