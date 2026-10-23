# Control protocol and completion

## Local delivery

`src/cli_transport.rs` uses protocol version 1, a loopback TCP listener, a random token and a private endpoint file for the selected window. Endpoint permissions and random-token implementation are platform-specific. The server admits at most eight connections and uses a bounded request channel; only the GUI receiver applies editor actions.

Requests are capped at 1 MiB and responses at 8 MiB. Strict decoding rejects duplicate object keys, including nested request keys, as well as unknown fields. Read complete frames before deserializing; fragmented TCP arrival is not malformed input. Tests in the transport owner cover these boundaries and endpoint ownership on shutdown.

The transport normally waits ten seconds for an application response, or fifty seconds for Git branches/history/switch and change review. A timeout, disconnected handler or incomplete response leaves mutation outcome unknown; it is not cancellation or evidence that nothing happened. Git's mutation process can outlive that client wait (`git/execution-and-history.md`). Inspect the operation's state before repeating it.

The CLI adds canonical caller `cwd` to its envelope. It retries once without `cwd` only when an older receiver explicitly rejects that unknown envelope field before dispatch. This compatibility path must never become a generic retry of timeouts or failed mutations. Other unsupported fields/variants retain their error and version guidance.

## Startup and input

Help and schema work offline. Open, manifest review and workspace commands can start the sibling GUI; background requests and `--no-start` prevent this. Other actions require an existing receiver. More than one live window without an explicit selector fails before startup/mutation. Tab IDs cannot be carried to another selected process.

Text/JSON file inputs accept UTF-8 with/without BOM and UTF-16 with BOM. Encoded and decoded inputs are bounded separately; the encoded transport packet also has its limit. Request-relative paths use the caller directory and manifest paths use the manifest directory. Literal option values are not reinterpreted as global flags; `--` terminates options for positional text beginning with a dash.

## Applied versus completed

| Operation kind | Completion evidence |
|---|---|
| Open or manifest review | Per-item outcome, then tab `loading:false`, no load error; inspect `location_error` for annotations/navigation |
| Save | Tab `saving:false`, `dirty:false`, no error |
| File action | Returned job reaches `complete:true`, `applied:true`, no error |
| Task | Project output reports `running:false` and an exit result |
| Change review | Reply follows comparison load; visible/background state and deferred patches remain separate |
| Launch | Process state is separate from application readiness; see `execution/launches.md` |

`complete:false` means some requested work failed or was not attempted; it does not undo successful items. A replacement manifest review replaces annotations across the intended set, so recovery requires the full intended replacement rather than only failed files.

`Request::Batch` accepts 1-100 ordered actions, validates every member first, and excludes nested batches and asynchronous search/file/Git-query/Git-switch/change-review/launch actions. Once execution starts, default stopping on an error or partial result retains completed effects. Responses include the stopping point and evidence; large nested details can be explicitly omitted rather than silently overflowing the response.

## Pagination and coordinates

List pages use `next_offset`. Buffer read and output use both `next_line` and `next_column` because one line can exceed a byte budget. Byte continuations concatenate directly; complete buffer-read line pages omit their final LF, whereas output retains separators. Keep a single buffer revision across reads. Chunking must not cut a Unicode scalar or CRLF pair.

User-mark delivery has two separate continuations and durable acknowledgments; its canonical contract is `reviews/receipts-and-comments.md`. Project search is bounded but has no next-page contract: truncation means narrowing the query, not incrementing an invented offset.

## Evidence

`src/cli_transport.rs` tests reject invalid tokens/versions/duplicate keys, accept fragmented frames and verify old-receiver retry without double dispatch. `src/control_documents.rs` tests include `ordered_batch_reports_applied_edit_and_stops_at_conflict_without_replay`, `batch_rejects_async_search_before_earlier_edits_can_apply` and agent drafts surviving autosave. `src/cli_workflow_tests.rs` covers UTF-16 and direct draft/edit input; `src/control_queries.rs` -> `long_buffer_and_output_pages_reconstruct_exact_unicode_and_crlf` verifies exact continuation. `docs/agent-interface.md` -> `CLI discovery and text workflows (0.4.19)` and `Observable contract` retain the rationale and version history.
