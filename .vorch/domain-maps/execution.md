# Execution

Execution runs saved project files in background processes and presents task and
application output without turning the editor into an interactive terminal.

## Overview

- `src/tasks.rs` owns the public command/event API, background worker and
  cancellation lifecycle. `src/tasks/output.rs` owns pipe readers, UTF-8
  decoding and shared output budgets; `src/tasks/process.rs` owns process-tree
  cleanup. They contain no UI code; regressions live in `src/tasks/tests.rs`.
- `src/project_tasks.rs` owns task profiles, interpreter/environment selection,
  start prerequisites and each project's `ProjectRun`.
- `src/project_launch.rs` owns application lifecycle, asynchronous launch-config
  persistence, selected variants, rolling logs and the launch UI.
  `src/cli_launch.rs` defines and validates the shared launch configuration/actions.
- `src/task_output_ui.rs` owns cached output presentation and clickable locations;
  `src/output_links.rs` parses locations and resolves their paths.
- App's `src/app_workers.rs` polls task completions; `src/task_controls.rs`
  integrates task controls and Output. CLI routing belongs to
  `control.md`. File writes and external-change protection belong to `documents.md`.

## Terms

Shared Task and Launch terms live in `.vorch/GLOSSARY.md`; their process lifetime
and output contracts are described below.

## Interfaces

Every project owns at most one `ProjectRun` task and one `LaunchState` application;
these can run simultaneously, and different projects run independently. `Other
files` is not an executable project. Output belongs to its originating project,
including when an agent starts work in a background project.

`tasks::CommandSpec::Program` carries an executable and separate arguments.
`Shell` is reserved for explicitly authored custom commands. Built-in tasks and
launch options do not interpolate file paths into shell code. `RunningTask`
exposes `TaskEvent` through a receiver; cancellation and dropping the handle
request process-tree termination on its worker.

`Started` means an application process spawned, not that a service is ready.
`Error` is diagnostic and is followed by `Finished`; retain the latter as the
completion boundary after process exit and output draining. A successful start
response or a stop request alone does not prove a running/terminated process.

## Constraints & Gotchas

- Tasks run saved disk content, have a ten-minute timeout, and cap captured output
  at 1 MiB/10,000 lines. Applications have no timeout and retain a rolling recent
  log with stable absolute positions for CLI pagination.
- Start prerequisites belong in project/task integration, including the recheck
  after asynchronous launch configuration reads. They must not silently save
  documents or approve agent drafts.
- Windows workers use hidden processes and Job Objects, with taskkill fallback;
  dropping a project/app handle ends its owned process tree. External services
  outside that tree, including externally managed containers, are not controlled.
  A launcher script must remain alive until its application ends.
  Task guards terminate remaining descendants even after the parent exits;
  Git's successful-command release policy in `src/git/command.rs` is different.
- Output-pane stdin is closed. `open_terminal` intentionally creates a visible,
  independent PowerShell window whose lifetime is separate from task handles.
- Output links resolve on a worker, trying project root before command working
  directory. Applying a result checks project root and request ID; clicking an old
  link must not navigate a newly selected project.

## Agent-facing text

This domain implements launch response text in `src/project_launch.rs`, including
revision conflicts, start/stop state and output continuation. Before changing its
Agent-facing sentences, read `control/agent-facing-text.md`; keep the rationale
with the control protocol and the lifecycle semantics in the references below.

## References

Read these only when your task matches.

- Changing task profiles, interpreter environment or save gates
  -> `execution/tasks.md`
- Changing application starts/stops, launch persistence or output pagination
  -> `execution/launches.md`
- Changing CLI responses or operating guidance -> `control/agent-facing-text.md`
- Changing file-save protection used for launch configuration -> `documents.md`
- Changing background result integration -> `application.md`
