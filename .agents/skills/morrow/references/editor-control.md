# Editor control

Use `--help` for ordinary commands and `schema` for current JSON request shapes. Submit a UTF-8 JSON request with `request FILE.json` or `request -` for stdin. PowerShell JSON files written with `Set-Content -Encoding utf8` are accepted, including a UTF-8 BOM. Relative request paths resolve against the CLI's working directory; review manifest paths resolve against the manifest's directory.

## Inspect and edit a buffer

`tabs` returns actual tab IDs and current revisions; `read TAB_ID --line N --count N` returns text from the open buffer, including unsaved changes. IDs last for the app process. Text coordinates use 1-based lines and Unicode scalar columns, including spaces and tabs. **Edit and selection ends are exclusive; review annotation ends are inclusive.**

For example, suppose `read` reports tab 42 at revision 7 and line 5 is `cache_mib = 64`. This request replaces its two digits, preserving the rest of the buffer:

```json
{
  "op": "edit",
  "tab_id": 42,
  "expected_revision": 7,
  "edits": [
    {
      "range": {"start": {"line": 5, "column": 13}, "end": {"line": 5, "column": 15}},
      "text": "96"
    }
  ]
}
```

Use the actual ID, revision, and positions from the current buffer. An edit is an atomic, undoable change: all ranges refer to the original revision and must not overlap. Supply either `edits` or `set_text`, not both. `set_text` replaces the entire buffer, so use it only when the complete desired text is known. A revision conflict applies no edit. Read again and reassess rather than replaying against another revision.

Ordinary Vim Insert is finished as a separate Undo transaction before editing. If finishing a counted Insert would add text beyond the observed revision, `counted_insert_active` applies no change; finish that transaction in the app and read again. A pending save also blocks mutations until it completes.

Agent text mutations stay dirty without autosaving. `save TAB_ID...` explicitly queues a save; verify `saving:false` and `dirty:false` before claiming the disk is updated. `save_as` uses the tab ID, expected revision, and a new path; it does not silently overwrite an existing destination. `new` creates a draft from `text` with an optional title or path; creation alone does not write a file. Close refuses dirty or saving buffers.

`selection` without a range inspects the current selection. To set one, supply `range` and `expected_revision`. A zero-length range sets the cursor. `command` applies a typed editor action with `tab_id` and `expected_revision`: undo/redo, selection, indentation, comments, line duplication/deletion, reload, find navigation, or Source/Preview. Inspect the returned revision after any text-changing command.

`find` locates literal text with case/word options and bounded results. It leaves the GUI search and cursor unchanged; use a returned range with `selection` to reveal a match. The `find_next` and `find_previous` editor commands use the visible GUI search query. `replace` requires the current revision and supports one or all matches. `diff` compares the open buffer with its saved state and returns one contiguous replacement hunk with bounded before/after text. `diagnostics` reports the language server's diagnostics for the specified tab, using exclusive range ends. A `location_clamped` item indicates an outdated or out-of-range diagnostic position. Check language-server readiness and truncation before interpreting an empty or partial result.

## Arrange panes and preferences

`layout` with `change:{"action":"status"}` reports the split tree and current pane IDs. Split the focused pane to display another open tab:

```json
{
  "op": "layout",
  "change": {"action": "split", "orientation": "vertical", "tab_id": 42, "view": "source"}
}
```

`vertical` places panes side by side; `horizontal` stacks them. `pane_id` optionally identifies which pane to split. Each pane owns its tab bar: layout reports its active `tab_id` and ordered local `tab_ids`. The new pane starts with the selected document; opening files adds them to the focused pane. Use returned pane IDs for `focus`, `assign`, `resize`, `close_tab`, and `close` layout actions. `assign` adds or focuses a tab in that pane without removing it from other panes, and accepts an optional view. The same Markdown buffer can appear in Source and Preview at once; text and Undo are shared while pane navigation and scroll positions are separate.

To close only one pane's tab, use the actual IDs from layout:

```json
{"op":"layout","change":{"action":"close_tab","pane_id":2,"tab_id":42}}
```

`close_tab` removes that local tab, leaving the shared buffer in any other pane. If it is the final reference to a dirty or saving buffer, it refuses without effects. Closing the last tab automatically collapses an empty split pane; the sole remaining pane stays available. Ordinary `close TAB_ID...` closes each clean, non-saving document in every pane. Layout `close` closes a whole pane and transfers tabs that have no other pane to a survivor, preserving unsaved work; the last pane cannot be closed. Query layout after changes to verify local tab membership and the requested documents remain visible.

`settings` without a patch returns the current preferences; `patch` changes supplied fields: `vim`, `autosave`, `font_size` (10–24), `tab_width` (1–8), `word_wrap`, and `smooth_scroll`. Null leaves a field unchanged. Changes persist. Do not change unrelated preferences merely to carry out an editor action.

## Inspect project work and run tasks

`search` searches indexed project files on disk; its optional `workspace` selects a known project. The response contains completed, bounded search results. Use `find` on a tab to search an unsaved proposal. `git_status` lists cached changed paths and reports cache age, loading state, and errors. `task` starts the supported `check`, `test`, or `build` action in the active Cargo workspace, or stops a running task. Tasks read files on disk, so save relevant changes only if the user's task includes saving them. `output` reads bounded task output; task dispatch alone does not mean it finished. Inspect task state and output for completion and errors.

## Manage project files

`file` renames a file/folder within its parent directory or creates a folder inside a registered workspace. Use `workspace DIRECTORY` to add/select the project if needed. Both operations use a new basename and refuse replacement. For example:

```json
{"op":"file","action":{"action":"rename","path":"C:/project/draft.md","name":"plan.md"}}
```

To create a folder, use `action:{"action":"create_folder","parent":"C:/project","name":"plans"}`. These requests start background operations. Use the returned `job_id` with `file_job` and require `complete:true`, `applied:true`, and no error before relying on the destination or retargeted documents. Up to four jobs can be pending; the most recent 32 are retained in the current app process. Retain the actual job ID; after expiration or uncertain delivery, inspect files and current app state before attempting another mutation.

## Ordered work and recovery

`batch` runs its `actions` in order and stops on error by default. Use it for actions whose IDs and revisions are already known and remain valid after earlier actions. Results preserve completed steps; it is not a transaction, and returned IDs cannot be substituted into later actions automatically. Nested batches, background project search, and file operations are excluded. For dependent edits or newly opened files, inspect the preceding result before constructing the next request.

An open/review can be partially successful; retry failed opens only, or send a deliberate complete replacement review. A review replaces earlier annotations, so preserve items that should remain. After an edit, old annotations become stale; submit updated ranges against the current buffer if marks are still needed. An open user dialog can block mutation; inspect the reported dialog and leave its decision to the user.
