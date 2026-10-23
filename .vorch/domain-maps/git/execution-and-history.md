# Git execution and history

## Process contract

`src/git.rs` funnels commands through `run_git` and `run_bounded_with_input`. Query timeout is 15 seconds; mutations allow 30 minutes. Both stdout and stderr are drained concurrently, and bounds apply while draining as well as while the main process runs. Once Git exits, descendants retaining its pipes receive a two-second grace period. Detached helpers outside those pipes are not covered by that condition.

The default output cap is 8 MiB. Status has a 64 MiB cap and, on overflow while enumerating all untracked files, retries with untracked folders collapsed. `RepoStatus.untracked_collapsed` keeps that loss of detail observable. Do not turn bounded output or parse errors into an apparently complete empty result.

The runner disables interactive credential prompts, pins parsing-sensitive Git settings and uses `GIT_OPTIONAL_LOCKS=0` for reads. A mutation still runs user hooks and Git's normal operation. `Cancellation` propagates through the worker's `cancellable` scope to its subprocesses. Windows process ownership and timeout cleanup live in the runner, not in panel callbacks.

`with_index_lock_hint` locates the actual worktree index lock, including linked-worktree `.git` indirection. It preserves Git's own lock error and otherwise appends recovery context when a lock remains. It does not remove the lock or claim rollback. Tests include `cancelling_a_commit_stops_its_hook_and_explains_the_remaining_index_lock` and the linked/main-worktree lock-hint regressions in `src/git.rs`.

## Discovery and refresh ownership

`git_repositories::RepositoryOverview` groups an active project's main/linked worktrees and caches listings across project switches. Its work plans own their inputs; workers return request-tagged results. The planner allows at most two jobs, refreshes listings on a 15-second interval and unopened-worktree status on a 30-second interval while a navigation surface is visible. Hover details share the same job budget; open projects reuse their existing status.

`src/app.rs` owns ordinary status refresh and history-page results. Request IDs, repository roots, history generation and expected offset are admission checks, not display-only metadata. Starting a Git action invalidates older status/indicator requests for every affected open project. The last complete view remains visible until the post-action refresh.

`HistorySnapshot` retains traversal tips, refs, upstream and reachability information. `current_branch()` narrows traversal roots to the current branch and upstream while retaining tag decorations. `history_page` traverses the frozen IDs, so moving refs between pages cannot reorder a loaded snapshot. `src/git_graph.rs` extends the prepared graph only for an accepted appended page.

Automatic refresh pauses while the window is unfocused or minimized. A visible history refresh can replace changed refs while preserving loaded depth and anchoring the reading position to the prior visible commit. An unchanged refresh preserves loaded pages and pending pagination. Hidden history retains its consistent pageable snapshot until reloaded.

Evidence: `src/git_history_tests.rs` covers active-branch/upstream scoping, background pauses, external commits, preserved scroll/depth, changed-ref replacement and stale page rejection. `src/git_repositories_tests.rs` covers cached discovery, unopened status, failed-job backoff and stale results.

## Comparison boundaries

`resolve_review` compares a working tree with HEAD (the empty tree before the first commit), a commit with its first parent (empty tree for a root commit), or HEAD with the merge base of an explicit base or local `main`/`master`. A working-tree comparison has no new object ID. `change_set` and `review_patches` preserve binary/large/deferred states instead of presenting unavailable patches as empty text.

`base_version` serves gutter comparisons with a separate bounded text read; its cache may depend on the index when no committed version is available. `git_gutter.rs` invalidates those index-dependent entries when status changes. Keep this cache distinct from review content keys and from a document's saved-text baseline.
