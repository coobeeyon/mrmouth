#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_codex_home="${CODEX_HOME:-$HOME/.codex}"

command -v git >/dev/null
command -v lb >/dev/null
command -v trk >/dev/null
command -v python3 >/dev/null

rm -rf "$fixture_dir/repo" "$fixture_dir/remotes" "$fixture_dir/reports"
mkdir -p "$fixture_dir/repo" "$fixture_dir/remotes" "$fixture_dir/reports"

cp -a "$fixture_dir/seed/bookkeeping/." "$fixture_dir/repo/"

git_init_repo() {
  local path="$1"
  local remote="$2"
  git -C "$path" init -q
  git -C "$path" config user.email "eval@example.com"
  git -C "$path" config user.name "Mr Mouth Eval"
  git -C "$path" add -A
  git -C "$path" commit -q -m "Initial fixture state"
  git init -q --bare "$remote"
  git -C "$path" remote add origin "$remote"
  git -C "$path" branch -M main
  git -C "$path" push -q -u origin main
}

bookkeeping_repo="$fixture_dir/repo"
bookkeeping_remote="$fixture_dir/remotes/bookkeeping.git"
git_init_repo "$bookkeeping_repo" "$bookkeeping_remote"

codex_home="$bookkeeping_repo/.codex-home"
mkdir -p "$codex_home"
for file in auth.json config.toml; do
  if [ -f "$source_codex_home/$file" ]; then
    cp -a "$source_codex_home/$file" "$codex_home/$file"
  fi
done
touch "$codex_home/config.toml"

append_codex_config_table() {
  local header="$1"
  shift
  if ! grep -Fqx "$header" "$codex_home/config.toml"; then
    {
      printf '\n%s\n' "$header"
      printf '%s\n' "$@"
    } >> "$codex_home/config.toml"
  fi
}

codex_hooks_path="$bookkeeping_repo/.codex/hooks.json"
append_codex_config_table "[projects.\"$bookkeeping_repo\"]" 'trust_level = "trusted"'
append_codex_config_table \
  "[hooks.state.\"$codex_hooks_path:session_start:0:0\"]" \
  'enabled = true' \
  'trusted_hash = "sha256:563f0ab4b9c866e904189c224dee1510951b34d7dcacfbbe77894451afcbb07e"'
append_codex_config_table \
  "[hooks.state.\"$codex_hooks_path:session_start:1:0\"]" \
  'enabled = true' \
  'trusted_hash = "sha256:36f49bcf89ea734587b7c4ab75849ee900d13d57259a982f80fbc54c7ae2d28c"'

