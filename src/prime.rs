use std::path::Path;

use crate::config::Config;

pub fn execute(config: &Config, repo_root: &Path) {
    print!("{}", text(config, repo_root));
}

fn text(config: &Config, repo_root: &Path) -> String {
    let repo = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("current repository");
    let agent = config.agent.as_str();
    let effective_model = config.effective_model_for_agent(&config.model);
    let model = if effective_model.is_empty() {
        "agent default"
    } else {
        effective_model.as_str()
    };
    let volume = config.effective_volume(repo_root);
    let branch = config.branch.as_deref().unwrap_or("current branch");

    format!(
        r#"# Mr Mouth Agent Context

Mr Mouth (`mrmouth`) runs Claude Code or Codex as autonomous coding agents inside Docker containers. Use it when the user wants bounded coding work delegated to an isolated agent, wants a litebrite item executed, or wants a supervising tool to monitor lifecycle JSON.

## Current Defaults

- repository: {repo}
- agent: {agent}
- model: {model}
- docker image: {image}
- dockerfile: {dockerfile}
- docker home volume: {volume}
- log directory: {log_dir}
- base branch: {branch}
- per-task timeout: {timeout} minutes
- max consecutive failures: {max_failures}

## Core Commands

- `mrmouth run` starts one fresh agent session. The runner reads project context, does one focused unit of work, commits, pushes, and exits.
- `mrmouth do <item-id>` works through one litebrite task, feature, or epic. This is the preferred bounded delegation command for supervising agents.
- `mrmouth ready` drains currently ready litebrite items until none are ready or failures exceed the configured limit. Use only when the user asked to process the ready queue.
- `mrmouth loop` runs the full autonomous loop with a decider, runner, reviewer, and shipper. Use only when the user asked for continuous autonomous operation.
- `mrmouth summary [log-file]` summarizes an agent JSONL log.
- `mrmouth setup codex` configures Codex hooks so new sessions load this context from `mrmouth prime`.
- `mrmouth codex-login` signs Codex into the persisted Docker home volume for `--codex` runs.

Global flags:

- `--claude` uses Claude Code for all AI roles.
- `--codex` uses Codex for all AI roles.

## Supervisor Output Contract

- `--json-events` on `run`, `do`, `ready`, or `loop` writes Mr Mouth lifecycle JSONL to stdout and disables the TUI.
- `--raw` on `run` writes the inner agent CLI JSONL stream instead. It is for debugging Claude/Codex protocol output, not for supervising Mr Mouth.
- Lifecycle JSON events describe Mr Mouth orchestration stages such as task selection, branch/container/run lifecycle, syncs, failures, and completion.
- The final `lifecycle_summary` event is the stable terminal record. Supervisors should prefer its `summary.status`, `summary.command`, `summary.item_id`, `summary.branch`, `summary.log_path`, `summary.jsonl_path`, `summary.exit_code`, `summary.failure`, and `summary.next_action` fields when present.
- Do not scrape TUI text, human logs, or raw inner-agent JSON for orchestration state.

## Recommended Agent Protocol

1. Use `lb prime` first when litebrite is installed; it provides the task-tracker protocol and ready/claimed state.
2. Use `lb ready` and `lb show <id>` to choose one executable item when the user has not specified an item.
3. Prefer `mrmouth do <id> --json-events` for bounded delegation.
4. Reserve `mrmouth ready --json-events` and `mrmouth loop --json-events` for explicit user requests to drain or continuously operate.
5. After a run, inspect the final lifecycle summary, verify the expected commit/task state, and run `lb sync` if litebrite state changed.
6. If a run fails, use the reported log paths and `next_action` before retrying.

## Safety Notes

- Mr Mouth may create branches, run Docker, invoke AI coding agents, commit code, push branches, and update litebrite state.
- The container has the repository, SSH agent access, configured AI credentials, and the persisted agent home volume.
- Agents can edit `.mrmouth/Dockerfile`; those changes are synced back and affect future runs.
- Keep delegation bounded unless the user explicitly asks for queue-draining or autonomous loop behavior.
"#,
        image = config.image,
        dockerfile = config.dockerfile,
        log_dir = config.log_dir,
        timeout = config.do_config.timeout,
        max_failures = config.do_config.max_failures,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentKind;

    #[test]
    fn prime_text_includes_agent_facing_contract() {
        let config = Config {
            agent: AgentKind::Codex,
            ..Config::default()
        };

        let output = text(&config, Path::new("/tmp/project"));

        assert!(output.contains("# Mr Mouth Agent Context"));
        assert!(output.contains("- agent: codex"));
        assert!(output.contains("- model: agent default"));
        assert!(output.contains("mrmouth do <id> --json-events"));
        assert!(output.contains("mrmouth setup codex"));
        assert!(output.contains("lifecycle_summary"));
        assert!(output.contains("lb prime"));
    }
}
