# Reviewer Fitness For Purpose

## September 17, 2026 recovery checkpoint

The split-repository behavior described below was implemented on the old
evaluation branch, not on main. Main `fedc1e2` still compares bookkeeping HEADs
and omits the reviewer's code mount. A fresh CLI regression reproduces both a
code-only commit skipping review and a bookkeeping-only commit triggering it.

[PR 10](https://github.com/coobeeyon/mrmouth/pull/10), exact candidate
`35fc23510d37a6c149f104b9744509cd4a65c849`, recovers `40e7574` onto main with
provider-free CLI regressions and the prerequisite narrow Clippy annotations
from `ff77fc8`. It also clarifies that code and bookkeeping branches can differ.
This is a review candidate, not a merged or installed change.

[Hosted CI run 35290723421](https://github.com/coobeeyon/mrmouth/actions/runs/35290723421)
completed successfully at that exact head, including strict Clippy and the
full test suite.

Local verification passes: 196 unit tests, three integration groups spanning
twelve cases across task, epic, ready and loop, locked build, and strict
all-target Clippy. The main control has two failing regression groups and one
passing ordinary-repository group. Fixtures use real Git repos and fake
Docker/agents; actual Docker execution and provider review quality are not
proved. See `docs/reviews/split-repository-review.md` in the candidate.

Source recovery is tracked by `lb-9t9e`; `lb-4x2d` owns follow-up review and
landing. Broader evaluation recovery `lb-h7vj` remains open. The dirty July
evaluation checkout, untracked source variants and generated evidence were
preserved without edits or deletion.

## Reviewer contract

Reviewer runs should evaluate both code quality and whether the diff satisfied
the work item that caused the run. This is separate from generic review against
`SPEC.md`: a change can be buildable and locally reasonable while still missing
the specific behavior, tests, documentation, or closure semantics requested by a
Litebrite item.

`reviewer::ReviewerOptions` carries an optional `ReviewTarget` with the
Litebrite item ID and label. It also carries an optional split worktree host
path. When present, reviewer Docker containers mount that path at
`/home/runner/worktree`, keep Litebrite/Trapperkeeper commands rooted in
`/home/runner/workspace`, and tell the reviewer to run git diff/log plus
build/test commands in the code worktree. `mrmouth do` passes the explicit
requested item for both task and epic reviews. `mrmouth ready` passes the
selected ready task. The reviewer prompt tells the agent to run
`lb show <item-id>`, treat that title, description, parent/child context, and
acceptance details as the primary purpose, and ask whether the diff is fit for
that purpose.

The autonomous `mrmouth loop` reviewer may not know a single item because the
runner claims work internally through the default prompt. In that case the
reviewer prompt explicitly falls back to inferring intended work from Litebrite
state, commit messages, git log, and the diff, then reviewing the change for
fitness against that inferred purpose plus `SPEC.md`.

Split bookkeeping/work-repo reviews calculate `head_before..head_after` from the
resolved code repo (`LocalWorktree.target_mount` or `RepoLayout.work_repo`), not
from the bookkeeping repo. This matters when a runner only commits task-state
bookkeeping in `/home/runner/workspace` but commits implementation changes in
`/home/runner/worktree`; the reviewer should inspect implementation commits, not
bookkeeping-only churn.

Issues that count for review include normal bugs and build/test failures, plus
failure to satisfy the requested item, missing tests for the requested behavior,
unfinished required documentation, and premature task closure.

Review issue placement matters. For a reviewed epic or feature, issue tasks
should be children of that reviewed item. For a reviewed task with a parent,
issue tasks should be siblings under the same parent. Top-level review issues
should be reserved for cases where no relevant work context exists.
