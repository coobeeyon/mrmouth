#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"

test -f "$report"
test -f "$worktree/inventory/cli.py"

python3 - "$report" "$bookkeeping_repo" <<'PY'
import json
import sys

report_path, bookkeeping_repo = sys.argv[1], sys.argv[2]
with open(report_path, encoding="utf-8") as f:
    report = json.load(f)

assert report["harness"] == "mrmouth-multi-do", report
assert report["success"] is True, report
assert report["closed_epic_by_harness"] is True, report
assert len(report["children"]) == 4, report
assert report["wall_ms"] > 0, report
assert report["summed_child_wall_ms"] > 0, report
tokens = report["token_usage"]
assert tokens["turn_count"] >= 4, tokens
assert tokens["input_tokens"] > 0, tokens
assert tokens["cached_input_tokens"] >= 0, tokens
assert tokens["total_uncached_tokens"] >= tokens["output_tokens"], tokens
for child in report["children"]:
    assert child["success"] is True, child
    summary = child["summary"]
    assert summary["status"] == "success", summary
    assert summary["command"] == "do", summary
    assert summary["item_id"] == child["item_id"], summary
PY

(cd "$worktree" && ./check.sh)

python3 - "$worktree" <<'PY'
import json
import subprocess
import sys

worktree = sys.argv[1]
completed = subprocess.run(
    [
        "python3",
        "-m",
        "inventory.cli",
        "data/catalog.csv",
        "data/movements.csv",
    ],
    cwd=worktree,
    text=True,
    check=True,
    stdout=subprocess.PIPE,
)
report = json.loads(completed.stdout)
assert report["item_count"] == 4, report
assert report["total_on_hand"] == 34, report
assert report["movement_summary"]["units_sold"] == 6, report
assert report["movement_summary"]["units_received"] == 10, report
assert report["movement_summary"]["adjustments"] == -8, report
assert report["reorder"]["total_cost"] == 72.5, report
assert [item["sku"] for item in report["reorder"]["items"]] == ["C100", "B100"], report
assert report["categories"]["Beverages"]["on_hand"] == 30, report
PY

root_commit="$(git -C "$worktree" rev-list --max-parents=0 HEAD)"
test -z "$(git -C "$worktree" diff --name-only "$root_commit"..HEAD -- tests data)"
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 5
test -z "$(git -C "$worktree" status --short)"

while IFS= read -r item_id; do
  lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
  printf '%s\n' "$lb_show" | grep -q "Status: closed"
done < "$bookkeeping_repo/.eval-leaf-ids"

lb_show="$(cd "$bookkeeping_repo" && lb show "$epic_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
