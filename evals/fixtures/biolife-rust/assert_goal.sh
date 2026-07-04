#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
report="$fixture_dir/reports/goal-result.json"
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

assert report["harness"] == "codex-goal-app-server", report
assert report["success"] is True, report
goal = report["goal"]["final"]
assert goal["status"] == "complete", goal
assert epic_id in goal["objective"], goal
tokens = report["token_usage"]
assert tokens["comparable_total_uncached_tokens"] > 0, tokens
events = report["app_server"]["event_counts"]
assert events["thread/tokenUsage/updated"] >= 1, events
diffs = "\n".join(report["evidence"]["diffs"])
assert "biolife_core" in diffs, diffs
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
