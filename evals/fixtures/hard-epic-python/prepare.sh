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

  epic_output="$(lb create "Complete support operations reporting package" \
    -t epic \
    -p 1 \
    -d "Goal: Complete the support operations Python package in ./worktree. Context: SPEC.md describes the full behavior and focused tests cover each child task. Acceptance: all child tasks are closed, ./worktree/check.sh passes, code changes are committed in ./worktree, and both repos are clean.")"
  epic_id="${epic_output##* }"
  printf '%s\n' "$epic_id" > .eval-epic-id
  : > .eval-leaf-ids

  task1_output="$(lb create "Implement ticket loading" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement supportops/loader.py ticket loading. Context: Read ./SPEC.md section 1 and inspect ./worktree/tests/test_loader.py. Requirements: parse tickets.csv, type minute fields, convert blank resolution to None, reject missing required columns and duplicate ticket ids, and sort by ticket_id. Acceptance: from ./worktree, python3 -m unittest tests.test_loader passes; commit the code change in ./worktree; close only this task. Out of scope: SLA, routing, metrics, risk, and CLI reporting.")"
  task1_id="${task1_output##* }"
  printf '%s\n' "$task1_id" >> .eval-leaf-ids

  task2_output="$(lb create "Implement SLA classification" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement supportops/sla.py SLA classification. Context: Depends on ticket loading from $task1_id. Read ./SPEC.md section 2 and inspect ./worktree/tests/test_sla.py. Requirements: add priority-based response/resolution targets, breach flags, open escalation flags, and reject unknown priority values. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_sla passes; commit the code change in ./worktree; close only this task. Out of scope: routing, metrics, risk, and CLI reporting.")"
  task2_id="${task2_output##* }"
  printf '%s\n' "$task2_id" >> .eval-leaf-ids

  task3_output="$(lb create "Implement queue routing" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement supportops/routing.py queue routing. Context: Depends on SLA-enriched tickets from $task2_id. Read ./SPEC.md section 3 and inspect ./worktree/tests/test_routing.py. Requirements: map security tickets to Security, billing to Billing, and other categories to '<region> Support'; add queue to routed ticket copies. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_sla tests.test_routing passes; commit the code change in ./worktree; close only this task. Out of scope: metrics, risk, and CLI reporting.")"
  task3_id="${task3_output##* }"
  printf '%s\n' "$task3_id" >> .eval-leaf-ids

  task4_output="$(lb create "Implement queue metrics" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement supportops/metrics.py queue metrics. Context: Depends on routed SLA tickets from $task3_id. Read ./SPEC.md section 4 and inspect ./worktree/tests/test_metrics.py. Requirements: aggregate ticket/open counts, response and resolution breaches, and closed-ticket average resolution by queue. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_sla tests.test_routing tests.test_metrics passes; commit the code change in ./worktree; close only this task. Out of scope: customer risk and CLI reporting.")"
  task4_id="${task4_output##* }"
  printf '%s\n' "$task4_id" >> .eval-leaf-ids

  task5_output="$(lb create "Implement customer risk scoring" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement supportops/risk.py customer risk scoring. Context: Depends on SLA-enriched routed tickets from $task4_id. Read ./SPEC.md section 5 and inspect ./worktree/tests/test_risk.py. Requirements: compute breach_count, open_escalations, and risk_score per customer; include customers with risk_score >= 2; sort by descending risk_score then customer. Acceptance: from ./worktree, python3 -m unittest tests.test_loader tests.test_sla tests.test_routing tests.test_metrics tests.test_risk passes; commit the code change in ./worktree; close only this task. Out of scope: CLI reporting.")"
  task5_id="${task5_output##* }"
  printf '%s\n' "$task5_id" >> .eval-leaf-ids

  task6_output="$(lb create "Implement support operations CLI" \
    -t task \
    -p 1 \
    --parent "$epic_id" \
    -d "Goal: Implement supportops/cli.py report assembly and CLI output. Context: Depends on customer risk scoring from $task5_id. Read ./SPEC.md section 6 and inspect ./worktree/tests/test_cli.py. Requirements: build the documented JSON report, support python3 -m supportops.cli <tickets.csv> [--output report.json], write stdout without --output and the same JSON with --output. Acceptance: from ./worktree, ./check.sh passes; commit the code change in ./worktree; close this task. Out of scope: adding dependencies or editing tests/data.")"
  task6_id="${task6_output##* }"
  printf '%s\n' "$task6_id" >> .eval-leaf-ids

  lb dep add "$task1_id" --blocks "$task2_id" >/dev/null
  lb dep add "$task2_id" --blocks "$task3_id" >/dev/null
  lb dep add "$task3_id" --blocks "$task4_id" >/dev/null
  lb dep add "$task4_id" --blocks "$task5_id" >/dev/null
  lb dep add "$task5_id" --blocks "$task6_id" >/dev/null

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
- In ./worktree, implement the support operations behavior described in ./SPEC.md.
- Run ./check.sh in ./worktree and make it pass.
- Commit the code changes in ./worktree.
- Close all child tasks and the parent epic in this bookkeeping repo.
- Leave both repositories with clean working trees except ignored eval artifacts.
EOF

cat > "$fixture_dir/repo/.goal-turn.txt" <<EOF
Work on the active goal now. Use ./worktree for code changes and this directory for Litebrite commands. Complete the epic by completing every child task, then mark the goal complete.
EOF

printf '%s\n' "$fixture_dir/repo"
