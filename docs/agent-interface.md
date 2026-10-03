# Agent control of Morrow

The CLI controls Morrow's editor state and presents plans and review targets. The `morrow` skill belongs to this repository at `.agents/skills/morrow/`; portable builds bundle it under `skills/morrow/`. Neither building nor using the application installs a personal skill.

The packaged CLI is `dist/Morrow/morrow-cli.exe`. `--help` documents the ordinary commands; `schema` returns the structured request contract. The skill entry point explains discovery, scope, and verification. `references/editor-control.md` is conditional guidance for buffers, layouts, preferences, user-selected snippets, persistent bookmarks, project queries, tasks, and file operations; `references/review.md` is loaded for marked reviews.

## Observable contract

Version 0.4.12 adds a complete `keybindings` override map and an effective action catalog in Settings responses. Read the current map before changing individual bindings: a supplied map replaces all overrides, missing IDs use their defaults, and `null` disables an action. Unknown IDs, ambiguous shortcuts, reserved native input and collisions reject the entire settings patch. This validation also protects unrelated fields in the same patch.

Indentation settings add `detect_indentation`, `indent_with_tabs`, `language_indent_widths` and `language_indent_with_tabs`. The two maps replace their respective overrides; `{}` clears that map. Detected file style takes priority when enabled. Preferences affect subsequent edits, not existing content. Native edits, Vim and CLI indentation use the same effective document setting.

Language status is per session and document, supporting Python, Dart, Rust and user-configured servers in mixed projects. Diagnostics return the requested document's analysis state; compatibility Rust status fields remain present alongside `languages`. Task actions `check`, `test`, `build` and `run_file` use the active project's configured commands and language/interpreter. A task never silently saves documents or replaces an active process; inspect task/output state to distinguish dispatch from completion. Configure language servers and per-project task profiles in the corresponding Settings pages.

Version 0.4.13 always lists ignored/generated files through complete, lazily loaded Explorer directories, independent of the bounded Quick Open index. `show_ignored_files` remains accepted for older clients but has no effect; Settings reports it as true. Project-search exclusions remain independent. Reading preferences (`ui_scale`, `line_height`, `line_numbers`, `highlight_current_line`), `autosave_delay_ms`, and font sizes 10–32 remain supported. Recovery timing and manual-save protection for agent drafts remain independent of autosave delay.

The panel targets `left_sidebar` and `right_sidebar` show/hide a side without changing assignments. Each side shows one active content panel, selected from its available views, or remains empty. Content targets use their saved side assignment; opening a content panel switches to it, and hiding it removes only that view. `status.panels.sidebars.left.panels` and `.right.panels` report available views, reached through the activity rail or panel menu. The compatibility `panel` field reports the active content, or `null` for an empty side. Side status also reports desired visibility, effective visibility and saved widths; `right_panels` retains saved right-side assignments. `collapsed_panels` is an empty compatibility array: content no longer stacks or collapses. Limited window space can temporarily hide a side without changing the persisted layout. Document IDs, revisions and editor-pane layout retain their contract.

- Buffer edits use current tab IDs and an exact `expected_revision`, validate the full change before applying it, create an Undo step, and pause autosave for that document until a successful explicit save. A stale revision applies no mutation. Normal Vim Insert is finished separately; a counted Insert that would change observed text is refused without effects.
- Editing, selections, search matches, and diagnostic ranges have exclusive ends. Review annotation endpoints are inclusive. All positions use 1-based Unicode scalar coordinates; CRLF boundaries are preserved.
- Each pane owns an ordered tab group. Layout results expose its active `tab_id` and local `tab_ids`, usable pane IDs, split orientations, and ratios. A new split starts with the selected document; new opens join the focused pane. Assigning a document adds and focuses it in the target pane without removing other references. Panes share document text and Undo, while their cursor, view, and scrolling are separate.
- Layout `move_tab` takes source and destination pane IDs, a tab ID, and its final zero-based index after removing any destination duplicate. Equal pane IDs reorder a local group. It validates before effects, focuses the destination, and collapses an empty source split. The source view, cursor, selection, and scrolling move with the tab; an existing destination view retains its own state. Inspect the returned layout after uncertain delivery because a successful move can remove the source pane.
- Layout `close_tab` removes one pane's reference to a document. It preserves a dirty or saving document shown elsewhere, and refuses to remove its final reference. Removing the final local tab collapses an empty split pane; the sole pane remains available. Ordinary `close TAB_ID...` closes clean, non-saving documents in every pane. Closing a whole pane transfers documents absent from the other panes to a surviving pane, preserving unsaved work; the last pane cannot be closed.
- Settings patches affect supplied preferences and persist. Optional `swap_jk` changes motion directions; `search_excludes` changes saved project-search filters. Invalid patches apply no preference changes. File rename/folder creation run as tracked background jobs; task dispatch and file-job dispatch are separate from completion.
- Bookmarks are session metadata across projects and retain their own IDs across restarts. Listing defaults to 100 records, accepts pages of 1–500, and supplies `next_offset`/`has_more`. Records expose path, line, column, and label, without stored document text. Toggle, rename, and removal do not edit or save documents; a jump may still have a pending file load. Toggle removes an existing line bookmark, so callers inspect the list after uncertain delivery before retrying.
- User-selected `agent_marks` are a separate global collection from agent-authored review annotations, ordinary selection, Vim's register, and the system clipboard. Marks capture exact in-memory excerpts with exclusive Unicode coordinates, optional notes, and stable IDs. Unrelated edits rebase their highlights; overlapping or unverifiable reopened text becomes stale while its captured excerpt stays available. Current marks survive file/project closure, and a recovered untitled snapshot cannot claim another buffer's runtime ID.
- `agent-marks` defaults to observation. `take` removes current highlights into one retained receipt, saved atomically before success; failed storage restores the previous live collection and receipt. `ack` is also atomically saved, releases the next take, and retains the old batch until replaced. Same-token retries do not consume new marks; an older expired token is rejected. A missing token is generated and returned. `batch` without an ID inspects the retained receipt after uncertain delivery.
- Marks pages accept 1–500 items and a 256 KiB target budget, with `next_offset` until null. Each item previews at most 4096 characters; `text_next_offset` leads to exact `text` chunks of 1–65536 Unicode characters. Read all pages and complete excerpts before `ack`. Closed sources expose captured text; `source_open` and `range_current` distinguish runtime source availability and verified coordinates. Retained batches always report `range_current:false` because their frozen coordinates do not verify live source positions. Clear/remove retain an undo collection; restore merges IDs without changing documents or overwriting current notes. Other mark mutations queue ordinary session persistence.
- The Vim `leader_key` preference accepts one printable Unicode character or the exact aliases `space`, `comma`, and `backslash`. Applied settings return the actual character. Option values such as a bookmark label containing `--help` or `--no-start` remain literal data rather than being interpreted as global CLI flags.
- Ordered batches stop on failure or partial execution by default and return completed effect evidence plus the stopping point. Large results are compacted explicitly; omitted details remain separately retrievable. Batches do not roll back or replay applied work. Nested batches, background project search, and asynchronous file actions are excluded.
- Project search walks files on disk in a bounded background job independently of the Explorer index; use buffer `find` for unsaved text. Omitted or null `excludes` uses saved filters, while a supplied list replaces them for that request. Responses report the normalized filters and their source. Git status and language diagnostics expose cached state. Diff compares the current buffer with its saved text using a bounded replacement hunk. Truncation and readiness remain observable.
- Opening Markdown with `--preview` selects its reading view. `--source` returns to the editable document and preserves the user's editor preferences.
- `open` and `review` refresh already-open clean buffers asynchronously from disk, preserving unsaved text. A marked file reveals its first annotation unless an explicit location overrides it.
- A review opens several files with notes on important line ranges. A successful new review replaces earlier review annotations, while preserving ordinary open documents.
- Responses provide usable tab IDs and per-item outcomes. Application of a request and completion of an asynchronous file load are distinct; `status` and `tabs` expose the current state for verification.
- An omitted annotation end marks the entire start line; omitted columns default to 1.
- Unknown manifest fields are rejected. File and later load failures can produce partial results, so callers inspect successful items and pending state before continuing.

