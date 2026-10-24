# Release Workflow

Use this workflow when delivering a runnable Windows update or an installation.
Documentation and repository-tooling changes alone do not require a new package.

## Prepare and validate

1. Keep `Cargo.toml`, `Cargo.lock`, the user manual `docs/handbuch.md`, the landing page
   `README.md` (claims and screenshots) and relevant developer
   documentation consistent. Preserve the historical scope of versioned reports.
   Read `vendor/winit/VEDIT_PATCH.md` before changing the native backend.
2. Run the affected regressions, then the complete checks required by `AGENTS.md`
   for editor-state/UI changes. Use `.vorch/domain-maps/development/testing.md` for test isolation and
   the Windows renderer caveat. No snapshot-update flags belong in the final run.
3. Confirm the local prerequisites: pinned Rust with MSVC/Windows SDK resource
   tools, Git, Node and npm. Packaging uses the pinned basedpyright lock file and
   obtains the bundled Node version's license when it is not cached.

## Build the portable package

From the source checkout:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/build-release.ps1
```

The script builds both release binaries, prepares `dist/vEdit/`, includes the
language runtime, documentation, vEdit operating skill, assets and licenses, writes
the hashed root `package.json` manifest, and creates the checkout's `vEdit.lnk`.
Build output belongs in the checkout's `target/`; temporary work is scoped by
`scripts/project-temp.ps1`.

If the existing GUI executable is running and locked, the script preserves it
under a PID-specific filename. That process and its documents stay open on the
old code. It removes stale preserved executables only after their PID no longer
exists. Do not terminate an editor to force packaging or imply that packaging
updated the already-running process. Restart remains a separate user action.

Check the packaged version and relevant copied contents against the source, and
compare both packaged executable hashes to `target/release/`. The manifest covers
the payload except itself and preserved running-binary backups. Do not deliver a
single binary detached from its accompanying runtime and documentation.

## Validate native and installation behavior

Run the checks relevant to the delivered native surfaces against the package:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-branding.ps1 -ExecutableDirectory dist/vEdit
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows.ps1 -ExecutableDirectory dist/vEdit
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-profile-migration.ps1 -ExecutableDirectory dist/vEdit
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-install.ps1
```

Branding checks executable resources and shell icon resolution; window and
migration checks launch owned fixture processes with private state. Installer
checks redirect files and registry registration to isolated fixtures. These checks
do not justify claims about physical DPI transitions, native dialogs or Snap;
check those interactively when the change affects them.

## Install and deliver

When installation is part of the user's request, use `scripts/install.ps1`.
From the checkout it builds and verifies the package; `-PackagePath` selects an
already prepared package. The packaged installer installs its containing package.
It registers only current-user integration and preserves existing file defaults.
Locked runtime changes can require the user to close vEdit normally before retrying.
See `docs/development.md` -> User installation and windows for parameters and limits.

Report the local commit, package/install path, actual checks and the status of any
already-running editor. Publishing or pushing requires authorization; follow
`push-workflow.md` for a requested push. Do not claim a release or installation
when only source changes were validated.
