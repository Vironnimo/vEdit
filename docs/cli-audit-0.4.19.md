# CLI and operating-skill audit — 0.4.19

The task was to review vEdit's complete agent-facing CLI, its text and its operating skill, and implement useful improvements. The result keeps the existing editor safety rules and JSON clients compatible while making discovery, text input and continuation usable directly.

## Findings fixed

1. Command help returned the same 8,594-character global document for every command. A shared command catalog now supplies focused help and the parser's allowed flags. The global index names each command's effect; per-command help names arguments, omission rules, result meaning and completion conditions. `schema OP` selects typed examples and related rules instead of requiring the full catalog.
2. Windows PowerShell's UTF-16 BOM files were rejected as invalid JSON. File inputs and explicit request stdin now decode UTF-8 with/without BOM and UTF-16 LE/BE with BOM, reject malformed sequences and preserve payload characters and line endings. The encoded and decoded size limits remain enforced. The skill sets console output decoding to UTF-8.
3. Whole-buffer proposals required constructing JSON despite already having a text file. `new --file`, `edit TAB_ID --file --revision`, and `save-as TAB_ID PATH --revision` use the existing application operations. Changes remain drafts; revision, overwrite and save-completion checks remain in force. Range edits remain structured requests.
4. Long buffer/output lines were cut with no usable continuation. `read` and `output` now accept `start_column` (`--column`), return `next_line`/`next_column`, and avoid splitting Unicode scalars or CRLF in byte continuations. Default column 1 is omitted on the wire for older receivers. Read's complete-line separator convention remains compatible.
5. `tabs` exposed `has_more` without `next_offset`. It now returns the next actual item index and uses a 256 KiB target payload budget. Paging follows the number of returned records, including when the byte budget ends a page early.
6. The request-size error told callers to split reviews, although each review replaces the previous annotations. Its recovery text now preserves that distinction. Argument errors explicitly say no request was sent; bookmark rename returns the resulting label.
7. Common workflows were buried in one long reference. The skill now contains four common recipes, a complete command index, completion/retry rules, source-data boundaries and window-only limitations. Conditional references separate buffer edits, layout/settings, marks, project work and reviews. Existing exact IDs, exclusive/inclusive coordinates, draft protection and take/ack recovery rules remain.
8. The schema's batch restriction text omitted asynchronous Git operations, though the validator already rejected them. Help, schema and the project-work reference now describe the actual restriction. Save completion requires clean text as well as no pending save/error.
9. Packaging attempted to overwrite the bundled Node executable while the running language server held it open. The installed and source executables had identical SHA-256 hashes. The packaging script now skips that identical copy; a live-language-server packaging run succeeded without terminating it.
10. The complete suite in a managed worktree exposed an existing render fixture that inherited parent-directory ignore rules and rendered an empty Explorer. The fixture now explicitly includes its checked-in files; all layout assertions remain unchanged. Its targeted regression passes in the managed checkout.

## Text decisions

These are the consequential wording changes; the source diff records the full replacement help and skill text verbatim. A comparison of non-heading paragraph/code blocks found 55 retained verbatim across the old and reorganized skill files, including moved blocks. This is a block count, not a model-behavior score.

- Old: `Split the review into smaller batches.` New: `Reduce its payload or use smaller independent edits. Keep a review's intended annotations together: each review replaces the previous one.` The old advice could silently drop earlier review annotations.
- Old: `No vEdit control endpoint exists. Start vEdit 0.3 or newer, then run the command again.` New: `No vEdit control endpoint exists. The request was not sent. Start vEdit, or use open/workspace to start the sibling executable.` The error now distinguishes a request that was never sent and removes an obsolete version threshold.
- Old skill startup: `$VEditCli = (Resolve-Path '.\dist\vEdit\vedit-cli.exe').Path`. New: `$VEditCli = [IO.Path]::GetFullPath((Join-Path $VEditSkill '../../../dist/vEdit/vedit-cli.exe'))`. The caller supplies the skill's known absolute folder; the portable alternative is `../../vedit-cli.exe`. This works from an unrelated working directory.
- Old: `Query layout after changes to verify local tab membership and the requested documents remain visible.` New: `Use the returned layout to verify tab membership and continue with current pane IDs.` The mutation already returns that evidence; a second read is unnecessary.
- Old: `Nested batches, background project search, and file operations are excluded.` New: `Nested batches, background project search, file operations and asynchronous Git branch/history/switch requests are excluded.` This prevents generating batches the parser refuses.
- Old UI prompt: `Use $vedit to arrange the editor and carry out this task in its open documents.` New: `Use $vedit to carry out this task in vEdit, preserving unrelated open work.` Reading marks or changing text does not inherently require rearranging the window.
- Old description: `Control the user's vEdit editor through its CLI: inspect and edit buffers, receive user-marked snippets, arrange views, and present Markdown plans or reviews. Use when the user wants work performed or shown in vEdit.` New: `Read and edit open buffers, receive marked snippets, arrange views and show reviews in vEdit. Use when the user wants work performed or shown in the vEdit editor.` Scope stays on operating the editor rather than ordinary work on its repository.
- Added recovery rule: `After uncertain delivery or a stopped shell, inspect relevant state before repeating a mutation: tabs, layout, status.file_jobs, bookmarks, or agent-marks batch.` The actual skill formats command names as code. Each recovery query observes the operation in question; it does not prescribe a generic status preflight.
- Added completion rule: `A save is complete when saving:false, dirty:false and error:null.` The actual skill formats field/value pairs as code. An accepted save or a later edit is not evidence that the current text is on disk.

