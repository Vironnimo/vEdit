# Developing vEdit

Commands and source paths in this guide require the source checkout. The portable
application includes this guide for reference, but not the Rust sources or scripts.

Use the stable Windows Rust toolchain with the MSVC build prerequisites. Git must
be on PATH for repository functionality and its integration tests. The normal
test suite uses temporary fixtures and headless egui harnesses; it does not need
to control a running vEdit window.

```powershell
cargo build
cargo test --bin vedit reopen::tests -- --test-threads=8
cargo test --bin vedit-cli cli::transport::tests -- --test-threads=8
```

The latter commands are examples of focused test filters. For a complete check:

```powershell
cargo fmt -- --check
cargo test --all-targets -- --test-threads=8
cargo clippy --all-targets -- -D warnings
git diff --check
```

`cargo run --release` starts the actual editor and can forward files to an
existing instance. Use it intentionally, not as an isolated test command.
`vedit-cli` controls that same live instance unless an isolated test profile has
been configured. Never use the live session for destructive test fixtures.

## Visual references

`src/visual_tests.rs` creates headless renders against `tests/snapshots/`.
Behavioral UI tests also live in feature-specific `*_tests.rs` files. WGPU rendering
requires a usable graphics backend even though it does not open a native window.

On this Windows setup, a parallel headless test run produced a native crash in
the NVIDIA OpenGL driver (`nvoglv64.dll`). An earlier suite passed serially and
with Direct3D 12 requested explicitly. The 0.4.18 run reproduced that driver crash
even with this environment variable, so use the serial fallback below if the
headless renderer still selects OpenGL:

```powershell
$previousGpuBackend = $env:WGPU_BACKEND
try {
    $env:WGPU_BACKEND = 'dx12'
    cargo test --all-targets -- --test-threads=8
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
}
finally {
    if ($null -eq $previousGpuBackend) {
        Remove-Item Env:WGPU_BACKEND -ErrorAction SilentlyContinue
    } else {
        $env:WGPU_BACKEND = $previousGpuBackend
    }
}
```

`--test-threads=1` is another verified fallback. These are test-runtime choices;
the shipped editor continues using its configured Glow renderer. Preserve crash
logs instead of replacing baselines or weakening assertions to obtain a pass.

A deliberate UI change can invalidate snapshots. First run normally, inspect the
reported image differences, then generate candidates separately. For example:

```powershell
$env:VEDIT_SNAPSHOT_DIR = Join-Path $PWD 'target/snapshot-candidates'
$env:UPDATE_SNAPSHOTS = '1'
try {
    cargo test --bin vedit app::visual_tests -- --test-threads=8
    if ($LASTEXITCODE -ne 0) { throw 'Candidate rendering failed.' }
}
finally {
    Remove-Item Env:VEDIT_SNAPSHOT_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:UPDATE_SNAPSHOTS -ErrorAction SilentlyContinue
}
```

Inspect candidates and replace only references justified by the intended change.
Do not regenerate baselines simply to silence a failure. Rerun the normal suite
against the checked-in references, with both variables unset. Custom chrome,
native dialogs and cross-monitor DPI behavior still require native desktop checks;
headless images cannot establish those results.

## Optional integration checks

The ignored tests document their prerequisites beside `#[ignore]`. Run individual
filters only after the relevant tool is available, for example:

```powershell
cargo test --bin vedit live_rust_analyzer_diagnostics_completion_and_definition -- --ignored --nocapture
cargo test --bin vedit live_python_diagnostics_completion_and_definition -- --ignored --nocapture
```

Rust needs `rust-analyzer` and `rust-src`. Python server tests use Node and the
pinned `tools/basedpyright` package; the virtual-environment test also needs Python.
The Dart test reads `VEDIT_TEST_DART_SERVER`. Other ignored tests include task
execution, hidden process-start helpers and manual performance measurements.

## Portable Windows build

```powershell
.\scripts\build-release.ps1
```

The script builds both release binaries and prepares `dist/vEdit/` plus
`vEdit.lnk`. Packaging needs Node and npm on PATH. It installs the pinned Python
language server with `npm ci` when needed, and obtains the license for the bundled
Node version if it is not cached. The bundle includes user/developer documentation,
the repository vEdit skill, visual references and third-party licenses.

A running editor is preserved under a process-specific filename if it locks an
output binary. Its window and documents remain open; it continues using the old
code until the user closes and starts vEdit again. Do not terminate it to force a
build. Verify both packaged binaries and relevant copied files against the build
outputs. `target/`, `dist/`, local backups and the shortcut are ignored by Git.

