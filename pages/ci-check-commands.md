# CI Check Commands

Concepts: CI gates, cargo fmt, cargo clippy, cargo test, Clippy lint exceptions.
Key files: `Cargo.toml`, `src/do_cmd.rs`, `src/events.rs`, `src/loop_cmd.rs`, `src/run.rs`.
Commands: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
Useful when: CI is broken, local checks need reproducing, or Clippy starts failing on orchestration helpers.

This repo does not currently carry GitHub workflow files in the checkout, but the local CI-equivalent Rust gates are:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo clippy --all-targets -- -D warnings` is the strict gate most likely to fail after compiler or Clippy updates. The orchestration layer intentionally has a few narrow `#[allow(clippy::too_many_arguments)]` annotations on boundary helpers that thread shared run context through Docker/session/task flows:

- `src/do_cmd.rs`: task/epic execution and Dockerfile-change session restart helpers.
- `src/loop_cmd.rs`: Dockerfile-change session restart helper.
- `src/run.rs`: `start_session` and run-options-to-container-args adapter.

`src/events.rs` has `#[allow(clippy::large_enum_variant)]` on `MrmouthEvent`. The enum is the stable serialized lifecycle event surface; boxing the larger lifecycle summary variant would only optimize enum size while adding type churn at a public event boundary.
