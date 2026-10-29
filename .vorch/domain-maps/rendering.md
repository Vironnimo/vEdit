# Rendering and Reading

This domain turns current document text into source views, Markdown reading views
and statistics while keeping reusable layout and worker results tied to their owners.

## Overview

- `src/render.rs` owns asynchronous syntax highlighting, bounded layout caches,
  source-line/visual-row mapping and reusable text/scroll painting helpers.
- `src/markdown.rs` owns CommonMark parsing, reading layout, links, source-line
  reveal and per-view preview scroll state.
- `src/typography.rs` selects bundled/system fonts and tab-width font families;
  `src/palette.rs` defines shared colors; `src/design.rs` provides shared controls
  and focus/selection painting. Its `src/design_hover.rs` child owns interactive
  details (selection, clipboard, bounded scrolling and pointer lifetime); ordinary
  `on_hover_text` hints remain passive. Use those for paths, navigation and status
  explanations; opt into details only for substantial inspectable content.
  Explorer hints remain beside the pointer, without interactive contents.
  `palette.rs` is not the command-palette owner.
- `src/text_statistics.rs` computes offline token counts and formats statistics.
  Its character totals come from `render::LineIndex`.
- App children `src/source_editor.rs`, `src/editor_view.rs` and `src/app_chrome.rs`
  compose the source editor, status bar and Markdown view with input,
  selection, search, diagnostics and Git decorations. Per-pane view ownership
  belongs to `projects.md`; text changes belong to `editing.md`.

Native windows, DPI transitions and the patched backend belong to `platform.md`.
Command palette behavior and settings dispatch belong to `application.md`.

## Interfaces

`LineIndex` exposes logical source-line counts and Unicode scalar positions.
Visual rows describe wrapped galley rows, so a wrapped row is not a source line.
The index keys on revision and byte length; callers must explicitly invalidate
it for disk reloads that change text without changing that identity. Its
line/column API excludes CR in raw CRLF line endings while character totals count
both scalars. Do not substitute screen row counts for document line counts.

Syntax parsing runs on a worker, outside the live egui context's memory lock.
Results survive inactive tabs, but must still match current document text before
installation. Font and wrapping changes affect layout independently of parsing.
Source highlighting has a deliberate document-size cutoff; plaintext fallback
must preserve editing, search and language analysis. Limits for syntax jobs,
galleys and visual-row caches are separate in `render.rs`.

`MarkdownCache::show` receives current text and document/view identity and returns
requests to open a file or reveal a source line. The app performs the navigation.
Multiple panes can share parsing while retaining their own width, scroll,
selection and table state. Moving a view uses `transfer_view`; document/view
cleanup has separate methods. Do not key reading position only by document ID.

Statistics identify document ID, revision, byte length and selected scalar range.
Tokenization waits for an input pause, with at most one active calculation and
one retained snapshot. Selection-only changes reuse the file count; stale
selection results cannot replace the current selection count. Closing the last
tab releases its snapshot without allowing a second worker to overlap the first.

Details cards receive keyboard events only after a pointer press inside them.
`draw_ui` brackets input with `design::begin_hover_frame` / `end_hover_frame`;
custom rich cards, including language documentation, use the same body and
registration. Keep this before source shortcuts so copying or typing in a card
cannot act on the underlying document. Plain details retain their copy value
while the pointer is inside the card. Column-anchored details exclude the full
sidebar rectangle and shrink to the space toward the window center; when that
space is too narrow, the owning view's click action remains available instead.
Keep list scrolling outside the card's hit area.
When browsing adjacent sources, details switch without a new dwell delay. Only
crossing the gap toward a card retains it briefly; leaving in another direction
closes it. This lifetime belongs to `design_hover.rs`, including suppression of
egui's directional reopening of a source the pointer has left.
Column-anchored details and project-tab hints appear immediately at full opacity,
using the shared instant hover path. Other hints and details retain their normal
delay and animation; instant hints remain passive.

## Constraints & Gotchas

Cached text geometry is not arbitrary UI state. Syntax selection painting keeps
glyph colors from the original galley; retained selections must also keep the
original clip rectangle. Theme/font/wrap changes and changed text at the same
revision have dedicated regressions because plausible cache keys can still
display stale text or wrong geometry.

Visual Block painting uses source spans plus virtual columns from the Vim owner.
Partial tabs and short rows require geometry beyond a contiguous text range;
selection rectangles stay behind the glyphs and follow wrapped visual rows.

Markdown is a reading surface: HTML remains inert, images are links rather than
automatic remote downloads, and link handling explicitly classifies allowed
destinations. Preserve those boundaries when adding rich content. Parsing is
bounded by a UTF-8-safe preview prefix and reports truncation. Table and inline
geometry caches must account for width/font/text changes without reusing a
previous link destination.
Footnote references reveal their definition through the same fragment navigation
as heading links. Prose only creates a hover hint when a link is under the pointer.
Linked image placeholders follow their surrounding link destination; standalone
image placeholders follow the image URL without downloading it automatically.

Font choices keep code and proportional UI faces independent. The bundled code
face is the fallback when a selected Windows font is unavailable; symbol and CJK
fallbacks remain later in the chain. Tab width uses a dedicated font family
rather than replacing tabs in document text. Colors for status meaning remain
independent of the brand accent.

Character counts are Unicode scalars in the live buffer, including whitespace and
raw CRLF, not bytes or user-perceived graphemes. Token counts use the bundled
`o200k_base` vocabulary through `tiktoken-rs`, entirely offline. A selection is
tokenized as independent text. The displayed estimate compares models; it does
not measure an agent conversation or available context. Preserve tokenizer
licenses through packaging (see `development.md`).

## Verification

`src/render.rs` tests cache freshness, UTF-8 edits, line positions, selections
and scrolling. `src/markdown.rs` tests inert content, links, split views, tables,
scroll and source mapping. Both contain explicitly ignored performance workloads;
choose these deliberately rather than enabling all ignored tests.

Useful focused commands are `cargo test --bin vedit render::tests` and
`cargo test --bin vedit markdown::tests`. `src/text_statistics.rs` checks debounce,
stale results and selection counts; `src/typography_ui_tests.rs` checks reading
appearance. `src/visual_tests.rs` and `tests/snapshots/` cover integrated renders.
Render fixtures must settle both syntax and statistics workers before snapshots.

Read `docs/development.md` -> Visual references before changing baselines or
choosing a headless backend. See `development.md` for final verification and
`platform.md` for native behavior that image tests cannot establish.
