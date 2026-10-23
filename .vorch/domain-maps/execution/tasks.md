# Project tasks and process output

## Task selection and saved-file gate

`TaskConfiguration::resolve` selects an explicit profile or, for Auto, the active
file's recognized extension before project markers. Unsupported languages require
an explicit custom task rather than falling back to Cargo. `RunFile` validates
the active file as well as the chosen profile. The Python check compiles source
directly without executing it; it includes `.py`, `.pyw` and `.pyi`, excludes
environment/dependency/VCS directories and linked entries, and avoids stale pyc
cache results. The source constant is the implementation authority for that walk.

`resolve_project_task_in` uses the target project's active document, even when
another project is visible. It rejects an already running task and dirty/saving
documents with paths under the target root, including documents in other open
projects; `RunFile` also checks the active document itself. `start_resolved_task`
opens Output only for the visible project. The custom-output-command path lives
in `src/task_controls.rs`; preserve its checks when changing the shared process layer.

Task configuration is session data, not `.vedit/launch.json`. The environment
exports `VEDIT_ROOT`, `VEDIT_FILE`, `VEDIT_PYTHON` and Python pipe encoding/buffering
settings. Explicit interpreter paths are project-relative; plain names use PATH.
Automatic Python selection checks `.venv`, `venv`, `env`, then the platform Python
command. Selected executable directories precede inherited PATH, and virtual-env
state follows the selected interpreter. Dart can inherit the selected language
SDK, with an explicit project task override taking precedence. See
`language-services/configuration.md` for the analysis side of interpreter changes.

## Worker behavior and output

`tasks::run_task` closes stdin, runs in the specified directory with environment
entries passed directly, drains stdout/stderr on separate readers and polls
process completion/cancellation. On shell exit it also terminates remaining
descendants before joining readers, because descendants can keep pipe handles
open. Do not join readers before ending that process tree.

The bounded output channel uses cancellable sends so a stalled UI cannot prevent
stopping a process. Task output shares its byte/line budget across both pipes;
readers continue draining after truncation and emit one truncation notice. UTF-8
decoding preserves characters split across reads and replaces invalid input.
Application output uses the streaming path and delegates retention to `OutputLog`.

`task_output_ui::TaskOutputCache` reuses layouts while content/style/font context
matches and bounds cached glyph data. Link parsing stays in `output_links`; a
click opens a location, while a drag still selects output for copying.

## Test entry points

- `cargo test --bin vedit project_tasks::tests`: task selection, literal arguments,
  interpreter/Dart environment, persistence and dirty/saving-buffer refusal.
- `cargo test --bin vedit tasks::tests`: process output/exit, cancellation, timeout,
  literal executable arguments, UTF-8 and truncation/draining.
- `cargo test --bin vedit output_links::tests`: compiler/traceback formats and
  root-before-cwd path resolution.
- `cargo test --bin vedit task_output_ui::tests`: cached layouts and link input.

Python subprocess tests and manual cache benchmarks have explicit ignore reasons;
read those before opting in. Integration fixtures must not run a user's projects.
