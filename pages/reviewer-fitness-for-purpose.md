# Reviewer Fitness For Purpose

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
