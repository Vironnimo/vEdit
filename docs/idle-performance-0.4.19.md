# Idle CPU investigation

The reported problem was recurring CPU spikes without editing. A read-only
30-second sample of the running 0.4.18 editor reproduced peaks up to 3.86% of
total machine CPU (12 logical processors). The sample measured the editor process,
excluding Git and language-server children. Working set stayed approximately
190.8 MiB during this short sample; it does not establish long-term memory behavior.
Source control was visible alongside a Python document, with two projects open.

## Changes and evidence

- Automatic Git refresh used an animated spinner, which requested continuous
  full-window repaints while Git subprocesses ran. A headless UI regression
  failed before the fix and identified `egui/widgets/spinner.rs` as the repaint
  source. Refresh now displays a static activity mark and the existing worker
  completion wakes the UI. Regressions cover both populated and empty history.
  Explicit operations such as fetching or loading more commits retain feedback.
- Status and branch tracking now share one porcelain-v2 status invocation. A
  tracked-branch fixture previously launched five Git processes for these values;
  it now launches one. Clean working trees no longer run two empty diff commands.
  Changed trees run only the comparisons needed by their staged/unstaged status.
  Real temporary repositories compare results for unborn and detached branches,
  divergence, renames, untracked files and merge conflicts.
- The language-server supervisor previously woke every 20 ms, even without
  commands or server output. Bounded wake signals now interrupt an idle wait for
  commands, responses, write acknowledgements and shutdown. The fallback process
  check is once per second; earlier request/write deadlines still take priority.
  This reduces scheduled idle wakeups from 50 to 1 per second per server, without
  imposing that delay on incoming work. The existing event queues stay bounded.
- The glyph cache previously rebuilt and copied the complete layout job before
  testing for a reusable result. It now checks text, revision, rendering options,
  font atlas and syntax generation first. Ten unchanged source frames previously
  caused ten additional layout constructions; the regression now requires zero.
  Fractional widths use the same rounding as egui, so equivalent widths reuse the
  same layout. Existing document, glyph and syntax-memory bounds remain in place.

## Validation and limits

Targeted checks cover Git process counts and behavior, Git UI settling and stable
header geometry, source/plaintext cache reuse and invalidation, and language-server
wakeups, backpressure and deadlines. The opt-in basedpyright integration test also
checks real Python diagnostics, completion and definition against an isolated
temporary project.

The complete parallel suite passed with `WGPU_BACKEND=dx12`: 1,276 editor tests
and 59 CLI tests, with 13 and 1 opt-in tests skipped respectively. The Python
language-server integration test was run explicitly and passed separately.
`cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings` and
`git diff --check` passed as well.

The old optional debug Rust tab-latency benchmark exceeded its ten-second syntax
worker timeout and produced no usable timing result. Work-count regressions are
the performance evidence; they are not an after-update CPU percentage measurement.
The user's running editor was preserved and continues to use its old executable
until the user closes and launches it again.

Git refresh cadence and external-file metadata checks were retained. The CLI
listener's existing polling and language servers' own CPU use were not changed.
This investigation does not claim zero background activity or rule out long-term
memory growth outside the caches reviewed here.
