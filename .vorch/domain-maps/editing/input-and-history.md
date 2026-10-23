# Input ordering and edit history

## Ordered input

`src/editor_input.rs::take_command` lets TextEdit consume the native prefix of
an event batch before an application edit acts on it. Remaining events continue
on a later frame, ahead of newly arrived keyboard input. `defer`,
`resume_editor_input` and `finish_editor_input` bind that continuation to the
resulting source owner.

The owner includes pane/document identity, revision, cursor/selection, multicursor
state, mode, focused widget, bindings and view. External edits, navigation or changed context
cancel stale input rather than replaying it into another document. Vim barriers
finish native editing before Escape, history or clipboard operations observe the
buffer. Modifier presses alone are not native text that requires a deferred
command.

`src/keybindings.rs` has a separate continuation for global/widget shortcuts;
`src/split.rs` has one for region focus. These serve different ownership
transitions. Preserve their coordination in `src/app.rs::draw_ui` rather than merging
the queues based only on their similar shape.

Ordinary Vim Normal and Visual source modes must reject native editing/IME
commits while still allowing real mouse selection. Active multicursor input uses
its own handler and permits text/IME editing independently of Vim mode. Dialogs
and popups own their input before source editing runs. In Insert/native mode, an
asynchronous save does not freeze typing; save completion compares against the
snapshot that was actually written.

## History boundaries

`VimState` owns document history; `VimNavigation` captures the pane-local modal
and multicursor state. Finish an active Insert transaction before switching views, then restore
navigation without replacing shared Undo/Redo. Counted insertion can change text
when it finishes, so merely observing the current buffer's dirty bit is not
always enough before a reload or navigation transition.

`src/context_actions.rs` binds native TextEdit undo history by document while
retaining each widget's cursor. Pane selection and scroll stay with `split.rs`.
The native checkpoint/history
helpers keep external edits reversible even when the relevant pane is not
currently drawn. `EditorTab::apply` handles the Vim side; callers that bypass
TextEdit must also use the appropriate native checkpoint path.

`src/disk_sync.rs::replace_with_disk_version` is the existing model for an
undoable disk replacement: checkpoint both histories, replace content, and map
every pane's navigation. The ordinary initial/manual load path in `app.rs` is a
different lifecycle and can reset history; do not substitute it for automatic
disk synchronization.

Clipboard payload and Vim register semantics differ. Native Copy/Cut uses the
selection or current source line; a Vim linewise register can have a synthetic
final newline with no corresponding source span. Preserve those source spans for
feedback and selection rather than treating register text length as a range.

## Multicursor transactions

`src/multicursor.rs::State` preserves directed anchor/head selections and creation
order while merging overlaps. Adjacent nonempty selections remain separate;
coincident carets do not cause duplicate edits. Offsets still count Unicode
scalars, while unmodified horizontal movement and deletion use grapheme
boundaries, including CRLF and combining characters. Line commands in
`multicursor_ui::edit_command` group affected lines before applying shared editor
operations so several cursors on one line do not indent/comment it repeatedly.

`commit_multi` checkpoints text and selection state as one edit transaction,
using Vim snapshots or the native history companion records. Text Undo/Redo
restores the full selection set, even after exiting multicursor mode. Native
restoration follows history direction rather than picking the newest matching
text: identical text can occur with different secondary cursors. Cursor Undo is
separate selection-only history; remapping through a text edit clears that
selection history. Ordinary explicit `VimState::set_cursor` or selection-boundary
navigation clears multicursor state; pane restoration carries it through
`VimNavigation` instead.

During keyboard cursor placement the last selection is a movable cursor and the
preceding selections remain marked. Keep coincident marked/movable selections
until placement finishes; then merge before any edit so a shared position is
edited once. Cursor Undo restores this placement state. Finish/exit are contextual
registry commands, with current bindings shown in the status bar. Plain Enter
finishes placement by default without inserting a newline; typing also finishes
and edits all selections. The cursor limit rejects the entire finish/edit if the
new unique selection would exceed it.

`multicursor_keys` consumes the ordered source prefix before global shortcut
dispatch. While active, TextEdit supplies layout and pointer selection without
also mutating text, and ordinary Vim handling yields. Activation after native
typing uses the existing continuation, whose owner includes the complete
multicursor state. Pointer placement and wrapped-row movement wait for geometry
at the current text revision. A pending context-menu paste additionally checks
pane, document, revision and multicursor state before insertion.

With spread-paste enabled, one clipboard line per selection is distributed in
document order; otherwise each selection receives the full text. Copy/Cut uses
each nonempty selection, or unique current lines when every selection is empty.
Language completion starts at the primary cursor and repeats only where the
same source context exists; auxiliary server edits apply once. Mismatched or
overlapping completion edits fail without changing text. The language-service
boundary remains in `language-services.md`.

## Regression entry points

- `src/editor_input_tests.rs`: mixed text/command batches, remapped save/history,
  canceled suffixes after external edits, generated Alt text, AltGr, line edits
  and shared Undo across panes.
- `src/native_input_tests.rs`: native IME, Copy/Cut, modal blocking, mouse
  selection and live typing while a save is pending.
- `src/vim_clipboard_tests.rs`: application-level register/clipboard integration.
- `src/multicursor.rs` inline tests: simultaneous Unicode replacements, overlap
  merging, atomic CRLF deletion and paste order. `src/multicursor_tests.rs`:
  native/Vim text history, repeated identical text states, selection Undo,
  independent pane selections, ordered input, pointer/box selection, IME,
  grapheme deletion, line-command grouping, wrapped-row geometry and completion.
- `src/editor_ux_tests.rs`: exact Undo before replace/replace-all and completion
  popup event ordering; `src/split_tests.rs`: pane navigation and shared content.
- `src/editor.rs` inline tests cover Unicode, raw CRLF, empty/final lines and
  selections ending at the next line start. Use these for pure operation changes
  before a widget test when the behavior crosses both layers.
