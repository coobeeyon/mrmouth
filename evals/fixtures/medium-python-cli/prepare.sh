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
  item_output="$(lb create "Implement medium sales summary CLI" \
    -t task \
    -p 1 \
    -d "In the code worktree, implement SPEC.md for sales_report.py. Run ./check.sh, commit the code change, and close this task.")"
  item_id="${item_output##* }"
  printf '%s\n' "$item_id" > .eval-item-id
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

cat > "$fixture_dir/repo/.goal-objective.txt" <<EOF
Complete Litebrite item $(cat "$fixture_dir/repo/.eval-item-id") in this bookkeeping repo.

Use the code worktree at ./worktree. Read ./SPEC.md in this bookkeeping repo and run lb show $(cat "$fixture_dir/repo/.eval-item-id").

Success criteria:
- In ./worktree, implement the medium sales summary behavior described in ./SPEC.md.
- Run ./check.sh in ./worktree and make it pass.
- Commit the code change in ./worktree.
- Close the Litebrite item in this bookkeeping repo.
- Leave both repositories with clean working trees except ignored eval artifacts.
EOF

cat > "$fixture_dir/repo/.goal-turn.txt" <<EOF
Work on the active goal now. Use ./worktree for code changes and this directory for Litebrite commands. When all success criteria are met, mark the goal complete.
EOF

printf '%s\n' "$fixture_dir/repo"
