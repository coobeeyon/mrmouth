#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/goal-result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"

test -f "$report"
test -f "$worktree/fulfillops/cli.py"

python3 - "$report" "$epic_id" <<'PY'
import json
import sys

report_path, epic_id = sys.argv[1], sys.argv[2]
with open(report_path, encoding="utf-8") as f:
    report = json.load(f)

assert report["harness"] == "codex-goal-app-server", report
assert report["success"] is True, report
goal = report["goal"]["final"]
assert goal["status"] == "complete", goal
assert epic_id in goal["objective"], goal
tokens = report["token_usage"]
assert tokens["comparable_total_uncached_tokens"] > 0, tokens
assert tokens["latest_update_normalized"]["input_tokens"] > 0, tokens
event_counts = report["app_server"]["event_counts"]
assert event_counts["hook/started"] == 2, event_counts
assert event_counts["hook/completed"] == 2, event_counts
assert event_counts["thread/tokenUsage/updated"] >= 1, event_counts
diffs = "\n".join(report["evidence"]["diffs"])
assert "fulfillops/" in diffs, diffs
PY

(cd "$worktree" && ./check.sh)

python3 - "$worktree" <<'PY'
import json
import subprocess
import sys

worktree = sys.argv[1]
completed = subprocess.run(
    ["python3", "-m", "fulfillops.cli", "data/products.csv", "data/orders.csv"],
    cwd=worktree,
    text=True,
    check=True,
    stdout=subprocess.PIPE,
)
report = json.loads(completed.stdout)
assert report["order_count"] == 7, report
assert report["line_count"] == 8, report
assert report["fully_allocated_orders"] == 3, report
assert report["backordered_units"] == 4, report
assert len(report["shipments"]) == 6, report
assert report["carrier_totals"]["AirSafe"] == 33.0, report
assert report["carrier_totals"]["Ground"] == 16.48, report
assert report["backorders"][0] == {
    "sku": "SKU-2",
    "total_backordered": 2,
    "affected_orders": ["O-100", "O-104"],
}, report
assert report["risk"][0]["order_id"] == "O-100", report
assert report["risk"][0]["risk_score"] == 11, report
PY

root_commit="$(git -C "$worktree" rev-list --max-parents=0 HEAD)"
test -z "$(git -C "$worktree" diff --name-only "$root_commit"..HEAD -- tests data)"
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

while IFS= read -r item_id; do
  lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
  printf '%s\n' "$lb_show" | grep -q "Status: closed"
done < "$bookkeeping_repo/.eval-leaf-ids"

lb_show="$(cd "$bookkeeping_repo" && lb show "$epic_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
