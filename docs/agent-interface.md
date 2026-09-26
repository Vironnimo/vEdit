# Agent control of Morrow

The CLI controls Morrow's editor state and presents plans and review targets. The `morrow` skill belongs to this repository at `.agents/skills/morrow/`; portable builds bundle it under `skills/morrow/`. Neither building nor using the application installs a personal skill.

The packaged CLI is `dist/Morrow/morrow-cli.exe`. `--help` documents the ordinary commands; `schema` returns the structured request contract. The skill entry point explains discovery, scope, and verification. `references/editor-control.md` is conditional guidance for buffers, layouts, preferences, project queries, tasks, and file operations; `references/review.md` is loaded for marked reviews.

## Observable contract

- Buffer edits use current tab IDs and an exact `expected_revision`, validate the full change before applying it, create an Undo step, and pause autosave for that document until a successful explicit save. A stale revision applies no mutation. Normal Vim Insert is finished separately; a counted Insert that would change observed text is refused without effects.
- Editing, selections, search matches, and diagnostic ranges have exclusive ends. Review annotation endpoints are inclusive. All positions use 1-based Unicode scalar coordinates; CRLF boundaries are preserved.
- Each pane owns an ordered tab group. Layout results expose its active `tab_id` and local `tab_ids`, usable pane IDs, split orientations, and ratios. A new split starts with the selected document; new opens join the focused pane. Assigning a document adds and focuses it in the target pane without removing other references. Panes share document text and Undo, while their cursor, view, and scrolling are separate.
- Layout `close_tab` removes one pane's reference to a document. It preserves a dirty or saving document shown elsewhere, and refuses to remove its final reference. Removing the final local tab collapses an empty split pane; the sole pane remains available. Ordinary `close TAB_ID...` closes clean, non-saving documents in every pane. Closing a whole pane transfers documents absent from the other panes to a surviving pane, preserving unsaved work; the last pane cannot be closed.
- Settings patches affect supplied preferences and persist. File rename/folder creation run as tracked background jobs; task dispatch and file-job dispatch are separate from completion.
- Ordered batches stop on failure or partial execution by default and return completed effect evidence plus the stopping point. Large results are compacted explicitly; omitted details remain separately retrievable. Batches do not roll back or replay applied work. Nested batches, background project search, and asynchronous file actions are excluded.
- Project search reads indexed files on disk and reports completed bounded results; use buffer `find` for unsaved text. Git status and language diagnostics expose cached state. Diff compares the current buffer with its saved text using a bounded replacement hunk. Truncation and readiness remain observable.
- Opening Markdown with `--preview` selects its reading view. `--source` returns to the editable document and preserves the user's editor preferences.
- `open` and `review` refresh already-open clean buffers asynchronously from disk, preserving unsaved text. A marked file reveals its first annotation unless an explicit location overrides it.
- A review opens several files with notes on important line ranges. A successful new review replaces earlier review annotations, while preserving ordinary open documents.
- Responses provide usable tab IDs and per-item outcomes. Application of a request and completion of an asynchronous file load are distinct; `status` and `tabs` expose the current state for verification.
- An omitted annotation end marks the entire start line; omitted columns default to 1.
- Unknown manifest fields are rejected. File and later load failures can produce partial results, so callers inspect successful items and pending state before continuing.

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
