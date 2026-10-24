# Push Workflow

Use this workflow when the user authorizes a push. Local commits do not authorize
publication. `python scripts/push.py` checks committed `main` in a private Git
checkout and pushes exactly that commit only after every configured check passes.

## Check locally

```powershell
python scripts/push.py --no-push
```

This validates local `main` even without a configured remote and makes no remote
Git requests. It checks committed files, including when the primary checkout has
uncommitted work. Each run gets `.tmp/push/<run>/checkout` and persistent sibling
`logs/`; the checkout is removed after the run. Simultaneous runs use different
folders and never remove each other's checkouts. An interrupted process can leave
its private checkout behind; inspect it before removing that exact worktree.
Cleanup also preserves checkouts containing links, junctions or nested Git
repositories and reports the unsafe path. Git environment routing inherited from
a hook or another checkout is cleared for private-checkout commands.

Rust 1.97.1 with rustfmt and clippy, Windows MSVC/SDK, Python 3.9+ and Git are
prerequisites. The installed Rust toolchain is used explicitly; the script does
not install one. `cargo fetch --locked` prepares the pinned Cargo dependencies.
Every private checkout builds into its own `target/`; no cache is redirected or
linked to another checkout.

The configured checks are Rust formatting, the complete default Rust test suite
through `test_survey.py` (8 workers, ignored tests excluded), Clippy with warnings
denied, the hook's source checks over every index blob, Python syntax and tooling
regressions, domain maps, committed whitespace,
and the isolated PowerShell temporary-directory and installer regressions.
`WGPU_BACKEND=gl` is the default for Windows headless rendering, matching the
existing visual harness. An explicit caller value is preserved; individual
harnesses can still select GL themselves. Follow the renderer and serial-run guidance in
`docs/development.md` if the driver fails. Snapshot-update environment flags are
removed, and survey reports are preserved under `logs/test-survey/` before cleanup.
Native branding, window and profile-migration
checks remain separate release gates; ignored SDK/benchmark tests run when their
prerequisites and task apply. See `release-workflow.md` and `live-testing-workflow.md`.

Allow more than 30 minutes for a cold build; the command reports each running
step and its log directory. Exit 0 means all checks passed (or, for a push, the
remote already contained that commit); 1 means failure; 2 means invalid arguments.
`--json` emits one final result document with the checked hash, step outcomes and
log paths instead of live progress.

## Fix failures

Read each failing step's complete log. Reproduce its smallest failing test or
checker, fix the cause, run the affected checks, and commit the repair. A failure
still blocks when a rerun passes; investigate the first failure. Follow
`worktree-workflow.md` for repairs that need an isolated working tree.

Run the complete command again after fixes. Failures in dependency setup skip
dependent Rust steps; independent checks still run. Changes to tracked files
made by a check also fail the run.

## Push the checked commit

After the user has authorized the push:

```powershell
python scripts/push.py
```

The command fetches `origin/main` before validation. If the remote has commits
local `main` lacks, nothing is checked or pushed; report the divergence instead
of pulling or force-pushing. A remote without `main`, such as a new empty
repository, has nothing local `main` lacks; the push creates the branch. It checks the remote again before pushing the exact
validated hash as a fast-forward. Commits added to local `main` meanwhile remain
for the next run. A failed transport can leave the remote outcome unknown; inspect
the remote before retrying.

This repository has no hosted CI workflow. Report the actual local checks and
pushed hash; do not claim a green CI run. If CI is introduced, add its concrete
monitoring procedure here and keep its checks aligned with `scripts/push.py`.
