# Developing vEdit

Commands and source paths in this guide require the source checkout. The portable
application includes this guide and the installer, but not Rust sources or development scripts.

The source checkout pins Rust 1.97.1, rustfmt and Clippy in `rust-toolchain.toml`.
Use Rustup with the Windows MSVC build prerequisites. Git must
be on PATH for repository functionality and its integration tests. The normal
test suite uses temporary fixtures and headless egui harnesses; it does not need
to control a running vEdit window.

Temporary work belongs in `.tmp/` in the checkout, including test fixtures,
logs, screenshots and agent helper scripts. Its contents are Git-ignored; the
tracked `.gitkeep` makes the directory available in fresh clones. Cargo sets
`TEMP`, `TMP` and `TMPDIR` to this absolute path for builds, tests and child
processes, overriding inherited system Temp paths. Run Cargo from this checkout
or one of its subdirectories so it loads `.cargo/config.toml`.

The PowerShell build and test scripts use the same directory and restore the
calling process's environment on exit. For other tools or direct test binaries:

```powershell
. ./scripts/project-temp.ps1
$projectTemp = Enter-ProjectTemp
try {
    # Run the tool here; store task-specific files under .tmp/<task>/.
} finally {
    Exit-ProjectTemp $projectTemp
}
```

Both configurations also stop Git's parent-directory discovery at `.tmp/`, so
an uninitialized test fixture cannot select the source repository. Normal
per-fixture cleanup remains in place. `cargo clean` leaves `.tmp/` intact.

vEdit adds `.tmp` to rust-analyzer's `files.exclude` in the resolved initialization
and workspace settings, including existing profiles. This keeps transient test
files out of the live editor's Rust analysis while tests run in this checkout.
Custom exclusions are retained. A stopped server's status message and Problems
entry open copyable details with its exit status and retained stderr.

`cargo test --test project_temp` checks Cargo's paths and Git isolation;
`powershell.exe -NoProfile -File scripts/test-project-temp.ps1` checks PowerShell
child processes and environment restoration.

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

## Repository workflow

The user guide is `README.md`; `docs/README.md` indexes current documentation and
historical reports. Coding agents read `AGENTS.md`, `.vorch/PROJECT.md` and the
relevant domain maps. The live-editor skill remains a separate operating guide.

Python 3.9+ and Git 2.38+ support the repository tools. Enable the local hooks once
per clone with `git config --local core.hooksPath .githooks`. The hooks validate
staged content without rewriting the worktree or index; full Clippy and tests are
separate checks. Details and commands live in `.vorch/domain-maps/development.md`.