## 0.4.5 preferences, search and input safeguards

The schema retains **31 operations**. This release extends the existing settings and project-search requests; it does not add an operation. `settings swap-jk true` or a JSON settings patch with `swap_jk` exchanges physical J/K motion directions in Vim Normal and Visual modes, including counts and operators. The preference also applies to Explorer navigation and Markdown scrolling. Inserted text, literal `f`/`t`/`r` targets, arrows and page keys retain their meaning. The default is `false`.

`search_excludes` is a persisted list of folder paths and gitignore-style patterns, such as `src/generated` or `**/*.min.js`. Settings patches validate the entire supplied patch before applying any preference. Rules are trimmed, Windows separators become `/`, and blank or duplicate rules are removed. A leading `#` is a literal filename character; negation with `!`, invalid globs and embedded control characters are rejected. Raw lists are limited to 128 entries, 1,024 UTF-8 bytes per entry and 16,384 bytes in total. Windows matching ignores case. An invalid patch can be corrected and resubmitted without repairing partially applied settings. Changing the saved filters cancels obsolete native project-search work and clears its old results.

The ordinary `search QUERY [WORKSPACE]` command inherits those saved filters. Repeating `--exclude PATTERN` replaces the list for that one search; `--no-excludes` supplies an empty list. The two forms cannot be combined. Structured requests provide the optional `excludes` field: omitted or null inherits preferences, while `[]` removes optional filters for that request. Neither form changes the saved list. Files excluded by `.gitignore`, structural VCS directories and symbolic links remain outside the search even with an empty optional list. Results include `scope: "workspace_files_on_disk"`, effective `excludes`, `excludes_source` (`settings` or `request`), and the existing completion/truncation evidence. Searching an inactive project leaves the visible project and pane layout unchanged. Invalid inherited settings produce `invalid_search_excludes`; callers can correct the settings or supply a valid one-query override.

The GUI exposes explicit focus controls: **Ctrl+Shift+E** selects Explorer, **Ctrl+1** selects the focused editor area, **F6/Shift+F6** cycle Explorer and the active project's panes, and **Ctrl+Alt+Arrow** moves to a neighboring pane. Dialogs and other active attention overlays guard these shortcuts. Vim input and native clipboard interception require keyboard focus in the active source editor; Explorer, search fields, dialogs and Markdown Preview do not dispatch their text input into a source document.

Windows **Copy/Cut/Paste** and the shared Vim unnamed register are separate. `y`, `d`, `c`, `p` and `P` use Vim state without overwriting the system clipboard. Native Copy preserves editor mode and selection, or copies the current line if no text is selected. Native Cut and Paste preserve the Vim register and participate in Undo. Normal-mode native Paste inserts at the cursor or replaces the selection; Vim `p`/`P` retain their after/before behavior. A delayed context-menu Paste retains its original document ID, selection and revision, so switching projects cannot redirect it into another buffer. A changed, closed or loading target is not overwritten.

Packaged context measurements and implementation evidence are recorded below. Historical measurements and first-use evidence remain separate. Each verification section states which production behavior and model trial it actually measured.

## Historical context cost: 0.3.0 to 0.4.0

The baseline is the packaged Morrow 0.3.0 interface captured before the 0.4.0 revision. The packaged Morrow 0.4.0 measurements use the same tokenizer, `tiktoken` 0.14.0 with `o200k_base`, and serialization: complete UTF-8 bytes of every skill file, plus unmodified stdout from the packaged `dist/Morrow/morrow-cli.exe --help` and `schema` commands. Line endings and trailing newlines are retained. Both offline discovery commands succeeded without starting or modifying the GUI. These captures describe 0.4.0; pane-local tab groups were added in 0.4.1.

| Surface | 0.3.0 | 0.4.0 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 638 | 802 | +164 |
| Conditional editor-control reference | 0 | 1,549 | +1,549 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 421 | 880 | +459 |
| Production CLI schema | 454 | 2,343 | +1,889 |
| Instruction/discovery total | 2,141 | 6,202 | +4,061 |
| UI metadata, counted separately | 46 | 49 | +3 |

The interface expands from 12 to 30 operations, with additional typed editor and layout actions. This is a capability expansion; no token saving is claimed. The entry point, conditional references, and CLI discovery are separate surfaces: ordinary tasks need not load every reference or both discovery outputs.

