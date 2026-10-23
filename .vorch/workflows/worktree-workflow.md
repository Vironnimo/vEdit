# Worktree Workflow

Use a separate checkout for large, uncertain or concurrent work. Small, low-risk
changes can stay on `main` (`AGENTS.md` -> Git).

## Choose the worktree owner

In Codex, reuse a suitable attached worktree or create one with the app's worktree
tool. Archive app-managed worktrees through the app; it retains a recoverable
snapshot. This script neither lists nor deletes those checkouts. Follow the app's
branch and review flow to land their changes; do not fabricate a script marker.

Outside that managed flow, use the repository commands below. They own only
marked checkouts under the primary checkout's `.worktrees/` directory.

## Create and work

```powershell
python scripts/worktree.py create <task-name>
python scripts/worktree.py list
```

`create` prints `status`, `name`, `branch` and the absolute `path`. It creates
`.worktrees/<task-name>` on `codex/<task-name>` from `main`. Names contain 1-63
lowercase letters, digits or hyphens, begin with a letter or digit and cannot be
Windows device names. `list` prints each task's uncommitted entry and unmerged
commit counts. It does not list Codex-managed or other Git worktrees.

Change to the printed path before running commands; a script's location does not
change the shell's working directory. Give delegated agents that path explicitly.
Each checkout has its own `target/` and `.tmp/`; do not share them through junctions
or symlinks. Cargo installs locked dependencies on demand; creation does not build
or launch the editor. vEdit needs no per-checkout TCP port. The ignored
`.vorch-worktree` marker records the name, branch, `port: null` and creation time.

For shell tools, scope temporary paths with `scripts/project-temp.ps1` as described
in `docs/development.md`. The worktree script scopes its subprocess temporary
paths to the respective checkout's `.tmp/worktree/`. Set `VEDIT_PROFILE_DIR` only
for an actual isolated application run; read `live-testing-workflow.md` first.
Commit completed changes through the hooks after the affected tests pass.

## Merge

Run from the primary checkout after committing the task:

```powershell
python scripts/worktree.py merge <task-name>
```

The primary checkout stays on `main`. All repository-script operations share an
OS file lock, released if their process exits. Merge waits up to 1800 seconds for
that lock; `--wait-timeout <seconds>` changes that wait. Allow the command time to
finish its checks. It runs no tests.

The command checks conflicts, makes a merge commit in a unique private landing
checkout under `.worktrees/`, runs its tracked commit hooks and fast-forwards
primary `main` to the checked commit. It retries up to three times if `main`
advances during checking. Git preserves unrelated unstaged primary changes and
refuses overlaps or an index it cannot merge safely; the script does not stash or
reset the primary checkout. A failed check leaves `main` unchanged by this merge.
Once fast-forward has completed, later cleanup failure does not undo the merge.

After landing, it removes the source checkout and branch if they still contain
the checked task revision and no pending work. If another session edited or
committed there meanwhile, it reports the completed merge and keeps the source.
Inspect the reported state before retrying. Run `delete` later for a clean,
already-merged source; `--force` is not a cleanup shortcut.

On a conflict, update the task branch in its own checkout, resolve and commit:

```powershell
git rebase main
```

Then repeat `merge`. When a commit hook fails, fix its reported problem in the task
checkout first. Exit codes are 0 for success, 1 for an operational failure and 2
for a merge conflict or invalid command usage.

## Abandon

```powershell
python scripts/worktree.py delete <task-name>
```

It refuses uncommitted files and commits absent from `main`. `--force` explicitly
discards that named task's pending work. Both modes delete ignored files such as
build output and scratch data; preserve any needed ignored files beforehand.
Removal refuses symlinks, junctions, nested Git repositories, mismatched markers
and unexpected branches even with `--force`. Remove a link itself separately after
checking its destination; never recursively delete through it.

## Shared-state limits

- Stashes belong to the repository, not a worktree. Use a temporary commit or a
  saved patch rather than an unqualified `git stash pop`.
- Keep the primary checkout on `main` while operations run. The script lock
  coordinates this script, not unrelated Git commands or arbitrary file writers.
  Do not hand-merge, switch the primary branch or edit a task being removed.
- Run merge/delete outside the checkout being removed. On Windows another shell
  or running process can hold it open; the command reports retained paths.
- Killed operations can leave a private `.landing-*` checkout. Inspect its state
  with `git worktree list` before removing that exact checkout with Git. The script
  does not recursively remove old landing paths or prune other worktree records.
