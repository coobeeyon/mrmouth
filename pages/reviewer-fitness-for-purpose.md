# Reviewer Fitness For Purpose

Reviewer runs should evaluate both code quality and whether the diff satisfied
the work item that caused the run. This is separate from generic review against
`SPEC.md`: a change can be buildable and locally reasonable while still missing
the specific behavior, tests, documentation, or closure semantics requested by a
Litebrite item.

`reviewer::ReviewerOptions` carries an optional `ReviewTarget` with the
Litebrite item ID and label. `mrmouth do` passes the explicit requested item for
both task and epic reviews. `mrmouth ready` passes the selected ready task. The
reviewer prompt tells the agent to run `lb show <item-id>`, treat that title,
description, parent/child context, and acceptance details as the primary
purpose, and ask whether the diff is fit for that purpose.

The autonomous `mrmouth loop` reviewer may not know a single item because the
runner claims work internally through the default prompt. In that case the
reviewer prompt explicitly falls back to inferring intended work from Litebrite
state, commit messages, git log, and the diff, then reviewing the change for
fitness against that inferred purpose plus `SPEC.md`.

Issues that count for review include normal bugs and build/test failures, plus
failure to satisfy the requested item, missing tests for the requested behavior,
unfinished required documentation, and premature task closure.
