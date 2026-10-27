# Language lifecycle and result routing

## Session and transport boundaries

`language_support::sync_languages` removes disabled/changed configurations and
starts wanted sessions. It skips loading tabs, opens a document at LSP version 1,
and transmits later revisions after a 160 ms edit delay unless forced. It advances
the recorded revision/version only when the client accepts the command. A close
is likewise removed from `opened` only after enqueueing succeeds.

`lsp::Protocol` coalesces document updates before initialization, honors negotiated
full/incremental/no-sync and save options, and rejects stale version updates.
Incremental replacement ranges describe the previous transmitted text, not the
new text. Save notifications include text only when the server requests it.

The client uses bounded command, event and reader/writer channels. Its supervisor
waits for wakeups and deadlines; dropping `LanguageServer` requests shutdown and
wakes it. Requests evicted from the bounded pre-initialization queue receive a
cancellation event. Event backpressure preserves lifecycle/results/diagnostics, extends
request/write deadlines, and remains interruptible by shutdown. Read the
`EventEmitter`/`ProcessGuard` in `src/lsp/runtime.rs` and `Protocol` operations in
`src/lsp/protocol.rs` together when changing queues or teardown. Protocol size
limits and request/write timeouts are named constants
near the top of `src/lsp.rs`; do not create a second timeout policy in UI code.

After process exit, the worker waits at most 200 ms for the final stderr read so
the exit event does not race away the cause. `server_exit_message` retains exit
status, project and a detected panic ahead of recent stderr (at most 16 KiB),
without presenting installation advice as the cause of an exited server. This
output stays in session memory. Language controls open its full text through
`open_message_details` in `src/status_messages.rs`; the stable, copyable modal and
its input guards belong to `application.md`, not the LSP transport.

## Diagnostics

`poll_languages` accepts diagnostics only from an active session, the configured
enabled owner (or an in-project support file without an extension owner), a path
not retired by rename/delete, and a version no older than the transmitted one.
The session's contribution is then merged with other sessions for that path.

`DiagnosticsOrigin` maps an exactly matching published LSP version to its
transmitted editor revision. Unversioned diagnostics may be known stale after an
edit but cannot be declared fresh merely because they just arrived. Combined
freshness is false if any contribution is stale, otherwise unknown if any
contribution is unknown. `close_language_paths` removes cached diagnostics and
retires affected paths so late events cannot restore them.

`problems::build_problem_list` orders by severity, file and position. Open text
allows Unicode scalar columns; unopened text retains explicitly marked UTF-16
columns until navigation loads it. The badge excludes hints; next/previous
problem navigation visits errors and warnings. Diagnostic navigation defers
position conversion until the destination document is loaded.

## Interactive requests

`app::poll_languages` and `apply_completion` both validate completion context.
Completion text edits and additional edits are converted from UTF-16, applied
from the end of the document, and grouped through Vim's external-edit boundary.
Keep revision updates and cursor repair with that edit transaction.

`language_intel::Origin` identifies the request and server instance; a restarted
server invalidates pending origins. Hover checks target tab/revision and keeps at
most one request in flight, subsequently considering the newest target. A
stopped server's queued successes cannot settle those origins before cancellation.
A reference result can remain useful after moving the cursor, but only takes focus
if the user is still at the original document/revision/cursor. Symbol requests
keep their document or workspace-query context. Read each feature's result and
palette/update path before generalizing stale-result behavior.

Hover markup distinguishes plain text from Markdown. Reference previews retain
the authoritative UTF-16 position as well as one-based scalar display columns;
opening a location converts against the current loaded buffer. Navigation through
`open_language_location` also records jump history and reveals in the focused
pane.

## Test entry points

Run targeted filters with `cargo test --bin vedit <filter>` as appropriate:

- `lsp::tests`: `src/lsp/tests/codec.rs` checks framing limits, Unicode/CRLF and
  reply conversion; `synchronization.rs` checks sync capabilities, coalescing and
  request failures; `runtime.rs` checks queue pressure, wakeups and shutdown.
- `server_exit_reports_status_and_panic_before_older_warnings`: exit context,
  panic prioritization and retained server log text.
- `language_support::tests` (`src/language_support_tests.rs`): restart isolation,
  stopped/error state, transient error recovery, diagnostic origins, retired paths
  and late definitions from another project or source position.
- `language_intel_tests` (`src/language_intel_tests.rs`): target changes,
  capability absence, request cancellation, references, symbol palettes and hover.
- `lsp::intel_tests` (`src/lsp_intel_tests.rs`): protocol payload parsing, result
  routing, capability advertisement and UTF-16 preview conversion.
- `problems::tests`: ordering, counts, Unicode columns and wraparound navigation.

These source-backed entry points do not imply that external SDK tests have run.
