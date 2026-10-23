# Input ordering and edit history

## Ordered input

`src/editor_input.rs::take_command` lets TextEdit consume the native prefix of
an event batch before an application edit acts on it. Remaining events continue
on a later frame, ahead of newly arrived keyboard input. `defer`,
`resume_editor_input` and `finish_editor_input` bind that continuation to the
resulting source owner.

The owner includes pane/document identity, revision, cursor/selection, mode,
focused widget, bindings and view. External edits, navigation or changed context
cancel stale input rather than replaying it into another document. Vim barriers
finish native editing before Escape, history or clipboard operations observe the
buffer. Modifier presses alone are not native text that requires a deferred
command.

`src/keybindings.rs` has a separate continuation for global/widget shortcuts;
`src/split.rs` has one for region focus. These serve different ownership
transitions. Preserve their coordination in `src/app.rs::draw_ui` rather than merging
the queues based only on their similar shape.

Normal and Visual source modes must reject native editing/IME commits while
still allowing real mouse selection. Dialogs and popups own their input before
source editing runs. In Insert/native mode, an asynchronous save does not freeze
typing; save completion compares against the snapshot that was actually written.

## History boundaries

`VimState` owns document history; `VimNavigation` captures the pane-local modal
state. Finish an active Insert transaction before switching views, then restore
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

## Regression entry points

- `src/editor_input_tests.rs`: mixed text/command batches, remapped save/history,
  canceled suffixes after external edits, generated Alt text, AltGr, line edits
  and shared Undo across panes.
- `src/native_input_tests.rs`: native IME, Copy/Cut, modal blocking, mouse
  selection and live typing while a save is pending.
- `src/vim_clipboard_tests.rs`: application-level register/clipboard integration.
- `src/editor_ux_tests.rs`: exact Undo before replace/replace-all and completion
  popup event ordering; `src/split_tests.rs`: pane navigation and shared content.
- `src/editor.rs` inline tests cover Unicode, raw CRLF, empty/final lines and
  selections ending at the next line start. Use these for pure operation changes
  before a widget test when the behavior crosses both layers.
