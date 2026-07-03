#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$fixture_dir/../../.." && pwd)"
mrmouth_bin="${MRMOUTH_BIN:-$repo_root/target/debug/mrmouth}"

if [ ! -x "$mrmouth_bin" ]; then
  (cd "$repo_root" && cargo build)
fi

bookkeeping_repo="$("$fixture_dir/prepare.sh")"
codex_home="$bookkeeping_repo/.codex-home"
epic_id="$(cat "$bookkeeping_repo/.eval-epic-id")"
start_ms="$(python3 -c 'import time; print(int(time.time() * 1000))')"

index=0
while IFS= read -r item_id; do
  index=$((index + 1))
  CODEX_HOME="$codex_home" "$mrmouth_bin" eval \
    --cwd "$bookkeeping_repo" \
    --output "$fixture_dir/reports/do-$index-$item_id.json" \
    -- "$mrmouth_bin" do "$item_id" \
      --json-events \
      --current-container \
      --worktree "$bookkeeping_repo/worktree" \
      --timeout "${MRMOUTH_EVAL_TIMEOUT:-20}" \
      --max-failures 1
done < "$bookkeeping_repo/.eval-leaf-ids"

(
  cd "$bookkeeping_repo"
  lb close "$epic_id" >/dev/null
  lb sync >/dev/null
)

end_ms="$(python3 -c 'import time; print(int(time.time() * 1000))')"
python3 - "$fixture_dir" "$bookkeeping_repo" "$start_ms" "$end_ms" <<'PY'
import json
import sys
from pathlib import Path

fixture_dir = Path(sys.argv[1])
bookkeeping_repo = Path(sys.argv[2])
started = int(sys.argv[3])
ended = int(sys.argv[4])
reports_dir = fixture_dir / "reports"
leaf_ids = [
    line.strip()
    for line in (bookkeeping_repo / ".eval-leaf-ids").read_text(encoding="utf-8").splitlines()
    if line.strip()
]
children = []
token_totals = {
    "turn_count": 0,
    "input_tokens": 0,
    "cached_input_tokens": 0,
    "uncached_input_tokens": 0,
    "output_tokens": 0,
    "reasoning_output_tokens": 0,
    "total_tokens": 0,
    "total_uncached_tokens": 0,
}
summed_wall_ms = 0
success = True

for index, item_id in enumerate(leaf_ids, start=1):
    path = reports_dir / f"do-{index}-{item_id}.json"
    report = json.loads(path.read_text(encoding="utf-8"))
    token_usage = report["lifecycle"].get("token_usage") or {}
    for key in token_totals:
        token_totals[key] += int(token_usage.get(key) or 0)
    summed_wall_ms += int(report["wall_ms"])
    child_success = bool(report.get("success"))
    success = success and child_success
    children.append(
        {
            "item_id": item_id,
            "report": str(path),
            "success": child_success,
            "wall_ms": report["wall_ms"],
            "token_usage": token_usage,
            "summary": report["lifecycle"].get("final_summary"),
        }
    )

aggregate = {
    "harness": "mrmouth-multi-do",
    "cwd": str(bookkeeping_repo),
    "success": success,
    "wall_ms": ended - started,
    "summed_child_wall_ms": summed_wall_ms,
    "epic_id": (bookkeeping_repo / ".eval-epic-id").read_text(encoding="utf-8").strip(),
    "leaf_ids": leaf_ids,
    "children": children,
    "token_usage": token_totals,
    "closed_epic_by_harness": True,
}
(reports_dir / "result.json").write_text(json.dumps(aggregate, indent=2) + "\n", encoding="utf-8")
print(f"multi-do eval report: {reports_dir / 'result.json'}")
PY
