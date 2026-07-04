#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$fixture_dir/../../.." && pwd)"
mrmouth_bin="${MRMOUTH_BIN:-$repo_root/target/debug/mrmouth}"

if [ ! -x "$mrmouth_bin" ]; then
  (cd "$repo_root" && cargo build)
fi

bookkeeping_repo="$("$fixture_dir/prepare.sh")"
codex_home="$bookkeeping_repo/.codex-home"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"

CODEX_HOME="$codex_home" "$mrmouth_bin" eval \
  --cwd "$bookkeeping_repo" \
  --output "$fixture_dir/reports/batch-result.json" \
  -- "$mrmouth_bin" batch "$epic_id" \
    --json-events \
    --current-container \
    --worktree "$bookkeeping_repo/worktree" \
    --max-items "${MRMOUTH_BATCH_MAX_ITEMS:-10}" \
    --context-ceiling-percent "${MRMOUTH_BATCH_CONTEXT_CEILING_PERCENT:-65}" \
    --timeout "${MRMOUTH_BATCH_EVAL_TIMEOUT:-120}"
