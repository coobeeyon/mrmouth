#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$fixture_dir/../../.." && pwd)"
harness="${CODEX_GOAL_HARNESS:-$repo_root/evals/codex_goal_harness.mjs}"

bookkeeping_repo="$("$fixture_dir/prepare.sh")"
codex_home="$bookkeeping_repo/.codex-home"

CODEX_HOME="$codex_home" node "$harness" \
  --cwd "$bookkeeping_repo" \
  --goal-file "$bookkeeping_repo/.goal-objective.txt" \
  --prompt-file "$bookkeeping_repo/.goal-turn.txt" \
  --output "$fixture_dir/reports/goal-result.json" \
  --raw-events "$fixture_dir/reports/goal-events.jsonl" \
  --timeout-ms "${CODEX_GOAL_EVAL_TIMEOUT_MS:-1800000}"
