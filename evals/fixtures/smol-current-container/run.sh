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

CODEX_HOME="$codex_home" "$mrmouth_bin" eval \
  --cwd "$fixture_dir/repo" \
  --output "$fixture_dir/reports/result.json" \
  -- "$mrmouth_bin" do "$(cat "$fixture_dir/repo/.eval-item-id")" \
    --json-events \
    --current-container \
    --worktree "$fixture_dir/repo/worktree" \
    --timeout "${MRMOUTH_EVAL_TIMEOUT:-10}" \
    --max-failures 1
