#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/batch-result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"

test -f "$report"
test -f "$worktree/crates/biolife_core/src/lib.rs"
test -f "$worktree/crates/biolife_app/src/main.rs"

python3 - "$report" "$epic_id" <<'PY'
import json
import sys

report_path, epic_id = sys.argv[1], sys.argv[2]
with open(report_path, encoding="utf-8") as f:
    report = json.load(f)

assert report["success"] is True, report
assert report["exit_code"] == 0, report
summary = report["lifecycle"]["final_summary"]
assert summary["status"] == "success", summary
assert summary["command"] == "batch", summary
assert summary["item_id"] == epic_id, summary
PY

(cd "$worktree" && ./check.sh)

root_commit="$(git -C "$worktree" rev-list --max-parents=0 HEAD)"
test -z "$(git -C "$worktree" diff --name-only "$root_commit"..HEAD -- crates/biolife_core/tests crates/biolife_app/tests)"
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

while IFS= read -r item_id; do
  lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
  printf '%s\n' "$lb_show" | grep -q "Status: closed"
done < "$bookkeeping_repo/.eval-leaf-ids"

lb_show="$(cd "$bookkeeping_repo" && lb show "$epic_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