JSON remains the default, and exits 1/2 retain failure/partial meanings. Changing to default plain text and assigning exit 2 to usage errors would break the established repository contract. Scoped discovery reduces routine context without that migration. Existing output budgets and asynchronous application behavior remain; this change adds continuations rather than introducing a general wait/job subsystem. No settings, permanent agent instructions or user-session layout were changed for the audit.

The skill body grows from 4,389 to 9,292 characters because frequent tasks no longer require a reference read. Skill plus references grow from 30,874 to 37,024 characters. Global help shrinks from 8,594 to 3,327 characters; `read --help` is 961 characters. These are normalized Unicode-character counts, not billed tokens; dividing by four gives rough context estimates (body about 1,097 to 2,323 tokens; global help about 2,149 to 832). The total material is larger; no overall token-saving claim is made.

## Skill loading and example review

Static description review, alongside the available repository-development skills:

Expected to load:

- “Zeig mir den Plan in vEdit als Vorschau.”
- “Ändere den offenen Puffer in vEdit und lass ihn ungespeichert.”
- “Bearbeite die Stellen, die ich in vEdit für den Agenten markiert habe.”
- “Zeig Quelltext und Markdown-Vorschau in vEdit nebeneinander.”
- “Öffne in vEdit die Review-Dateien mit Kommentaren.”

Expected to stay unloaded:

- “Baue die CLI von vEdit neu.”
- “Verbessere den Parser in src/cli.rs.”
- “Reviewe den vEdit-Agent-Skill.”
- “Ändere die README dieses Repositorys.”
- “Schreibe einen Plan als Antwort hier im Chat.”

These ten cases match the revised description by inspection. They are not fresh-agent trials. The supported operating harness here is Windows PowerShell/Codex; the executable workflow also covers installed PowerShell 7. No model-specific instructions were introduced. A separate `agent-eval` comparison would be useful for measuring first-use behavior in weaker models; no such model trial is claimed.

## Validation

The initial full suite passed 1,261 tests and failed only the welcome-screen snapshot: the version label changed from 0.4.18 to 0.4.19 (15 pixels). The old image, candidate and diff were inspected before accepting that one baseline. Final results follow below. Regressions first demonstrated rejection of UTF-16, missing direct text-file workflows/scoped schema, and missing long-line continuation. The executable workflow then exposed the missing tab continuation field. All fixtures use test-owned files and a private endpoint; the user's editor is not a test target.

Completed checks on the isolated task snapshot:

- `cargo fmt -- --check`: passed.
- `cargo test --all-targets -- --test-threads=8`, with `WGPU_BACKEND=dx12`: 1,321 passed (1,262 GUI and 59 CLI), 0 failed, 14 intentionally ignored.
- `cargo clippy --all-targets -- -D warnings`: passed.
- Two explicit executable round trips against the packaged release CLI: passed. One covers drafts, edits, stale revisions, tabs, marks, JSON and exit semantics, with Unicode input/output in Windows PowerShell 5 and PowerShell 7. The other covers asynchronous Git branches, switch, paged history and error responses.
- Skill checker: `vedit: description 162 characters, body 123 lines, 5 other files (226 reference lines); no findings`.
- `scripts/build-release.ps1`: passed, producing the complete 0.4.19 bundle with both executables, language tools, documentation and skill references.
- `git diff --check`: passed.

The final integration with completed performance commit `538f9f6` also passed formatting, Clippy and the full suite: 1,335 tests passed (1,276 GUI and 59 CLI), 0 failed, 14 intentionally ignored.

Native dialogs, physical DPI changes and the user's current window are outside this CLI validation. The running editor and an identical bundled Node runtime are preserved during packaging. The editor continues using its old code until the user restarts it normally. Concurrent Git/LSP/rendering work appeared in the shared checkout during this task and is excluded from this commit. Validation uses an isolated source snapshot and a private Cargo output directory, because parallel Cargo builds also shared the original target directory. The completed performance commit `538f9f6` is included in the final integrated build; subsequent unfinished changes remain excluded.
