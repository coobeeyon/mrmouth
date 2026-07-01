#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_codex_home="${CODEX_HOME:-$HOME/.codex}"

command -v git >/dev/null
command -v lb >/dev/null

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

(
  cd "$bookkeeping_repo"
  lb init >/dev/null
  item_output="$(lb create "Make the smol message explicit" \
    -t task \
    -p 1 \
    -d "In the code worktree, change message.txt to exactly 'hello from smol eval'. Run ./check.sh, commit the code change, and close this task.")"
  item_id="${item_output##* }"
  printf '%s\n' "$item_id" > .eval-item-id
)

mkdir -p "$fixture_dir/repo/worktree"
cp -a "$fixture_dir/seed/worktree/." "$fixture_dir/repo/worktree/"
worktree_remote="$fixture_dir/remotes/worktree.git"
git_init_repo "$fixture_dir/repo/worktree" "$worktree_remote"

cat > "$fixture_dir/repo/.goal-objective.txt" <<EOF
Complete Litebrite item $(cat "$fixture_dir/repo/.eval-item-id") in this bookkeeping repo.

Use the code worktree at ./worktree. Read SPEC.md and run lb show $(cat "$fixture_dir/repo/.eval-item-id").

Success criteria:
- In ./worktree, change message.txt to exactly: hello from smol eval
- Run ./check.sh in ./worktree and make it pass.
- Commit the code change in ./worktree.
- Close the Litebrite item in this bookkeeping repo.
- Leave both repositories with clean working trees except ignored eval artifacts.
EOF

cat > "$fixture_dir/repo/.goal-turn.txt" <<EOF
Work on the active goal now. Use ./worktree for code changes and this directory for Litebrite commands. When all success criteria are met, mark the goal complete.
EOF

printf '%s\n' "$fixture_dir/repo"
