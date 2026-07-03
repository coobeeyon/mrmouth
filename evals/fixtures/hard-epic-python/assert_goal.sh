#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/goal-result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"

test -f "$report"
test -f "$worktree/supportops/cli.py"

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
assert "supportops/" in diffs, diffs
PY

(cd "$worktree" && ./check.sh)

python3 - "$worktree" <<'PY'
import json
import subprocess
import sys

worktree = sys.argv[1]
completed = subprocess.run(
    ["python3", "-m", "supportops.cli", "data/tickets.csv"],
    cwd=worktree,
    text=True,
    check=True,
    stdout=subprocess.PIPE,
)
report = json.loads(completed.stdout)
assert report["ticket_count"] == 8, report
assert report["open_count"] == 3, report
assert report["breach_totals"] == {"response": 4, "resolution": 3}, report
assert report["queues"]["Billing"]["ticket_count"] == 3, report
assert report["queues"]["Security"]["open_count"] == 1, report
assert report["customer_risk"][0]["customer"] == "Acme", report
assert report["customer_risk"][0]["risk_score"] == 12, report
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
