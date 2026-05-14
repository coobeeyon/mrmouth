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
2. Update run log symlinks.
3. Pull remote code changes when using a real remote.
4. Copy the container Dockerfile to a temporary host path.
5. Compare bytes with the post-pull host Dockerfile.
6. Rename over the host file only when the content differs.
7. Remove the container and continue litebrite sync.

This still preserves unpushed Dockerfile edits from the container: when the
remote pull does not bring in the container content, the byte comparison differs
and the temp file replaces the host Dockerfile. When the pushed content is
already present after pull, the host file is left untouched.

The reviewer path originally skipped the pre-extraction host pull. That could
leave a dirty host Dockerfile even when the reviewer had committed and pushed
the same Dockerfile edit from inside the container. Reviewer extraction now uses
the same pull-before-copy ordering as runner extraction for real remotes.
