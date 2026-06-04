# Mr Mouth Prime

`mrmouth prime` is the AI-facing context surface for supervising agents. It is
similar in spirit to `lb prime`: a safe command that prints operating context
without starting a TUI, running Docker, claiming work, or mutating task state.

The command loads local `.mrmouth/config.toml` when available and falls back to
defaults, so it can be run from shell hooks or top-level agent startup. Outside
a git repo, it falls back to the current directory and default config.

The prime output covers:

- effective local defaults: repository, bookkeeping repo, work repo, repo layout,
  agent, model, Docker image, Dockerfile, volume, log dir, base branch, timeout,
  and failure limit
- core command semantics for `run`, `do`, `ready`, `loop`, `summary`,
  `setup codex`, and `codex-login`
- global `--claude` and `--codex` agent override flags
- the supervisor output contract for `--json-events`, `--raw`, and the final
  `lifecycle_summary` event
- the recommended protocol: use `lb prime`, pick one item with `lb ready` and
  `lb show`, prefer `mrmouth do <id> --json-events`, and reserve queue-draining
  commands for explicit user requests

The most important operational distinction is bounded delegation versus
autonomous draining. Supervising agents should default to `mrmouth do <id>
--json-events` because it maps cleanly to one explicit litebrite item and
provides machine-readable lifecycle status. `mrmouth ready` and `mrmouth loop`
are intentionally broader and should be used only when the user asks for that
behavior.

`mrmouth` resolves both a bookkeeping repo and a work repo for each run. By
default they are the same path. `.mrmouth/config.toml` can set `work_repo =
"path/to/code"` for fake-monorepo layouts where `.mrmouth`, `lb`, and `trk`
state live in an outer repo while code lives in an inner repo; `--worktree
<path>` overrides that configured work repo for one invocation. When the
canonical paths differ, Docker keeps bookkeeping at `/home/runner/workspace`,
bind-mounts the work repo at `/home/runner/worktree`, sets
`MRMOUTH_BOOKKEEPING_REPO` and `MRMOUTH_WORK_REPO`, and keeps
`MRMOUTH_WORKTREE` as a compatibility alias. The generated prompt tells the
runner where to run task-state commands and where to make code changes. The path
model is owned by `src/repo_layout.rs` and wired through `src/main.rs`,
`src/run.rs`, `src/do_cmd.rs`, `src/ready.rs`, `src/loop_cmd.rs`, and
`src/docker.rs`.

`mrmouth do` still supports local execution modes. `--local` bind-mounts the
bookkeeping repo at `/home/runner/workspace` instead of cloning; if the resolved
work repo is distinct, it is mounted at `/home/runner/worktree` too.

`--current-container` (also `--no-docker`, and `--in-place` for `do`) skips
Docker entirely and runs the configured agent CLI directly in the current
checkout. It keeps normal logs and lifecycle JSON, uses host `git`/`lb`/`trk`
tools, and skips Docker reviewers for `do` so the whole path remains usable
from an already-running development container. It requires the resolved work
repo to be distinct from bookkeeping, either through `work_repo` config or a
`--worktree <path>` override; in that case the prompt and environment point the
agent at the local code checkout while `lb` stays in the bookkeeping repo.

`mrmouth setup codex` follows the Trapperkeeper-style hook setup pattern. It
enables Codex hooks in `.codex/config.toml`, adds a `SessionStart` hook in
`.codex/hooks.json` that runs `mrmouth prime`, and adds a
`.codex/rules/default.rules` prefix allow rule for `mrmouth`. It is not the
Codex auth flow; auth remains `mrmouth codex-login`.
