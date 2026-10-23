# Agent control

The local CLI exposes editor state and actions through a bounded protocol to one selected vEdit window.

## Overview

- `src/cli_main.rs` is the executable entry point. `src/cli.rs` owns request/response types, argument parsing, validation, input decoding, schema selection and command execution; `src/cli_help.rs` owns the command catalog and focused help, and `src/cli_schema.json` holds structured examples.
- `src/cli_transport.rs` owns endpoint discovery, authenticated local framing, strict JSON decoding and delivery. `src/cli_spawn.rs` starts the sibling GUI when the selected operation permits it. Window/profile identity belongs to `platform.md`.
- `src/control.rs` is the GUI-thread adapter and dispatch gate. Its children `src/control_documents.rs` and `src/control_queries.rs` implement document mutations and bounded observations. Feature adapters remain with their existing owners: marks/reviews (`reviews.md`), launches (`execution.md`) and notifications (`application.md`).
- `.agents/skills/vedit/` is the operating skill, bundled as `skills/vedit/` in portable builds. `docs/agent-interface.md` retains protocol rationale and dated evaluation records. Maps route implementation work; they do not replace the operating skill.

## Interfaces

Ordinary commands and structured requests reach the same typed `Request` validation and receiver. Unknown fields and duplicate JSON object keys are rejected before dispatch. Extend the existing typed operation and its owner rather than adding a second state-mutating route.

Operation responses are one JSON document with `ok`, `result` and failure `error`. Exit 0 means read/applied, 1 failure and 2 partial execution. This is an application response, not a socket-delivery receipt; individual operations can still report pending loads, saves or jobs. Completion evidence and uncertain-delivery rules are in `control/protocol.md`.

Tab/pane IDs are process-local and dependent calls retain their window selection. Project commands resolve an explicit open workspace, then the deepest open project containing caller `cwd`, then the active folder project. They report the selected workspace and source without implicitly switching the visible project. Folderless Other files is not a folder-project query target.

Document reads/mutations can address inactive projects by tab ID. Explicit focus/navigation and editor command actions can select the document's project. Layout actions operate on the active project's panes and reject foreign pane/tab IDs; layout ownership belongs to `projects.md`.

Edits to existing document text require the observed revision and validate the whole edit before effects. Positions use one-based Unicode scalar coordinates; edit/selection ranges are exclusive while review annotations are inclusive. Agent edits remain undoable drafts until explicit saving. Preserve these receiver contracts when changing the editing or document owner.

## Constraints & Gotchas

- User interaction gates distinguish observation from mutation. An open dialog, Vim search or tab drag can block an action; notifications and specific owner-dialog operations have deliberate exceptions. Reuse the gate instead of dismissing user interaction to admit a request.
- Status observations expose `status_message` and `message_details` as null or `{text,error}`. The first is the latest message, the second the stable open snapshot; `dialog:message_details` blocks mutations while observations and notify remain available. The presentation owner is `src/status_messages.rs`; `src/cli_round_trip_tests.rs` checks the actual CLI boundary.
- Ordered batches preserve already-applied effects and stop on failure/partial execution by default. They neither roll back nor replay completed actions; invalid batch members are rejected before dispatch.
- Settings patches validate as a whole. Some supplied maps replace overrides rather than merging them; retain that meaning in UI, CLI and skill guidance.
- Cached Git/diagnostic observations expose readiness; empty output alone is not proof of a clean repository or completed analysis. Project search reads saved files, while buffer find reads current text.
- CLI mutations do not offer implicit overwrite or permanent deletion. Save As targets a nonexistent destination; file deletion uses the Recycle Bin and guarded background jobs.

## Verification pointers

Inline `src/cli.rs` and `src/cli_transport.rs` tests cover parser, validation and transport admission. `src/cli_workflow_tests.rs` covers direct file input/discovery and Unicode encoding. `src/control_documents.rs` tests cover actual mutation, revision, CRLF, Undo and batch effects; `src/agent_projects_tests.rs` covers caller routing/background presentation. `src/cli_round_trip_tests.rs` exercises the actual CLI executable against an isolated receiving editor. Feature-specific receiver tests live with their owner.

## Agent-facing text

This domain owns CLI help/schema, receiver recovery guidance and the bundled operating skill. Before changing, adding or deleting their sentences, read `control/agent-facing-text.md`; update the relevant reason alongside the text. It also routes agent-readable startup and feature response text to their semantic owners. Existing rationale coverage is explicitly partial; do not infer a historical failure for an unrecorded sentence.

## References

Read these only when the task matches.

- Changing transport, input decoding, completion signals, batching or client compatibility -> `control/protocol.md`
- Changing agent-readable help/schema, recovery guidance or the operating skill -> `control/agent-facing-text.md`
