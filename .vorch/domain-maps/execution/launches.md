# Application launches

## Configuration and ownership

`cli::launch::Configuration` is the shared validation authority for
`.vedit/launch.json`: named profiles identify an executable, separate arguments,
working directory and environment, with an optional default. Decode rejects
unknown fields and duplicate JSON keys. `Configuration::selected` falls back
from the saved local selection to configured default, then the first profile.
Only the local selection belongs to the saved editor session; the configuration
can be versioned with the project. Opening a project does not launch it.

`LaunchState::begin` performs disk reads/configuration writes on a worker. The
state owns its pending action/receiver/reply so completion applies to that project.
The periodic reread detects external config changes; unchanged background reads
need not wake an idle UI. Explicit operations and active applications are polled
more frequently. Do not perform configuration I/O in toolbar rendering.

`read_or_configure` hashes existing content into a nonzero revision below 2^53;
zero means absent configuration. This avoids numeric roundoff in JSON clients.
Earlier unmasked revisions remain accepted after normalization. Configure compares
the supplied revision, validates the new configuration, and uses
`workspace::save_document` with the read fingerprint for atomic conflict-checked
replacement. A malformed file still exposes its revision so it can be repaired.
An open dirty/saving/agent draft of the configuration blocks replacement.

## Start, stop and lifetime

After reading configuration, `apply_launch_loaded` rechecks dirty, saving, loading
and agent-unsaved tabs and the editor close flow before spawning. Paths containing
separators (or starting with a dot) resolve relative to the project root; plain
program names use PATH. The working directory is joined to the project root.
The launch's own environment is passed directly and `VEDIT_ROOT` is supplied by
the editor. Tasks' automatic Python environment is a separate contract.

The initial state is `Starting`; `TaskEvent::Started` advances it to `Running`.
Both mean process lifecycle, not HTTP/port/readiness validation. Spawn errors
remain visible with a subsequent terminal result. Stop can cancel a pending
start before spawning; otherwise it requests cancellation and leaves `Stopping`
until the worker finishes. A new start is rejected while the previous task handle
still exists. Closing the project/editor drops its process handle.

The application is a direct executable invocation. Scripts must name their
interpreter explicitly, propagate failure and remain alive with the child
application. Stop controls the owned process tree; it cannot promise cleanup of
pre-existing or separately managed services. `tasks.rs` owns OS termination.

## Rolling output and continuation

`OutputLog` keeps recent output within the task layer's byte/line constants by
removing complete old lines first. A single oversized line retains its Unicode
tail with an absolute column offset. Retained line numbers never change within a
run; a new run resets numbering to 1. Joined display text is rebuilt lazily.

CLI output pages use absolute one-based line and Unicode scalar column positions.
If old output has been evicted, paging clamps to retained content and reports
skipped lines; continuations remain absolute even for an oversized trimmed line.
The position just after the last line yields an empty page until more output
arrives. Preserve the distinction between log truncation, page truncation and
byte-budget continuation. A read is a snapshot, not a promise of process readiness
or completion; read the accompanying run state.

## Test entry points

`cargo test --bin vedit project_launch::tests` covers UI start/stop, per-project
independence, pending-start cancellation, restart, spawn errors, saved-buffer
gates, revision conflicts and malformed configuration repair, selection recovery,
rolling Unicode logs and absolute continuations. The tests live in
`src/project_launch_tests.rs`.

`docs/development.md` documents the ignored
`launch_cli_round_trip_in_isolated_editor` executable fixture, which uses a private
endpoint and temporary projects for CLI configuration/selection/start/stop/output.
Use that fixture only when the CLI/process boundary needs live validation; use
`development/testing.md` for test infrastructure and isolation.