Use Codex's managed worktree tools in Codex, or `python scripts/worktree.py create
<task-name>` in a terminal. Read `.vorch/workflows/worktree-workflow.md` for merging
and cleanup. Each checkout owns its dependencies, profile fixtures and `target/`;
never connect worktrees with junctions. Local commits do not authorize a push.
For an authorized push follow `.vorch/workflows/push-workflow.md`.

`python scripts/test_survey.py --jobs 8` builds and runs all non-ignored Rust tests
with individual deadlines and a top-20 timing report below `.tmp/test-survey/`.
Each test gets a separate process and isolated user profile; timings include that
startup and fresh caches. This is distinct from the shared-process `cargo test`
commands above. The testing guide documents the isolation limits and existing
ignored SDK/manual cases: `.vorch/domain-maps/development/testing.md`.

## Visual references

`src/visual_tests.rs` creates headless renders against `tests/snapshots/`.
Behavioral UI tests also live in feature-specific `*_tests.rs` files. WGPU rendering
requires a usable graphics backend even though it does not open a native window.

On this Windows setup, a parallel headless test run produced a native crash in
the NVIDIA OpenGL driver (`nvoglv64.dll`). An earlier suite passed serially and
with Direct3D 12 requested explicitly. The 0.4.18 run reproduced that driver crash
even with this environment variable, so use the serial fallback below if the
headless renderer still selects OpenGL. In the 0.6.0 integration run, the new
multicursor harness failed during device creation with DX12 requested, while GL
passed against its unchanged reference. The survey and push tools therefore
default to GL, preserving explicit caller overrides. For a backend-sensitive
test, use a scoped choice (`dx12` remains an alternative for OpenGL driver failures):

```powershell
$previousGpuBackend = $env:WGPU_BACKEND
try {
    $env:WGPU_BACKEND = 'gl'
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
$env:VEDIT_SNAPSHOT_DIR = Join-Path $PWD '.tmp/snapshot-candidates'
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
cargo test --bin vedit live_rust_analyzer_hover_references_and_symbols -- --ignored --nocapture
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
the repository vEdit skill, all Markdown documentation under `docs/`, visual
references and third-party licenses. Repository maps and developer scripts remain
source-checkout material.

The maintained [window backend patch](../vendor/winit/VEDIT_PATCH.md),
[keyboard input patch](../vendor/egui-winit/VEDIT_PATCH.md) and
[startup presentation patch](../vendor/eframe/VEDIT_PATCH.md) retain their upstream
licenses and patch notes in the bundle's `licenses/` directories. Review the
corresponding notes before upgrading these dependencies.

The startup presentation regression opens and closes its own small Windows
window, using private storage under `.tmp/startup-flash/`. Run it on a Windows
desktop with OpenGL. It checks normal and maximized startup, rejects uncloaked
native show requests before app construction, and reads the first visible front
buffer synchronously. A headless snapshot cannot verify these stages:

```powershell
cargo test --test startup_window -- --ignored --nocapture
```

A running editor is preserved under a process-specific filename if it locks an
output binary. Its window and documents remain open; it continues using the old
code until the user closes and starts vEdit again. Do not terminate it to force a
build. On each run the script deletes `vedit-running-<pid>.exe` backups in
`target/release/` and `dist/vEdit/` whose process no longer exists; a backup
stays while any process with that PID runs (also a reused PID), and a backup that
cannot be deleted only produces a warning. Verify both packaged binaries and relevant copied files against the build
outputs. `target/`, `dist/`, local backups and the shortcut are ignored by Git.

## User installation and windows

`powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install.ps1`
builds with `build-release.ps1`, verifies the package hashes and installs for the
current user in `%LOCALAPPDATA%/Programs/vEdit`. Repeating it updates that
installation. The packaged `scripts/install.ps1` installs its containing package;
`-PackagePath PATH` uses an extracted package without building. No administrator
rights or repository files are required to run the installed application.

The installer registers HKCU capabilities, OpenWithProgids, App Paths, a Start
menu shortcut, an uninstall entry and the user PATH. It preserves existing
extension defaults and raw expandable PATH entries. It broadcasts environment
and association changes; already running agent/terminal processes retain their
inherited environment. Windows owns the actual default-app choice.
`-OpenDefaultApps` opens `ms-settings:defaultapps?registeredAppUser=vEdit`.
Registration follows Microsoft's [Default Programs](https://learn.microsoft.com/en-us/windows/win32/shell/default-programs)
and [Open With](https://learn.microsoft.com/en-us/windows/win32/shell/how-to-include-an-application-on-the-open-with-dialog-box)
documentation. The bundled icon uses the same geometry/colors as the window icon.

Replacement files are staged alongside their destinations. Renamed running
executables remain alive; failed file replacement rolls earlier replacements
back. Locked runtime changes can require closing vEdit normally before retrying.
The installation marker tracks owned files; uninstall refuses while an installed
process runs, removes owned files and registration, and retains user profiles and
unknown files. Installer validation uses a directory under `.tmp/` and a redirected
registry subtree, never the live installation:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-install.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows.ps1 -ExecutableDirectory dist/vEdit
```

`startup.rs` parses shell arguments and obtains the selected window's lock before
loading any session. Main keeps the legacy single-instance lock and profile;
additional windows use `recovery/windows/wN` with separate storage, recovery,
control endpoints and locks. `window-open` survives abnormal exits; a main-window
startup restores interrupted additional windows. A normal close removes the
marker but retains the explicitly restorable saved session. `external-open.json`
is a shared launch preference; only explicit changes write it. Other preferences
are window-local after initial inheritance. CLI operations always target a
specific window, ignoring the external-open preference. The prefix `--window ID`
is parsed before the command, preserving flag-looking literal command payloads.
`windows` reports running IDs/projects and saved IDs; status includes its window
ID and recovery directory. More than one live window without an ID is an error
before dispatch. GUI launches reuse the window with the newest focus timestamp.

For executable integration tests, set `VEDIT_PROFILE_DIR` to an isolated absolute
fixture directory. Both recovery and eframe storage then live below that directory,
and the main lock is scoped to it. `VEDIT_CONTROL_DIR` remains an endpoint-only
override, with per-window subdirectories; it does not isolate recovery by itself.
Never use the user's profile for integration fixtures or terminate their editor.

## vEdit branding and compatibility

The product, binaries, CLI help, skill and package use vEdit (`vedit` for command
and crate names). Version 0.4.17 adopts the shared dark colors from vBot's
`webui/src/styles/app/foundation.css` and Watchtower's `ui/app.css`.
`src/palette.rs` owns the editor palette; green success/diff additions are
independent of the orange action/selection accent.

Since 0.5.4, all active state lives below `%APPDATA%/vEdit/recovery`,
including the main window's `app.ron`; additional windows use `windows/<id>`.
`src/profile_migration.rs` imports the former `Morrow/recovery`,
`morrow-editor/data` and `vedit-editor/data` locations once, before any window
starts. It adjusts recovery references and saved task variables, preserving
buffer bytes, comparison baselines, formatting, geometry and independent windows.
Original directories move to `vEdit/migration-backup` after a staged copy is ready.
A journal resumes interrupted publication; malformed or conflicting profiles
fail without replacing either original. Live older editors must close normally
before migration. `VEDIT_PROFILE_DIR` bypasses migration entirely.

Normal runtime locks and newly written recovery headers use vEdit. The old lock
is checked only during migration; the reader still accepts the old record header
to recover existing drafts. Task processes export `VEDIT_ROOT`, `VEDIT_FILE` and
`VEDIT_PYTHON`. `VEDIT_CONTROL_DIR` selects a control endpoint directory; using
only the obsolete `MORROW_CONTROL_DIR` fails instead of targeting another profile.
Historical review/evaluation results retain their original product names and versions.

`build.rs` embeds `assets/vedit.ico` plus product/version metadata in both Windows
executables using `winresource` and the Windows SDK resource compiler. Shell
shortcuts use the unquoted executable path followed by `,0`; registry icon
locations retain their normal quoted path syntax. The running window uses the
same design. Validate resources and actual Shell icon resolution (including a
shortcut) with `powershell.exe -NoProfile -File scripts/test-branding.ps1`.
Pass `-ExecutableDirectory` and `-ShortcutPath` to check the installed files.
Profile migration regressions run with `cargo test profile_migration`.
`powershell.exe -NoProfile -File scripts/test-profile-migration.ps1` exercises
the actual startup import and recovery in isolated `APPDATA`, including a second
window and a repeated launch. It accepts `-ExecutableDirectory` for release tests.

## Text statistics

`src/text_statistics.rs` counts ordinary text with the bundled `o200k_base`
vocabulary from `tiktoken-rs`. It makes no network requests. A 250 ms debounce
precedes copying the current buffer; at most one background calculation and one
cached buffer snapshot are retained. Counts are matched to document ID, revision
and selection before display. Moving the cursor does not count again; changing
only the selection reuses the full-file count. The line index supplies Unicode
scalar counts, including raw CRLF from agent edits. Tokenizer licenses are in
`licenses/tokenizer/` and are copied into the portable bundle. Render fixtures
wait for both syntax and token workers before taking snapshots.

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

Source control offers a compact selector when the active repository has multiple
worktrees. Cached rows list the main working tree first; selecting a row defers
the project switch until the frame has drawn, preserving documents and pane state.
Unopened worktrees open as projects; missing or bare entries cannot be selected.
The project picker remains an alternative entry point.

Git history freezes traversal roots and remote reachability within each page
generation. The sidebar traverses only HEAD and the configured upstream, retaining
merged ancestry and tags on reachable commits. Other branch decorations are hidden.
The full snapshot remains available to the branch switcher and repository-wide CLI
queries. Upstream configuration participates in snapshot identity so changing it
also refreshes the graph, without requiring an additional Git process to discover it.
While Source control is visible in the focused, non-minimized window,
the active repository is polled every three seconds after the previous read.
Changed snapshots automatically replace the loaded history depth; unchanged ones
keep the graph and pending page requests. Scroll position follows the first visible
commit where it remains in the loaded range, while the top follows new commits.
Old page results cannot enter the new generation. History failures retry on the
next visible poll; hidden histories catch up when shown again. Periodic Git reads,
including project indicators and worktree discovery, pause in inactive or
minimized windows; explicit operations can still complete there. Regaining focus
or restoring the window bypasses the polling interval and refreshes the active
repository immediately. Worktree discovery and unopened-worktree status reads run
while Source control is visible or the project picker is open. A pending read receives one follow-up;
an active Git action delays the read until it completes. Background reads do not
show activity markers or disable actions. Starting an action invalidates pending
status results for that repository; optional index locks are disabled, so those
read workers can finish without delaying the action. The subsequent refresh
replaces the last complete view. History paging keeps the viewport geometry and
scroll offset unchanged, without an activity footer or reserved bottom gap.
The graph reserves its left lane for HEAD and its first-parent ancestry, using
the frozen refs even when HEAD is on a later page. Pending tracks keep separate
lanes and colors until the shared commit row; paging does not rewrite earlier rows.
The branch picker uses the latest refreshed refs; a switch runs without force or automatic stash
and returns to the CLI after completion. The Changes/History divider is saved per
project. Hover timestamps use the author time zone and a relative age.
