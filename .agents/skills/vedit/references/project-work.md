# Bookmarks and project work

Submit structured actions with `request FILE.json`; `schema OP` gives focused examples. JSON files accept UTF-8 with/without BOM and UTF-16 with BOM. Write files in PowerShell with `Set-Content -Encoding utf8`. Request paths resolve against the working directory; review paths resolve against the manifest directory. In the examples, replace IDs, revisions and paths with current task values.

## Keep persistent bookmarks

`bookmarks` lists bookmarks across projects, including files that are currently closed. Lists default to 100 items; `bookmarks list --limit N --offset N` accepts 1–500 items per page. Follow `next_offset` while `has_more` is true. Items contain ID, path, line, column, and label, without document text. Each bookmark has its own persistent ID; obtain it from this list rather than using a tab ID. `bookmarks toggle --tab TAB_ID --label "Review this flow"` toggles the current cursor line in that named file. Omit `--tab` to use the active document and omit `--label` for the default label. To mark a specific location, first focus that tab at the desired line, then toggle:

```json
{"op":"bookmarks","change":{"action":"toggle","tab_id":42,"label":"Review this flow"}}
```

`bookmarks rename ID --label "Follow up"` changes the label; an empty label clears it. Labels are single-line text of at most 256 UTF-8 bytes. `bookmarks jump ID` opens or focuses that file and reveals the bookmarked location; inspect status if file loading is still pending. `bookmarks remove ID` removes only that bookmark. The corresponding JSON actions are `rename` with `id`/`label`, and `jump`/`remove` with `id`, all inside `change`.

Bookmarks preserve their IDs across app restarts and survive closing their files. They capture the cursor and line context without editing or saving the document. Toggle is not idempotent: toggling an existing bookmark removes it. After uncertain delivery, inspect `bookmarks` before deciding whether to repeat it.

## Inspect project work and run tasks

`search` reads saved files during a bounded background workspace walk; it does not require a completed Explorer index. Its optional `workspace` queries a registered project without selecting it. Use `find` on a tab to search unsaved text. Search inherits the persistent `settings.search_excludes` list; optional JSON `excludes` replaces it for one request, while `[]` removes only these optional filters. The CLI equivalent is repeated `--exclude PATTERN`, or `--no-excludes`; those flags cannot be combined. `.gitignore`, VCS directories, symlink guards and search limits still apply, so clearing filters does not guarantee every disk file is searched. The response identifies the actual `excludes`, `excludes_source` (`settings` or `request`), truncation, warnings and files searched.

For example, search generated code without changing the user's saved preferences:

```json
{"op":"search","query":"needle","workspace":"C:/project","excludes":[],"limit":100}
```

To persist exclusions, supply `{"op":"settings","patch":{"search_excludes":["node_modules","src/generated","**/*.min.js"]}}`. Rules use gitignore folder/glob syntax, match case-insensitively on Windows, and support literal names beginning with `#`. Whitespace is trimmed, blank entries dropped, duplicates collapsed and Windows separators normalized to `/`. Negation with `!`, controls and malformed globs are refused before any setting changes. Limits are 128 input rules, 1024 bytes per rule and 16 KiB total. If saved filters are invalid, correct them or supply a valid per-query override; `search_busy` means wait for running searches. Changing persistent filters cancels stale GUI results and reruns its current query.

`git_status` lists cached changed paths and reports cache age, loading state, and errors. `task` starts `check`, `test`, `build`, or `run_file` using the active project’s configured commands and source language (Python, Dart or Rust defaults), or stops a running task. Python uses the project interpreter shared with analysis. Task dispatch refuses unsaved or saving buffers; save relevant changes only if the user’s task includes saving them. Missing commands for other languages are configured in Settings → Tasks. `output` reads bounded task output; task dispatch alone does not mean it finished. Inspect task state and output for completion and errors.

## Manage project files

`file` renames a file/folder, creates a folder, or moves an item to the Windows Recycle Bin within a registered workspace. Use `workspace DIRECTORY` to add/select the project if needed. Rename and folder creation take a new basename and refuse replacement. For example:

```json
{"op":"file","action":{"action":"rename","path":"C:/project/draft.md","name":"plan.md"}}
```

To delete a file or folder, use `action:{"action":"delete","path":"C:/project/obsolete.txt"}`. The item goes to the Windows Recycle Bin; restore it there to undo. Unsaved buffers and agent drafts under that path block deletion.

To create a folder, use `action:{"action":"create_folder","parent":"C:/project","name":"plans"}`. These requests start background operations. Use the returned `job_id` with `file_job` and require `complete:true`, `applied:true`, and no error before relying on the destination or retargeted documents. Up to four jobs can be pending; the most recent 32 are retained in the current app process. Retain the actual job ID; after expiration or uncertain delivery, inspect files and current app state before attempting another mutation.

## Ordered work and recovery

`batch` runs its `actions` in order and stops on error by default. Use it for actions whose IDs and revisions are already known and remain valid after earlier actions. Results preserve completed steps; it is not a transaction, and returned IDs cannot be substituted into later actions automatically. Nested batches, background project search, file operations and asynchronous Git branch/history/switch requests are excluded. For dependent edits or newly opened files, inspect the preceding result before constructing the next request.

An open/review can be partially successful; retry failed opens only, or send a deliberate complete replacement review. Files stay with their existing open buffer's project, otherwise the deepest registered project containing the path; an unmatched path opens a project for its parent directory. Successful files appear in that project's tab group. `open` leaves the last successful file's project visible; `review` reveals the first successful file's project. A review can span projects: it retains all successful buffers and marks, and `focus TAB_ID` reveals an item from another project in its saved layout. Inspect `tabs` for ownership and `status`/`layout` for the final visible views; an item's success does not mean every reviewed file is visible at once.

A review replaces earlier annotations across projects, so preserve items that remain relevant. After an edit, old annotations become stale; submit updated ranges against the current buffer if marks are still needed. An active user dialog, Vim search, or tab drag can block mutation; inspect the reported interaction and wait for the user to finish.

## Git branches and history

`git-branches [WORKSPACE]` lists local and fetched remote branches; `git-history [WORKSPACE]` reads the history shown by the editor, including `on_remote`, author date/time and Unix timestamp. Both accept `--limit` and `--offset`; continue with `next_offset` while `has_more` is true. Remote status reflects the last fetch.

`git-switch-branch REF [WORKSPACE]` takes the exact `ref` from `git-branches`, waits for Git, and returns the resulting branch. A remote ref creates a local tracking branch. Unsaved buffers and Git conflicts refuse the switch without discarding changes. Omit WORKSPACE for the active project. Run these commands separately from batches.
