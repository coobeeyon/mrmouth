#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_codex_home="${CODEX_HOME:-$HOME/.codex}"

command -v git >/dev/null
command -v lb >/dev/null
command -v trk >/dev/null
command -v python3 >/dev/null
command -v cargo >/dev/null

rm -rf "$fixture_dir/repo" "$fixture_dir/remotes" "$fixture_dir/reports"
mkdir -p "$fixture_dir/repo" "$fixture_dir/remotes" "$fixture_dir/reports"

python3 "$fixture_dir/generate_seed.py" "$fixture_dir/repo"

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

  epic_output="$(lb create "Complete Biolife Rust simulation prototype" \
    -t epic \
    -p 1 \
    -d "Goal: Complete the Rust Biolife simulation prototype in ./worktree. Context: SPEC.md describes chromosome-driven graph organisms, energy systems, combat/defense, and a simple viscous-fluid propulsion model. Acceptance: all child tasks are closed, ./worktree/check.sh passes, code changes are committed in ./worktree, and both repos are clean.")"
  epic_id="${epic_output##* }"
  printf '%s\n' "$epic_id" > .eval-epic-id
  : > .eval-leaf-ids

  previous_id=""
  while IFS=$'\t' read -r index title description; do
    task_output="$(lb create "$title" -t task -p 1 --parent "$epic_id" -d "$description")"
    task_id="${task_output##* }"
    printf '%s\n' "$task_id" >> .eval-leaf-ids
    if [ -n "$previous_id" ]; then
      lb dep add "$previous_id" --blocks "$task_id" >/dev/null
    fi
    previous_id="$task_id"
  done < .eval-task-list.tsv

  lb sync >/dev/null
  git push -q origin litebrite
  git push -q origin trapperkeeper
  git add .codex .gitattributes .gitignore .trapperkeeper.json
  if ! git diff --cached --quiet; then
    git commit -q -m "Initialize eval agent integrations"
    git push -q origin main
  fi
)

worktree_remote="$fixture_dir/remotes/worktree.git"
(cd "$fixture_dir/repo/worktree" && cargo generate-lockfile)
git_init_repo "$fixture_dir/repo/worktree" "$worktree_remote"

epic_id="$(cat "$fixture_dir/repo/.eval-epic-id")"
leaf_ids="$(paste -sd ' ' "$fixture_dir/repo/.eval-leaf-ids")"

cat > "$fixture_dir/repo/.goal-objective.txt" <<EOF
Complete Litebrite epic $epic_id in this bookkeeping repo.

Use the Rust worktree at ./worktree. Read ./SPEC.md in this bookkeeping repo, inspect the child tasks, and complete all children in dependency order: $leaf_ids.

Success criteria:
- In ./worktree, implement the Biolife Rust simulation behavior described in ./SPEC.md.
- Run ./check.sh in ./worktree and make it pass.
- Keep simulation/backend logic in biolife_core and frontend/CLI logic in biolife_app.
- Commit the code changes in ./worktree.
- Close all child tasks and the parent epic in this bookkeeping repo.
- Leave both repositories with clean working trees except ignored eval artifacts.
EOF

cat > "$fixture_dir/repo/.goal-turn.txt" <<EOF
Work on the active goal now. Use ./worktree for code changes and this directory for Litebrite commands. Complete the epic by completing every child task, then mark the goal complete.
EOF

printf '%s\n' "$fixture_dir/repo"
