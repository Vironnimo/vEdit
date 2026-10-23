# Language services

Language services connect project documents to asynchronous language servers and
present diagnostics, completions and code navigation in the editor.

## Overview

- `src/lsp.rs` owns the language-client types, nonblocking handle and protocol
  state. Its private `src/lsp/` units own process/event supervision (`runtime.rs`),
  negotiated sync and request dispatch (`protocol.rs`), bounded reply conversion
  (`replies.rs`) and wire framing/UTF-16/path conversion (`codec.rs`). These are
  internal units of the same owner, not separate language services.
- `src/language_support.rs` owns per-project, per-language `LanguageSession`
  lifecycle, document transmission state, diagnostics ownership and restart.
- `src/language_settings.rs` validates language configurations and resolves server
  executables; `src/project_tasks.rs` supplies the project's Python interpreter.
- App's `src/language_integration.rs` integrates `poll_languages`, definition/completion requests and
  completion edits. `src/language_intel.rs` owns hover, references and symbols;
  `src/problems.rs` owns diagnostic lists and next/previous problem navigation.
- These UI modules are children of `app`, not separate crate-level services.
  Syntax coloring and file classification do not require a running server; see
  `rendering.md`. Navigation and document edits retain their existing owners.

## Interfaces

`LanguageServer` accepts nonblocking queued commands and exposes an event receiver.
Document operations return whether enqueueing succeeded; request ID `0` means the
request could not be queued. Neither result proves that analysis completed.
The worker handles process I/O and emits `LspEvent`; the app polls those events
with the originating project and language attached.

Each `Project` owns independent language sessions. A session tracks transmitted
document revisions separately from LSP versions, and owns its diagnostics before
they are combined into the project's view. Restarting one language must not clear
another language's diagnostics or alter document text. Stopped sessions retain
visible failure state until an explicit restart.

LSP positions are zero-based UTF-16 units; editor cursor offsets are Unicode
scalar indices. Use `lsp::char_to_position` and `position_to_char`, including for
CRLF and supplementary Unicode characters. Navigation converts against loaded
text; a displayed one-based line/column is not an LSP position.

Results must still belong to their request and current UI/document context.
Completion checks include project, path, transmitted revision/version and cursor;
definition requests also guard source revision, cursor and pane widget. Hover,
references and symbols carry request origins identifying project root, language
and server instance, with feature-specific applicability checks. Do not flatten
these checks into a single global "latest result".

## Constraints & Gotchas

- `Other files` has no project language sessions. Rust sessions are wanted for
  detected Rust projects; other configured languages start for matching open
  files. An unsupported or disabled language remains editable.
- A missing optional server capability is an unavailable feature, not a successful
  empty response or a server failure. `RequestFailed` with no error represents
  cancellation/staleness and must still settle pending UI state.
- Diagnostics without a version do not prove freshness. Support-file diagnostics
  can belong to a session even when their extension is not that session's source
  extension. Deleted/renamed paths must not return through late publications.
- Process shutdown, bounded queues and backpressure belong in `lsp/runtime.rs`; moving
  work into UI polling would bypass the client's cancellation and timeout rules.

## References

Read these only when your task matches.

- Changing synchronization, diagnostics, completion or intelligence result routing
  -> `language-services/lifecycle.md`
- Changing server settings, executable discovery, Python/Dart environments or SDK
  integration tests -> `language-services/configuration.md`
- Changing how background results enter app state -> `application.md`
- Changing source navigation or pane targeting -> `projects.md`
