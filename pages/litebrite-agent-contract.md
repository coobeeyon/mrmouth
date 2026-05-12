# Litebrite Agent Contract

Litebrite (`lb`) is the task substrate for mrmouth. It stores epics, features,
tasks, dependencies, status, and claims as JSON on an orphan `litebrite` branch,
not in the working tree. Local reads are fast and do not touch the network;
claiming and sync are the operations that coordinate with the remote.

## Agent-Facing Context

`lb prime` is intentionally the AI-facing summary. It exits successfully and
prints nothing outside a git/litebrite repo, so hooks can run it globally. In a
litebrite repo it prints:

- active claimed open items
- ready unblocked/unclaimed items
- the session protocol
- the CLI quick reference

For Codex or Claude sessions, treat the prime output as sufficient operating
context unless there is a concrete reason to inspect litebrite internals.

## Default Work Protocol

The normal single-agent protocol is:

1. `lb ready` to discover open, unblocked, unclaimed work.
2. `lb show <id>` to read full task context.
3. `lb claim <id>` before starting work.
4. Make code changes and commit them.
5. `lb close <id>` after the implementation commit exists.
6. `lb sync` to publish task-state changes.

`lb close` clears any claim. It rejects closing an item that still has open
children, which is why epic-level work should usually close leaf tasks first.

## Command Semantics That Matter To Mr Mouth

`lb ready` is discovery, not claiming. It prints open, unclaimed items whose
blocking dependencies are closed, sorted by numeric priority. A supervisor agent
can use it to choose work, but ownership does not begin until `lb claim`.

`lb show <id>` resolves unique ID prefixes and prints status, priority,
description, parent, children, blockers, blocked items, and claim status.

`lb claim <id>` is the atomic work-assignment boundary. It fetches/fast-forwards
when a remote exists, sets `claimed_by` to `git config user.name`, commits to the
litebrite branch, and pushes. If another worker already claimed the item, it
fails instead of stealing the claim. Without a remote, it still works locally.

`lb unclaim <id>` clears a claim and syncs when possible.

`lb sync` requires a remote. It fetches, fast-forwards when possible, otherwise
performs a schema-aware three-way merge and pushes.

## Data Model

Items have:

- `id`: generated `lb-XXXX` style ID; unique prefixes are accepted by commands
- `item_type`: `epic`, `feature`, or `task`
- `status`: `open` or `closed`
- `priority`: numeric priority, lower sorts earlier
- `claimed_by`: optional worker name
- optional description plus created/updated timestamps

Dependencies have two forms:

- `parent`: child item points to parent item
- `blocks`: blocker item points to blocked item

Parent/child is the fundamental decomposition relationship. A parent does not
depend on its children in the `blocks` sense; instead, the parent is complete
when all of its children are complete, and `lb close` enforces that by rejecting
parents with open children. Use nested parents to represent a work breakdown:
epic -> feature -> task, or any hierarchy that fits the project.

Use `blocks` only for ordering constraints between items, especially sibling
items. If sibling B cannot start until sibling A lands, model that explicitly
with `lb dep add <A> --blocks <B>`. Do not encode sibling sequencing by making
one sibling the parent of another.

Blocked is derived from open blockers; it is not a separate status. Claimed is a
separate field and claimed items are excluded from `lb ready`.

## Concurrency And Merge Rules

Litebrite uses git as the coordination mechanism. `claim` is first-push-wins:
if a push races, the loser fetches remote state and fails if the remote item is
already claimed. For general sync, litebrite merges stores field-by-field:
non-conflicting item changes merge, remote `claimed_by` wins, and dependencies
are merged as a union minus removals.

## Implications For Mr Mouth Skills

A useful Codex skill here is more general than mrmouth operation: it should
author good litebrite work graphs. Mr Mouth is one important downstream consumer,
but the core capability is converting a user goal into a nested brite hierarchy
with executable leaves and explicit sibling ordering.

A mrmouth operator skill should not need deep litebrite knowledge beyond
`lb prime` and the protocol above. The outer agent should use litebrite for task
selection and ownership, then delegate bounded work to mrmouth. The safest
default is:

- prepare nested parent/child brites as the primary work structure
- use `blocks` dependencies only for explicit ordering between siblings
- use `lb ready`/`lb show` to pick one executable leaf or bounded item
- use `mrmouth do <id>` for bounded delegation
- reserve mrmouth queue-draining commands for explicit user requests
- after a mrmouth run, verify commits, task closure, and `lb sync`

Do not treat `lb ready` as a lock. Do not close an item before committing the
code intended to satisfy it.

## Brite Authoring Guidance

When preparing work for autonomous agents, prefer a hierarchy first:

- parent brites describe user-facing outcomes or coherent areas of work
- child brites decompose the parent into reviewable slices
- leaf brites are sized for one focused worker session
- sibling `blocks` dependencies represent real sequencing constraints

Leaf descriptions should serve as ramp-in notes. They should name the goal,
relevant files/modules, existing patterns to follow, concrete requirements,
acceptance checks, and out-of-scope nearby work. The worker should not need a
conversation or broad rediscovery pass before starting.

Parent descriptions should explain the outcome and why it matters, but should
not duplicate every child requirement. A parent is done when its children are
closed and the integrated behavior works.
