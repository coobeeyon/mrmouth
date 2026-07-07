# Runner Context Hygiene

Concepts: runner prompt context hygiene, generated file context, agent/plugin caches, source-oriented repo exploration
Key files: `src/prompt.rs`
Useful when: changing the default runner prompt, reducing agent token waste, or deciding which generated/cache paths should not be treated as source context

The default runner prompt in `src/prompt.rs` owns context-hygiene guidance for
normal `run`, `do`, `ready`, and `batch` executions that use the built-in
prompt. Custom `.mrmouth/prompt.md` overrides still replace the default prompt
verbatim.

The guidance tells runner agents to treat generated files, build outputs, logs,
preserved eval artifacts, and agent home/plugin caches as non-source context
unless the task explicitly asks for them. The named examples include
`.codex-home/`, `.claude/`, `.tmp/`, `.tmp/plugins/`, `logs/`, `target/`,
`node_modules/`, Python caches, `tmp/`, `preserved/`, and generated eval fixture
outputs under `evals/fixtures/*/{repo,reports,remotes}/`.

The prompt steers agents toward `git status`, `git diff`, `git ls-files`, and
targeted `rg` searches. If a runner creates its own file inventory, it should
use tracked files or an explicit ignore filter instead of recursively listing
the full checkout. Tests in `src/prompt.rs` lock the key path names into the
embedded default prompt and verify that custom prompt overrides remain
unchanged.
