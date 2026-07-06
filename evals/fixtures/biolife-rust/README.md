# Biolife Rust Fixture

This fixture is a Rust game/simulation eval built around a biologically
inspired creature system.

The generated worktree is a Rust workspace:

- `crates/biolife_core`: deterministic offline simulation backend
- `crates/biolife_app`: front-end boundary using the core API, with a required
  live animated HTML/SVG GUI export

The task is intentionally not a text-processing pipeline. Agents must reason
about architecture, graph-shaped organisms, chromosome-driven development,
energy flows, combat/defense, and a small viscous-fluid locomotion model.
The frontend must show organisms moving and evolving in real time, expose
controls for simulation parameters and playback, support visual inspection of
organisms/segments, and keep simulation rules in `biolife_core`.

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

## Observed Baseline

Initial clean runs on 2026-07-06:

| Harness | Wall time | Comparable uncached tokens | Commit shape |
| --- | ---: | ---: | --- |
| Codex Goal | 359,959 ms | 123,707 | 1 combined implementation commit |
| `mrmouth batch` | 670,820 ms | 233,213 | 10 focused implementation commits |

Both passed deterministic assertions, ran `cargo test --workspace`, left tests
unchanged, closed all child tasks plus the parent epic, and left generated
worktrees clean. Goal still completed in one turn without compaction; its max
per-step live input was about 64,911 tokens against a reported 258,400-token
context window.
