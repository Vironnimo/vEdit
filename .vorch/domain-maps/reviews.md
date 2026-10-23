# Reviews and user marks

Reviews present changes and preserve the user's selected feedback for agents without making feedback itself a document edit.

## Overview

- `src/change_review.rs`, an app child module, owns each project's review state, seen files, parked reading position, lazy patch requests, CLI completion replies and view composition. Private `src/change_review/chrome.rs` owns the header/checklist; `input.rs` owns command mapping, shortcut hints and ordered reading input; `viewport.rs` owns continuous diff geometry, navigation effects and sticky file headers. They use comparisons from `src/git.rs` and patch rendering from `src/git_diff_view.rs` (`git.md`).
- `src/change_review_feedback.rs`, a child of change review, owns diff-row selection, frozen comment drafts, freshness notices and conversion into user marks.
- `src/agent_marks.rs` owns captured excerpts, source attachment, notes and recoverable delivery receipts. `src/agent_marks_control.rs` adapts that model to app buffers, session persistence and bounded CLI responses; `src/agent_marks_ui.rs` owns the collection and highlights.
- Agent-authored review annotations use `src/cli.rs` types and the open/review receiver in `src/control.rs`. That protocol belongs to `control.md`; it is separate from the user's mark collection.

## Terms

Shared Agent mark and Change review terms live in `.vorch/GLOSSARY.md`.

### Receipt

The retained, recoverable batch produced when an agent takes user marks out of the live collection. It records delivered content independently of later edits or newly created marks.

## Data Model

Change-review state is per project. Seen marks are session-memory data keyed by comparison, path and content key, so a changed file version is no longer implicitly seen. Closing parks the view and restores the existing panes; it does not close or save their documents.

User marks form an app-wide collection across projects. Each mark owns exact captured text and persistent identity. Source highlights can move after unrelated edits; overlapping edits or unverifiable reopened text make the range stale while the capture stays readable. Restored untitled marks cannot identify a new buffer from a saved runtime tab ID alone.

Change-review comments are user marks with frozen comparison metadata. They never attach to a live buffer. Their inclusive old/new line bounds are distinct from the exclusive navigation range used by ordinary mark responses; see `reviews/receipts-and-comments.md`.

## Interfaces

Review worker results carry project root and an app-unique generation. Closing/reopening the same project root must not admit an earlier review's result. Lazy patches also match the entry's content key. Background requests can load a parked review without taking focus; completion reports whether it is visible.

A change-review CLI reply waits until comparison loading completes. A replaced or closed pending review replies with its specific error. Loaded comparison does not mean every deferred large patch was read. Git's bounded patch classification remains visible to the reader.

Working-tree reviews exclude unsaved buffers and expose that fact in the UI. Repository changes produce a freshness warning and an explicit refresh action; they do not replace the comparison while the user reads or comments. Seen state belongs to the user; the CLI reports it rather than marking files seen.

Review file navigation, seen-and-continue, opening and commenting are scoped actions in the shared shortcut registry. The comment dialog owns its separate submit action. Local handlers use the same bindings and hints as Keyboard settings; reading motions and selection arrows remain view navigation. Palette commands queue against the current review until its diff geometry is available.

Taking or acknowledging marks saves session metadata atomically before success. A storage failure restores the prior live marks and receipt, including live attachment caches. Other mark changes queue normal session persistence. None of these operations saves document text.

## Verification pointers

`src/change_review_tests.rs` covers pane preservation, review entry points, generations, background review and screenshots. `src/change_review_feedback_tests.rs` covers selected diff excerpts and captured old/new coordinates. Model tests in `src/agent_marks.rs`, receiving-state tests in `src/agent_marks_receiver_tests.rs`, and input/UI tests in `src/agent_marks_input_tests.rs` / `src/agent_marks_ui_tests.rs` cover attachment, receipts, persistence failure and chronological input.

## Agent-facing text

The review skill references and mark receiver guidance are inventoried in `control/agent-facing-text.md`. Read it before changing those texts and update the relevant reason alongside the changed sentence. This map owns their review/mark semantics; control owns the shared agent-facing rationale entry point.

## References

Read these only when the task matches.

- Changing mark delivery, restoration, source attachment, pagination or diff-comment coordinates -> `reviews/receipts-and-comments.md`
- Changing agent-readable review/mark instructions, errors or response guidance -> `control/agent-facing-text.md`
