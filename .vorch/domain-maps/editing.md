# Editing

The editing domain transforms text and coordinates native and Vim input while
preserving the document's edit history and each pane's navigation.

## Overview

- `src/editor.rs` contains pure editing operations returning `EditResult`.
  Shared search entry points delegate to the search owner; see `search.md`.
- `src/vim.rs` owns modal state and the `handle_key` transaction returning an
  `Action`. Private units in `src/vim/` implement command decoding, navigation,
  operators, paste/surround edits, history/insert replay and text geometry.
  The public owner remains `vim`; it does not own pane placement or app commands.
- `src/multicursor.rs` owns directed selection sets and simultaneous text edits;
  `src/multicursor_ui.rs` owns their input, pointer/layout integration, preferences
  and application transactions. Active multicursor editing is a separate input
  context even when Vim is enabled.
- `src/editor_input.rs` orders native TextEdit input around application editing
  commands. App's `src/vim_input.rs` and `src/source_editor.rs` integrate `handle_vim`, the source widget and
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
pane navigation snapshot, including multicursor selections; `restore_navigation`
preserves shared history. Do not
clone a whole `VimState` into each pane to preserve its cursor. Layout ownership
and parked view states are in `projects.md`.

Multicursor replacements refer to one original text snapshot and commit together.
Normalize selections before editing; do not apply cursor edits successively to
an already changed buffer. The primary selection is the last entry, not
necessarily the last one in document order.

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
- Vim's private units share the document's `VimState`; moving navigation logic
  does not give panes separate histories or registers. `handle_key` keeps the
  before/after transaction and repeat bookkeeping in the owner.
- `src/multicursor_ui.rs` remains a review hotspot; preserve its boundary with
  the pure simultaneous-edit operations in `src/multicursor.rs`.

Pure operation regressions live inline in `src/editor.rs` and
`src/multicursor.rs`; Vim contracts are grouped under `src/vim/tests/`, with shared
input helpers in `src/vim/tests.rs`. Indentation regressions are inline in
`src/indentation.rs`.
`src/multicursor_tests.rs` covers the multicursor UI and history integration.
Use the integration tests below when an edit crosses the widget or history boundary.

## References

Read these only when the task matches.

- Changing input routing, native/Vim Undo, IME, clipboard handling or shared
  editing history, including multicursor transactions
  -> `editing/input-and-history.md`
- Changing commands, remapping or shortcut hints
  -> `application/commands-and-settings.md`
- Replacing text from disk or changing reload/restore behavior
  -> `documents/saving-and-recovery.md`
