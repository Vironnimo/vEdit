# Development

The development domain owns repository checks, build and test infrastructure,
packaging and installation tooling; product behavior stays with its feature owner.

## Overview

`Cargo.toml` defines the GUI and control-client binaries, dependency features and
the `vendor/winit` and `vendor/egui-winit` patches. `Cargo.lock` fixes dependencies;
`rust-toolchain.toml` pins the compiler and its formatting/lint components.
`tools/basedpyright/package-lock.json` pins the bundled Python language server.
The application backend patches belong to `platform.md`, not a dependency cleanup.

Tests mostly live beside their owner or in app child modules named
`src/<feature>_tests.rs`. `tests/project_temp.rs` checks the Cargo subprocess
environment. `tests/snapshots/` contains visual references, not a native desktop
test suite. `docs/development.md` is the human-facing command and packaging guide.

## Interfaces

The repository-local Git hooks call `scripts/commit_authorship.py` and
`scripts/commit_check.py`. The static checker reads staged blobs and leaves both
the index and working files unchanged. It runs Rustfmt 1.97.1 through stdin for
each staged Rust file, compiles staged Python syntax without writing bytecode,
parses staged JSON, checks staged whitespace, and validates domain-map references
and prose. TOML parsing uses `tomllib` on Python 3.11+; older supported Python
versions report the omitted TOML check. Vendored files are excluded from these
per-file source checks. This hook neither installs tools nor runs application
tests or Clippy. A format failure requires an intentional edit and staging.

`scripts/push.py` validates committed `main` in a unique private Git checkout.
Its steps prepare locked Cargo dependencies, check Rust formatting, run the
default Rust test survey, deny Clippy warnings, compile Python syntax, run Python
tooling regressions, check domain maps and whitespace, and run the isolated
PowerShell temp-directory and installer checks. It checks the exact revision it
pushes; `--no-push` performs local validation without contacting a remote. There
is no hosted CI configuration. The procedure and failure handling belong to
`.vorch/workflows/push-workflow.md`.

`scripts/test_survey.py` builds test executables through Cargo, discovers their
stable libtest names and ignored cases, then runs each ordinary test separately.
The normal command has eight workers and a 30-second per-process test deadline;
it writes per-test logs, counts, durations and the slowest 20 into a unique
`.tmp/test-survey/` run. This bounds a hanging test on stable Rust, whose
`--report-time` requires nightly. Direct Cargo tests retain their shared-process
harness and do not acquire this deadline or per-case environment isolation.

`scripts/worktree.py` owns marked `.worktrees/<name>` checkouts on `codex/<name>`;
Codex app-managed checkouts have their own lifecycle. The script installs nothing
at creation and allocates no ports. Cargo builds on demand into that checkout's
own `target/`. Its merge lock protects script operations; the checked landing
commit advances `main` only after the commit hook passes. Use
`.vorch/workflows/worktree-workflow.md` for ownership, cleanup and merge limits.

## Constraints & Gotchas

`.cargo/config.toml` and `scripts/project-temp.ps1` set temporary paths and Git's
discovery ceiling to the checkout's `.tmp/`. The direct Cargo integration test
asserts this exact path. The survey preserves it while adding private per-test
home directories and vEdit profiles. These are environment boundaries, not an OS
sandbox; see `development/testing.md` for the precise guarantees and omissions.

`scripts/build-release.ps1` owns the complete portable package and running-binary
preservation. `scripts/package-manifest.ps1` hashes its payload into the root
`package.json`; `scripts/install.ps1` verifies and installs that payload. Do not
replace these procedures with one copied executable. Installer regression tests
redirect their files and registry writes; installation itself changes the user's
registration and application files.

Headless rendering still requires a working graphics driver. In particular,
`src/visual_tests.rs::harness` explicitly requests OpenGL. The survey and push
runner default to `WGPU_BACKEND=gl`, preserving explicit caller overrides.
A backend environment value does not prove that every harness honors it.
Preserve driver crash evidence and use the
documented serial fallback rather than accepting changed snapshots.

## References

- Writing or changing a test, selecting a regression, or interpreting survey
  results -> `development/testing.md`
- Preparing a runnable package or installation -> `.vorch/workflows/release-workflow.md`
- Starting an isolated editor or checking native behavior -> `.vorch/workflows/live-testing-workflow.md`
- Changing the patched window backend or its dependency -> `platform.md`
