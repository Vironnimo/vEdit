# Release Workflow

Use this workflow when the user asks for a release. A release is complete when
all intended changes are committed on GitHub, the version tag identifies that
checked commit, and a published GitHub Release offers the complete Windows ZIP,
its checksum and release notes. Install the same package when requested.
The release request authorizes these push and publication steps; no second
permission question is needed.

An explicit request for only a local build or installation uses the package and
installation sections without publishing. `scripts/build-release.ps1` performs
only the local build; its successful exit is not a published release.

## Prepare the source and version

1. Inspect local changes, outstanding task worktrees, local `main`, the remote
   and existing GitHub Releases/tags. Use the repository configured by `origin`.
   Finish and commit the intended changes and merge completed task work before
   selecting the release commit. Do not silently omit pending changes or publish
   unfinished concurrent work; resolve its scope with the responsible session.
2. Choose an unused version. If the current version was already published and
   there are new changes, normally increment the patch version. Never move a
   published tag or replace its assets with a different build. Continue an
   interrupted draft only after checking its target and existing assets.
3. Keep `Cargo.toml`, the vEdit entry in `Cargo.lock`, the user manual version,
   changelog, README claims and relevant developer documentation consistent.
   Preserve historical reports. Describe the user-visible changes since the
   previous published tag in the release notes; include install/update commands,
   prerequisites and the running-session restart behavior.
   The welcome screen displays the package version, including behind Open recent.
   Run the affected visual tests after a version bump, inspect their differences,
   and update only the references changed by that version display. Keep the final
   verification free of snapshot-update flags.
4. Commit the version and documentation changes through the normal hooks, and
   land them on `main`. Build from clean `main`; record its full commit hash.
   The checked, built, pushed and tagged commit must be the same. If source changes
   during this workflow, reconcile them and repeat the affected build/checks
   before publishing; do not attach an older package to a newer commit.
5. Confirm pinned Rust with MSVC/Windows SDK resource tools, Git, Python, Node,
   npm and authenticated GitHub CLI (`gh`). Read `vendor/winit/VEDIT_PATCH.md`
   before changing the native backend. Keep scratch files below `.tmp/<task>/`
   and use `scripts/project-temp.ps1` for non-Cargo tools.

## Build the portable package

From clean `main` in the source checkout:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/build-release.ps1
```

The script builds both release binaries into this checkout's `target/`, prepares
`dist/vEdit/`, includes the language runtime, documentation, vEdit operating
skill, assets and licenses, writes the hashed root `package.json` manifest, and
creates the checkout's `vEdit.lnk`.

If an existing GUI executable is locked, the script preserves it under a
PID-specific filename. Its process and Documents stay open on the old code.
Stale preserved executables are removed only after their PID no longer exists.
Never terminate the user's editor for packaging or claim that a running process
has adopted the new code. Restart remains a normal user action.

Check the manifest, executable resources and manual against the source version.
Compare both packaged executable hashes to `target/release/`, and relevant copied
contents to source. Verify every manifest payload hash. The manifest excludes
itself and preserved running-binary backups; those backups must not enter an
archive. Deliver the complete runtime and documentation with the binaries.

## Validate the package and push the source

Run affected regressions and the complete checks required by `AGENTS.md` for
source changes. The push command below performs the full default Rust test survey,
formatting, Clippy and repository checks on the exact commit it pushes. No
snapshot-update flags belong in a final check. See `development/testing.md` in
the domain maps for test isolation and the Windows renderer caveat.

Run the native package gates with private fixture state:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-branding.ps1 -ExecutableDirectory dist/vEdit
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows.ps1 -ExecutableDirectory dist/vEdit
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-profile-migration.ps1 -ExecutableDirectory dist/vEdit
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-install.ps1
```

These checks cover resources/icons, executable startup/windows, migration and
isolated installation. They do not establish physical DPI transitions, native
dialogs or Snap; check those interactively when affected.

Push through the existing owner, following `push-workflow.md`:

```powershell
python scripts/push.py
```

Wait for successful completion. Record its checked hash and require it to match
the package's recorded source commit. Verify that the remote `main` contains
that commit and that local `main` has no remaining commits to push. If remote
history diverges, follow the push workflow; do not force-push. A successful push
alone does not complete the release.

## Archive and publish on GitHub

1. In a fresh `.tmp/<task>/` staging directory, copy only the manifest-listed
   package files plus root `package.json`, preserving their relative paths.
   Reject paths outside the package root and verify copied hashes. Exclude
   running-binary backups, `installation.json`, development build output and
   user profiles. Keep the executables and installer at the archive root so the
   documented installation command works from the extracted folder.
2. Create `vEdit-<version>-windows-x64.zip` from this staging directory and write
   `SHA256SUMS.txt` containing its SHA-256 and filename. Reopen the ZIP, check its
   exact file set against the manifest, and verify every archived payload hash.
   Store the notes as UTF-8 in the task directory; pass them with `--notes-file`.
3. Create a GitHub draft with the explicit tag `v<version>`, full source commit,
   notes, ZIP and checksum. Continue the existing prerelease channel unless the
   user requests a stable release. Set the variables below to the repository,
   version, checked commit and actual files prepared above:

   ```powershell
   gh release create $releaseTag $releaseZip $releaseChecksums --repo $releaseRepo --target $releaseCommit --title "vEdit $releaseVersion" --notes-file $releaseNotes --draft --prerelease
   ```

   `--target` binds automatic tag creation to the checked commit instead of a
   moving branch. Do not pass `--verify-tag` for a new tag. Check every command's
   exit status; after uncertain delivery inspect the draft before retrying.
4. Read back the draft and require its intended tag, target, notes and both
   uploaded assets. Compare GitHub's asset SHA-256 digests and sizes to the local
   files; if digests are unavailable, download the assets and hash them. Fix a
   partial upload while it is still a draft; never overwrite a published asset.
5. Publish the verified draft:

   ```powershell
   gh release edit $releaseTag --repo $releaseRepo --draft=false
   ```

6. Read the published release back and require `isDraft:false`, the expected
   version/channel, both assets and their download URLs. Resolve the remote tag
   to its commit (peel annotated tags) and require the recorded source hash.
   Verify remote `main` contains it and no intended local commits remain. Fetch
   that tag locally. A draft, a source-only tag or GitHub's automatic source ZIP
   is not the downloadable Windows release.

The [GitHub CLI release documentation](https://cli.github.com/manual/gh_release_create)
describes tag targets, drafts and asset uploads. Use
[`gh release edit`](https://cli.github.com/manual/gh_release_edit) to publish the
draft after verification.

## Install and deliver

When installation was requested, install the verified package used for the ZIP:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install.ps1 -PackagePath dist/vEdit
```

Do not trigger a second build after publication. The installer validates the
manifest, registers current-user integration and preserves existing file defaults.
Verify the installed version and payload hashes, Start menu target and registration.
Locked runtime changes can require the user to close vEdit normally before retrying;
never terminate it. The packaged installer works without the source checkout.
See `docs/development.md` -> User installation and windows for parameters and limits.

Report the published GitHub Release link, version/tag and commit, package and
installation paths, actual checks and remaining limits. State whether a running
editor still uses old code. If publication or installation failed, name the exact
unfinished step and any already completed remote effects. Never report a release
as complete while its source push, published assets or requested installation is
still missing.
