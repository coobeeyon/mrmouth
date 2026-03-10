# Mr Mouth

Run Claude Code as an autonomous coding agent inside Docker containers.

You run `mrmouth` from inside any git repo. It builds a Docker image, launches a container with the repo cloned, runs Claude Code with a structured prompt, streams formatted output, and pulls changes when the agent exits.

## Install

```bash
cargo install --git https://github.com/coobeeyon/mrmouth.git
```

## Prerequisites

- **Docker** — container runtime for agent isolation
- **SSH agent** — running with keys that have access to your repo (`ssh-add`)
- **Credentials** — set `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` in your environment

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

### `mrmouth run`

Run one agent session.

```bash
mrmouth run [--raw] [--model <model>] [--timeout <minutes>] [--local]
```

- `--raw` — output raw JSONL instead of formatted stream
- `--model` — override the Claude model (default: `opus`)
- `--timeout` — kill container after N minutes
- `--local` — bind-mount current directory instead of cloning

### `mrmouth loop`

Run the agent repeatedly until work is done.

```bash
mrmouth loop [--delay <seconds>] [--max-runs <n>] [--no-summary] [--model <model>]
```

After each run: a reviewer agent (inside Docker) checks the code and files litebrite tasks for any issues found; an AI decider decides whether to continue, ship, or stop. When the decider says "ship", a readiness check (inside Docker) verifies builds and tests pass before merging and starting a new branch. The loop stops when the decider says done or max iterations are reached.

### `mrmouth epic <epic-id>`

Work through a litebrite epic's tasks sequentially.

```bash
mrmouth epic <epic-id> [--timeout <minutes>] [--max-failures <n>] [--model <model>]
```

Creates a feature branch and works through each child task. Aborts after N consecutive failures.

### `mrmouth summary [log-file]`

Generate an AI summary of a run log.

```bash
mrmouth summary [path/to/log.jsonl]
```

## Customization

Everything works out of the box. To customize, create files in `.mrmouth/` in your repo and commit them.

### `.mrmouth/config.toml`

All fields are optional — defaults shown:

```toml
model = "opus"
image = "mrmouth-runner"
dockerfile = ".mrmouth/Dockerfile"
volume = "mrmouth-claude-home"
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

The Docker image the agent runs in. mrmouth has a built-in default (Node 22 + Claude Code + litebrite + SSH). To add project-specific dependencies, create this file with your customizations and commit it.

The agent itself can create or edit this file during a run — changes are committed and rebuilt on the next run.

### `.mrmouth/prompt.md`

The prompt given to Claude Code. The built-in default implements the relay pattern: read tasks, claim one, do the work, commit, push, exit. Override this to change agent behavior.

## How It Works

**Relay pattern:** Each run is a fresh agent session. The agent reads task state and the spec, picks a task, does it, commits, pushes, and exits. The next run picks up where the last one left off. Each agent gets a full context window.

**Container lifecycle:**
1. Host builds Docker image (from `.mrmouth/Dockerfile` or built-in default)
2. Container clones the repo fresh (or bind-mounts in `--local` mode)
3. Claude Code runs with `--dangerously-skip-permissions` and the agent prompt
4. Agent reads spec, claims a task, implements, commits, pushes
5. Container exits; host pulls changes

**Self-modification:** The agent can create or edit `.mrmouth/Dockerfile` to add tools and dependencies. Changes are committed and rebuilt on the next run.

**Local mode:** `mrmouth run --local` bind-mounts the current directory. Works with repos that have no remote, or even directories that aren't git repos yet.

## License

MIT
