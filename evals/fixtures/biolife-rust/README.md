# Biolife Rust Fixture

This fixture is a Rust game/simulation eval built around a biologically
inspired creature system.

The generated worktree is a Rust workspace:

- `crates/biolife_core`: deterministic offline simulation backend
- `crates/biolife_app`: thin CLI/front-end boundary using the core API

The task is intentionally not a text-processing pipeline. Agents must reason
about architecture, graph-shaped organisms, chromosome-driven development,
energy flows, combat/defense, and a small viscous-fluid locomotion model.

Run the Goal path:

```sh
./evals/fixtures/biolife-rust/run_goal.sh
./evals/fixtures/biolife-rust/assert_goal.sh
```

Run the Mr Mouth batch path:

```sh
./evals/fixtures/biolife-rust/run_batch.sh
./evals/fixtures/biolife-rust/assert_batch.sh
```

Each run rebuilds `repo/`, `remotes/`, and `reports/` from the deterministic
generator.
