# Application

The application domain integrates feature state, background results, commands and
the egui frame without becoming the owner of every feature it displays.

## Overview

- `src/app.rs` owns `VEditApp`, shared application types, worker dispatch and
  `poll`, frame orchestration, the command palette and `command_action`.
  Its `#[path]` declarations make many flat `src/` files child modules with
  access to this state; check those declarations before changing visibility.
- `src/keybindings.rs` owns command identities, shortcut defaults and overrides,
  matching, recording, dispatch integration and visible shortcut labels.
  `src/app_menu.rs` owns the main menu and status-bar composition.
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
`platform.md`. The definitions being in `app.rs` do not move those responsibilities.

## Interfaces

`WorkerResult` and `poll` are the integration boundary for many background jobs.
Dispatch captures the owning identity; completion checks the identity that can
have become stale before publishing. Use the existing feature's keys: for
example index root/generation/request, search root/generation/query/excludes,
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

Settings and sessions use serde defaults and compatibility aliases in `app.rs`.
Additions must survive old saved sessions and mark persistence dirty where the
existing owner does so. Layout restoration belongs to `projects.md`; recovery
publication belongs to `documents.md`.

## Constraints & Gotchas

- Commands have several entry points. A registry entry alone does not implement
  palette, menu, keyboard and source-editor behavior. Keep `keybindings.rs`,
  `command_action`, applicable menus and README behavior aligned.
- Notifications are session-only and do not change focus or open documents on
  receipt. The user activates their file/review action from the inbox. Identical
  unread messages coalesce; read messages can start a new entry.
- `app.rs` (~9,700 lines) and `keybindings.rs` (~1,300 lines) are review hotspots.
  Their size includes integration and tests; it is not proof of a defect. Keep
  changes with the existing feature owner, and preserve the above boundaries if
  extracting cohesive responsibilities.

## References

Read these only when the task matches.

- Adding a command, changing shortcuts, settings persistence, notification UI
  or status-message details
  -> `application/commands-and-settings.md`
- Reordering input phases, modal handling, source focus or native/Vim history
  -> `editing/input-and-history.md`
- Changing document worker completion, save or recovery publication
  -> `documents/saving-and-recovery.md`