These are local context estimates, excluding provider wrappers and billing semantics. Definitions recur whenever included in a model request, subject to caching and framing. UI metadata may not be delivered as model instructions. Before/after raw payloads, hashes, and counts are captured in the test-owned `target/agent-interface-audit/{before,after}/` directories. Comparable call counts and argument/result costs for the broader unsaved-edit-and-split workflow are unavailable because 0.3 had no buffer mutation or pane control.

## Context cost: 0.4.0 to 0.4.1

The packaged 0.4.1 CLI and repository skill were captured using the same exact-byte serialization and tokenizer. Both offline discovery commands succeeded without mutating the running GUI. The schema still exposes 30 operations; `close_tab` is an additional typed layout action.

| Surface | 0.4.0 | 0.4.1 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 802 | 802 | 0 |
| Conditional editor-control reference | 1,549 | 1,746 | +197 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 880 | 880 | 0 |
| Production CLI schema | 2,343 | 2,499 | +156 |
| Instruction/discovery total | 6,202 | 6,555 | +353 |
| UI metadata, counted separately | 49 | 49 | 0 |

The extra guidance and schema describe local tab ownership, closure, and preservation of shared buffers. They add 892 and 747 UTF-8 bytes respectively; the other surfaces retain identical bytes and hashes. This is a context increase, with no token saving claimed. Raw payloads, SHA-256 hashes, byte counts, and token counts are retained separately in `target/agent-interface-audit/after-0.4.1/`; the historical 0.4.0 captures and trial remain unchanged.

## Context cost: 0.4.1 to 0.4.2

The 0.4.2 production CLI contract and repository skill use the same tokenizer, `tiktoken` 0.14.0 with `o200k_base`, and serialization as 0.4.1: complete UTF-8 bytes without line-ending normalization, including unmodified CLI stdout and its trailing newlines. Offline help and schema capture do not start or mutate the GUI. Provider wrappers are excluded; these local estimates are not billing measurements. UI metadata remains counted separately because it may not be delivered as model instructions.

| Surface | 0.4.1 | 0.4.2 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 802 | 802 | 0 |
| Conditional editor-control reference | 1,746 | 2,174 | +428 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 880 | 951 | +71 |
| Production CLI schema | 2,499 | 2,911 | +412 |
| Instruction/discovery total | 6,555 | 7,466 | +911 |
| UI metadata, counted separately | 49 | 49 | 0 |

Including UI metadata, all six surfaces total 6,604 before and 7,515 after, also an increase of 911 tokens. The schema expands from 30 to 31 operations: `bookmarks` adds five typed actions, while `leader_key` extends settings. The reference teaches persistent IDs, bounded listing, exact leader aliases, pending navigation, and recovery from uncertain toggle delivery. This is a capability expansion, with no token saving claimed; tasks need not load every conditional reference or both discovery outputs.

Raw payloads, hashes, byte counts, and token counts are retained in `target/agent-interface-audit/after-0.4.2/`. `target/agent-interface-audit/measure-0.4.2.py` accepts a built CLI path and captures its actual help/schema output alongside the repository skill files. No new first-use model trial or comparable argument/result cost measurement was performed for bookmarks or leader preferences. The historical 0.4.0 success and 0.4.1 scripted checks below remain unchanged and do not establish first-use success for these additions.

## Context cost: 0.4.2 to 0.4.3

The 0.4.3 release CLI and repository skill use the same exact UTF-8 byte capture and `tiktoken` 0.14.0 `o200k_base` tokenizer as 0.4.2. Offline help and schema do not start or mutate the GUI. Provider wrappers and billing semantics are excluded; UI metadata is counted separately.

| Surface | 0.4.2 | 0.4.3 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 802 | 802 | 0 |
| Conditional editor-control reference | 2,174 | 2,392 | +218 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 951 | 1,027 | +76 |
| Production CLI schema | 2,911 | 3,083 | +172 |
| Instruction/discovery total | 7,466 | 7,932 | +466 |
| UI metadata, counted separately | 49 | 49 | 0 |

All six surfaces total 7,981 tokens, including metadata. The schema retains 31 operations; `move_tab` adds a typed layout action for atomic reorder and cross-pane movement. Its guidance explains final indices, destination deduplication, preserved view state, collapsed source panes, and inspection after uncertain delivery. This is a context increase; no token saving or new model-first-use success is claimed.

Raw help/schema and skill payloads, SHA-256 hashes, bytes, and token counts are retained in `target/agent-interface-audit/after-0.4.3/`. The repeatable capture is `target/agent-interface-audit/measure-0.4.3.py`, supplied with the production CLI executable path. The historical first-use trial remains separate from the scripted implementation checks.

## Context cost: 0.4.3 to 0.4.4

The packaged 0.4.4 CLI and repository skill were captured with the same complete UTF-8 byte serialization and `tiktoken` 0.14.0 `o200k_base` tokenizer as 0.4.3. Offline help/schema capture does not start or mutate the GUI. Provider wrappers and billing semantics are excluded; UI metadata is counted separately.

| Surface | 0.4.3 | 0.4.4 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 802 | 802 | 0 |
| Conditional editor-control reference | 2,392 | 2,841 | +449 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 1,027 | 1,162 | +135 |
| Production CLI schema | 3,083 | 3,346 | +263 |
| Instruction/discovery total | 7,932 | 8,779 | +847 |
| UI metadata, counted separately | 49 | 49 | 0 |

Including metadata, all six surfaces total 8,828 tokens. The schema retains 31 operations. The additional guidance describes per-project layouts, active-project pane admission, stable pane IDs across switches, global buffer inspection/mutation, explicit document focus and cross-project open/review outcomes. This is a context increase; no token saving or new first-use model success is claimed.

Exact payloads, SHA-256 hashes, bytes and token counts are retained in `target/agent-interface-audit/after-0.4.4/`; the capture script is `target/agent-interface-audit/measure-0.4.4.py`. Measurement used the final packaged `dist/Morrow/morrow-cli.exe`, with historical trial evidence left separate.

## Context cost: 0.4.4 to 0.4.5

