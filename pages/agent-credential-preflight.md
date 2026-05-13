# Agent Credential Preflight

`src/run.rs` owns host-side preflight checks for both single-run and long-lived session startup. The credential portion must be keyed by `AgentKind`, because `--codex` and `agent = "codex"` should not require Claude credentials.

Claude preflight is allowed to fail early when neither `ANTHROPIC_API_KEY` nor `CLAUDE_CODE_OAUTH_TOKEN` is present. That mirrors Claude Code's host-passed credential model and gives a clear error before Docker build/run work starts.

Codex preflight deliberately does not require Claude env vars, and it also should not hard-require an OpenAI env var on the host. Codex can authenticate through `OPENAI_API_KEY` or `CODEX_API_KEY`, but it can also use device auth stored in the persisted Docker home volume created by `mrmouth setup codex`. Host preflight cannot reliably inspect that volume without producing false negatives, so Codex auth failures are left to the Codex CLI inside the container. `mrmouth codex-login` remains a legacy alias for the same setup flow.

The regression tests live in `src/run.rs` near the other preflight tests:

- `claude_credentials_require_claude_env`
- `codex_credentials_do_not_require_claude_env`

When changing auth behavior, keep both run paths wired through the same check:

- `execute(...)` calls `preflight(repo_root, config.agent, ...)`
- `start_session(...)` calls `preflight(repo_root, config.agent, ...)`
