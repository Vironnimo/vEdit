# Interaction polish — Morrow 0.4.13

## Explorer and sidebars

Each sidebar has one active panel. Explorer, Search, Source control and Review
can be assigned independently to either side; panels assigned to the same side
switch through tabs. Existing sessions migrate their assignments and widths.
Switching panels restores keyboard focus, including text entered in the same
frame as a shortcut. Hidden Search panels retain their query.

The Explorer has its own lazy directory cache. Opening a folder reads its
complete contents on a background worker, including ignored and generated
files. The visibility checkbox and index-limit banner have been removed.
Quick Open and project search retain their separate indexing policies and
resource limits. VCS metadata remains hidden.

Folder-first ordering, outlined icons, 16-pixel indentation, subtle branch
guides and full-width 26-pixel rows clarify the tree. Painting is virtualized.
Refresh retains existing rows; failed reads expose Retry. At most eight folder
reads per project run concurrently. Generations reject completions from stale
refreshes and closed projects. Immutable snapshots preserve navigation after
renames without copying large parent listings.

## Windows scaling

The bundled winit 0.30.13 has a Windows-only patch for titlebar drags between
monitors with different scaling. During an interactive move, WM_DPICHANGED
uses Windows' suggested physical rectangle directly, preventing the old
monitor-nudge feedback loop. Other resize behavior is retained.

The patch, upstream evidence and upgrade guidance are documented in
[MORROW_PATCH.md](../vendor/winit/MORROW_PATCH.md). Four regression tests cover
both directions, negative monitor coordinates, repeated transfers and the
noninteractive path. Native titlebar dragging still needs verification on the
user's monitor arrangement after restarting the new build.

## Editing and Vim

- Alt+Up/Down moves a line or selected block and preserves the Vim selection,
  cursor and undo history. Tab insertion uses the next indentation stop.
- Unicode case changes support `~`, `gu`, `gU`, `g~` and Visual mode `u`/`U`.
  Additional motions include `ge`, `gE`, `g_`, `+`, `-`, `_`, Enter and `|`.
  `gJ` joins without inserted spaces; `J` correctly handles CRLF input.
- Ex substitutions support current, whole-document, numeric, relative and
  Visual line ranges; captures and flags; repeat commands; and one-step undo.
  Unsupported interactive confirmation is rejected before any edit. Patterns
  use Rust regex syntax, documented in the README.
- Ex buffer navigation, splits, editing/saving paths and settings return focus
  to the resulting document. Command history retains edited drafts. Batched
  Enter/Escape and subsequent source text preserve their original order.
- Find handles asynchronous query changes and queued navigation; Shift+Enter
  moves backward. Vim search history, Unicode word deletion, bookmarks and
  unique screen-jump confirmation received matching input fixes.
- Shortcut recording supports additional keys and invalid input recovery;
  AltGr evidence protects text input without swallowing ordinary shortcuts.
  Git navigation and refresh indicators retain stable geometry and focus.
- The project picker clamps its scroll position before painting so visible
  rows remain stable under the pointer.

## Validation and packaging

Run `cargo test --bins -- --test-threads=2`,
`cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all --check`.
GUI fixtures include dense trees at native 100% and 125% scaling, compact
sidebars, keyboard focus transitions and asynchronous filesystem races.
The final run passed 1,155 editor tests and 48 CLI tests; twelve explicitly
manual or isolated tests remain ignored. Clippy with warnings denied and the
format check passed. Forty-nine GUI reference images cover the editor's views.

`scripts/build-release.ps1` builds and packages the editor, CLI, language tools,
documentation and licenses. When an older editor is running from a build path,
its locked executable is preserved in the same folder without closing its
window or documents; failed builds restore the original path when necessary.
Restarting Morrow is required to activate the new executable and DPI patch.
