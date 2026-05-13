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
- core command semantics for `run`, `do`, `ready`, `loop`, `summary`, and
  `setup codex`
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
