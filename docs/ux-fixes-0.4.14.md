# UX fixes — Morrow 0.4.14

- Git diffs wrap long lines to the available viewport width. Old/new line numbers
  appear once per source line, with continuous addition/deletion backgrounds.
  Resizing recalculates the virtualized row heights; Copy patch keeps the exact
  original patch. Off-screen height measurements release their text layouts
  immediately instead of filling the font cache with the whole patch.
- Sidebars measure their header controls before assigning tab width. Their
  actual size stays within the painted frame, removing the unpainted strip at
  the editor boundary while retaining scrollbars and resize controls.
- Opening an editor context menu retains the visible selection as well as its
  logical range. The selection background remains behind the text; cached
  syntax layouts and clipping stay intact. Copy and Mark for agent retain their
  existing behavior.
- Clicking a folder name or disclosure arrow selects and focuses that Explorer
  row. J/K/H/L then navigate from the clicked folder without opening a document.
  Competing pending editor focus requests are cleared, while Find text is kept.
- Git History uses one compact row with the subject followed by a muted author.
  Hashes, branch badges and dates remain available in hover/details. Long subjects
  may clip the author rather than reserving a second column.
- History hover adds changed-file counts and green/red added/removed line totals.
  Git workers read compact numstat output without loading patches; merge commits
  compare with the first parent. Results are cached per repository/commit, with
  bounded requests and storage. Request IDs reject stale results after project
  switches, repository changes or reopening a project. Successful Git refreshes
  allow failed statistics requests to retry.
- Syntax workers share immutable grammar and theme data. Each worker retains
  its own parser and document cache, avoiding repeated grammar initialization
  when multiple editor/test instances start together.

Regressions cover actual context-menu selection painting, full-editor folder
focus, sidebar boundary pixels at native 100%/125% scaling, diff reflow and
virtual scrolling, compact History rows, asynchronous hover caches and real Git
repositories including merges, renames, root/empty commits and binary changes.

Validation: 1,169 editor tests and 48 CLI tests pass, including 51 visual
references. Twelve manual performance/isolated process tests remain excluded
from the normal suite.
Clippy passes with warnings denied; rustfmt and Git whitespace checks pass.

The portable release is built with `scripts/build-release.ps1`. It preserves a
running editor and its documents; restart Morrow to activate the new version.
