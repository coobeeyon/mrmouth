//! Exercise real CLI dispatch with disposable Git repos and fake Docker/agents.
//! No provider, Docker daemon, remote repository, or user configuration is used.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn init_repo(repo: &Path) {
    fs::create_dir(repo).unwrap();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.name", "Fixture"]);
    git(repo, &["config", "user.email", "fixture@example.invalid"]);
    git(repo, &["commit", "--allow-empty", "-m", "seed"]);
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn exercise(command: &str, split: bool, change_code: bool) {
    let root = tempfile::tempdir().unwrap();
    let bookkeeping = root.path().join("bookkeeping");
    let code = root.path().join("code with spaces");
    let bin = root.path().join("bin");
    init_repo(&bookkeeping);
    init_repo(&code);
    fs::create_dir(&bin).unwrap();
    fs::create_dir(bookkeeping.join(".mrmouth")).unwrap();
    let config = if split {
        format!("agent = 'claude'\nwork_repo = '{}'\n", code.display())
    } else {
        "agent = 'claude'\n".to_string()
    };
    fs::write(bookkeeping.join(".mrmouth/config.toml"), config).unwrap();
    fs::write(bookkeeping.join(".gitignore"), "logs/\n").unwrap();
    git(&bookkeeping, &["add", "."]);
    git(&bookkeeping, &["commit", "-m", "fixture configuration"]);
    let review_repo = if split { &code } else { &bookkeeping };
    let before = git(review_repo, &["rev-parse", "HEAD"]);
    let mutate_repo = if change_code {
        review_repo
    } else {
        &bookkeeping
    };

    executable(
        &bin.join("lb"),
        r##"#!/bin/sh
set -eu
case "$1" in
  show) printf 'Title: Fixture\nType: %s\n' "$MM_TEST_ITEM_TYPE" ;;
  list|ready) if [ ! -f "$MM_TEST_ROOT/ran" ]; then echo 'lb-test task open P1 Fixture'; fi ;;
esac
"##,
    );
    executable(&bin.join("trk"), "#!/bin/sh\nexit 0\n");
    // Only loop's branch-name helper should run a host agent command.
    executable(
        &bin.join("claude"),
        r##"#!/bin/sh
cat >/dev/null
echo '{"type":"result","result":"{\"name\":\"fixture-review\"}"}'
"##,
    );
    executable(&bin.join("codex"), "#!/bin/sh\nexit 99\n");
    executable(
        &bin.join("docker"),
        r##"#!/bin/sh
set -eu
run_task() {
  test ! -f "$MM_TEST_ROOT/ran"
  echo changed > "$MM_TEST_MUTATE/change.txt"
  git -C "$MM_TEST_MUTATE" add change.txt
  git -C "$MM_TEST_MUTATE" commit -qm 'code change'
  touch "$MM_TEST_ROOT/ran"
}
case "$1" in
  run)
    script=''
    for arg in "$@"; do
      case "$arg" in *:/run.sh:ro) script=${arg%:/run.sh:ro} ;; esac
    done
    if [ -n "$script" ]; then
      if grep -q 'You are the \*\*Reviewer\*\*' "$script"; then
        printf '%s\n' "$@" > "$MM_TEST_ROOT/review-args"
        cp "$script" "$MM_TEST_ROOT/review-script"
      else
        run_task
      fi
    fi ;;
  exec)
    for arg in "$@"; do
      case "$arg" in /mrmouth-scripts/task.sh) run_task ;; esac
    done ;;
  inspect) echo fixture-image ;;
  cp) exit 1 ;;
  build|volume|rm|stop) ;;
  *) echo "Unexpected fake Docker call: $1" >&2; exit 99 ;;
esac
"##,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_mrmouth"));
    child
        .current_dir(&bookkeeping)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", root.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("MRMOUTH_SKIP_PREFLIGHT", "1")
        .env("MM_TEST_ROOT", root.path())
        .env("MM_TEST_MUTATE", mutate_repo)
        .env(
            "MM_TEST_ITEM_TYPE",
            if command == "epic" { "epic" } else { "task" },
        );
    match command {
        "do" | "epic" => {
            child.args(["do", "lb-test", "--json-events"]);
        }
        "ready" => {
            child.args(["ready", "--json-events"]);
        }
        "loop" => {
            child.args(["loop", "--max-runs", "1", "--no-summary", "--json-events"]);
        }
        _ => unreachable!(),
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.path().join("ran").exists(), "runner was not exercised");
    let after = git(review_repo, &["rev-parse", "HEAD"]);
    let review = root.path().join("review-script");
    if split && !change_code {
        assert_eq!(before, after);
        assert!(
            !review.exists(),
            "bookkeeping-only commit triggered code review"
        );
        return;
    }
    assert_ne!(before, after);
    let script = fs::read_to_string(&review).unwrap_or_else(|e| {
        panic!(
            "{command}: code change was not reviewed: {e}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let container_repo = if split {
        "/home/runner/worktree"
    } else {
        "/home/runner/workspace"
    };
    let scoped_diff = format!("git -C {container_repo} diff {before}..{after}");
    assert!(
        script.contains(&scoped_diff)
            || (!split && script.contains(&format!("git diff {before}..{after}")))
    );
    let args = fs::read_to_string(root.path().join("review-args")).unwrap();
    if split {
        assert_eq!(git(&code, &["branch", "--show-current"]), "main");
        assert!(args
            .lines()
            .any(|arg| arg == format!("{}:/home/runner/worktree", code.display())));
        assert!(args
            .lines()
            .any(|arg| arg == "MRMOUTH_WORK_REPO=/home/runner/worktree"));
        assert!(script.contains("Run `lb` and `trk` commands from `/home/runner/workspace`"));
    } else {
        assert!(args
            .lines()
            .any(|arg| arg == "MRMOUTH_WORK_REPO=/home/runner/workspace"));
        assert!(!args
            .lines()
            .any(|arg| arg.ends_with(":/home/runner/worktree")));
    }
}

#[test]
fn code_only_commits_reach_review_in_every_command() {
    for command in ["do", "epic", "ready", "loop"] {
        exercise(command, true, true);
    }
}

#[test]
fn bookkeeping_only_commits_do_not_trigger_code_review() {
    for command in ["do", "epic", "ready", "loop"] {
        exercise(command, true, false);
    }
}

#[test]
fn ordinary_single_repo_review_still_works() {
    for command in ["do", "epic", "ready", "loop"] {
        exercise(command, false, true);
    }
}
