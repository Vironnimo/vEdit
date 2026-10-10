# Performance audit, 0.4.20

This audit extends the earlier idle-CPU investigation to editing, navigation,
project search, Explorer, Git, Markdown, language services and command output.
The aim is to remove repeated document/tree work from ordinary interactions,
while preserving selection, Unicode positions, undo, background-result ownership
and the running editor's documents.

## Changes and measurements

The following are local before/after comparisons of the named operations on
identical fixtures. They are not whole-application speedups or CPU percentages.
Core benchmarks use optimized builds and warmed, alternating comparison runs;
headless UI measurements exclude native rendering and GPU presentation.

- Text replacement, Tab and smart Enter resolve character-to-byte positions once
  and copy UTF-8 slices, instead of repeatedly expanding complete documents into
  character arrays. On a 3.85 MB document with 50,000 Unicode/CRLF lines and a
  cursor in the middle, replacement took 14.55 to 2.95 ms, Tab 23.04 to 3.54 ms,
  and Enter 17.51 to 3.52 ms. These measure edit helpers, excluding layout/undo.
- Vim cursor clamping avoids a full character array and line index. Ordinary
  motions inspect the required lines; insert replay uses prefix/suffix slices.
  On the same fixture, cursor setting took 12.34 to 1.14 ms and a j/k pair
  50.31 to 18.04 ms. A differential harness compared 597,190 operations across
  1,555 small Unicode/CRLF documents with the previous implementation.
- Project search maintains line/column state and bounds preview scanning around
  each match. A 1,849,500-byte Unicode line with 900 matches took 127.50 to
  8.71 ms with optimized code and release dependencies. Disk reading, decoding
  and regex matching are included; UI result drawing is not.
- Explorer reuses flattened rows and requested directories until its immutable
  directory snapshot or expanded folders change. Preparing 50,100 unchanged
  rows took 3.22 ms to 4.61 microseconds in an optimized harness. The first
  cached build costs 11.14 ms and retains owned row data; closing the project
  releases it. Rendering those rows is a separate cost.
- Git history obtains the active branch from its existing ref query and reads
  HEAD once. An unchanged refresh uses two subprocesses instead of four. Real
  temporary repositories took 212.9 to 92.3 ms with one ref and 329.4 to
  200.1 ms with 1,000 refs. Process creation contributes to these Windows timings.
- Git graph layout replaces repeated lane searches with indexed lookup and
  handles linear sections without copying the full frontier. Layouts were
  checked row-for-row against the previous implementation: 10,000 linear commits
  took 3.79 to 1.73 ms; 2,000 commits with 250 parallel lanes took 132.2 to
  37.6 ms; 500 lanes took 368.4 to 60.9 ms.
- Git panel tree comparisons happen when worker results arrive; unchanged frames
  reuse keyboard navigation rows. Both changed paths and changed line statistics
  invalidate the cache. History generation, paging and folder expansion are
  included in invalidation; existing offscreen keyboard focus is retained.
  Closed context menus skip setup until invoked; file tooltip strings are
  formatted when actually displayed. In the final paired debug UI benchmark,
  1,000 changed files took 64.21 to 56.47 ms per frame and 5,000 took 351.01 to
  316.16 ms. This modest improvement shows that widget construction still
  dominates large change lists.
- Markdown tables retain column widths and row offsets and shape only visible
  cells on warmed frames. The regression previously observed 1,002 cell layouts
  for a 500-row table on every frame; it now limits work to the viewport. Stable
  cell IDs preserve selection when earlier rows leave the viewport. First layout,
  width changes and font changes still measure the entire table.
  The existing debug headless 500-row benchmark fell from 16.84 to 0.78 ms per
  warmed frame, including layout and widget generation.
- Source gutters and overlays reuse a visual-row index instead of scanning all
  preceding rows when scrolled far down. Each index is bounded at 8 MiB and
  participates in the existing 64 MiB syntax-cache budget. Oversized indices
  fall back to the old traversal before allocating, and weak layout identity
  avoids retaining discarded glyph geometry.
  For the last 60 rows of 20,000 source lines, the debug mapping benchmark took
  0.439 to 0.002 ms per frame, excluding shaping/painting. Its 67.9 MiB galley
  also exercises layouts above the glyph cache's 32 MiB entry limit.
