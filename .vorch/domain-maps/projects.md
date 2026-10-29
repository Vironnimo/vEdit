# Projects and Navigation

This domain assigns open documents to workspaces and presents their panes, file
trees and navigation without taking ownership of document text or disk saves.

## Overview

- `src/project_workspace.rs`, private to App, owns `Project`, file opening,
  project activation and index-result integration. `src/workspace_ui.rs` owns the project tabs, picker and deferred
  worktree selection; Git discovers worktrees (see `git.md`).
  The titlebar's plus button opens the native folder dialog directly; the
  searchable project/worktree picker remains a keyboard and palette action.
- `src/split.rs` owns pane layouts, document views, pane-local navigation and
  focus routing. `src/tab_drag.rs` handles tab gestures;
  `src/spatial_navigation.rs` selects visible regions geometrically.
- `src/other_files.rs` routes loose files and drops. It reuses the document and
  layout lifecycle while withholding folder-based services.
- `src/workspace.rs` owns the Quick Open index; its private
  `src/workspace/explorer.rs` owns the lazy Explorer directory cache. Document
  I/O is a separate private unit (see `documents.md`). `src/fs_watch.rs` reports
  filesystem changes; `src/explorer_tree_ui.rs`, `src/explorer_navigation.rs` and
  `src/explorer_git.rs` own tree presentation, selection and Git decorations.
- `src/jump_list.rs` owns per-pane back/forward history. `src/navigation.rs`
  supplies leader discovery and bookmark UI; `src/bookmarks.rs` manages bookmark
  records/relocation and `src/screen_jump.rs` implements visible-text targets.

File load/save, external changes and recovery belong to `documents.md`. Text
mutation and input ordering belong to `editing.md`. Project content search is a
separate filesystem walk, owned by `search.md`.

## Data Model

`Project.tabs` owns shared `EditorTab` documents. A pane owns an ordered set of
document IDs and per-document navigation/view state. A split is another view of
the same document, including its edit history, rather than a copied buffer.
`VimNavigation` also carries each pane's multicursor selections; transaction and
selection-history rules belong to `editing/input-and-history.md`.
Inactive projects park their `EditorLayout`; switching projects restores that
layout without reopening their documents. Project tab drags reorder the session's
project list without switching the active workspace or its layout; the picker
and cycling shortcuts follow that same order. Gesture handling belongs to
`src/workspace_ui.rs` and its private `src/workspace_ui/project_tab_drag.rs` unit.

The `Other files` workspace uses a synthetic, non-filesystem layout key. Use
`Project::folder_root` or `VEditApp::active_folder` for folder services; do not
pass every `Project.root` to filesystem, Git, language or launch operations.
`file_workspace` first reuses an already open document's owner, then considers
containing projects, then creates/reuses `Other files`. Opening a folder later
does not migrate a previously opened loose document into it.

## Interfaces

- Activate projects through `switch_project`/`set_active_project_context` and
  documents through `activate_tab`. These coordinate layout, pending input,
  search invalidation, focus and active document state. Assigning the active
  indices alone misses those responsibilities.
- `capture_pane_navigation`, `bind_current_pane` and
  `shift_shared_document_positions` bridge a focused `EditorTab` with pane-local
  state. An edit must move other views' cursor/selection positions with the text;
  simply clamping them after a deletion is insufficient.
- Explorer uses `ExplorerTree::begin`/`complete` request tokens. Refresh invalidates
  old requests while retaining visible rows. A completed listing publishes a new
  snapshot identity; immutable directory arrays remain shared. Navigation uses
  snapshot identity when waiting for renamed rows to arrive.
- New location jumps use `record_jump` or `reveal_line_column`. History observes
  document switches and large cursor moves, but small feature-driven jumps need
  an explicit record. History is pane-local and session-only; bookmarks and
  recently closed files have different persistence contracts.

## Constraints & Gotchas

The Explorer and Quick Open deliberately have different completeness contracts.
Explorer reads every ordinary entry in each expanded directory, including ignored
and generated content; only VCS metadata is hidden. Quick Open uses a bounded,
filtered recursive index. Never use its truncation or exclusions to hide Explorer
rows. Explorer may follow a user-expanded linked folder one directory at a time;
this does not authorize recursive search or indexing through links.

Explorer selection is separate from the active document. Context menus and
keyboard actions target the selected row, including when another file is open. Rename and delete
commands use the shared shortcut registry. Explorer, Git rows and project tabs
share the registry's context-menu command; their focused widgets execute it.
These local commands do not run against the source editor or a modal text field.
Rendering and navigation consume cached listings; filesystem enumeration belongs
to the existing worker path. Watch changes must refresh both their appropriate
directory listings and index state without erasing selection or expansion.

Project switching, pane focus changes and tab drags are input boundaries. Text
before the boundary belongs to the previous owner; trailing input must reach the
new owner exactly once. Existing focus helpers also preserve Vim insert undo
groups and prevent picker text from leaking into the editor.

An Explorer preview tab is a replaceable tab, distinct from Markdown's Preview
document view. Editing, saving and keeping/moving/splitting a tab promote it to a
durable tab. Restored session tabs are durable. Use existing close handling so
closing a view does not discard a shared dirty document.

## Verification

Behavioral entry points are `src/split_tests.rs`, `src/project_layout_tests.rs`,
`src/workspace_ui_tests.rs`, `src/other_files_tests.rs`,
`src/navigation_history_tests.rs` and the inline Explorer navigation tests.
`src/explorer_watch_tests.rs` exercises external changes;
`src/explorer_filter_tests.rs` checks the Explorer/index boundary.
`src/workspace/explorer_tests.rs` and `src/workspace/index_tests.rs` cover their
distinct completeness, publication and resource-bound contracts.

For a targeted pane run use
`cargo test --bin vedit app::split::tests -- --test-threads=8`.
Read `development.md` for validation policy and `docs/development.md` before
headless visual checks. User manual sections on projects, loose files and editor
navigation describe the corresponding user contract.
