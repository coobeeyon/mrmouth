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

  epic_output="$(lb create "Complete fulfillment operations reporting package" \
    -t epic \
    -p 1 \
    -d "Goal: Complete the fulfillment operations Python package in ./worktree. Context: SPEC.md describes the full behavior and focused tests cover each child task. Acceptance: all child tasks are closed, ./worktree/check.sh passes, code changes are committed in ./worktree, and both repos are clean.")"
  epic_id="${epic_output##* }"
  printf '%s\n' "$epic_id" > .eval-epic-id
  : > .eval-leaf-ids

  task1_output="$(lb create "Implement product and order loading" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/loader.py product and order CSV loading. Context: Read ./SPEC.md section 1 and inspect ./worktree/tests/test_loader.py. Requirements: parse products and order lines with the documented types, preserve order line file order, reject duplicate product SKUs, and reject missing required columns. Acceptance: from ./worktree, python3 -m unittest tests.test_loader passes; commit the code change in ./worktree; close only this task. Out of scope: catalog enrichment and downstream fulfillment behavior.")"
  task1_id="${task1_output##* }"
  printf '%s\n' "$task1_id" >> .eval-leaf-ids

  task2_output="$(lb create "Implement catalog enrichment" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/catalog.py catalog enrichment. Context: Depends on loading from $task1_id. Read ./SPEC.md section 2 and inspect ./worktree/tests/test_catalog.py. Requirements: copy order lines, join product fields, compute requested_subtotal, and reject unknown SKUs. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog passes; commit the code change in ./worktree; close only this task. Out of scope: allocation and later reporting.")"
  task2_id="${task2_output##* }"
  printf '%s\n' "$task2_id" >> .eval-leaf-ids

  task3_output="$(lb create "Implement stock allocation" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/allocation.py stock allocation. Context: Depends on enriched lines from $task2_id. Read ./SPEC.md section 3 and inspect ./worktree/tests/test_allocation.py. Requirements: allocate by order line file order, add allocated_quantity/backordered_quantity/fully_allocated, and avoid mutating product stock. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation passes; commit the code change in ./worktree; close only this task. Out of scope: shipments and reporting.")"
  task3_id="${task3_output##* }"
  printf '%s\n' "$task3_id" >> .eval-leaf-ids

  task4_output="$(lb create "Implement shipment grouping" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/shipments.py shipment grouping. Context: Depends on allocated lines from $task3_id. Read ./SPEC.md section 4 and inspect ./worktree/tests/test_shipments.py. Requirements: group allocated quantities by order, exclude zero-allocation orders, compute weights/hazmat/status. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation tests.test_shipments passes; commit the code change in ./worktree; close only this task. Out of scope: carrier quoting and invoices.")"
  task4_id="${task4_output##* }"
  printf '%s\n' "$task4_id" >> .eval-leaf-ids

  task5_output="$(lb create "Implement carrier quoting" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/carriers.py carrier quoting. Context: Depends on shipments from $task4_id. Read ./SPEC.md section 5 and inspect ./worktree/tests/test_carriers.py. Requirements: choose AirSafe/AirFast/GroundHaz/Ground from service and hazmat flags, compute rounded shipping_cost, and return shipment copies. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation tests.test_shipments tests.test_carriers passes; commit the code change in ./worktree; close only this task. Out of scope: invoices and summaries.")"
  task5_id="${task5_output##* }"
  printf '%s\n' "$task5_id" >> .eval-leaf-ids

  task6_output="$(lb create "Implement invoice totals" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/invoices.py invoice totals. Context: Depends on quoted shipments from $task5_id. Read ./SPEC.md section 6 and inspect ./worktree/tests/test_invoices.py. Requirements: build one invoice per order, use allocated quantities for merchandise subtotal, add shipment cost when present, and set invoice_status from paid. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation tests.test_shipments tests.test_carriers tests.test_invoices passes; commit the code change in ./worktree; close only this task. Out of scope: backorder planning and risk.")"
  task6_id="${task6_output##* }"
  printf '%s\n' "$task6_id" >> .eval-leaf-ids

  task7_output="$(lb create "Implement backorder planning" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/backorders.py backorder planning. Context: Depends on allocated lines from $task3_id and can run after invoices from $task6_id. Read ./SPEC.md section 7 and inspect ./worktree/tests/test_backorders.py. Requirements: group backordered units by SKU, collect affected orders, and sort by descending units then SKU. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation tests.test_backorders passes; commit the code change in ./worktree; close only this task. Out of scope: risk and CLI reporting.")"
  task7_id="${task7_output##* }"
  printf '%s\n' "$task7_id" >> .eval-leaf-ids

  task8_output="$(lb create "Implement order risk scoring" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/risk.py order risk scoring. Context: Depends on allocated lines and invoices from $task6_id. Read ./SPEC.md section 8 and inspect ./worktree/tests/test_risk.py. Requirements: score express, backordered, unpaid, hazmat, and high-value orders; include orders with risk_score >= 4; include reasons; sort by descending score then order_id. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation tests.test_shipments tests.test_carriers tests.test_invoices tests.test_risk passes; commit the code change in ./worktree; close only this task. Out of scope: summary metrics and CLI.")"
  task8_id="${task8_output##* }"
  printf '%s\n' "$task8_id" >> .eval-leaf-ids

  task9_output="$(lb create "Implement fulfillment summary metrics" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/metrics.py fulfillment summary metrics. Context: Depends on invoices and backorders from $task7_id. Read ./SPEC.md section 9 and inspect ./worktree/tests/test_metrics.py. Requirements: compute order_count, line_count, fully_allocated_orders, backordered_units, and carrier_totals. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_catalog tests.test_allocation tests.test_shipments tests.test_carriers tests.test_invoices tests.test_backorders tests.test_metrics passes; commit the code change in ./worktree; close only this task. Out of scope: CLI output wiring.")"
  task9_id="${task9_output##* }"
  printf '%s\n' "$task9_id" >> .eval-leaf-ids

  task10_output="$(lb create "Implement fulfillment CLI report" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement fulfillops/cli.py report assembly and CLI output. Context: Depends on all package slices through $task9_id and risk scoring from $task8_id. Read ./SPEC.md section 10 and inspect ./worktree/tests/test_cli.py. Requirements: build the documented JSON report, support python3 -m fulfillops.cli <products.csv> <orders.csv> [--output report.json], write stdout without --output and the same JSON with --output. Acceptance: from ./worktree, ./check.sh passes; commit the code change in ./worktree; close this task and close the parent epic if all children are closed. Out of scope: adding dependencies or editing tests/data.")"
  task10_id="${task10_output##* }"
  printf '%s\n' "$task10_id" >> .eval-leaf-ids

  lb dep add "$task1_id" --blocks "$task2_id" >/dev/null
  lb dep add "$task2_id" --blocks "$task3_id" >/dev/null
  lb dep add "$task3_id" --blocks "$task4_id" >/dev/null
  lb dep add "$task4_id" --blocks "$task5_id" >/dev/null
  lb dep add "$task5_id" --blocks "$task6_id" >/dev/null
  lb dep add "$task6_id" --blocks "$task7_id" >/dev/null
  lb dep add "$task7_id" --blocks "$task8_id" >/dev/null
  lb dep add "$task8_id" --blocks "$task9_id" >/dev/null
  lb dep add "$task9_id" --blocks "$task10_id" >/dev/null

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
- In ./worktree, implement the fulfillment operations behavior described in ./SPEC.md.
- Run ./check.sh in ./worktree and make it pass.
- Commit the code changes in ./worktree.
- Close all child tasks and the parent epic in this bookkeeping repo.
- Leave both repositories with clean working trees except ignored eval artifacts.
EOF

cat > "$fixture_dir/repo/.goal-turn.txt" <<EOF
Work on the active goal now. Use ./worktree for code changes and this directory for Litebrite commands. Complete the epic by completing every child task, then mark the goal complete.
EOF

printf '%s\n' "$fixture_dir/repo"
