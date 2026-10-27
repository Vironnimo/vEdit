# Application

The application domain integrates feature state, background results, commands and
the egui frame without becoming the owner of every feature it displays.

## Overview

- `src/app.rs` owns `VEditApp`, construction and frame orchestration.
  Its `#[path]` declarations make many flat `src/` files child modules with
  access to this state; check those declarations before changing visibility.
- `src/app_workers.rs` owns worker execution, `WorkerResult` and `poll` routing;
  feature modules apply their results beside the requests they complete.
- `src/command_registry.rs` owns command identities, defaults and contexts;
  `src/keybindings.rs` owns overrides, matching, recording and visible hints.
  Its private `src/keyboard_settings.rs` child renders alias editing, conflict
  reassignment and command filtering over that same state.
  `src/application_commands.rs` executes typed actions; `src/command_palette.rs`
  retains those actions independently of displayed labels.
- `src/app_menu.rs` owns the main menu.
  `src/app_chrome.rs` renders surrounding workspace chrome and the status bar;
  `src/app_overlays.rs` orders settings, close and diff dialogs.
- `src/settings_navigation.rs` routes settings sections;
  `src/editor_preferences.rs` owns reading preferences and their UI.
  Feature-specific settings remain with indentation, language, task and sidebar
  owners rather than being reimplemented in the settings router.
- `src/notifications.rs` owns the session inbox and its presentation. Its CLI
  entry point and response contract also cross `control.md`.
- `src/status_messages.rs` owns copyable status details and their stable modal
  snapshot. Language-server failures originate in `language-services.md`.

Document lifecycle is in `documents.md`, text/input semantics in `editing.md`,
pane/project ownership in `projects.md`, and window startup/chrome in
`platform.md`. App child modules integrate those owners without replacing them.

## Interfaces

`WorkerResult` and `poll` are the integration boundary for many background jobs.
Dispatch captures the owning identity; completion checks the identity that can
have become stale before publishing. Use the existing feature's keys: for
example index project instance/generation/request, search root/generation/query/excludes,
or document ID and its load/save state. A surviving project path alone does not
prove a result belongs to a reopened project.

`worker` catches panics and reports them. Use `worker_or` for jobs whose pending
state must be released on failure: its failure closure returns the matching
error result, so the normal completion path clears loading, saving or other
pending flags. Sending a result normally requests a repaint; empty periodic disk
checks deliberately do not wake an idle window.

`draw_ui` orders worker/control polling, input routing, feature rendering and deferred
actions. Event order and focus are behavioral contracts: drawing an overlay late
in the frame does not by itself prevent earlier source handlers from receiving
its keys. Read `editing/input-and-history.md` before moving input phases.
The hover-details frame begins before application/source input and ends after
language hover rendering, before deferred input owners finish. This preserves
the selectable details card's input ownership; see `rendering.md`.

Settings and sessions use serde defaults and compatibility aliases in
`src/app_session.rs` (sidebar aliases remain on `SidebarView` in `app.rs`).
Additions must survive old saved sessions and mark persistence dirty where the
existing owner does so. Layout restoration belongs to `projects.md`; recovery
publication belongs to `documents.md`.

## Constraints & Gotchas

- The exhaustive action dispatcher serves shortcuts, palette and command menu
  items behind existing input gates. Source-editor input ordering still belongs
  to `editor_input.rs` and `workspace_shortcuts.rs`; registry additions must
  preserve those gates, menu availability and the user manual behavior.
- Git completion uses `GitActionKind` in `src/git_actions.rs` for follow-up work;
  translated or revised display labels must never select behavior. Snapshot and
  history request/completion guards live in `src/git_refresh.rs`.
- Notifications persist in the window's session and do not change focus or open
  documents on receipt. Read-state and retention rules belong to
  `application/commands-and-settings.md`.
- App modules remain methods on the same application state. A file split does
  not introduce a second document, session or worker owner. Keep shared-document
  and pane-local state separate when changing those boundaries.

## References

Read these only when the task matches.

- Adding a command, changing shortcuts, settings persistence, notification UI
  or status-message details
  -> `application/commands-and-settings.md`
- Reordering input phases, modal handling, source focus or native/Vim history
  -> `editing/input-and-history.md`
- Changing document worker completion, save or recovery publication
  -> `documents/saving-and-recovery.md`