The packaged 0.4.5 CLI and repository skill were captured with the same complete UTF-8 byte serialization and `tiktoken` 0.14.0 `o200k_base` tokenizer. Offline discovery does not start or mutate the GUI. Provider wrappers and billing semantics are excluded; UI metadata is counted separately.

| Surface | 0.4.4 | 0.4.5 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 802 | 802 | 0 |
| Conditional editor-control reference | 2,841 | 3,267 | +426 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 1,162 | 1,355 | +193 |
| Production CLI schema | 3,346 | 3,690 | +344 |
| Instruction/discovery total | 8,779 | 9,742 | +963 |
| UI metadata, counted separately | 49 | 49 | 0 |

All six surfaces total **9,791 tokens**, including metadata. The schema retains 31 operations. New guidance covers reversed J/K, separate clipboard stores, persisted and one-query exclusions, validation limits and observable search scope. This is a context increase; no efficiency saving or new model-first-use success is claimed.

Exact payloads, SHA-256 hashes, bytes and token counts are retained in `target/agent-interface-audit/after-0.4.5/`; the repeatable capture is `target/agent-interface-audit/measure-0.4.5.py`, using `dist/Morrow/morrow-cli.exe`.

## Historical 0.4.0 verification

The 0.4.0 repository skill passed `skill-creator/scripts/quick_validate.py`; all supporting reference links resolved, and the packaged production schema exposed 30 distinct operations. Structural validation checks packaging and placeholders, rather than proving task success.

The earlier 0.3 presentation-only trial succeeded on its first attempt, 1/1, with six CLI calls and no recovery. Independent receiving-state checks confirmed its Markdown preview and two marked source documents while retaining existing documents. That historical task differs from the broader 0.4 workflow and is not a successful baseline for editing or split control.

The 0.4.0 implementation checks passed **348 tests**: 321 application tests and 27 CLI tests, with seven tests ignored in the normal suite. Formatting and strict Clippy passed. The separately invoked Windows captured-output startup regression also passed.

Live checks against the packaged Windows 0.4.0 build opened Markdown from a closed app in 781 ms. Ten Rust files opened and twenty focus switches succeeded; the mean CLI round trip was 31.18 ms, which measures command application rather than full visual latency. An asynchronous rename preserved the tab ID and its unsaved 96 MiB draft while disk text remained at 64 MiB. Folder creation reported actual application, Undo restored a clean buffer, and a Cargo check task reached Completed/Finished. These are implementation checks, separate from first-use model evaluation and limited to this Windows environment.

## Independent 0.4.0 first-use trial

A fresh agent with no inherited conversation received an ordinary goal and the repository skill: show a Markdown plan beside its Rust implementation, propose a 64→96 MiB budget change in the open buffer without saving, mark the budget and its boundary check, and retain existing documents and preferences. It received disposable fixture paths in `target/morrow-control-eval/`, without prescribed commands, JSON, a call count, or the expected answer. It retained the normal file and shell tools alongside the CLI and used the inherited model/settings; the exact provider identifier is not present in the captured trial evidence.

The first attempt succeeded **1/1**, with **21 actual CLI invocations**, all exit code 0, and no retry or recovery. Initial file loads and a review refresh were observed until ready rather than mistaken for completed work. Command arguments, output, exit codes, timestamps, submitted requests, and disk verification are preserved in `target/morrow-control-eval/first-use-04-trace/`; `REPORT.md` lists every invocation. A separate agent audited those saved records and fixture files without invoking the app or changing its state.

The supervising agent independently checked the production receiver: pane 1 showed the plan in Preview and pane 2 the Rust source in a vertical split at ratio 0.5. The source buffer contained 96, retained its Unicode comment, was dirty with autosave paused, and its saved file still contained 64. Current annotations marked columns 28–29 on line 8 and the boundary check on line 12. The plan and both fixture disk hashes stayed unchanged; all three earlier tab IDs and the original preferences were preserved. There were no pending fixture loads/saves or fixture errors.

All 13 supervising receiving-state assertions passed and are recorded in `target/agent-interface-audit/trial-independent-checks.json`, including exact buffer equality with only the budget changed. After recording the outcome, the supervisor undid the disposable draft with its current revision, verified the exact clean restoration, and closed the fixture tabs and added pane.

One observer change occurred during the trial: the supervisor updated the existing README on disk to clarify Smooth scrolling defaults. Its already-open clean buffer refreshed from revision 1 to 2. The evaluator noticed and left the unrelated update intact; unchanged original revisions are therefore not claimed.

This evidence covers actual CLI state and disk hashes, rather than screenshot inspection or visual latency. The window was raised for review; no user reading or approval was inferred. One ordinary model trial does not establish general reliability, and its broader outcome has no comparable successful 0.3 workflow.

## 0.4.1 tab-group verification

The 0.4.1 implementation checks passed **368 tests**: 339 application tests and 29 CLI tests, with seven tests ignored in the standard run. The 29 split regressions cover pane-local groups, real Ctrl+W and middle-click closure, shared Undo with counted Vim Insert, session group order and inactive Markdown states, and migration of older sessions. A real tab-X interaction and all 14 strict screenshot comparisons passed. Formatting and strict Clippy passed.

Scripted checks against the running packaged Windows 0.4.1 receiver confirmed that a split starts with only its selected document, and a second new document joins only the focused group. Local closure kept a nonempty right pane open; closing its last tab collapsed it and returned focus to the adjacent pane. Shared dirty Unicode text remained intact, and removing its final view was refused without effects. After clearing only the disposable draft with an exact revision guard, the original tab IDs, groups, tree, and preferences were restored. Raw requests and responses, plus the resulting assertions, are recorded in `target/pane-groups-live/` and `proof.json`.

The ordinary upgrade restart preserved all previously open paths and preferences; empty panes from the older layout collapsed during migration. Before/after receiving-state snapshots are retained in `target/pane-groups-before-restart.json` and `target/pane-groups-after-restart.json`. Tab IDs changed with the new process as expected.

These scripted checks target local tab groups and their receiving state, without claiming visual latency or user approval. The broader 0.4.0 first-use trial above remains historical; it was not rerun for 0.4.1 and does not establish first-use success for the new local closure action.

## 0.4.2 navigation and Git verification

