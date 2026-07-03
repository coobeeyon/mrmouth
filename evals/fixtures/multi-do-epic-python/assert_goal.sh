#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/goal-result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"

test -f "$report"
test -f "$worktree/inventory/cli.py"

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
assert report["thread_id"], report
assert report["turn_id"], report
assert report["wall_ms"] > 0, report
tokens = report["token_usage"]
assert tokens["final_goal_tokens_used"] > 0, tokens
assert tokens["latest_update_normalized"]["input_tokens"] > 0, tokens
assert tokens["latest_update_normalized"]["cached_input_tokens"] >= 0, tokens
assert tokens["comparable_total_tokens"] > 0, tokens
assert tokens["comparable_total_uncached_tokens"] > 0, tokens
event_counts = report["app_server"]["event_counts"]
assert event_counts["hook/started"] == 2, event_counts
assert event_counts["hook/completed"] == 2, event_counts
assert event_counts["thread/tokenUsage/updated"] >= 1, event_counts
commands = report["evidence"]["command_executions"]
diffs = "\n".join(report["evidence"]["diffs"])
assert "inventory/" in diffs, diffs
assert any(command["exit_code"] == 0 and "./check.sh" in command["command"] for command in commands), commands
assert any(command["exit_code"] == 0 and "git commit" in command["command"] for command in commands), commands
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
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

while IFS= read -r item_id; do
  lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
  printf '%s\n' "$lb_show" | grep -q "Status: closed"
done < "$bookkeeping_repo/.eval-leaf-ids"

lb_show="$(cd "$bookkeeping_repo" && lb show "$epic_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
