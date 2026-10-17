# Performance audit, 0.4.25

This audit looks at vEdit left open all day while agents edit, commit and merge:
files changing on disk, Markdown preview, the Source control panel with its
periodic status and history graph, project switching and occasional editing.
The aim was fewer idle wakeups and frames, cheaper periodic refreshes and cheaper
re-layout after edits or reloads, with behavior unchanged.

## Method

All numbers are local before/after comparisons on isolated temporary fixtures,
not whole-application CPU percentages or measurements of a running editor.

- Timings use optimized (`--release`) headless tests and exclude native GPU
  presentation.
- Idle frames were counted through egui's repaint callback, with an emulated
  vsync of at most one frame per 1/60 s, after a 5 s warm-up.
- Git process counts come from the test-only command counter. Git timings use
  real temporary repositories with 2,000 commits, 200 branches, remotes and
  tags, in paired alternating runs. On this machine each Git process costs about
  67 ms (Git 2.43.0.windows.1), so the process count dominates refresh time.

## Changes

- **Control listener.** The CLI control listener used a nonblocking accept with a
  10 ms sleep, so an idle editor woke this thread about 32 times per second (16
  wakeups in a 500 ms test window). It now blocks in accept, and shutdown wakes
  it with one loopback connection. Idle wakeups dropped from about 32 per second
  to 0.
- **Periodic Git refresh.** An unchanged repository launched six Git processes
  per refresh. Status already reports the HEAD commit, and the history ref
  listing already names local main or master, so the refresh now reuses both. It
  skips the ahead/behind count when HEAD is the base commit, and refs that moved
  after the status read fall back to a separate HEAD read.
  - On main: 6 to 3 processes, 462 to 265 ms of worker time per refresh.
  - On a feature branch: 6 to 4 processes, 445 to 310 ms.
  - Tests compare the result with separate reads for unborn, detached, orphan,
    master and trunk repositories, tags and `origin/HEAD`.
- **Retained paragraph layouts.** The syntax cache serves idle frames from its
  retained layout without asking egui, so egui's one-frame paragraph cache was
  empty by the next edit or disk reload, and the whole document was shaped
  again. A retained layout now keeps its paragraphs, so the next layout shapes
  only the paragraphs that changed.
  - Highlighted 120 KiB source: edit frame 73 to 8.7 ms, colored follow-up
    frame 60 to 5.6 ms.
  - Plain 192 KiB file: edit frame 43 to 4.5 ms.
  - The paragraphs add about 19% to a highlighted file's retained layout (3.8 MiB
    on 20.1 MiB). They count against the existing 64 MiB cache budget and the
    32 MiB per-layout limit. Larger layouts keep using egui's cache as before.
- **Unchanged disk checks.** Every worker result requested an immediate repaint,
  and egui answers each such request with two frames. A disk check that found no
  change now waits for the next scheduled frame. Any other result, including a
  changed file, still wakes the window at once. Steady idle frames per minute,
  with three open files in a non-Git project:
  - Source control hidden: 58 to 42.
  - Source control visible: 70 to 58.

## Checked without change

- **History panel.** History rows and graph with 3,000 commits and 200 refs cost
  0.12 ms per frame.
- **Project switching.** Switching to a project whose open document is already
  laid out costs a 0.2 ms frame. The first display of a 129 KiB source that was
  never laid out costs 28 ms once, for full shaping. Highlighting follows in the
  background.
- **Startup.** A fresh headless app loaded and indexed two projects with three
  120–193 KiB sources each in 26 ms, on workers. Its first frame took 44 ms,
  including font setup. Window, GPU and session restore are not part of this
  measurement.
- **Recovery flush.** Every 30 s, the recovery flush writes the session file with
  fsync on the UI thread. It takes 2.6 ms median with clean tabs and 5.1 ms with
  one dirty 26 KB tab.

## Gaps left

- **Remaining idle frames.** These come from the 2 s heartbeat and from Git
  refresh results, which still wake the window even when nothing changed.
  Skipping that wake needs a snapshot comparison that also covers error,
  progress and pending-refresh states. A settled headless idle frame costs about
  0.2 ms of UI and 0.1 ms of tessellation, so this pass did not take that risk.
- **Working tree list.** The list is not virtualized: 1.5 ms per frame at 1,000
  changes and 11.3 ms at 5,000, while it is visible and repainting. Keyboard
  focus, retained off-screen focus, scrolling to the focused row and the folder
  tree make an identical-behavior virtualization a larger rework.
- **Markdown preview.** Off-screen blocks still allocate every frame, at 2.7 ms
  per frame with 4,000 sections.
- **Very large plain documents.** Documents whose layout exceeds 32 MiB, around
  100,000 lines, cost about 9.6 ms plus 4.5 ms of tessellation per repainted
  frame, because egui hashes the full job each frame. Fixing this needs viewport
  layout.
- **Feature-branch comparison.** A feature branch still runs one ahead/behind
  count per refresh. Caching by commit pair was not done, because shallow
  deepening, grafts and replace refs can change the counts without changing
  either commit.
- **Repository detection.** Each refresh still runs one repository detection
  (`rev-parse --show-toplevel`), so a repository that appears, moves or needs
  parent approval is noticed.
- **Recovery flush.** This 2.6–5.1 ms every 30 s stays on the UI thread. Moving it
  would change the ordering guarantees of recovery writes around shutdown.
- **History refs.** Assigning refs to a history page is O(commits × refs) on the
  worker. This is minor.
- **Not covered.** No long-running memory soak, native GPU profile or physical
  DPI transition is claimed.

## Validation

The full suite ran with `WGPU_BACKEND=dx12`: 1,337 editor tests and 60 CLI tests
passed, with 20 and 1 opt-in tests skipped. The isolated CLI round-trip tests
also passed with `VEDIT_CLI_TEST_BINARY` set to the built debug `vedit-cli`.
`cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings` and
`git diff --check` passed, and no snapshot baseline changed.

The new benchmarks can be rerun individually:

```powershell
cargo test --release --bin vedit benchmark_idle_repository_refresh -- --ignored --nocapture
cargo test --release --bin vedit source_edit_after_idle_frames_benchmark -- --ignored --nocapture
```
