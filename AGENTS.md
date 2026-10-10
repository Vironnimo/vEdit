# Working in vEdit

vEdit is a native Windows code editor written in Rust (edition 2024), using
eframe/egui. It has two binaries: `vedit` (GUI) and `vedit-cli` (local editor
control). The product UI is English; the user README is German. Start with
`README.md` for the intended user behavior. `docs/development.md` describes build,
test, snapshot and packaging workflows. Older versioned review documents are
historical evidence, not current requirements or current test counts.

Paths and build/test commands below refer to a source checkout. A portable bundle
contains documentation and `skills/vedit/SKILL.md` for live editor control, but
does not contain the Rust sources or development scripts.

## Find the owner

- `src/app.rs` owns application state, worker result handling and UI integration.
  Most UI features are child modules selected with `#[path = "..."]`, rather than
  a conventional `src/app/` directory. Add behavior to its existing owner.
- `src/editor.rs`, `src/vim.rs`, `src/editor_input.rs`, `src/find.rs` and
  `src/search.rs`: text operations, Vim, input ordering and search. Byte offsets,
  Unicode scalar indices and LSP UTF-16 positions are different coordinate spaces.
- `src/workspace.rs`, `src/recovery.rs`, `src/save_state.rs`: disk I/O, format
  preservation, conflict checks and recovery. `src/split.rs` owns pane-local
  navigation over shared documents; `src/reopen.rs` owns closed-file history.
- `src/keybindings.rs` is the command/shortcut registry. Keep its dispatch,
  `command_action` in `app.rs`, menu hints and README aligned for new commands.
- `src/lsp.rs`, `src/language_support.rs`, `src/language_settings.rs`,
  `src/project_tasks.rs`, `src/tasks.rs`: language services and project commands.
- `src/git.rs` owns Git subprocesses; `git_panel.rs`, `git_graph.rs` and
  `git_diff_view.rs` present their results. `src/markdown.rs` owns Markdown preview.
- `src/cli.rs`, `cli_schema.json`, `cli_transport.rs` and `control*.rs`: CLI and GUI
  receiver. Agent usage lives in `.agents/skills/vedit/`; protocol rationale and
  past evaluations live in `docs/agent-interface.md`.
- `vendor/winit` is intentionally patched. Read `vendor/winit/VEDIT_PATCH.md`
  before window/backend changes or dependency upgrades; preserve the patch until
  its behavior has a verified replacement.

## Preserve the user's work

Keep disk I/O, Git, indexing, project search and language services off the UI
thread. Background results must still belong to the originating document,
revision, request and project when applied. A queue acknowledgement is not a
completed load or save.

Documents are shared across panes; cursor, selection, view and scroll state are
pane-local. Preserve native and Vim Undo, input event order, Unicode positions,
dirty buffers, agent drafts and existing session layouts. Disk-loaded text is
normalized, but CLI edits can contain raw CRLF. Include that path when changing
line operations.

Saving must retain external-change protection and atomic replacement. Agent edits
remain drafts until explicitly saved. Recovery preserves the original comparison
baseline; reading newer disk content is not permission to overwrite it. Save As
uses the source for formatting and the destination only for overwrite checks.

For live vEdit interaction use `.agents/skills/vedit/SKILL.md`; ordinary repo
changes can use file tools. In a portable bundle use `skills/vedit/SKILL.md`
instead. Never kill or restart the user's editor merely to run
tests or package a release. The release script preserves a running editor; a
restart is a separate user action. Tests should use isolated temporary fixtures,
not the user's current session or repositories.

## Validate and deliver

Finish completed, validated work with focused local Git commits without waiting
for another prompt. Commit at meaningful task boundaries so finished changes do
not accumulate between tasks. Before staging, inspect the diff and separate the
task's changes from pre-existing or concurrent work; stage only reviewed changes
that belong to the task. Never include unrelated or unfinished work merely to
make the working tree clean. If a commit is blocked, report the reason and what
remains uncommitted. The final response names the commits and actual validation.
Pushing or publishing requires authorization beyond this local-commit rule.

Start with a targeted regression for behavioral bugs. Once the affected checks
pass, a change spanning editor state or UI integration should finish with:

```powershell
cargo fmt -- --check
cargo test --all-targets -- --test-threads=8
cargo clippy --all-targets -- -D warnings
git diff --check
```

On Windows, use `WGPU_BACKEND=dx12` for the headless render tests if they
crash in the NVIDIA OpenGL driver. The full parallel suite has been
verified with that setting; see the scoped PowerShell command in
`docs/development.md`. Serial tests also provide a verified fallback.

Use proportional checks for documentation-only or small, reversible edits.
Ignored tests require particular SDKs or are manual benchmarks; do not blanket
run them. Inspect rendered changes before replacing snapshot baselines, and run
the final suite without snapshot-update variables. See `docs/development.md`.

When delivering a runnable update, keep `Cargo.toml`, `Cargo.lock`, README,
relevant docs and packaged contents consistent. Use `scripts/build-release.ps1`
instead of copying only one binary. Report actual validation and remaining limits;
headless egui tests do not verify native Windows dialogs, Snap or physical DPI
transitions. Do not rewrite historical review results as if they describe the new
build.
