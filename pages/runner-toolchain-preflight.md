# Runner Toolchain Preflight

Concepts: runner toolchain diagnostics, Rust preflight, Cargo.lock format, rustfmt, current-container tools, Docker runner image tools
Key files: `src/run.rs`, `src/docker.rs`, `.mrmouth/Dockerfile`
Useful when: changing startup checks before agent execution, debugging Rust eval failures, or adjusting Docker/current-container runner toolchains.

Mr Mouth performs Rust project toolchain diagnostics when the work repo contains
`Cargo.toml`. The project root is the resolved work repo, not necessarily the
bookkeeping repo, so split `work_repo` and `--worktree` runs inspect the code
checkout.

Current-container mode checks before starting the agent CLI:

- `cargo` must exist on `PATH`
- `rustfmt` must exist on `PATH`
- `cargo --version` must be parseable
- `Cargo.lock` format version 4 requires Cargo >= 1.78
- future lockfile formats greater than 4 fail with upgrade guidance

Docker mode checks after `docker build` and before volume setup/container
startup. This is intentionally post-build because it probes the actual configured
runner image instead of guessing from Dockerfile text. The probe runs
`cargo --version`, verifies `rustfmt`, and applies the same `Cargo.lock`
compatibility rules. Failures are `RunError::Preflight`, so they surface as
early runner failures with lifecycle summaries before any expensive agent turn.

The built-in fallback Dockerfile in `src/docker.rs::DEFAULT_DOCKERFILE` copies
the Rust toolchain from the `rust:slim` tools builder into the runtime image and
adds `rustfmt`, matching the project `.mrmouth/Dockerfile` pattern. This avoids
Debian distro Cargo lag, such as Cargo 1.63 failing on lockfile format 4.

Escape hatch: `MRMOUTH_SKIP_PREFLIGHT=1` bypasses these diagnostics along with
the other runner preflight checks.
