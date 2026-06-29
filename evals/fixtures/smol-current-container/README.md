# Smol Current-Container Fixture

This is the smallest useful Mr Mouth eval fixture. The requested code change is
one line in `message.txt`, so orchestration overhead dominates the measurement.
That intentionally puts Mr Mouth at a disadvantage and gives us a baseline for
how much time the wrapper, task state, Codex startup, and current-container path
cost around a tiny task.

Run from this directory:

```sh
./run.sh
./assert.sh
```

`run.sh` rebuilds generated `repo/`, `remotes/`, and `reports/` directories from
`seed/` on each invocation. It initializes a bookkeeping git repo with
Litebrite state, initializes a separate code worktree repo, then runs:

```sh
mrmouth eval --cwd repo --output reports/result.json -- \
  mrmouth do <item-id> --json-events --current-container --worktree repo/worktree
```

The fixture requires `lb`, `git`, and the configured agent CLI on `PATH`.
