# Mark receipts and review comments

## Captures and live attachment

`UserMark` stores exact snippet text, note, persistent ID, captured source identity and fingerprint. Its model range is zero-based Unicode scalar offsets with an exclusive end; exported line/column positions are one-based Unicode scalar positions. Text chunk offsets also count Unicode scalars, not UTF-8 bytes.

The snippet remains frozen when its highlight follows a nonoverlapping edit. An overlapping edit invalidates the live range. A named reopened document must match the complete recorded source before reattachment; a persisted runtime ID is not enough to reattach an untitled document. `source_open`, `stale` and `range_current` answer different questions. A retained batch always reports `range_current:false`, even when an open file happens to resemble its capture.

The collection is independent of ordinary selection, native Undo, Vim registers, clipboard and agent-authored annotations. Capturing via CLI requires an existing nonempty source selection; it does not select an arbitrary range on the user's behalf.

## Delivery and durability

`take` moves current marks into one retained batch identified by a request token. Repeating that token replays the retained receipt instead of consuming new marks. A different take while the batch awaits acknowledgment fails with `batch_pending`; an expired token fails rather than collecting fresh content.

`ack` permits the next take but leaves the acknowledged batch readable/restorable until replacement. Marks created while a batch is being read stay live. `restore` merges IDs and preserves current notes; clear/remove also keep a restoration collection. Read-only listing does not consume highlights.

`control_agent_marks` retains the previous live model while applying Take/Ack to a copy. It serializes the session and uses the recovery writer before reporting `persistence:saved`. On failure it restores the original model, including attachment caches. Ordinary mutations use `persistence:queued`. These are session-metadata guarantees, not document-save acknowledgments.

Pages default to 100 items, accept 1-500 and target 256 KiB. Each snippet preview is limited to 4096 scalars; `text_next_offset` leads to chunks accepting 1-65536 scalars. Item-page continuation and snippet-text continuation are separate. A client must finish both before acknowledging a batch. Exact first-item/page behavior is implemented in `agent_marks_page`; do not infer a transport-wide hard limit from the page target.

Evidence: `src/agent_marks_receiver_tests.rs` checks same-token retries with new live marks, receipt expiry, merging restoration, exact Unicode/CRLF continuations, frozen-range metadata, session restart and failed durable Take/Ack rollback. `docs/agent-interface.md` -> `0.4.8 first-use agent trial` records a historical single first-use trial of full collection and continuation; it is not a current general reliability claim.

## Diff comments

`CommentDraft::new` in `src/change_review_feedback.rs` freezes the selected diff rows and comparison when the comment opens. The excerpt has unified `+`, `-` and space prefixes without hunk headers. Submission uses that frozen origin even if a later comparison would have different paths or lines.

`ReviewLocation` names repository, resolved old/new revisions, old/new paths and inclusive bounds for the selected lines on each side. A null path means that side is absent; null bounds mean no selected line on that side. Bounds may cover gaps between hunks. A null new revision means the working tree at review load.

The compatibility `range` is a new-side navigation hint with an exclusive end. A removal-only selection anchors near a surviving new-side line and provides no evidence that any new-side text was selected. Old saved comments lacking `location` retain unknown old-side coordinates; do not reconstruct invented locations from the hint.

`src/change_review_feedback_tests.rs` verifies deleted lines/files, renames/additions, frozen comparisons, JSON/recovery and take/ack/restore. `src/agent_marks.rs` defines the persisted shapes; change those shapes with backward-compatible defaults where prior snapshots lack fields.