The 0.4.2 implementation checks passed **487 tests**: 450 application tests and 37 CLI tests, with seven tests ignored in the standard suite. Formatting and strict Clippy passed. The separately invoked Windows captured-output startup regression also passed. All 20 strict screenshot comparisons passed, including the new Git changes/history/commit/diff, bookmark picker, and screen-jump views.

Real pointer and keyboard regressions cover the blank Explorer context menu, leader dispatch in Normal/Visual/Insert modes, visible targets in wrapped and split editors, Unicode cursor positions, bookmark opening of a closed file, and preservation of Visual selections and unchanged text. Scroll regressions use separate wheel events and check accumulation, reversal, both bounds, and retained final content in the editor, Markdown preview/tables, and the virtualized diff. Diff tests check readable code presentation separately from the exact original patch used by Copy patch, including multiple files, quoted Unicode paths, empty files, mode changes, renames, and binary changes.

Scripted checks against the packaged Windows 0.4.2 receiver created and renamed a temporary bookmark, verified bounded listing and the exact cursor destination, and confirmed the bookmark reached the recovery session on disk. A normal window close and a new process restored the same persistent bookmark ID, label, file, line, and column. After jumping again, the check removed only its temporary bookmark and restored the original tabs, groups, views, cursor positions, panels, and preferences. Original file hashes remained identical. Receiving-state traces and restart process IDs are retained in `target/navigation-live/`, including `proof.json` and `restart.json`.

The final offline help/schema capture used `dist/Morrow/morrow-cli.exe`; it confirmed 31 operations and the context counts listed above without GUI mutation. Packaged GUI and CLI executable hashes matched their release build outputs. The skill stays in the repository and portable package; it was not installed into the user's personal skills directory.

These are implementation and rendering checks. They do not establish general first-use agent success, visual latency on other machines, or user approval.

## 0.4.3 workspace, input and interaction verification

The final implementation checks passed **644 tests**: 605 application tests and 39 CLI tests, with seven tests ignored in the standard suite. All 27 strict screenshot comparisons passed. Formatting and strict Clippy passed; the separately invoked Windows captured-output startup regression also passed. Final results are retained in `target/verification-0.4.3/release-final-tests.log`, `release-final-clippy.log`, and `windows-startup-regression.log`.

Three bug reviewers and two UX reviewers checked editor input, search, pane ownership, CLI admission, project navigation, dialogs, completion, Git, and Markdown presentation. Their fixes were integrated before the final run. Headless egui pointer, keyboard, and wheel events exercise tab insertion markers, reorder and cross-pane movement, cancellation, preserved Insert/Visual state and shared Undo, preview and table scroll retention, project-picker focus, context Copy/Cut/Paste, IME, and narrow or long-content controls. Drag screenshots wait for bounded syntax readiness rather than depending on worker timing.

Search regressions check Unicode and whole-word semantics, incremental Vim `/` and `?`, counts and search operators, all visible highlights, cancellation, and rejection of stale background results. A separate optimized-core measurement scanned an 8 MiB one-character fixture with all 8,388,608 matches retained: literal scans took 108.8–116.3 ms, and the regex scan took 360.4 ms; conversion to shared results is measured separately. This measures isolated search work, not GUI latency. The dense result array uses 128 MiB; retained-cache budgets exclude results pinned by visible panes. Raw samples are in `target/verification-0.4.3/search-performance.log`.

Scripted checks against the running packaged Windows 0.4.3 receiver passed all nine assertions: same-pane reorder, cross-pane movement, destination deduplication, empty-source collapse, dirty Unicode preservation, shared Undo/Redo, invalid-index rejection without effects, protection of the final dirty view, and restoration of the original session. Only unnamed test-owned drafts were edited during these checks. Original tabs, pane layout, cursor positions, settings, panels, bookmarks, and project-file hashes were preserved; requests and independent receiving-state checks are in `target/interaction-live-0.4.3/{trace,proof}.json`.

The test harness now uses process-owned recovery paths under `target/test-state` and suppresses session, disk-refresh, recovery, and Git timers while rendering fixtures. During this work, an earlier fixture's shared temporary recovery path was found in two saved session entries. Their original project files were intact. Before cleanup, the profile, session, and buffer contents were backed up; only the exact fixture text and those two confirmed recovery aliases were cleared. A normal close and restart restored the original files, groups, positions, and preferences with fresh, distinct recovery paths. Evidence is retained in `target/interaction-live-0.4.3/startup-recovery-backup/` and `startup-restoration-proof.json`. The final full suite was then checked independently against the live receiver and recovery session; `after-headless-tests-proof.json` confirms it caused no further changes.

The final offline discovery capture used `dist/Morrow/morrow-cli.exe`, confirmed 31 operations, and produced the 0.4.3 context counts above. Packaged GUI and CLI executable hashes match the release build outputs. The skill remains in the repository and portable package, without installation into the user's personal skills directory. No new model-first-use trial or visual-latency measurement was performed.

## 0.4.4 project layouts, Git graph and modal verification

Final implementation checks passed **696 tests**: 657 application tests and 39 CLI tests, with seven tests ignored in the standard suite. All 29 strict screenshot comparisons passed, including the new bounded commit and nested diff modals. Formatting and strict Clippy passed; the separately invoked Windows captured-output startup regression also passed. Results are retained in `target/verification-0.4.4/final-app-tests.log`, `final-cli-tests.log`, `final-clippy.log` and `windows-startup-regression.log`.

Eleven project-layout regressions cover independent tab groups and split trees, Source/Preview, selections, scroll positions, shared document Undo, inactive-project buffer operations, rejection of foreign pane/tab IDs, restart restoration, missing project roots and migration of old shared tab strips. Existing pointer and keyboard project-switch tests retain their draft and no-reload assertions while checking the new per-project ownership. A legacy decoder regression caught lost or reordered tabs; the final decoder restores every owning project's file order without carrying foreign tabs into its groups.

Fourteen palette regressions use actual egui keyboard and pointer events to verify Home/End/Page Up/Down, selection visibility, the persistent scrollbar and dismissal without click-through. Eleven Git modal checks cover topmost-only outside click and Escape, source/preview focus restoration, pending/error diffs, a held tab-drag release when a modal first opens, and bounded long commit metadata with a visible close control. Independent CLI and modal source reviews were performed; the modal review's metadata overflow and first-frame input findings were repaired before the final run. These checks use the headless egui input harness, not native Windows pointer automation.

