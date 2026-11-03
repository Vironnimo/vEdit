# Worktree Workflow

Use a separate checkout for large, uncertain or concurrent work. Small, low-risk
changes can stay on `main` (`AGENTS.md` -> Git).

## Repository script

Use the repository script below for the complete task lifecycle: create, work and
commit, then merge and automatically remove the checkout and branch. It owns only
marked checkouts under the primary checkout's `.worktrees/` directory.

Use this script exclusively for task worktrees. Keep the chat attached to the
primary checkout and pass the task checkout as the working directory for its
commands. Worktree creation, merging and deletion require no chat handoff or
application archive operation. A landed task whose source remains is incomplete.

## Create and work

```powershell
python scripts/worktree.py create <task-name>
python scripts/worktree.py list
```

`create` prints `status`, `name`, `branch` and the absolute `path`. It creates
`.worktrees/<task-name>` on branch `<task-name>` from `main`, without a prefix.
Names contain 1-63 lowercase letters, digits or hyphens, begin with a letter or digit and cannot be
Windows device names. `list` prints each task's uncommitted entry and unmerged
commit counts. It lists only the script's own task worktrees.

Change to the printed path before running commands; a script's location does not
change the shell's working directory. Give delegated agents that path explicitly.
Each checkout has its own `target/` and `.tmp/`; do not share them through junctions
or symlinks. Cargo installs locked dependencies on demand; creation does not build
or launch the editor. vEdit needs no per-checkout TCP port. The ignored
`.vorch-worktree` marker records the name, branch, `port: null` and creation time.

Before creating a task, the script retries recorded cleanup from explicit earlier
removals and removes owned private landing checkouts left by interrupted merges.
The shared OS lock excludes live script operations. A fresh task with no commits
is active work, not cleanup: ordinary checkouts are never selected by age or by
having no commits ahead of `main`. Changed branches and dirty checkouts are kept
and reported. Unknown private directories and other Git worktrees are untouched.

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
committed there meanwhile, it reports `merged-cleanup-pending`, exits 1 and keeps
that work. Resolve the reported error, finish any concurrent work, leave the source
directory, then repeat `merge`. Exit 0 means the merge and cleanup both completed;
do not finish the task after a cleanup failure. `--force` is not a cleanup shortcut.

An atomic `.worktrees/.cleanup-<name>.json` record binds a removal to its task name,
owned branch and exact commit before Git removes the checkout. `list` includes
pending records even after their checkout is gone. `merge` or `delete` can finish
the recorded removal after interruption, including when only the branch remains.
If Git deregistered the checkout but left locked files, removal retries transient
locks and read-only files without stopping processes. A later retry without Git
metadata accepts only build/scratch output or the ownership marker; unexplained
source files require restoring metadata or an explicitly forced deletion.
Pending records are removed only after the directory and branch are gone.

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
- Private `.landing-*` checkouts have a sidecar ownership record tied to this repository,
  written before creation and removed after cleanup, so it survives partial deletion.
  Create, merge and delete sweep these under the operation lock. Only detached,
  unlocked checkouts with the matching marker are removed; push checkouts and
  unknown directories are outside this sweep. Removal refuses nested repositories
  and redirected paths here too. The script does not broadly prune Git records.
