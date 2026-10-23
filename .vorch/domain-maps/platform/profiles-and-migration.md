# Windows, profiles and migration

## Startup and independent windows

`main` handles help/version before profile work, then validates the CLI
environment, runs migration, calls `startup::prepare`, marks the open window and
starts eframe. `--new-window`, `--reuse-window` and `--window ID` are mutually
exclusive. `--` makes remaining arguments literal paths, including flag-like
filenames. The shared external-open preference affects ordinary launches with
paths; it does not override explicit flags.

`prepare` selects an explicit window, allocates a new `wN` directory atomically,
or chooses the live window with the newest `last-active` timestamp, falling back
to `main`. It tries forwarding before taking the per-window mutex and retries
while a peer is starting/closing, with a bounded wait. An explicit non-main ID
must already have a saved directory. File opening belongs to the normal app
dispatch after the window owner is known.

Every window has `session.json`, recovery content, eframe's `app.ron` and a
`window-open` marker under its own recovery directory. A normal eframe return
removes the marker; an abnormal exit leaves it. Starting `main` restores saved
additional windows only when their marker and session exist and no live endpoint
already reports them. Normal close retains an explicitly restorable saved
session. `external-open.json` is shared across windows and is written only after
an explicit preference change; other preferences become window-local after
initial inheritance.

Use `VEDIT_PROFILE_DIR` with an isolated absolute fixture directory when launching
an executable test. An endpoint-only override is insufficient. Native test
scripts and their invocation are described in `docs/development.md` and the
live-testing workflow; never substitute the active user's profile.

## One-time legacy migration

`profile_migration::initialize` returns immediately for private profiles. Normal
startup uses an APPDATA-scoped migration mutex and checks the old main-window
mutex before importing. `ensure_closed` recursively checks endpoint/marker PIDs
for both old and current recovery trees. Older editors must close normally;
migration never terminates them.

Sources are the former `Morrow/recovery` session tree and `morrow-editor/data`
or `vedit-editor/data` window storage. `migrate_under` prepares a copy below
`vEdit/.profile-migration-stage`, rewrites session recovery references and saved
task variable names, and updates the old recovery record prefix while preserving
the record body. Geometry, formatting and comparison baselines remain part of
the copied data. Transient control endpoints, activity records and dot-temp files
are excluded from the copy.

A synced completion marker is placed in the staged profile; the pending journal
is published atomically before original locations are moved into
`vEdit/migration-backup`. Publication then renames the staged directory into the
new recovery location. Once journaled, a retry resumes publication instead of
rebuilding from partially archived sources. A completion marker prevents a second
import, including silently accepting a reappeared legacy profile.

Conflicting populated profiles, malformed session/storage data, pre-existing
backup destinations and linked/reparse profile paths fail without replacing
originals. Do not replace staged preparation with in-place rewriting or relax
these checks to make a migration "succeed". The normal runtime path resolver has
no fallback to the legacy product name.

## Test entry points

- `cargo test --bin vedit startup::tests`: literal arguments, mode exclusion,
  non-overwriting window allocation and shared-open preference.
- `cargo test --bin vedit profile::tests` and
  `cargo test --bin vedit single_instance`: disjoint storage and native ownership.
- `cargo test --bin vedit profile_migration`: draft/baseline/settings preservation,
  live-secondary-window refusal, conflicts/corruption and interrupted publication.
- `cargo test --bin vedit process_liveness`: current versus exited processes.

`scripts/test-profile-migration.ps1` exercises actual startup import in isolated
APPDATA, including additional windows and repeated launch;
`scripts/test-windows.ps1` checks executable window routing and independent state.
Use `docs/development.md` for their release-directory options and prerequisites.
