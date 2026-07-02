#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
item_id="$(cat "$bookkeeping_repo/.eval-item-id")"

test -f "$report"
test -f "$worktree/sales_report.py"

python3 - "$report" "$item_id" <<'PY'
import json
import sys

report_path, item_id = sys.argv[1], sys.argv[2]
with open(report_path, encoding="utf-8") as f:
    report = json.load(f)

assert report["success"] is True, report
assert report["exit_code"] == 0, report
summary = report["lifecycle"]["final_summary"]
assert summary["status"] == "success", summary
assert summary["command"] == "do", summary
assert summary["item_id"] == item_id, summary
markers = report["lifecycle"]["timing_markers"]
assert any(marker["phase"] == "current-container-wall" for marker in markers), markers
PY

(cd "$worktree" && ./check.sh)
python3 - "$worktree" <<'PY'
import json
import subprocess
import sys

worktree = sys.argv[1]
completed = subprocess.run(
    ["python3", "sales_report.py", "data/orders.csv"],
    cwd=worktree,
    text=True,
    check=True,
    stdout=subprocess.PIPE,
)
summary = json.loads(completed.stdout)
assert summary["net_revenue"] == 109.0, summary
assert summary["refund_count"] == 1, summary
assert summary["top_category"] == "Books", summary
assert summary["regions"]["North"]["net_revenue"] == 83.0, summary
assert summary["categories"]["Kitchen"]["units"] == 6, summary
PY

test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