Git tests cover real repository merges, local/remote/tag references, first-parent diffs, unmerged remote-only branches, and frozen pagination beyond the first page while refs move. Pure graph tests cover lane continuity, merge edges, branch colors and page boundaries. History UI checks verify one pending page, append-only graph preparation, stale generation/offset rejection, error retry, automatic near-end loading and preservation of scroll/pages during status refresh. An explicit refresh starts a new history snapshot when references change.

The packaged Windows 0.4.4 receiver passed seven scripted assertions against the user's actual saved session: all files and preferences restored, original active project retained, each project's own ordered tab strip and selected view restored, pane IDs unique across projects, project-switch round trip restoring layouts/cursors/views/revisions, all project layouts persisted with bookmarks unchanged, and unchanged hashes of the original project files. The existing tiny-project and vBot projects were switched and the original active project restored; no buffer text was edited. The pre-upgrade recovery directory was backed up, and the final headless suite left its session hash unchanged. Receiving-state requests, responses, persisted session and proof are in `target/project-layout-live-0.4.4/`.

Production GUI and CLI executables match their release build hashes. The repository skill and supporting documentation are included in the portable package, without personal skill installation. Offline discovery confirms 31 operations and the exact context counts above. No new model-first-use trial, native-pointer evaluation or visual-latency measurement was performed.

## 0.4.5 keyboard, search, clipboard and rendering verification

Final implementation checks passed **790 tests**: 748 application tests and 42 CLI tests, with seven tests ignored in the standard suite. All **31 strict screenshot comparisons** passed, including the new wide and narrow inline-code Markdown views. Formatting and strict Clippy passed. The separately invoked Windows captured-output startup regression also passed. Results are retained in `target/verification-0.4.5/final-app-tests.log`, `final-cli-tests.log`, `final-clippy.log`, and `windows-startup-regression.log`.

Headless egui input tests cover double-Escape cancellation and highlight restoration, async search cancellation, independent system and Vim clipboards in every editor mode, Unicode Cut/Paste Undo, reversed J/K motions and literal targets, Explorer navigation and selection, F2 stem/folder selection, stable keyboard and pointer context menus, and return from rename dialogs. Three same-frame focus handoff regressions keep Insert text and search input in their owning panes with independent Undo; a destination changed by CLI focus cannot receive stale deferred input. These checks use actual egui events, without claiming native Windows pointer automation.

Project-search checks cover generated-folder defaults, custom Windows and Unicode paths, anchored and nested globs, empty overrides, raw limits, fresh disk walking independent of the Explorer index, cancellation, stale query/filter/generation rejection, visible scan warnings, and unchanged preferences for one-query CLI overrides. Actual editor geometry confirms a clicked Unicode result is revealed near the viewport center with temporary line feedback. Yank feedback covers its exact character range or full source line and stops after an incompatible revision. Source line counts include empty/trailing lines and CRLF, independent of wrapped display rows.

Markdown geometry tests cover common text baselines, rounded code backgrounds, wrapped inline text, aligned table cells, exact Unicode copy content, and bounded geometry-cache reuse without stale link destinations. Compact Git checks keep all commit rows at two fixed lines, align graph edges, make every row region clickable, expose the active section, and reserve a stable 14-pixel refresh slot. Independent input and UX source/screenshot reviews caught native Explorer arrow-focus transfer, rename modal focus restoration and an idle repaint loop; all were repaired before the final suite.

The final headless suite left the user's pre-upgrade recovery session hash unchanged; the full recovery directory was backed up before testing. Evidence is retained in `target/interaction-live-0.4.5/after-headless-tests-proof.json` and `pre-update-recovery/`.

The packaged Windows 0.4.5 receiver passed seven upgrade/project-switch checks: all original files and preferences restored, original active project retained, each ordered tab group and selected view retained, unique pane IDs, a project round trip preserving layouts/cursors/views/revisions, persistent layouts and unchanged bookmarks, and unchanged original project-file hashes. New preferences acquired their intended defaults. Five additional production search checks verify inherited filters, effective per-query exclusions, explicit empty overrides, an unchanged visible layout when searching an inactive workspace, and preserved preferences/buffers. Receiving-state traces and proofs are in `target/interaction-live-0.4.5/{proof,search-proof}.json`.

Packaged GUI and CLI executable hashes match the release build outputs. The repository skill, references, documentation and 31 preview images are included in the portable package; no personal skill installation was performed. No new model-first-use trial, native-pointer evaluation or visual-latency measurement was performed.

## 0.4.6 mouse-selection scrolling verification

Final implementation checks passed **799 tests**: 757 application tests and 42 CLI tests, with seven tests ignored in the standard suite. All **31 strict screenshot comparisons** passed; only the welcome screen's version label changed. Formatting and strict Clippy passed, as did the separately invoked Windows captured-output startup regression. Results are retained in `target/verification-0.4.6/final-app-tests.log`, `final-cli-tests.log`, `final-clippy.log`, and `windows-startup-regression.log`.

Nine new regressions use actual headless egui pointer events through `draw_ui`: stationary vertical dragging in Vim Normal, Insert and native editing with both scrolling preferences; horizontal Unicode selection; diagonal scrolling and reversal through the original anchor; immediate stop on release or return inside; scrollbar ownership; vertical-only scrolling with word wrap; isolation between split views of the same document; modal suspension; and cancellation of an already-running animated cursor reveal without resumption after release. They also check unchanged document text, dirty state, original Undo/Redo availability and bounded offsets. The animation test requires a real pending target before capture. An independent source and test review found no remaining confirmed issues after the animation cancellation fix.

Edge scrolling is bounded, follows the captured text widget, and leaves egui in charge of the selection anchor. Existing scroll targets are cleared once on capture through egui's persisted State representation, retaining scrollbar geometry and resetting kinetic velocity. This compatibility path is covered by the real pending-animation regression. These checks do not claim native Windows pointer automation or a new first-use agent trial; the CLI and repository skill interface are unchanged.

