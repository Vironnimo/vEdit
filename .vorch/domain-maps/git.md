# Git

Git supplies repository state, bounded subprocess operations and diff presentation to the editor.

## Overview

- `src/git.rs` owns Git invocation, repository detection, status/history parsing, mutations, comparison resolution and patch loading. It is independent of egui.
- `src/app.rs` owns project Git caches, request identities, worker dispatch/results and the running Git action. `src/git_panel.rs` presents those caches and translates input into actions; it does not run Git while painting.
- `src/git_repositories.rs`, a child of the panel, caches related worktrees for Source control and the project picker. It reuses open projects' status and separately queries unopened worktrees.
- `src/git_graph.rs` prepares history lanes. `src/git_diff_view.rs` parses and renders unified patches without modifying buffers. `src/explorer_git.rs` projects cached status onto Explorer rows.
- `src/git_gutter.rs` compares a document with its committed base and owns gutter previews/reverts. Whole-project change reviews and user comments belong to `reviews.md`; disk conflicts and saved-buffer comparison belong to `documents.md`.

## Interfaces

The opened project root and Git repository root are distinct. `project_repository` enables an enclosing repository only when it is the opened folder itself or the user approved that particular parent. Preserve the inactive-parent result rather than treating every ancestor repository as active.

Git helpers return `GitResult<T>`; absence is explicit where valid, such as no repository or detached HEAD. A malformed/corrupt repository is an error, not an empty clean state. Status keeps index and worktree changes separate, including both paths of renames.

Paths passed to path-based Git helpers are repository-relative. Invocation uses direct arguments, literal pathspecs and `-C`; do not route these through a shell. Preview diffs disable external diff and textconv helpers. Keep process execution in this owner so bounds, cancellation and deterministic parsing remain shared.

`HistorySnapshot` freezes object IDs and ref decorations for subsequent pages. UI history is scoped to the current branch and its configured upstream; tags decorate reachable commits without adding unrelated history. A changed snapshot replaces the graph; accepted pages extend it. See `git/execution-and-history.md` for refresh ownership.

`ReviewComparison` resolves working-tree, commit and branch comparisons for the review domain. Git reads saved repository content; unsaved buffer text is not included. Gutter comparisons are different: they include the current buffer, and a gutter revert performs one undoable buffer edit without writing disk or index.

## Constraints & Gotchas

- One user Git action runs at a time in the app. Starting an action invalidates affected pending status reads; a late pre-action result cannot replace post-action state.
- A branch switch refuses affected dirty, saving, loading or agent-draft documents. Pull/switch mark their repository as being rewritten, and saves into it are blocked until completion. The disk/document owner applies that save guard too.
- Read-only queries and user mutations have different deadlines. Cancellation can leave a partially updated repository or an index lock; vEdit reports it and never deletes the lock automatically.
- Remote reachability and ahead/behind data reflect fetched refs. Reading status/history does not fetch.
- Gutter markers are keyed by repository, HEAD, path and document revision. Stale markers may remain visible while recomputing, but cannot be reverted. Base reads and larger comparisons run in workers; binary/oversized bases do not produce ordinary text markers.

## Verification pointers

`src/git.rs` tests use real temporary repositories for status, renames, worktrees, frozen history, comparisons and process limits. `src/git_history_tests.rs` covers refresh/page generations and CLI branch switching; `src/git_repositories_tests.rs` covers discovery/cache ownership. `src/git_gutter_tests.rs` covers revision guards, CRLF and Undo. `src/git_ui_tests.rs`, `src/git_ux_tests.rs` and `src/git_modal_tests.rs` cover actual egui interaction and modal ownership; diff parser and graph tests are inline in their owners.

## References

Read these only when the task matches.

- Changing subprocess execution, cancellation, repository discovery or history refresh/paging -> `git/execution-and-history.md`
- Changing Git error or help wording exposed through the agent CLI -> `control/agent-facing-text.md`
