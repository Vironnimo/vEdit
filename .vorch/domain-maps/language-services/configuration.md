# Language configuration and executables

`LanguageServerSettings::validate` assigns each enabled extension to one language
owner and validates the JSON/settings draft before it becomes active. Applying
server settings restarts affected analysis; document edits remain independent.
Default configurations cover Rust, Python and Dart, while user configurations
can supply other language IDs, extensions, executable arguments and settings.

`LanguageServerSettings::resolve` builds `lsp::ServerConfig`. Arguments are
separate process arguments. On Windows, known SDK/npm launchers are unwrapped by
`unwrap_windows_launcher`; the LSP process layer rejects unresolved `.cmd` and
`.bat` servers rather than running their paths as shell source.

Python discovery tries installed basedpyright, pyright and pylsp servers before
the bundled basedpyright JavaScript/Node pair. The bundled search checks the
application's `tools` directory and the source checkout's `tools` directory.
The project task configuration selects the interpreter used for both task
execution and analysis. Its automatic order is `.venv`, `venv`, `env`, then the
platform Python command; explicit relative paths are project-relative, while
plain executable names use PATH. Interpreter changes request a Python-language
restart. Python settings use `python.pythonPath`; pylsp/Jedi receives an
environment path only when it can be resolved, not an unresolved command name.

Dart uses the installed Dart/Flutter executable and its language-server mode;
Rust uses rust-analyzer. Portable server packaging belongs to `development.md`,
not to the runtime resolver. Keep task interpreter/SDK selection aligned with
`execution/tasks.md` when changing resolution.

Configuration regressions live in `src/language_settings.rs` and
`src/project_tasks_tests.rs`. `docs/development.md` describes ignored SDK tests:
rust-analyzer needs Rust's `rust-analyzer` and `rust-src` components; Python tests
need Node and the pinned `tools/basedpyright` package, with Python additionally
needed for virtual-environment coverage; the Dart fixture reads
`VEDIT_TEST_DART_SERVER`. Inspect each ignore reason before selecting an SDK test.
