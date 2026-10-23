# Editing

The editing domain transforms text and coordinates native and Vim input while
preserving the document's edit history and each pane's navigation.

## Overview

- `src/editor.rs` contains pure editing operations returning `EditResult`.
  Shared search entry points delegate to the search owner; see `search.md`.
- `src/vim.rs` contains modal operations, motions, registers, repeat and Vim
  history. It does not own pane placement or application commands.
- `src/editor_input.rs` orders native TextEdit input around application editing
  commands. `src/app.rs` integrates it with `handle_vim`, the source widget and
  global command routing. `src/ex_commands.rs` handles Vim command-line actions.
- `src/indentation.rs` derives document indentation and exposes its preferences;
  `src/editor_feedback.rs` provides transient source feedback.
- Native history integration is in `src/context_actions.rs`; shared history and
  local view restoration meet in `src/split.rs` (see `projects.md`).

Disk formats and durable drafts belong to `documents.md`. Shortcut identities and
settings routing belong to `application.md`; drawing and line indexes belong to
`rendering.md`.

## Interfaces

Editing selections and Vim cursor offsets count Unicode scalar values, matching
egui's character cursor. Selection ends are exclusive; user-facing line/column
numbers are one-based. Convert to byte offsets before slicing UTF-8, and use the
language-service conversion boundary for UTF-16 positions.

`EditResult` carries replacement text and the resulting selection. Applying it
through `EditorTab::apply` checkpoints Vim history, sets pending selection and
calls `changed`. `changed` updates the revision, dirty comparison, edit timing
and preview/draft state. Native history needs its own existing integration;
assigning `text` alone is not a complete edit transaction.

The document owns content and edit history. `VimNavigation` is the transferable
pane navigation snapshot; `restore_navigation` preserves shared history. Do not
clone a whole `VimState` into each pane to preserve its cursor. Layout ownership
and parked view states are in `projects.md`.

## Constraints & Gotchas

- Disk loading and native clipboard input normalize endings, but CLI edits can
  contain raw CRLF. Line operations must preserve their applicable raw ending
  behavior and keep `\r` out of line content; include that path when changing
  indentation, deletion, insertion, duplication, movement or selections.
- Native input, commands, IME, clipboard events and later typing can arrive in
  one frame. Preserve their order and modal ownership through the continuation
  helpers; do not process all commands before their preceding text.
- Indentation settings affect later edits without reformatting existing text.
  Detection is bounded and cached by revision/byte length; reloads explicitly
  invalidate it because unchanged length/revision alone does not prove identity.
- `src/vim.rs` (~6,200 lines, including inline tests) is a review hotspot.
  Its navigation/history boundary matters more than reducing line count.

Pure operation regressions live inline in `src/editor.rs` and `src/vim.rs`;
indentation regressions are inline in `src/indentation.rs`. Use the integration
tests below when an edit crosses the widget or history boundary.

## References

Read these only when the task matches.

- Changing input routing, native/Vim Undo, IME, clipboard handling or shared
  editing history -> `editing/input-and-history.md`
- Changing commands, remapping or shortcut hints
  -> `application/commands-and-settings.md`
- Replacing text from disk or changing reload/restore behavior
  -> `documents/saving-and-recovery.md`
