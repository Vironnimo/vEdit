# Search

This domain finds text in live document buffers and across projects, combining
open document snapshots with closed files read from disk.

## Overview

- `src/search.rs` is the pure buffer matcher and match-navigation engine;
  `src/search_cache.rs` retains bounded match results.
- `src/find.rs` connects matching to the Find toolbar and Vim search prompt,
  including history, highlight state, cancellation and pending navigation.
  `src/find_worker.rs` performs large-buffer searches off the UI thread.
- App children `src/find_toolbar.rs` and `src/project_search_ui.rs` render
  Find/Replace and project search, perform replacements, schedule project workers
  and accept their results. The worker poll in `src/app_workers.rs` routes completions.
- `src/workspace/search.rs` implements production project search through
  `workspace::search_with_buffers`. `src/cli.rs` validates/normalizes exclusion settings.
  Ex substitutions are owned by `src/ex_commands.rs` (see `editing.md`).

Quick Open searches filenames using the workspace index; it is not content
search (see `projects.md`). Language symbol/reference search belongs to
`language-services.md`. Disk decoding belongs to `documents.md`.

## Interfaces

`search` returns ordered, non-overlapping ranges in Unicode scalar coordinates.
It supports literal text or Rust regex, case sensitivity and whole-word checks.
Empty queries return no matches; invalid regex returns an error. Zero-width
matches remain navigable even though they have no background to paint.
`next_match` consumes sorted results, supports counted wrapping and distinguishes
including the current position from strict next/previous movement.

The toolbar defaults to literal matching; Vim search uses regex. Regex matching
handles multiline anchors and raw CRLF, and recognizes Vim's unescaped case
overrides. It is not the complete Vim regex dialect. Toolbar replacements use
Rust capture expansion through `regex_replacements`; Ex substitutions have their
own replacement-language adapter. Do not silently share those replacement parsers.

`MatchCache` and worker results identify the document, revision, byte length and
pattern. Large buffers use a cancellable worker; newer work replaces queued work
for the same document. Pending navigation additionally identifies the pane and
motion epoch, so a valid cached match is not by itself permission to move the
current cursor. `find.rs` checks the live owner before applying a result.

Project search uses literal matching and overlays current Documents owned by the
selected Project onto a fresh bounded disk walk. Named Documents replace their
disk content, including when their buffer has no match; exclusions and ignore/link
guards still apply. Unnamed Documents belong to that Project and have no path
filters. Missing named drafts use the same path/ignore policy. Loading Documents
are not captured; completion invalidates the search. Oversized snapshots suppress
disk fallback and report a skipped file. It does not use the Quick Open index.
`SearchMatch` carries an optional path, optional Document identity/revision and
unsaved state, one-based line/column, Unicode scalar range and display preview.
`SearchResults` also reports skipped files, truncation and
warnings; a bounded or unreadable search must not look complete.

The project worker result carries root, Project instance, Document snapshots,
generation, query and exclusions. The receiver accepts only the current combination.
Document edits, loads, saves, path changes and closure clear obsolete UI results
and use the existing 250 ms debounce to search again. Result clicks validate the
Document revision and reveal that Document without saving or opening a duplicate.
CLI search uses the same snapshots and backend; a changed owner returns
`search_changed` with no results instead of publishing stale coordinates.

## Constraints & Gotchas

Find highlight state and navigation identity are different. Hiding highlights
must preserve the last pattern for repeat navigation. Vim prompt cancellation
restores its original navigation and scroll state; confirmation can need to wait
for an asynchronous match. Deferred commands/operators and subsequent input must
stay ordered across that wait. Change the dispatch in `find.rs` with its async
regressions, rather than adding another input queue in a caller.

Replacement must use the original matching snapshot and the editor's mutation
path, with one undo step per Replace/Replace all action. Unicode range conversion,
empty regex matches and changing patterns are not safe to handle by slicing bytes
at the returned range indices.

Project exclusions are independent from Quick Open pruning and Explorer
visibility. Clearing optional exclusions permits generated folders, but Git
ignore rules and VCS metadata filtering still apply. The production walker does
not follow symbolic links. Exclusion validation rejects unsupported patterns;
keep its CLI/settings behavior aligned rather than accepting a pattern only in
one caller.

Project search has separate match, file-size, total-read, traversal and time
bounds in `workspace/search.rs`. Bytes read from binary or undecodable files still count
toward the read budget. Buffer match caching evicts retained results rather than
truncating a document's matches; do not import project-search truncation into
in-buffer navigation.

`workspace/search.rs::search_project` is a private test-only index-based helper. Production calls
`search_with_buffers`; changing only that helper cannot change shipped project
search behavior.

## Verification

Start with `cargo test --bin vedit search::tests -- --test-threads=8` for matcher
semantics. `src/find_tests.rs`, `src/find_async_tests.rs` and
`src/find_paint_tests.rs` cover prompt/navigation, delayed input and painting.
The worker and match cache also have inline cancellation and eviction tests.

`src/project_search_backend_tests.rs` covers exclusions and resource bounds;
`src/project_search_ui_tests.rs` covers settings and stale worker integration.
`src/workspace/search_tests.rs` contains disk-search decoding and coordinate regressions.
See `development.md` for application-level checks after behavior changes.
