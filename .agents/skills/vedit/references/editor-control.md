# Buffer edits and selections

Submit structured actions with `request FILE.json`; `schema OP` gives focused examples. JSON files accept UTF-8 with/without BOM and UTF-16 with BOM. Write files in PowerShell with `Set-Content -Encoding utf8`. Request paths resolve against the working directory; review paths resolve against the manifest directory. In the examples, replace IDs, revisions and paths with current task values.

## Inspect and edit a buffer

`tabs` returns actual tab IDs and current revisions; `read TAB_ID --line N --count N` returns text from the open buffer, including unsaved changes. IDs last for the app process. Text coordinates use 1-based lines and Unicode scalar columns, including spaces and tabs. Edit and selection ends are exclusive; review annotation ends are inclusive.

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

When a file changes on disk while its buffer has unsaved edits, `tabs` reports `disk_conflict:true`. `read`, `find`, `diff`, `selection`, `edit`, `replace` and `save_as` keep working. `save` returns the error `disk_conflict` and writes nothing, because it would replace the newer disk version. Do not retry it: save the buffer elsewhere with `save_as`, or ask the user to choose **Use disk version** or **Keep my version** in the editor banner; after **Keep my version**, `save` replaces the disk file. A buffer without unsaved edits follows the disk version as one Undo step, so `command undo` restores the previous text.

`selection` without a range inspects the current selection. To set one, supply `range` and `expected_revision`. A zero-length range sets the cursor. `command` applies a typed editor action with `tab_id` and `expected_revision`: undo/redo, selection, indentation, comments, line duplication/deletion, reload, find navigation, or Source/Preview. Inspect the returned revision after any text-changing command.

`find` locates literal text with case/word options and bounded results. It leaves the GUI search and cursor unchanged; use a returned range with `selection` to reveal a match. The `find_next` and `find_previous` editor commands use the visible GUI search query. `replace` requires the current revision and supports one or all matches. `diff` compares the open buffer with its saved state and returns one contiguous replacement hunk with bounded before/after text. `diagnostics` reports the language server's diagnostics for the specified tab, using exclusive range ends. A `location_clamped` item indicates an outdated or out-of-range diagnostic position. Check language-server readiness and truncation before interpreting an empty or partial result.

Buffers belong to their project. `tabs` lists buffers across all projects and identifies each `workspace`; reading, finding, editing, replacing, or saving an inactive project's buffer leaves the visible project selected. `focus TAB_ID`, setting a selection, and editor `command` actions select that buffer's project and restore its saved panes before revealing it. `workspace DIRECTORY` adds or selects a project and restores its saved layout.

When switching projects to focus a tab, vEdit prefers a pane already displaying it, keeping the focused pane if it matches. Otherwise it uses the focused pane containing the tab, then another containing pane, or adds the tab to the focused group. Omitted `view` preserves that pane's Source/Preview choice; an explicit view changes only the chosen pane. Use layout `focus` to select a particular pane when the same buffer has several views.
