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

Blocked is derived from open blockers; it is not a separate status. Claimed is a
separate field and claimed items are excluded from `lb ready`.

## Concurrency And Merge Rules

Litebrite uses git as the coordination mechanism. `claim` is first-push-wins:
if a push races, the loser fetches remote state and fails if the remote item is
already claimed. For general sync, litebrite merges stores field-by-field:
non-conflicting item changes merge, remote `claimed_by` wins, and dependencies
are merged as a union minus removals.

## Implications For Mr Mouth Skills

A mrmouth operator skill should not need deep litebrite knowledge beyond
`lb prime` and the protocol above. The outer agent should use litebrite for task
selection and ownership, then delegate bounded work to mrmouth. The safest
default is:

- use `lb ready`/`lb show` to pick one item
- use `mrmouth do <id>` for bounded delegation
- reserve mrmouth queue-draining commands for explicit user requests
- after a mrmouth run, verify commits, task closure, and `lb sync`

Do not treat `lb ready` as a lock. Do not close an item before committing the
code intended to satisfy it.