## vEdit branding and compatibility

The product, binaries, CLI help, skill and package use vEdit (`vedit` for command
and crate names). Version 0.4.17 adopts the shared dark colors from vBot's
`webui/src/styles/app/foundation.css` and Watchtower's `ui/app.css`.
`src/palette.rs` owns the editor palette; green success/diff additions are
independent of the orange action/selection accent.

Existing `%APPDATA%/Morrow/recovery` and `morrow-editor/data` profiles are reused
in place because recovery manifests include absolute paths. Fresh profiles use
`vEdit/recovery` and `vedit-editor/data`. The recovery format signature and Windows
single-instance lock keep their original identifiers so an older running editor
and its unsaved documents remain safe. `VEDIT_CONTROL_DIR` takes precedence over
the accepted `MORROW_CONTROL_DIR` alias. Task processes export both `VEDIT_*` and
`MORROW_*` path variables so saved custom commands continue working. Historical
review/evaluation results retain their original product names and versions.

## Documentation ownership

`README.md` describes current user behavior. `AGENTS.md` provides repository
working instructions. `.agents/skills/vedit/` teaches control of a live editor;
update its references, CLI help and schema together when that interface changes.
`docs/agent-interface.md` separates the current observable contract from historical
evaluations. Versioned review notes retain their original results and scope.

## CLI integration fixtures

The general CLI workflow fixture exercises the executable against a headless editor
and private endpoint, including Unicode input, drafts, revisions, paging, errors and
partial results. It also tests the available Windows PowerShell/PowerShell 7 shells.
It does not contact the user's window:

```powershell
cargo build --bin vedit-cli
$env:VEDIT_CLI_TEST_BINARY = (Resolve-Path 'target/debug/vedit-cli.exe').Path
cargo test --bin vedit cli_agent_workflow_round_trip_in_isolated_editor -- --ignored --nocapture
```

The Git CLI round trip uses its own temporary repository and private control endpoint,
without opening or restarting a real editor. Build the CLI, then run just this check:

```powershell
cargo build --bin vedit-cli
$env:VEDIT_CLI_TEST_BINARY = (Resolve-Path 'target/debug/vedit-cli.exe').Path
cargo test --bin vedit git_cli_executable_round_trip_in_isolated_editor -- --ignored --nocapture
```

The launch CLI fixture uses a private endpoint and temporary projects. It tests
configuration conflicts, an explicit inactive project, a real PowerShell script,
Unicode paths/environment/output, variant selection, Stop and a second start:

```powershell
cargo build --bin vedit-cli
$env:VEDIT_CLI_TEST_BINARY = (Resolve-Path 'target/debug/vedit-cli.exe').Path
cargo test --bin vedit launch_cli_round_trip_in_isolated_editor -- --ignored --nocapture
```

The regular `project_launch` tests also cover Play/Stop UI input, independent
project runs, unsaved buffers, configuration repair, local selection persistence,
bounded rolling output and launch toolbar snapshots. These use isolated fixtures;
they do not run or reconfigure the user's applications.

Git history freezes traversal roots and remote reachability within each page
generation. While Source control is visible in the focused, non-minimized window,
the active repository is polled every three seconds after the previous read.
Changed snapshots automatically replace the loaded history depth; unchanged ones
keep the graph and pending page requests. Scroll position follows the first visible
commit where it remains in the loaded range, while the top follows new commits.
Old page results cannot enter the new generation. History failures retry on the
next visible poll; hidden histories catch up when shown again. Periodic Git reads,
including project indicators and the worktree overview, pause in inactive or
minimized windows; explicit operations can still complete there. Regaining focus
or restoring the window bypasses the polling interval and refreshes the active
repository and worktree overview immediately. A pending read receives one follow-up;
an active Git action delays the read until it completes. Background reads do not
show activity markers or disable actions. Starting an action invalidates pending
status results for that repository; optional index locks are disabled, so those
read workers can finish without delaying the action. The subsequent refresh
replaces the last complete view. History paging keeps the viewport geometry and
scroll offset unchanged, without an activity footer or reserved bottom gap. The branch
picker uses the latest refreshed refs; a switch runs without force or automatic stash
and returns to the CLI after completion. The Changes/History divider is saved per
project. Hover timestamps use the author time zone and a relative age.
