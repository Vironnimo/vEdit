# Platform

Platform code selects and owns an editor window/profile, imports legacy state and
connects the custom titlebar to the maintained Windows backend.

## Overview

- `src/main.rs` orders startup and creates the eframe window. `src/startup.rs`
  parses GUI arguments, selects/forwards to windows and restores interrupted ones.
- `src/profile.rs` is the authority for profile/window paths and the selected
  window ID. `src/single_instance.rs` owns per-window locking;
  `src/process_liveness.rs` supplies conservative process checks.
- `src/profile_migration.rs` imports old profiles before window startup. It is an
  `app` child because it reads the session model; ordinary document recovery stays
  in `documents.md`.
- `src/window_chrome.rs` lays out and paints the titlebar. `src/app.rs` guards
  close requests and publishes its geometry through the patched `vendor/winit`.
- `vendor/egui-winit` preserves keyboard provenance before clipboard translation;
  the editor input owner uses it to route configurable Ctrl chords to Vim.
- `vendor/eframe` presents the first Glow frame before showing the native window;
  see `vendor/eframe/VEDIT_PATCH.md` before changing startup rendering or upgrading it.
- Endpoint discovery/forwarding lives in `src/cli_transport.rs`; see `control.md`.
  Packaging, installation, registry integration and embedded executable resources
  belong to `development.md`.

## Interfaces

The default profile is `%APPDATA%/vEdit/recovery`. `main` uses that directory;
additional window IDs use its `windows/<id>` children. `profile::select_window`
sets process-local identity once, and recovery/eframe storage/locks derive from
it. `VEDIT_PROFILE_DIR` relocates the complete profile under its `recovery`
child and bypasses legacy migration. `VEDIT_CONTROL_DIR` only relocates control
endpoints; it does not isolate recovery or window settings.

GUI startup validates the environment and migrates profiles before choosing a
window. `startup::prepare` either forwards paths to the selected live window or
returns paths together with a held `SingleInstanceGuard`. Keep the guard alive
for the window lifetime. Windows mutex names are scoped to the recovery path;
the non-Windows implementation is a stub, not equivalent locking support.

The custom chrome returns logical client-coordinate geometry and a close request.
The app converts hit regions using current pixels-per-point, publishes physical
coordinates to winit, and routes closing through the dirty-document close flow.
Native code owns drag/resize/Snap/maximize pointer behavior; egui owns rendering,
ordinary minimize/close controls and keyboard/accessibility activation.

## Constraints & Gotchas

- A process appearing alive is not proof that it owns a particular old PID file.
  `process_liveness::running` deliberately treats uncertainty/access denial and
  reused PIDs conservatively, keeping potentially live files.
- Independent windows have separate sessions and recovery. GUI reuse chooses the
  newest focus timestamp; ambiguous CLI targeting is a different policy owned by
  `control.md`. Shared external-open preferences must not silently retarget CLI.
- `vendor/winit` is selected through Cargo's patch section. Read
  `vendor/winit/VEDIT_PATCH.md` before changing native behavior or upgrading it;
  the copy includes hidden startup, DPI, titlebar and Explorer-drop changes.
- `vendor/egui-winit/VEDIT_PATCH.md` owns the keyboard/clipboard event contract.
  Preserve both the original key and adjacent clipboard event when upgrading it.
- A headless titlebar image cannot verify native Windows Snap Layouts, physical
  monitor transitions or Explorer drops. Preserve native checks alongside pure
  geometry tests when replacing backend behavior.

## Agent-facing text

GUI command help in `src/startup.rs` is also read by agents. Before changing its
sentences, read `control/agent-facing-text.md` and keep its window/argument
semantics aligned with startup and control routing.

## References

Read these only when your task matches.

- Changing window startup, profile layout, crash restoration or legacy import
  -> `platform/profiles-and-migration.md`
- Changing startup rendering, custom chrome, native hit testing, DPI, Explorer
  drops or native input
  -> `platform/native-window.md`
- Changing endpoint/window selection or command help -> `control.md`
- Changing runtime command wording -> `control/agent-facing-text.md`
- Changing session or draft serialization -> `documents.md`
