# Long Epic Python Fixture

This fixture compares `mrmouth batch` with the Codex Goal harness on a longer
ordered epic. The worktree contains an incomplete `fulfillops` Python package
for fulfillment reporting.

The generated Litebrite graph has ten dependent leaves:

1. product and order loading
2. catalog enrichment
3. stock allocation
4. shipment grouping
5. carrier quoting
6. invoice totals
7. backorder planning
8. risk scoring
9. summary metrics
10. CLI reporting

Run the Mr Mouth batch path:

```sh
./evals/fixtures/long-epic-python/run_batch.sh
./evals/fixtures/long-epic-python/assert_batch.sh
```

Run the Codex Goal path:

```sh
./evals/fixtures/long-epic-python/run_goal.sh
./evals/fixtures/long-epic-python/assert_goal.sh
```

Each run rebuilds `repo/`, `remotes/`, and `reports/` from `seed/`.

## Observed Baseline

Initial clean runs on 2026-07-03:

| Harness | Wall time | Comparable uncached tokens | Commit shape |
| --- | ---: | ---: | --- |
| `mrmouth batch` | 529,397 ms | 179,688 | 10 focused implementation commits |
| Codex Goal | 247,101 ms | 84,566 | 1 combined implementation commit |

Both passed deterministic assertions, closed all child tasks plus the parent
epic, left tests/data unchanged, and left the generated worktree clean.