- Incremental LSP synchronization counts newlines and the last line's UTF-16
  length without repeatedly scanning every character. A 2 MiB multiline document
  took 1.94–2.29 to 0.53–0.56 ms; full-sync servers skip this calculation entirely.
  Stopped servers no longer rebuild other languages' diagnostic vectors each frame.
- Command-output readers continue draining pipes after the shared display budget
  is exhausted, but stop decoding discarded chunks. A 69 MB stream with identical
  retained output took 22.24 to 2.07 ms. Output display reuses the shaped text of
  its existing selectable label. Text, style, font atlas and DPI invalidate
  the layout; hidden output releases it and retained geometry is capped at
  32 MiB. Larger layouts keep the original rendering path without persistent
  retention. The paired output benchmark reports cache hits and estimated
  layout bytes so that results cannot imply reuse above that limit.
  With the final bound, 128 KiB of output (17.75 MiB geometry) took 0.490 to
  0.091 ms per debug headless frame, with 220 hits and no misses after warmup.
  The 256/561 KiB cases exceeded the budget and were not cached: 0.962 to
  0.975 ms and 1.915 to 2.032 ms respectively. The bound favors predictable
  retained memory over faster repeated display of very large logs.

## Validation

Regression coverage includes Unicode and raw CRLF edits, replay and cursor
positions, search previews, Explorer refresh/expansion/release, Git subprocess
counts and graph layout, changed line statistics, Markdown scrolling/selection,
font/DPI/text invalidation, oversized source indices, output truncation/draining
and stopped language servers. The final full run with `WGPU_BACKEND=dx12` passed
1,303 editor tests and 59 CLI tests, with 18 and 1 opt-in tests skipped
respectively. The real basedpyright diagnostics/completion/definition test passed
separately in an isolated temporary project. All six named performance benchmarks
were run explicitly. The welcome snapshot was visually inspected and updated
only for the version label changing from 0.4.19 to 0.4.20.
`cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings` and
`git diff --check` also passed.

Focused optional benchmarks can be rerun individually; do not run all ignored
tests indiscriminately:

```powershell
cargo test --release --bin vedit benchmark_search_many_matches_in_long_unicode_line -- --ignored --nocapture
cargo test --release --bin vedit benchmark_explorer_unchanged_rows -- --ignored --nocapture
cargo test --bin vedit markdown_table_preview_benchmark -- --ignored --nocapture
cargo test --bin vedit deep_source_viewport_index_benchmark -- --ignored --nocapture
cargo test --bin vedit benchmark_large_working_tree_panel -- --ignored --nocapture
cargo test --bin vedit benchmark_task_output_cache -- --ignored --nocapture
```

## Remaining limits

A read-only 30-second sample of the running 0.4.19 build still showed periodic
peaks up to 2.83% of total machine CPU, mostly on one thread. Its working set
stayed approximately 115.56–115.65 MiB during that sample. This does not establish
a memory leak or its absence. Windows refused the requested CPU stack recording
because the profiling policy could not be enabled; no machine policy was changed.
The exact remaining native idle spike has therefore not been attributed to a
stack or verified eliminated by this audit.

Large source files still use egui's full-document text layout. Editing helpers
still copy resulting text, and Vim still has character-array paths. Source index
fallback above its budget still scans preceding rows. Git widgets are not fully
virtualized. CLI listener polling and language servers' own CPU use remain.
Saving/recovery and the already-virtualized Git diff were reviewed without a
demonstrated hotspot justifying behavioral changes; their protections are retained.
No long-running leak soak test, native GPU profile, physical DPI transition or
interactive desktop selection test is claimed here.

Packaging preserves a running editor. The new code becomes active only after the
user closes and launches it again; these measurements do not report post-restart
live CPU usage.
