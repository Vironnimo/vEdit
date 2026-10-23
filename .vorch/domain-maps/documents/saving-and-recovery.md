# Saving, disk synchronization and recovery

## Disk format and save guard

`src/workspace/documents.rs::read_bytes` reads regular files with a size bound and rejects
metadata changes during the read. `decode_document` accepts UTF-8 with an optional
BOM and rejects binary/invalid UTF-8 input. Document files are not the same input
surface as CLI text files, which can accept UTF-16 before sending an edit.
Loaded CRLF/CR text becomes LF in memory, with its original format retained.

`save_document` normalizes raw input endings for encoding, retains the BOM and
chosen line-ending format, writes and flushes a sibling temporary file, then
publishes it. Existing destinations are checked before writing and immediately
before replacement. A new destination uses no-overwrite publication. Read-only
files and symbolic-link destinations are rejected by this path.

Mixed endings are reconstructed only from the verified original source. Save As
uses the destination solely for its overwrite fingerprint, including an existing
destination confirmed through the native dialog. If the format source cannot be
verified, saving can succeed with an explicit LF fallback; the returned format
and user status reflect that fallback. Do not infer formatting from the target.

`document_saving.rs::save_tab_expected` prevents concurrent document load/save jobs, retains
a confirmed Save As for later completion, and checks filesystem-operation and
Git checkout guards. Pull/branch-switch operations defer autosave and reject
explicit saves into their working tree; see `git.md`. The opened path is retained
for ordinary saves, including junction paths, so reopening it finds the same
document.

## Completion and external changes

`document_loading.rs::finish_document_load` accepts a load only while the document is loading, not saving and
has no new edits. A successful save updates saved text and fingerprint from the
written snapshot, then recomputes dirty state against the current buffer. If
newer edits remain, recovery must be rewritten with the new baseline even though
their text revision did not change.

`src/disk_sync.rs::apply_disk_checks` rejects changed paths, changed baselines and
results arriving during loading/saving. A clean changed file follows disk through
`replace_with_disk_version`, which checkpoints native and Vim history and maps
every view's positions. For combined texts above the mapping limit, it preserves
line/column rather than computing the line diff on the UI thread.

Unsaved edits, including a counted Vim insert that would add text when finished,
produce a conflict instead. Compare is read-only; Use disk replaces as an Undo
step; Keep my version adopts the observed disk baseline before saving over it;
Save As preserves the buffer elsewhere. If a buffer already equals the observed
disk text, it becomes clean and its obsolete recovery is removed. A conflict does
not disable ordinary editing or reading through the CLI.

`src/save_state.rs` derives visible state from document facts. Preserve its
distinction between conflict/error, saving, agent draft, new file, unsaved,
reloaded and saved; a transient success message is not authoritative save state.

## Recovery publication and restoration

`src/recovery.rs::encode_document` encodes text, original fingerprint, line
ending and BOM in one record. `read_document` accepts the current record header,
the legacy product header and older raw-text recovery. Legacy raw text has no
provable baseline and cannot authorize replacement of a named file.

One cloned `RecoveryWriter` orders document snapshots, removals and session writes
with stamps. The writer's publication stamp is distinct from a document's text
revision. Removal tombstones prevent late older snapshots from recreating a
deleted recovery file, including when deletion must be retried.

The app also checks recovery generation and revision when accepting completions.
A completed save can change the baseline of unsaved newer text; a late recovery
completion for the old baseline must not mark that text secured. Recovery and
autosave timers are separate: recovery still runs for dirty drafts when autosave
is disabled or held for an agent. Dirty session metadata schedules its own delayed
frame, so it is saved even without open projects or another source of repaints.

On restore, a fresh disk read is an observation. Dirty recovered text receives
its recorded baseline; differing disk content becomes a conflict, and missing
baseline/disk information keeps the original protected. Unreadable recovery
records remain available for inspection, and subsequent recovery uses another
name. Offline projects retain restorable drafts across restarts.

Recovery filenames include a session-specific random component as well as the
process ID. Startup orphan cleanup requires an unreferenced old file, a stopped
owner process and a readable complete record; absent/unreadable session state
must not license broad deletion. Saved or closed documents remove their recovery
off the UI thread with retry behavior. Profile migration is a separate boundary
in `platform.md`.

## Regression entry points

- `src/workspace/documents_tests.rs` covers UTF-8/size rejection, BOM and CRLF,
  mixed endings, existing/deleted Save As targets, same-size external changes,
  read-only files and stale temporary-file cleanup.
- `src/recovery_audit_tests.rs` covers source-versus-target formatting, missing
  format sources, original recovery baselines, legacy recovery, offline projects,
  late load/save/recovery completions, locked cleanup, panicking workers, checkout
  save guards and junction paths.
- `src/disk_sync_tests.rs` covers one-step native/Vim reload Undo, all-pane
  position preservation, conflict choices, agent-draft transitions and stale
  disk results. `src/native_input_tests.rs` also checks typing during a save.
- Inline `src/recovery.rs` tests cover ordered concurrent writes, tombstones,
  record decoding and orphan eligibility. For session layouts and reopened tabs,
  use the test owners in `projects.md` rather than a storage-only fixture.
