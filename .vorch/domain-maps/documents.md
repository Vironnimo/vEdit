# Documents

The document domain protects open text, disk formats and recovery while loads,
saves and external file changes complete asynchronously.

## Overview

- `src/app.rs::EditorTab` owns document identity, content, saved comparison,
  revisions, draft and load/save state. The app also starts and completes
  document jobs and constructs session snapshots.
- `src/workspace.rs` owns document reads, decoding, fingerprints and atomic
  writes. Its indexing/explorer/search responsibilities belong to `projects.md`
  and `search.md`; the file name is not an ownership boundary.
- `src/disk_sync.rs` observes external versions, performs undoable clean reloads,
  tracks conflicts and presents saved/disk comparisons.
- `src/recovery.rs` owns recovery records and ordered atomic publication/removal.
  `src/save_state.rs` presents durable document status independently from transient
  application messages.

Panes reference shared documents and history; their cursor, selection, view and
scroll remain local through `src/split.rs`. Layout/session reconstruction and
closed-file history in `src/reopen.rs` are covered by `projects.md`. Profile
locations and migration belong to `platform.md`.

## Data Model

Current text, saved text, disk fingerprint and recovery baseline have distinct
roles. Dirty state compares the buffer with its saved text; a fingerprint guards
disk replacement. A recovered buffer must retain its recorded comparison
baseline even when startup has observed a newer disk version.

Load/save completion addresses the originating document ID rather than the
currently visible tab. A save returns the text snapshot actually written; later
typing can leave the document dirty after a successful save. Loading, saving and
queued requests are distinct states, also exposed through `control.md`.

Agent edits stay drafts until explicitly saved. Undo or a disk version that makes
the buffer clean ends that draft state. Autosave and recovery are independent:
turning off autosave or retaining an agent draft must not disable recovery.

## Interfaces

`workspace::read_document` decodes supported UTF-8 files into normalized text and
format metadata. `workspace::save_document` uses an expected destination
fingerprint; no expected fingerprint means create without overwriting an
existing destination. Its separate format-source argument preserves original
mixed endings during Save As. Source formatting and destination overwrite
permission must not be conflated.

The app's `worker_or` paths preserve pending-state cleanup on failure; see
`application.md`. `disk_sync::DiskCheck` additionally binds results to the path
and fingerprint observed when the check began. A rename, reload or completed save
can invalidate a late disk result.

## Constraints & Gotchas

- Preserve atomic replacement and external-change checks when changing save
  behavior. File existence, timestamps or a fresh read alone are not overwrite
  permission; content fingerprints also detect same-size changes.
- A disk conflict preserves the editable buffer and blocks saving to its
  original path until resolved. Clean external reloads remain one Undo step in
  both histories and retain each pane's logical reading position.
- `workspace.rs` (~2,750 lines, including tests) also owns unrelated filesystem
  concerns. Treat it as a review hotspot and follow the function's domain before
  extracting code or changing a shared filesystem helper.

## References

Read these only when the task matches.

- Changing decoding, saves, Save As, autosave, disk conflicts, recovery records,
  restoration or cleanup -> `documents/saving-and-recovery.md`
- Changing native/Vim history during document replacement
  -> `editing/input-and-history.md`
