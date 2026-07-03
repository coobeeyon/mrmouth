#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/result.json"
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

assert report["success"] is True, report
summary = report["lifecycle"]["final_summary"]
assert summary["status"] == "success", summary
assert summary["command"] == "do", summary
assert summary["item_id"] == item_id, summary
phases = {marker["phase"] for marker in report["lifecycle"]["timing_markers"]}
assert "current-container-wall" in phases, phases
token_usage = report["lifecycle"]["token_usage"]
assert token_usage["turn_count"] >= 1, token_usage
assert token_usage["input_tokens"] > 0, token_usage
assert token_usage["total_uncached_tokens"] >= token_usage["output_tokens"], token_usage
PY

grep -qx "hello from smol eval" "$worktree/message.txt"
(cd "$worktree" && ./check.sh)
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
