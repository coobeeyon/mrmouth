# Mr Mouth Prime

`mrmouth prime` is the AI-facing context surface for supervising agents. It is
similar in spirit to `lb prime`: a safe command that prints operating context
without starting a TUI, running Docker, claiming work, or mutating task state.

The command loads local `.mrmouth/config.toml` when available and falls back to
defaults, so it can be run from shell hooks or top-level agent startup. Outside
a git repo, it falls back to the current directory and default config.

The prime output covers:

- effective local defaults: repository, agent, model, Docker image,
  Dockerfile, volume, log dir, base branch, timeout, and failure limit
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

`mrmouth do` supports three local execution modes. `--local` bind-mounts the
current tracking repo at `/home/runner/workspace` instead of cloning. `--worktree
<path>` preserves the normal cloned tracking repo at `/home/runner/workspace`
and additionally bind-mounts the resolved host path at `/home/runner/worktree`
with `MRMOUTH_WORKTREE=/home/runner/worktree`. The generated task prompt tells
the runner to use `/home/runner/workspace` for Litebrite/tracking commands and
`/home/runner/worktree` for code edits, commits, and pushes. The path is wired
through `DoOptions` in `src/do_cmd.rs`, `RunOptions` and session setup in
`src/run.rs`, and Docker command construction in `src/docker.rs`; tests cover
CLI parsing, prompt wording, canonical host-path resolution, run/session arg
propagation, and Docker bind-mount arguments.

`--current-container` (also `--no-docker`, and `--in-place` for `do`) skips
Docker entirely and runs the configured agent CLI directly in the current
checkout. It keeps normal logs and lifecycle JSON, uses host `git`/`lb`/`trk`
tools, and skips Docker reviewers for `do` so the whole path remains usable
from an already-running development container. It can be combined with
`--worktree <path>` for split tracking/code repos; in that case the prompt and
`MRMOUTH_WORKTREE` point the agent at the local code checkout while `lb` stays
in the tracking repo.

`mrmouth setup codex` follows the Trapperkeeper-style hook setup pattern. It
enables Codex hooks in `.codex/config.toml`, adds a `SessionStart` hook in
`.codex/hooks.json` that runs `mrmouth prime`, and adds a
`.codex/rules/default.rules` prefix allow rule for `mrmouth`. It is not the
Codex auth flow; auth remains `mrmouth codex-login`.