The running packaged Windows receiver was closed normally after confirming clean buffers and no running task or file jobs. Its latest recovery directory was backed up after the close. Seven read-only upgrade checks passed against the new 0.4.6 process: all seven open files with their views and locations, preferences and active project, active pane groups and selected view, clean ready buffers, unchanged original file hashes, and preserved saved project layouts, scroll positions and bookmarks. Snapshots and independent assertions are retained in `target/verification-0.4.6/{before-close-status,after-upgrade-status,upgrade-proof}.json` and `pre-update-recovery/`. Packaged GUI and CLI hashes match their release outputs; the repository skill, documentation and all 31 preview images are included.

## Context cost: 0.4.6 to 0.4.7

Both captures use complete UTF-8 instruction files serialized as LF Git blobs, unmodified production CLI stdout including its final newline, and `tiktoken` 0.14.0 `o200k_base`. This newly captured 0.4.6 baseline uses the same serialization as 0.4.7; the earlier historical tables retained working-file line endings. Provider wrappers and billing semantics are excluded; metadata is counted separately.

| Surface | 0.4.6 | 0.4.7 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 800 | 800 | 0 |
| Conditional editor-control reference | 3,267 | 3,371 | +104 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 1,355 | 1,411 | +56 |
| Production CLI schema | 3,690 | 3,784 | +94 |
| Instruction/discovery total | 9,740 | 9,994 | +254 |
| UI metadata, counted separately | 49 | 49 | 0 |

All six surfaces total **10,043 tokens**, including metadata. The schema retains 31 operations. The existing settings patch gains a typed font choice, canonical JSON values, ordinary CLI aliases, the bundled default and missing-system-font fallback guidance. Omitted or null fields retain the existing preference; invalid values apply nothing. This is a context increase; no efficiency saving or new model-first-use success is claimed. Exact payloads, SHA-256 hashes, bytes and counts are retained in `target/agent-interface-audit/{before,after}-0.4.7/`; `measure-0.4.7.py` captures the final packaged CLI and repository skill.

## 0.4.7 regular editor typography verification

Final implementation checks passed **813 tests**: 769 application tests and 44 CLI tests, with seven ignored in the standard suite. All **31 strict screenshot comparisons** passed. Formatting and strict Clippy passed, as did the separately invoked Windows captured-output startup regression. Results are retained in `target/verification-0.4.7/final-app-tests.log`, `final-cli-tests.log`, `final-clippy.log`, `final-format.log` and `windows-startup-regression.log`.

JetBrains Mono NL Regular 2.304 is embedded with its upstream OFL license included in the repository and portable package. Light grayscale hinting reduces stem widening, and a fixed source line box keeps plain, pending and colored syntax on the same grid. Source emphasis does not introduce italics or underlining; Markdown prose emphasis remains independent. Windows Consolas and Cascadia Mono are optional preferences, with the bundled font used when unavailable. Font changes invalidate shaped caches through the existing font-atlas probe rather than retaining obsolete glyph positions.

Twelve new application regressions and two CLI regressions cover actual source glyph and cursor geometry before/after syntax work, Unicode, wrapping, inserted lines, resizing, missing-system-font fallback, persisted font patches, aliases and invalid values. Headless egui events verify the settings chooser with Vim Visual and native mouse selections, and a receiver-driven switch in two source panes retains document revisions, selection, view state and search rectangles. Gutter checks cover all three fonts at 14/20 pixels, wrapped/unwrapped, with interior and trailing blank lines. An independent source review led to shaping blank-line baseline probes with the same pixel-rounded line box as source. These checks do not claim native Windows pointer automation.

An initial heavily concurrent strict run had one existing transport size-test failure: it returned `invalid_request` rather than `request_too_large`. Read-only review established that the newline-free test cannot reach deserialization; this code implies an incomplete/failed socket frame before the size threshold. The fixture ignores write errors, so the exact socket cause was not recorded. The unchanged test then passed alone in 0.10 seconds and in the complete final strict suite at four test threads. No transport assertion or production limit was relaxed.

The application was already closed during delivery. Its latest saved recovery directory was backed up before the headless suite, which left the session bytes unchanged. The normal packaged 0.4.7 startup passed eight independent receiving-state checks: the bundled font default, all seven files with views and locations, existing preferences and active project, ready error-free buffers, unchanged original file hashes, exact restored buffer text, active pane groups and selected view, and all saved project layouts, scroll positions, selections and bookmarks. Clean recovery copies had already been removed by the earlier ordinary close; buffer checks used the unchanged saved files. Evidence is retained in `target/verification-0.4.7/{headless-profile-proof,after-upgrade-status,upgrade-proof}.json`, per-buffer read responses and `pre-update-recovery/`. Packaged GUI and CLI hashes match their final release outputs; the repository skill and font license remain part of the portable package, without personal skill or system font installation. No new model-first-use trial or visual-latency measurement was performed.

## 0.4.8 user selections for agents: implementation verification

The final suite passed **877 tests**: 829 application tests and 48 CLI tests, with seven ignored in the standard suite. All **34 strict screenshot comparisons** passed, including new source, collection and narrow collection views. Formatting and strict Clippy passed; the separately invoked Windows captured-output startup regression also passed. Evidence is retained in `target/verification-0.4.8/final-{app-tests,cli-tests,clippy,format}.log` and `windows-startup-regression.log`.

Purple user selections form an independent global collection without changing source text, ordinary selections, Vim registers, document Undo or project/pane state. Model tests cover unlimited multi-file capture, Unicode/exclusive ranges, exact frozen snippets, outside-edit tracking, stale overlapping changes, identity after reopening and restoring untitled documents, chunk boundaries, removal/clear restoration, idempotent delivery tokens and receipt expiry. Receiver tests verify actual editor state, bounded pages and complete text continuations, durable Take/Ack, atomic-storage failure rollback with live edit caches retained, and frozen batch coordinates that never claim a current live range.

