# Dockerfile Extraction Lifecycle

After an agent or reviewer exits, mrmouth may need to persist a container-edited
`.mrmouth/Dockerfile` back to the host checkout so the next run can rebuild with
new tooling.

For normal remote-backed runs, host sync must happen before extraction. Agents
are instructed to commit, close, sync, and push before exiting; if the agent
already pushed the Dockerfile change, extracting the same file into the stale
host checkout before `git pull --ff-only` dirties the worktree and can make the
pull fail.

The intended flow, for both runner and reviewer containers, is:

1. Wait for the container command to exit.
2. If the command succeeded, the wrapper runs task-state syncs, checks
   `.mrmouth/Dockerfile`, and auto-commits just that path with
   `Update mrmouth Dockerfile` when the agent/reviewer left it uncommitted.
3. Update run log symlinks.
4. Pull remote code changes when using a real remote.
5. If the command succeeded, copy the container Dockerfile to a temporary host
   path. Failed runs skip extraction so half-finished Dockerfile edits do not
   dirty the host checkout.
6. Compare bytes with the post-pull host Dockerfile.
7. Rename over the host file only when the content differs.
8. Remove the container and continue litebrite sync.

This still preserves successful Dockerfile self-updates from the container:
when the wrapper's path-scoped auto-commit/push succeeds, the host pull brings
in the change before extraction. If the push fails but the run succeeded, the
byte comparison can still replace the host Dockerfile, preserving the content
for the next image build. When the pushed content is already present after
pull, the host file is left untouched.

The reviewer path originally skipped the pre-extraction host pull. That could
leave a dirty host Dockerfile even when the reviewer had committed and pushed
the same Dockerfile edit from inside the container. Reviewer extraction now uses
the same pull-before-copy ordering as runner extraction for real remotes.

The more recent poisoning mode was a successful agent/reviewer run that edited
`.mrmouth/Dockerfile` but forgot to include it in a commit. Host extraction
would then copy the file out as an uncommitted host change, and later runs could
fail or stash it as leftover state. Runner, session-task, and reviewer wrapper
scripts now run a shared path-scoped shell step before `git push`: if
`git status --porcelain -- .mrmouth/Dockerfile` is non-empty, they stage and
commit only that file. This avoids sweeping unrelated uncommitted task work into
the tooling-image commit.
