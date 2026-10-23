# Git execution and history

## Process contract

`src/git.rs` exposes the shared Git types and operation API. `src/git/command.rs` owns `run_git` and `run_bounded_with_input`; repository/status and staging live in `src/git/repository.rs`, diffs/statistics in `src/git/changes.rs`, frozen refs/traversal in `src/git/history.rs`, and reviews/baselines in `src/git/review.rs`.

Query timeout is 15 seconds; mutations allow 30 minutes. Both stdout and stderr are drained concurrently, and bounds apply while draining as well as while the main process runs. Once Git exits, descendants retaining its pipes receive a two-second grace period. A successful command releases its Job Object policy so helpers with detached pipes survive; the grace-expiry path terminates the remaining owned tree. Keep this distinct from task processes, whose trees always terminate with their guards.

The default output cap is 8 MiB. Status has a 64 MiB cap and, on overflow while enumerating all untracked files, retries with untracked folders collapsed. `RepoStatus.untracked_collapsed` keeps that loss of detail observable. Do not turn bounded output or parse errors into an apparently complete empty result.

The runner disables interactive credential prompts, pins parsing-sensitive Git settings and uses `GIT_OPTIONAL_LOCKS=0` for reads. A mutation still runs user hooks and Git's normal operation. `Cancellation` propagates through the worker's `cancellable` scope to its subprocesses. Windows process ownership and timeout cleanup live in the runner, not in panel callbacks.

`with_index_lock_hint` locates the actual worktree index lock, including linked-worktree `.git` indirection. It preserves Git's own lock error and otherwise appends recovery context when a lock remains. It does not remove the lock or claim rollback. Tests include `cancelling_a_commit_stops_its_hook_and_explains_the_remaining_index_lock` and the linked/main-worktree lock-hint regressions in `src/git/tests/process.rs`.

Backend regressions follow the owners in `src/git/tests/`; their common fixture in `src/git/tests.rs` creates a separate real repository and writes its local configuration together. The Windows timeout/cancel/pipe-grace tests hold a handle to the exact native writer PID published by Git-for-Windows shell procfs. The timeout test uses a test-only readiness gate before its deadline starts; the ordinary bounded-command regression covers normal startup timing. The attached hook stays alive until the handle is acquired, then Git's real exit starts the unchanged grace period. A signalled process handle, followed by the absent late-write marker, proves shutdown without a fixed post-cancellation sleep. The successful detached writer is kept alive until after `commit` returns, then explicitly released. These are real subprocess contracts, not mocked Git output.

## Discovery and refresh ownership

`git_repositories::RepositoryOverview` groups an active project's main/linked worktrees and caches listings across project switches. Its work plans own their inputs; workers return request-tagged results. The planner allows at most two jobs, refreshes listings on a 15-second interval and unopened-worktree status on a 30-second interval while a navigation surface is visible. Hover details share the same job budget; open projects reuse their existing status.

App's `src/git_refresh.rs` owns ordinary status refresh and history-page results. Request IDs, repository roots, history generation and expected offset are admission checks, not display-only metadata. Starting a Git action in `src/git_actions.rs` invalidates older status/indicator requests for every affected open project. The last complete view remains visible until the post-action refresh.

`HistorySnapshot` retains traversal tips, refs, upstream and reachability information. `current_branch()` narrows traversal roots to the current branch and upstream while retaining tag decorations. `history_page` traverses the frozen IDs, so moving refs between pages cannot reorder a loaded snapshot. `src/git_graph.rs` extends the prepared graph only for an accepted appended page.

Automatic refresh pauses while the window is unfocused or minimized. A visible history refresh can replace changed refs while preserving loaded depth and anchoring the reading position to the prior visible commit. An unchanged refresh preserves loaded pages and pending pagination. Hidden history retains its consistent pageable snapshot until reloaded.

Evidence: `src/git_history_tests.rs` covers active-branch/upstream scoping, background pauses, external commits, preserved scroll/depth, changed-ref replacement and stale page rejection. `src/git_repositories_tests.rs` covers cached discovery, unopened status, failed-job backoff and stale results.

## Comparison boundaries

`resolve_review` compares a working tree with HEAD (the empty tree before the first commit), a commit with its first parent (empty tree for a root commit), or HEAD with the merge base of an explicit base or local `main`/`master`. A working-tree comparison has no new object ID. `change_set` and `review_patches` preserve binary/large/deferred states instead of presenting unavailable patches as empty text.

`base_version` serves gutter comparisons with a separate bounded text read; its cache may depend on the index when no committed version is available. `git_gutter.rs` invalidates those index-dependent entries when status changes. Keep this cache distinct from review content keys and from a document's saved-text baseline.
