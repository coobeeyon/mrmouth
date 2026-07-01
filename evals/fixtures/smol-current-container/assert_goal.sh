#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/goal-result.json"
bookkeeping_repo="$fixture_dir/repo"
worktree="$bookkeeping_repo/worktree"
item_id="$(cat "$bookkeeping_repo/.eval-item-id")"

test -f "$report"
test -f "$worktree/message.txt"

python3 - "$report" "$item_id" <<'PY'
import json
import sys

report_path, item_id = sys.argv[1], sys.argv[2]
with open(report_path, encoding="utf-8") as f:
    report = json.load(f)

assert report["harness"] == "codex-goal-app-server", report
assert report["success"] is True, report
goal = report["goal"]["final"]
assert goal["status"] == "complete", goal
assert item_id in goal["objective"], goal
assert report["thread_id"], report
assert report["turn_id"], report
assert report["wall_ms"] > 0, report
PY

grep -qx "hello from smol eval" "$worktree/message.txt"
(cd "$worktree" && ./check.sh)
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