Actual headless pointer/keyboard events verify marking/removal in native and Vim editing, wrapped Unicode geometry, a fixed inline remove gutter during horizontal scrolling, grouped/filterable collections, notes, Clear/Restore, keyboard actions and outside-click dismissal. Eleven chronological input regressions additionally exercise Visual entry/motion before capture, arrow/leader sequences within one event batch, native selection extensions, Insert text before opening the collection, suffix ownership after the Modal sizing pass, preserved Undo on close, capture/region focus order across distinct buffers, pending-leader Escape order, ignored capture while a search prompt opens, and cancellation after a CLI Vim settings change. Deferred ownership records source identity, revision, selection, mode, view and relevant Vim settings. Ordinary Insert text bypasses leader parsing; a same-batch transition into Insert hands its complete suffix to native editing without repeated scans.

Independent source/input and UX reviews led to repairs for Unicode overlay alignment, inline pointer ownership, stale untitled IDs, current-source navigation, clipboard-independent capture, atomic receipt failure recovery, chronological input and frozen-batch freshness. Initial compile/assertion and GPU-device failures are retained in the earlier logs; snapshot harnesses now use the existing GL configuration. No snapshot tolerance or production payload limit was relaxed. These are headless interaction and receiving-state checks, not native Windows pointer automation or measured visual latency.

## 0.4.8 first-use agent trial

One fresh agent received an ordinary goal: read the user's marked areas and notes across projects, including an unsaved decision, summarize requirements/direction, remove highlights while retaining restoration, and preserve documents/settings/views. It received the repository skill descriptor/path and ordinary competing file/terminal tools, without a prescribed action JSON, grading outcome, suspected defect or implementation history. Model/settings were inherited from the root conversation without an override. Independent expected texts, notes and disk hashes were recorded before dispatch.

The real packaged Windows receiver returned **103 complete snippets and notes**, with pages of 100 and 3 items plus two continuations for a **9,154-scalar Unicode unsaved excerpt**. The decision at its tail was reflected in the final summary. All **nine production calls** succeeded with empty stderr, no malformed invocation or recovery call. Final receiving state was zero live marks, 103 restorable, delivery acknowledged. A separate grader reconstructed every snippet directly from full raw responses and checked all 11 loaded buffers, settings, active project/tab, views, revisions and exposed active layout against the pre-trial state. Windows eframe fallback storage had also restored existing tabs into the fixture; they remained unchanged. Inactive saved layouts were not separately snapshotted in this trial.

First-attempt task success is **1/1**, success after recovery **0/1**, remaining task failures **0/1**. This single sample establishes only this workflow; there was no equivalent feature in 0.4.7 for a before-task comparison. Subsequent deterministic reviews repaired input ordering and conservative frozen-receipt metadata without changing the collection/pagination/ack workflow. Trial executable hashes, supplied context/scope, final response, every invocation/full response and independent grade are retained under `target/verification-0.4.8/first-use/{trial-manifest.json,consumer-trace/,receiving-proof.json}`.

The nine calls contain **140 tokens of compact full argument arrays** and **24,865 tokens of complete stdout**, counted once: **25,005 tokens** using `tiktoken` 0.14.0 `o200k_base`. This includes discovery help and status as actually returned; no truncated previews substitute for full responses. Provider wrappers are absent. These local payload estimates are not billing or whole-run usage, and do not measure repeated history exposure. No call/token saving against an equivalent older task is claimed.

## Context cost: 0.4.7 to 0.4.8

Both captures use complete UTF-8 instruction files serialized as LF Git blobs, unmodified production CLI stdout including its final newline, and `tiktoken` 0.14.0 `o200k_base`. Provider wrappers are excluded; metadata is counted separately. These are local estimates rather than provider billing. Definitions recur in each model request containing them, subject to caching and framing.

| Surface | 0.4.7 | 0.4.8 | Delta |
| --- | ---: | ---: | ---: |
| `SKILL.md`, including frontmatter | 800 | 930 | +130 |
| Conditional editor-control reference | 3,371 | 4,216 | +845 |
| Conditional review reference | 628 | 628 | 0 |
| Production CLI help | 1,411 | 1,851 | +440 |
| Production CLI schema | 3,784 | 4,824 | +1,040 |
| Instruction/discovery total | 9,994 | 12,449 | +2,455 |
| UI metadata, counted separately | 49 | 49 | 0 |

All six surfaces total **12,498 tokens**, including metadata; operations grow from 31 to 32. One typed `agent_marks` operation owns collection, delivery, continuation and restoration. This is a measured context increase for a new capability, with no efficiency saving claimed. Exact full payloads, SHA-256 hashes, bytes and counts are retained in `target/agent-interface-audit/{before,after}-0.4.8/`; `measure-0.4.8.py` captures the final packaged CLI and repository skill. The first-use trial used the earlier 0.4.8 surface captured in its own full traces; final guidance additionally explains conservative frozen-batch coordinates.

## 0.4.8 packaged receiving-state and upgrade verification

After the first-use trial was independently graded, only its disposable unsaved test buffer was undone, making the fixture clean for normal shutdown. The final packaged receiver reopened the isolated profile and retrieved the acknowledged batch across process restart. All 103 complete texts and notes, including the original unsaved tail, matched the independent expectation exactly. Every frozen batch item reported `range_current:false`. Explicit restoration returned all 103 live marks without changing saved files or creating dirty buffers. The fixture then closed normally. Production traces and independent assertions are retained in `target/verification-0.4.8/first-use/{cleanup-trace,restart-trace,restart-proof}.json` and `verify-receipt-restart.py`.

The user's latest saved session was backed up after its earlier ordinary close. All headless checks and isolated trial/restart checks left that recovery session's bytes unchanged. The final normal 0.4.8 process passed eight independent upgrade checks: regular bundled font default, all seven original files with views and cursor positions, existing preferences and active project, ready buffers without errors, unchanged project-file hashes, exact restored buffer text, active pane groups/view, and all saved project layouts, scroll positions, selections and bookmarks. The initial raise immediately after process creation arrived before endpoint publication and returned `connection_failed`; after read-only startup readiness verification it succeeded. Evidence is retained in `target/verification-0.4.8/{headless-profile-proof,after-upgrade-status,after-upgrade-session,upgrade-proof}.json`, per-buffer reads and `pre-update-recovery/`.

Packaged GUI and CLI SHA-256 hashes match the release outputs. The portable package includes the repository skill, final references/documentation, font license and all 34 preview images. No personal skill or system font was installed.
