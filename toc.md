# Table of Contents
<!-- Hierarchical navigation. Group pages under `## <Section>` headers; sort sections and entries alphabetically. -->

## Concepts
- [Agent Credential Preflight](pages/agent-credential-preflight.md) — Agent-aware host credential checks for Claude and Codex modes.
- [CI Check Commands](pages/ci-check-commands.md) — Local Rust gates and narrow Clippy exceptions used to reproduce CI failures.
- [Codex Role Model Defaults](pages/codex-role-model-defaults.md) — Normalizing Claude role-model aliases before launching Codex reviewers and loop roles.
- [Dockerfile Extraction Lifecycle](pages/dockerfile-extraction-lifecycle.md) — Host sync, auto-commit, and success-only extraction for container-edited Dockerfiles.
- [Event Rendering Architecture](pages/event-rendering-architecture.md) — Separation of core lifecycle events from TUI, human, and JSON renderers, plus review notes on output-mode coupling.
- [Litebrite Agent Contract](pages/litebrite-agent-contract.md) — Task-tracker contract mrmouth and supervising agents rely on.
- [Mr Mouth Prime](pages/mrmouth-prime.md) — AI-facing command context for supervising mrmouth safely.
- [Mr Mouth Speed And Evals](pages/mrmouth-speed-and-evals.md) — Runtime bottlenecks, Codex session options, and eval harness direction.
- [Reviewer Fitness For Purpose](pages/reviewer-fitness-for-purpose.md) — Reviewer prompt context for requested Litebrite items and split worktree diff scope.
- [Runner Context Hygiene](pages/runner-context-hygiene.md) — Default runner prompt guidance for avoiding generated files, eval artifacts, logs, and agent/plugin caches as source context.
- [Split Bookkeeping/Work Repos](pages/split-bookkeeping-work-repos.md) — Configured fake-monorepo layout that separates task bookkeeping from the code repo agents edit.
