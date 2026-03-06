# Mr Mouth

Run Claude Code as an autonomous coding agent inside Docker containers.

You run `mrmouth` from inside any git repo. It builds a Docker image, launches a container with the repo mounted or cloned, runs Claude Code with a structured prompt, streams formatted output, and pulls changes when the agent exits. Config lives in the target repo under `.mrmouth/`.

## Install

```bash
cargo install mrmouth
```

Or download a prebuilt binary from [GitHub Releases](https://github.com/coobeeyon/mrmouth/releases).

## Prerequisites

- **Docker** — container runtime for agent isolation
- **SSH agent** — running with keys that have access to your repo (`ssh-add`)
- **Anthropic API key** or Claude Code OAuth token

Optional:
- **lb** ([litebrite](https://github.com/coobeeyon/litebrite)) — task tracking CLI for the relay pattern

## Quick Start

```bash
# Initialize config in your repo
cd my-project
mrmouth init

# Add your API key
echo 'ANTHROPIC_API_KEY=sk-...' > .env

# Run one agent session
mrmouth run
```

## Commands

### `mrmouth run`

Run one agent session. This is the core command.

```bash
mrmouth run [--raw] [--model <model>] [--timeout <minutes>] [--local]
```

- `--raw` — output raw JSONL instead of formatted stream
- `--model` — override the Claude model (default: from config or `opus`)
- `--timeout` — kill container after N minutes
- `--local` — bind-mount current directory instead of cloning

### `mrmouth loop`

Run the agent repeatedly until work is done.

```bash
mrmouth loop [--delay <seconds>] [--max-runs <n>] [--no-summary]
```

After each run, an AI decider checks whether open work remains. The loop stops when the decider says done or max iterations are reached.

### `mrmouth epic <epic-id>`

Work through a litebrite epic's tasks sequentially.

```bash
mrmouth epic <epic-id> [--timeout <minutes>] [--max-failures <n>]
```

Creates a feature branch and works through each child task. Aborts after N consecutive failures.

### `mrmouth init`

Scaffold `.mrmouth/` config in the current repo.

```bash
mrmouth init
```

Creates `config.toml`, `Dockerfile`, and `prompt.md` in `.mrmouth/` with documented defaults.

### `mrmouth summary [log-file]`

Generate an AI summary of a run log.

```bash
mrmouth summary [path/to/log.jsonl]
```

## Configuration

Config file: `.mrmouth/config.toml` — all fields are optional.

```toml
model = "opus"
image = "mrmouth-runner"
dockerfile = ".mrmouth/Dockerfile"
volume = "mrmouth-claude-home"
log_dir = "logs"
env_file = ".env"

[loop]
delay = 0
max_runs = 0
decider_model = "sonnet"
summary_model = "haiku"

[epic]
timeout = 15
max_failures = 3
```

## How It Works

**Relay pattern:** Each run is a fresh agent session. The agent reads task state and the spec, picks a task, does it, commits, pushes, and exits. The next run picks up where the last one left off. Each agent gets a full context window.

**Container lifecycle:**
1. Host builds Docker image from `.mrmouth/Dockerfile`
2. Container clones the repo fresh (or bind-mounts in `--local` mode)
3. Claude Code runs with `--dangerously-skip-permissions` and a structured prompt
4. Agent reads spec, claims a task, implements, commits, pushes
5. Container exits; host pulls changes

**Self-modification:** The agent can edit `.mrmouth/Dockerfile` to add tools and dependencies. Changes are committed and rebuilt on the next run.

**Local/bootstrap mode:** `mrmouth run --local` bind-mounts the current directory. Works with repos that have no remote, or even directories that aren't git repos yet.

## License

MIT
