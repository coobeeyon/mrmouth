# Mr Mouth

Run Claude Code or Codex as an autonomous coding agent inside Docker containers.

You run `mrmouth` from inside any git repo. It builds a Docker image, launches a container with the repo cloned, runs an agent CLI with a structured prompt, streams formatted output, and pulls changes when the agent exits.

## Install

```bash
cargo install --git https://github.com/coobeeyon/mrmouth.git
```

## Prerequisites

- **Docker** — container runtime for agent isolation
- **SSH agent** — running with keys that have access to your repo (`ssh-add`)
- **Credentials** — for Claude, set `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`; for Codex, set `OPENAI_API_KEY` or log in with Codex in the persisted container home

Optional:
- **lb** ([litebrite](https://github.com/coobeeyon/litebrite)) — task tracking CLI for the relay pattern

## Quick Start

```bash
export ANTHROPIC_API_KEY=sk-...
cd my-project
mrmouth run
```

That's it. No setup needed — mrmouth uses sensible defaults for everything.

## Commands

### `mrmouth prime`

Print an agent-facing summary of what mrmouth is, which commands are safe for
supervisors to use, and how to consume lifecycle JSON.

```bash
mrmouth prime
```

This command is intended for top-level coding agents and global shell hooks,
similar to `lb prime`. It includes the effective local defaults when run inside
a repository, but it also works outside a git repo with built-in defaults.

### `mrmouth run`

Run one agent session.

```bash
mrmouth run [--claude|--codex] [--raw|--json-events] [--model <model>] [--timeout <minutes>] [--local]
```

- `--claude` / `--codex` — override the configured agent for all AI roles
- `--raw` — output the inner Claude/Codex stream JSONL instead of the formatted terminal stream
- `--json-events` — output mrmouth lifecycle events as JSONL for supervisors; conflicts with `--raw`
- `--model` — override the agent model (default: `opus`)
- `--timeout` — kill container after N minutes
- `--local` — bind-mount current directory instead of cloning

### `mrmouth loop`

Run the agent repeatedly until work is done.

```bash
mrmouth loop [--claude|--codex] [--delay <seconds>] [--max-runs <n>] [--no-summary] [--model <model>] [--json-events]
```

Each iteration starts with an AI **decider** that reads SPEC.md and litebrite state, decomposes epics into tasks if needed, and returns `continue`, `ship`, or `stop`. On `continue` the **runner** agent executes inside Docker; afterward a **reviewer** agent checks the diff and files litebrite tasks for any issues. On `ship` a readiness check verifies builds and tests pass before merging and starting a new branch. The loop stops when the decider says `stop` or max iterations are reached.

### `mrmouth do <item-id>`

Work through a litebrite item — either an epic or a single task.

```bash
mrmouth do <item-id> [--claude|--codex] [--timeout <minutes>] [--max-failures <n>] [--model <model>] [--json-events]
```

Creates a feature branch and dispatches based on item type. For epics, loops through child tasks one at a time and runs a reviewer on the full diff at the end. For individual tasks, runs a single agent session focused on that task and then runs a reviewer. Aborts after N consecutive failures.

- `--json-events` — output mrmouth lifecycle events as JSONL and disable the TUI

### `mrmouth ready`

Pick up unblocked items from litebrite and work through them.

```bash
mrmouth ready [--claude|--codex] [--timeout <minutes>] [--max-failures <n>] [--model <model>] [--json-events]
```

Creates a timestamped feature branch, then loops: picks the highest-priority unblocked and unclaimed item from `lb ready`, runs a runner agent, runs a reviewer on the diff, and repeats until no ready items remain or max failures is reached.

### `mrmouth summary [log-file]`

Generate an AI summary of a run log.

```bash
mrmouth summary [path/to/log.jsonl]
```

### `mrmouth setup codex`

Configure Codex hooks so new Codex sessions load `mrmouth prime` context.

```bash
mrmouth setup codex
```

This writes `.codex/config.toml`, `.codex/hooks.json`, and
`.codex/rules/default.rules` idempotently.

### `mrmouth codex-login`

Sign in to Codex inside the shared persisted Docker volume used by `--codex`.
Use this when you want Codex CLI to use your ChatGPT Codex subscription instead
of passing `OPENAI_API_KEY` into the container.

```bash
mrmouth codex-login
mrmouth do <item-id> --codex
```

## Customization

Everything works out of the box. To customize, create files in `.mrmouth/` in your repo and commit them.

### `.mrmouth/config.toml`

All fields are optional — defaults shown:

```toml
model = "opus"
agent = "codex" # or "claude"; can be overridden with --claude/--codex
image = "mrmouth-runner"
dockerfile = ".mrmouth/Dockerfile"
# volume is optional; Claude defaults per repo, Codex defaults to shared mrmouth-codex-home
log_dir = "logs"
branch = "main"

[loop]
delay = 0
max_runs = 0
decider_model = "sonnet"
summary_model = "haiku"
reviewer_model = "sonnet"
shipper_model = "sonnet"

[epic]
timeout = 15
max_failures = 3
```

### `.mrmouth/Dockerfile`

The Docker image the agent runs in. mrmouth has a built-in default (Node 22 + Claude Code + Codex + litebrite `lb` + trapperkeeper `trk` + SSH). To add project-specific dependencies, create this file with your customizations and commit it.

The agent itself can create or edit this file during a run — changes are committed and rebuilt on the next run.

### `.mrmouth/prompt.md`

The prompt given to the agent. The built-in default implements the relay pattern: read tasks, claim one, do the work, commit, push, exit. Override this to change agent behavior.

## How It Works

**Relay pattern:** Each run is a fresh agent session. The agent reads task state and the spec, picks a task, does it, commits, pushes, and exits. The next run picks up where the last one left off. Each agent gets a full context window.

**Output streams:** Mr Mouth keeps its own orchestration events separate from the inner agent stream. Normal sessions use the TUI when stderr is a terminal; the inner agent JSONL is still written to `logs/*.jsonl`, and formatted human output is written to `logs/*.log`. `mrmouth run --raw` means stdout is the raw inner Claude/Codex JSONL stream, so it is useful for debugging the agent CLI protocol rather than supervising Mr Mouth itself. `--json-events` on `run`, `do`, `ready`, or `loop` instead reserves stdout for Mr Mouth lifecycle JSONL, disables the TUI, and reports lifecycle sink errors on stderr.

**Lifecycle JSON:** Lifecycle JSON events describe Mr Mouth actions such as stage changes, task selection, branch/container/run lifecycle, syncs, failures, and completion. They are not passthrough Claude/Codex events. Supervising tools should read stdout line by line and use the final `lifecycle_summary` event as the stable terminal record. Its `summary` object includes fields such as `status`, `command`, `item_id`, `branch`, `log_path`, `jsonl_path`, `exit_code`, `failure`, and `next_action` when those values are available. Treat earlier event shapes as progress reporting and avoid scraping TUI text, human log text, or raw inner-agent JSON for orchestration state.

**Container lifecycle:**
1. Host builds Docker image (from `.mrmouth/Dockerfile` or built-in default)
2. Container clones the repo fresh (or bind-mounts in `--local` mode)
3. The configured agent runs with the agent prompt
4. Agent reads spec, claims a task, implements, commits, pushes
5. Container exits; host pulls changes

**Self-modification:** The agent can create or edit `.mrmouth/Dockerfile` to add tools and dependencies. Changes are committed and rebuilt on the next run.

**Local mode:** `mrmouth run --local` bind-mounts the current directory. Works with repos that have no remote, or even directories that aren't git repos yet.

## Roadmap

- Allow per-role agent configuration so runner, decider, reviewer, summary, and shipper roles can mix Claude and Codex when that is useful. Today `agent = "codex"` switches all AI calls to Codex.

## License

MIT
