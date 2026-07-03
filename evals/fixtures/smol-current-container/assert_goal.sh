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
token_usage = report["token_usage"]
assert token_usage["final_goal_tokens_used"] > 0, token_usage
assert token_usage["latest_update_normalized"]["input_tokens"] > 0, token_usage
assert token_usage["latest_update_normalized"]["cached_input_tokens"] >= 0, token_usage
assert token_usage["comparable_total_tokens"] > 0, token_usage
assert token_usage["comparable_total_uncached_tokens"] > 0, token_usage
event_counts = report["app_server"]["event_counts"]
assert event_counts["hook/started"] == 2, event_counts
assert event_counts["hook/completed"] == 2, event_counts
assert event_counts["thread/tokenUsage/updated"] >= 1, event_counts

evidence = report["evidence"]
commands = evidence["command_executions"]
diffs = "\n".join(evidence["diffs"])

def completed(command_fragment, output_fragment=None, cwd_suffix=None):
    for command in commands:
        if command["exit_code"] != 0:
            continue
        if command_fragment not in command["command"]:
            continue
        if cwd_suffix and not command["cwd"].endswith(cwd_suffix):
            continue
        if output_fragment is not None and output_fragment not in (command["output_excerpt"] or ""):
            continue
        return True
    return False

assert any(
    command["exit_code"] == 0
    and command["cwd"].endswith("worktree")
    and "message.txt" in command["command"]
    and "before" in (command["output_excerpt"] or "")
    for command in commands
), commands
assert "-before\n+hello from smol eval" in diffs, diffs
assert completed("./check.sh", None, "worktree"), commands
assert completed("git commit", "1 file changed", "worktree"), commands
assert completed("lb close", f"closed {item_id}", "."), commands
assert completed("lb show", "Status: closed", "."), commands
PY

grep -qx "hello from smol eval" "$worktree/message.txt"
(cd "$worktree" && ./check.sh)
test "$(git -C "$worktree" rev-list --count HEAD)" -ge 2
test -z "$(git -C "$worktree" status --short)"

lb_show="$(cd "$bookkeeping_repo" && lb show "$item_id")"
printf '%s\n' "$lb_show" | grep -q "Status: closed"
