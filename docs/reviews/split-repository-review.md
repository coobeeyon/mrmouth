# Split-repository review recovery

When `work_repo` or `do --worktree` selects a separate code checkout, the
runner writes code there while task tracking stays in the bookkeeping repo.
Main at `fedc1e2` measures the bookkeeping HEAD before and after the run and
does not mount the code checkout for its reviewer. A code-only commit can
therefore produce `Reviewer skipped: no new commits`; bookkeeping-only commits
can trigger a review of the wrong repository.

This candidate recovers `40e7574` from the preserved
`mrmouth-eval-defects-20260707` branch. It measures the code HEAD in task, epic,
ready and loop dispatch, passes the code mount into the reviewer, and directs
diff/build/test commands to that mount. Task commands remain in bookkeeping.
The prompt also avoids claiming that the code checkout shares the bookkeeping
branch, and tells the reviewer to keep the mounted branch unchanged.

The seven narrow Clippy annotations from `ff77fc8` are recovered separately
because existing argument-count and event-size warnings otherwise fail the
repository's CI gate. They change no runtime behavior. Other evaluation work
and preserved run outputs are outside this candidate.

## Verification

`tests/split_review.rs` drives the compiled CLI through `do` task, `do` epic,
`ready`, and a one-iteration `loop`. Each command is checked for:

- a code-only commit starting review with the exact before/after code SHAs;
- a bookkeeping-only commit not triggering code review;
- ordinary single-repository review continuing to work.

The fixtures use real disposable Git repositories, including a code path with
spaces and a different code branch. Fake Docker records the actual launch
arguments and generated reviewer script. Fake agents and task commands keep
the tests provider-free. The fixtures use no remote repositories or inherited
user configuration or credentials.

On main, the code-only and bookkeeping-only regression groups fail. The
single-repository group passes. On this candidate, all twelve cases pass,
along with 196 unit tests. `cargo build --locked` and
`cargo clippy --locked --all-targets -- -D warnings` also pass.

The proof covers orchestration, mount arguments and generated instructions.
It does not execute a real Docker reviewer or establish the quality of an
agent's eventual review. Current-container mode continues to skip Docker
reviewers by design.

Run the regression with `cargo test --locked --test split_review`. On hosts
whose temporary directory forbids execution, set `TMPDIR` to an executable
temporary directory before running it.