(
  cd "$bookkeeping_repo"
  lb init >/dev/null
  trk init >/dev/null
  lb setup codex >/dev/null
  trk setup codex >/dev/null

  epic_output="$(lb create "Complete inventory planning package" \
    -t epic \
    -p 1 \
    -d "Goal: Complete the inventory planning Python package in ./worktree. Context: SPEC.md describes the full behavior and the worktree contains focused tests for each child. Acceptance: all child tasks are closed, ./worktree/check.sh passes, code changes are committed in ./worktree, and both repos are clean.")"
  epic_id="${epic_output##* }"
  printf '%s\n' "$epic_id" > .eval-epic-id
  : > .eval-leaf-ids

  task1_output="$(lb create "Implement catalog loading" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement inventory/catalog.py catalog loading. Context: Read ./SPEC.md section 1 and inspect ./worktree/tests/test_catalog.py. Requirements: parse catalog CSV rows into dictionaries with typed numeric fields, reject missing required columns, reject duplicate SKUs, and keep rows sorted by SKU. Acceptance: from ./worktree, python3 -m unittest tests.test_catalog passes; commit the code change in ./worktree; close only this task. Out of scope: stock movements, reorder planning, and CLI reporting.")"
  task1_id="${task1_output##* }"
  printf '%s\n' "$task1_id" >> .eval-leaf-ids

  task2_output="$(lb create "Implement stock movements" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement inventory/stock.py movement application. Context: Depends on the catalog loader from $task1_id. Read ./SPEC.md section 2 and inspect ./worktree/tests/test_stock.py. Requirements: load movement CSV rows, support receipt/sale/adjustment types, update on_hand counts, accumulate units_sold, units_received, and adjustments, and reject unknown SKUs or movement types. Acceptance: from ./worktree, python3 -m unittest tests.test_catalog tests.test_stock passes; commit the code change in ./worktree; close only this task. Out of scope: reorder planning and CLI reporting.")"
  task2_id="${task2_output##* }"
  printf '%s\n' "$task2_id" >> .eval-leaf-ids

  task3_output="$(lb create "Implement reorder planning" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement inventory/reorder.py reorder planning. Context: Depends on movement-adjusted inventory from $task2_id. Read ./SPEC.md section 3 and inspect ./worktree/tests/test_reorder.py. Requirements: build a plan for items whose on_hand is at or below reorder_point, include reorder_qty and estimated_cost, sort items by category then SKU, and round money to two decimals. Acceptance: from ./worktree, python3 -m unittest tests.test_catalog tests.test_stock tests.test_reorder passes; commit the code change in ./worktree; close only this task. Out of scope: CLI reporting.")"
  task3_id="${task3_output##* }"
  printf '%s\n' "$task3_id" >> .eval-leaf-ids

  task4_output="$(lb create "Implement inventory report CLI" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement inventory/cli.py and package entrypoint behavior. Context: Depends on reorder planning from $task3_id. Read ./SPEC.md section 4 and inspect ./worktree/tests/test_cli.py. Requirements: support python3 -m inventory.cli <catalog.csv> <movements.csv> [--output report.json], produce the documented JSON report, write stdout without --output and the same JSON file with --output. Acceptance: from ./worktree, ./check.sh passes; commit the code change in ./worktree; close this task. Out of scope: adding new dependencies or editing tests.")"
  task4_id="${task4_output##* }"
  printf '%s\n' "$task4_id" >> .eval-leaf-ids

  lb dep add "$task1_id" --blocks "$task2_id" >/dev/null
  lb dep add "$task2_id" --blocks "$task3_id" >/dev/null
  lb dep add "$task3_id" --blocks "$task4_id" >/dev/null

  lb sync >/dev/null
  git push -q origin litebrite
  git push -q origin trapperkeeper
  git add .codex .gitattributes .gitignore .trapperkeeper.json
  if ! git diff --cached --quiet; then
    git commit -q -m "Initialize eval agent integrations"
    git push -q origin main
  fi
)

mkdir -p "$fixture_dir/repo/worktree"
cp -a "$fixture_dir/seed/worktree/." "$fixture_dir/repo/worktree/"
worktree_remote="$fixture_dir/remotes/worktree.git"
git_init_repo "$fixture_dir/repo/worktree" "$worktree_remote"

epic_id="$(cat "$fixture_dir/repo/.eval-epic-id")"
leaf_ids="$(paste -sd ' ' "$fixture_dir/repo/.eval-leaf-ids")"

cat > "$fixture_dir/repo/.goal-objective.txt" <<EOF
Complete Litebrite epic $epic_id in this bookkeeping repo.

Use the code worktree at ./worktree. Read ./SPEC.md in this bookkeeping repo, inspect the child tasks, and complete all children in dependency order: $leaf_ids.

Success criteria:
- In ./worktree, implement the inventory planning behavior described in ./SPEC.md.
- Run ./check.sh in ./worktree and make it pass.
- Commit the code changes in ./worktree.
- Close all child tasks and the parent epic in this bookkeeping repo.
- Leave both repositories with clean working trees except ignored eval artifacts.
EOF

cat > "$fixture_dir/repo/.goal-turn.txt" <<EOF
Work on the active goal now. Use ./worktree for code changes and this directory for Litebrite commands. Complete the epic by completing every child task, then mark the goal complete.
EOF

printf '%s\n' "$fixture_dir/repo"
