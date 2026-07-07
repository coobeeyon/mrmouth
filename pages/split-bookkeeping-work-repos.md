# Split Bookkeeping/Work Repos

Concepts: fake monorepo, bookkeeping repo, work repo, split repo layout, `work_repo`, `--worktree`, `/home/runner/worktree`
Key files: `src/repo_layout.rs`, `src/config.rs`, `src/main.rs`, `src/run.rs`, `src/do_cmd.rs`, `src/ready.rs`, `src/loop_cmd.rs`, `src/docker.rs`, `src/prime.rs`
Useful when: changing how mrmouth mounts repositories into Docker, explaining fake-monorepo support, adjusting agent prompts for task state vs code edits, or debugging `work_repo`/`--worktree` behavior.

Mr Mouth models every run with two paths:

- **bookkeeping repo**: the repo mrmouth was launched from, containing `.mrmouth/`, Litebrite (`lb`) state, and Trapperkeeper (`trk`) state
- **work repo**: the repo where product code edits, code commits, and code pushes should happen

By default both paths are the same. `.mrmouth/config.toml` can set
`work_repo = "relative/or/absolute/path"`; relative config paths resolve from
the bookkeeping repo. The CLI `--worktree <path>` remains as a per-invocation
override and is resolved relative to the caller's current directory for backward
compatibility.

`src/repo_layout.rs` owns canonicalization and split detection. It returns a
`RepoLayout` with canonical bookkeeping and work paths. If the canonical paths
are equal, no split mount is used. If they differ, Docker receives the work repo
as the `/home/runner/worktree` bind mount.

Container conventions:

- bookkeeping repo is always `/home/runner/workspace`
- split work repo is `/home/runner/worktree`
- `MRMOUTH_BOOKKEEPING_REPO` is always set
- `MRMOUTH_WORK_REPO` points to `/home/runner/workspace` for same-repo runs and `/home/runner/worktree` for split runs
- `MRMOUTH_WORKTREE` is still set for split runs as a compatibility alias

Local/file remote conventions:

- If the bookkeeping repo has no `origin`, Docker clone mode uses the host repo
  itself as `file:///host-repo` and configures `receive.denyCurrentBranch =
  updateInstead`; host pull is skipped because pushes update the mounted host
  checkout directly.
- If `origin` is a host-local path or `file://` URL, Docker clone mode mounts
  the canonical target at `/host-repo` and clones from `file:///host-repo`.
  Host pull still runs afterward because the local origin may be a separate bare
  remote.
- If a bind-mounted bookkeeping repo or split worktree has a host-local origin,
  the runner mounts that origin and configures in-container global
  `url.<container-file-url>.insteadOf` rewrites. This makes `git push` use a
  container-visible file URL without rewriting the host repo's `.git/config`.
- Runner cleanup pushes both `/home/runner/workspace` and, when distinct,
  `/home/runner/worktree`. A cleanup push failure emits
  `::mrmouth::push-error` and exits nonzero so lifecycle summaries show a
  structured failure instead of silently continuing.

Prompt conventions:

- Plain `run`, `ready`, and `loop` use the shared `RepoLayout` so default runner prompts get a repository-layout block when split.
- `do` builds item-specific prompts and includes equivalent split guidance there.
- Current-container mode uses host paths in the prompt and environment instead of container paths.

Command flow:

- `src/main.rs` resolves `RepoLayout` for `run` and `do`, passes only distinct work repos as `worktree_path`, and removes the old Clap requirement that `--current-container` must have a literal `--worktree` flag. Runtime preflight still rejects current-container mode unless the resolved layout is split.
- `src/ready.rs` and `src/loop_cmd.rs` resolve `RepoLayout` from config and pass it into runner options/session setup.
- `src/docker.rs` mounts the bookkeeping repo at `/home/runner/workspace` in local/file-remote modes and mounts the distinct work repo at `/home/runner/worktree`.
- `src/run.rs` injects default prompt guidance only when using the default prompt, avoiding duplicate layout blocks for `do`/`ready` prompt overrides.
- `src/do_cmd.rs`, `src/ready.rs`, and `src/loop_cmd.rs` calculate reviewer commit ranges from the resolved work repo when split, then pass the worktree mount into `reviewer::ReviewerOptions`.

Reviewer conventions:

- Reviewer containers still clone/use the bookkeeping repo at `/home/runner/workspace` so `lb` and `trk` can inspect and update task state.
- When a distinct work repo exists, reviewer containers mount it at `/home/runner/worktree`, mark it as a safe Git directory, and prompt the reviewer to run git diff/log plus build/test commands there.
- Split-repo agents are still responsible for committing and pushing code in the work repo themselves.
