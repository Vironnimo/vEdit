# Live Testing Workflow

Use this workflow to check an actual vEdit executable, native windows or a CLI
change against an isolated editor. Headless tests need no live user session.

## Prefer the existing isolated checks

Build from the current checkout, keeping its own `target/`:

```powershell
cargo build --locked --bins
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows.ps1
```

The window script uses private projects and `VEDIT_PROFILE_DIR`, checks startup,
forwarding, independent windows and recovery, and cleans up only its recorded
fixture processes. Pass `-ExecutableDirectory dist/vEdit` to check a packaged build.
For migration use `scripts/test-profile-migration.ps1`; it redirects `APPDATA`
because a profile override deliberately bypasses migration.

Executable CLI round-trip tests use a headless editor and private endpoints.
`docs/development.md` -> CLI integration fixtures documents the individual
ignored commands and `VEDIT_CLI_TEST_BINARY`. Run the relevant fixture after
building that binary, rather than sending a test to the user's current window.

## Start an interactive fixture

For a behavior the existing fixture cannot show, load
`.agents/skills/vedit/SKILL.md` before controlling the editor. Create fixture files
under a unique `.tmp/<task>/` directory and pass the same absolute
`VEDIT_PROFILE_DIR` to both the GUI and every CLI invocation. That override
isolates recovery, eframe settings, control discovery and instance locks;
`VEDIT_CONTROL_DIR` alone does not isolate persisted user data.

A scoped PowerShell launch from the checkout:

```powershell
. ./scripts/project-temp.ps1
$liveTemp = Enter-ProjectTemp
$livePrevious = @{}
foreach ($key in @('VEDIT_PROFILE_DIR', 'VEDIT_CONTROL_DIR', 'MORROW_CONTROL_DIR')) {
    $livePrevious[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
try {
    $liveRoot = Join-Path $liveTemp.Root ('native-review/' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $liveRoot -Force | Out-Null
    $env:VEDIT_PROFILE_DIR = Join-Path $liveRoot 'profile'
    Remove-Item Env:VEDIT_CONTROL_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:MORROW_CONTROL_DIR -ErrorAction SilentlyContinue
    $liveExecutable = (Resolve-Path 'target/debug/vedit.exe').Path
    $liveCli = (Resolve-Path 'target/debug/vedit-cli.exe').Path
    $liveProcess = Start-Process -FilePath $liveExecutable -ArgumentList '--reuse-window' -WorkingDirectory $liveRoot -WindowStyle Hidden -PassThru
    Write-Output "Fixture: $liveRoot; profile: $env:VEDIT_PROFILE_DIR; PID: $($liveProcess.Id)"
    # Run the check here. Every CLI call uses this environment and --window main.
    # After startup, verify that status.recovery_directory is inside this profile.
    # Close the fixture normally before leaving this scope.
} finally {
    foreach ($key in $livePrevious.Keys) {
        [Environment]::SetEnvironmentVariable($key, $livePrevious[$key], 'Process')
    }
    Exit-ProjectTemp $liveTemp
}
```

An editor can still be starting after `Start-Process` returns. Use the operating
skill's CLI status and completion checks; do not interpret startup or a queued
open/save as completion. Keep the printed profile path if later shell calls are
needed and explicitly set that same value again for each call.

`cargo run --release` without this environment starts or forwards into the real
profile. A CLI call after restoring the environment can also target the real
editor. Keep fixture operations inside the scoped environment or reapply its
explicit profile before continuing.

## Inspect and clean up

Check only the changed behavior: for example native dialogs, window chrome,
multiple windows, shell forwarding, Snap or physical monitor-DPI transitions.
Headless snapshot evidence does not prove these desktop behaviors. Store captures
and logs below the fixture directory and describe which checks were manual.

Close fixture windows normally when finished. If a broken fixture requires forced
cleanup, verify its private endpoint, executable path and recorded process start
before stopping that exact process. Never stop all `vedit` processes by name.
Use the existing native scripts when a test needs several spawned windows and
automated cleanup; their ownership checks already cover those processes.
Retain failure logs and remove only a verified fixture directory inside `.tmp/`.
