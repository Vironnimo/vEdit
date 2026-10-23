# Agent-facing text: ownership and recorded reasons

## Scope and evidence

The existing operating skill, references and `docs/agent-interface.md` are kept in place. This file records verified reasons for selected consequential sentences, not a new wording review or a claim that every runtime sentence has an empirical history. Where a row cites implementation/tests, its reason is contract-derived; it does not assert an observed agent failure. Historical first-use and context-cost evidence remains in `docs/agent-interface.md` with its original versions and limitations.

Before changing an unlisted sentence, read its source, semantic owner and relevant historical record. Record an evidence-backed reason for the change; an unknown historical reason stays unknown. Do not invent a failed trial, claim universal reliability from one trial, or duplicate the command catalog here.

## Owners

- `.agents/skills/vedit/SKILL.md` and its five `references/*.md` files teach operation, scope and verification. `agents/openai.yaml` supplies skill display metadata and the default prompt. `docs/agent-interface.md` -> `CLI discovery and text workflows (0.4.19)` routes to `docs/cli-audit-0.4.19.md`; earlier versioned measurements remain historical.
- `src/cli_help.rs` renders focused command help, `src/cli_schema.json` supplies request examples/rules, and the private `src/cli/` parsing, validation and client units add validation/recovery messages. Typed requests in `src/cli.rs` and shared validation are the executable contract, not a second command list in this map.
- `src/cli_transport.rs`, `src/control.rs`, `src/control_documents.rs` and `src/control_queries.rs` provide admission, uncertain-delivery and recovery guidance. Semantic sources: `control.md` and `control/protocol.md`.
- `src/agent_marks_control.rs` and `src/change_review.rs` provide mark/review responses. Semantic source: `reviews/receipts-and-comments.md`; operating references: `references/agent-marks.md` and `references/review.md` under the skill.
- `src/project_launch.rs` provides launch revision/lifecycle/output responses. Read `execution/launches.md` for those semantics. `src/startup.rs` owns the GUI executable's offline HELP; read `platform.md` for window identity/startup. These sources have no complete sentence-level historical rationale inventory here.

## Recorded sentences

Text is identified by its exact sentence or unmistakable start. Paths named as skill/reference sources are relative to `.agents/skills/vedit/`.

| Text | Reason |
|---|---|
| `SKILL.md`: "Its CLI reads in-memory documents and changes the visible window; text edits remain unsaved drafts until explicitly saved." | Prevents equating a successful edit with a disk write. `src/control_documents.rs` -> `agent_edits_survive_autosave_and_only_explicit_save_writes` verifies the distinction. |
| `SKILL.md`: "A revision conflict changes nothing: read the current buffer and reassess the edit." | Prevents replay against a changed document. Edit validation checks the observed revision and all ranges before effects; stale-revision and invalid-final-range tests in `src/control_documents.rs` preserve the original text. |
| `SKILL.md`: "These codes do not establish background completion." | Exit codes summarize the response, while loads, saves and file jobs have separate completion fields. `src/cli_main.rs`, receiver code and `control/protocol.md` establish this; the historical 0.4.0 first-use trial explicitly observed pending loads. |
| `SKILL.md`: "After uncertain delivery or a stopped shell, inspect relevant state before repeating a mutation" | Transport send/read failure can occur after application. `src/cli_transport.rs` retains unknown-outcome errors; `src/control_documents.rs` verifies batches keep already-applied edits. This is not a general retry protocol. |
| `SKILL.md`: "Keep the same window ID for dependent calls because tab IDs belong to one process." | Prevents carrying local IDs across independent windows. `src/cli_main.rs` selects the profile before dispatch; `src/cli/client.rs` refuses ambiguous execution. `docs/agent-interface.md` -> `Installed commands and independent windows (0.5.3)` records the contract. |
| `SKILL.md`: "Run commands from your own working directory" | Caller-directory routing selects the deepest open project, including nested worktrees, without changing the visible project. `src/agent_projects_tests.rs` verifies explicit/caller/visible precedence. |
| `SKILL.md`: "Do not restart the user's editor to recover a CLI error; report it and preserve their session." | Startup/version guidance must not be read as permission to interrupt a session. This preserves the repository's explicit user-work rule; no sentence-specific model failure is recorded. |
| `references/editor-control.md`: "A revision conflict applies no edit." | Makes the range-edit recovery contract explicit at the detailed operation point; `src/control_documents.rs` tests atomic edits and conflict behavior. |
| `references/agent-marks.md`: "Never treat the first page or a truncated preview as the whole collection." | Collection pages and snippet continuations are independently bounded. `src/agent_marks_receiver_tests.rs` verifies full Unicode/CRLF retrieval; the historical 0.4.8 trial retrieved 103 items and an oversized snippet. |
| `references/agent-marks.md`: "Retained batches always report `range_current:false`" | A receipt freezes coordinates and does not verify a live source. The retained-batch receiving test checks changed live buffers; `docs/agent-interface.md` records the conservative metadata repair in 0.4.8. |
| `references/review.md`: "A working-tree review does not include unsaved buffers" | Git comparisons read saved repository state; the review separately lists drafts. `src/git.rs` comparison loading and `src/change_review_feedback.rs` supply the two views. |
| `SKILL.md`: "`Starting` or `Running` proves process state, not a working UI or service." | Process creation/ownership is distinct from application readiness. `execution/launches.md` owns this contract; this row records a contract-derived reason, not an observed first-use failure. |
| `src/cli_transport.rs`: "The request may have been applied. Inspect vEdit's status before retrying a change." | A broken response does not undo dispatch. Keep this uncertainty when rewriting transport errors; transport tests include a disconnected GUI and clients waiting during shutdown. |
| `src/agent_marks_control.rs`: "The handoff could not be saved; marks and the previous receipt are unchanged." | Take/Ack report success only after atomic session persistence. The failed-durable-take/ack receiver test verifies rollback of receipt and live marks, including untitled attachment caches. |

The remaining help/schema sentences and domain errors require verification when touched. Their absence from this table is an evidence gap, not authorization to discard their behavior or wording without reading the owner.
