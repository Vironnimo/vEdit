# vEdit project context

## Project

vEdit is a native Windows code editor for working across projects alongside coding
agents. The `vedit` GUI and `vedit-cli` local control client share the Rust crate;
CLI edits operate on open documents and remain drafts until explicitly saved.
The German `README.md` describes user behavior; the product UI is English.
Active profiles live under `%APPDATA%/vEdit/recovery`; isolated fixtures use
`VEDIT_PROFILE_DIR` (see `platform.md`).

## Architecture

**Stack:** Rust edition 2024, Rust/Cargo 1.97.1, eframe/egui 0.36.2 with Glow for
the shipped application. Cargo.lock fixes dependencies; `vendor/winit` is a
patched dependency, not application code. Python 3.9+ and Git 2.38+ support the
repository tools; PowerShell and Windows MSVC/SDK tools support native packaging.
Node/npm package the pinned basedpyright server under `tools/basedpyright`.

```text
src/main.rs            GUI startup, crate modules, window lifecycle
src/app.rs             App state, frame integration, worker result ownership
src/<feature>.rs       Core modules or app child modules selected via #[path]
src/cli_main.rs        Local control client entry point, shared CLI protocol
scripts/              Developer checks, packaging, installation, worktrees
tests/                Integration fixtures and headless visual references
vendor/winit/         Maintained Windows backend patch
```

Extend the existing owner. The flat file layout does not imply independent
top-level modules: inspect declarations in `main.rs`, `app.rs` and `cli.rs`.
Use about 1,000 lines as a review signal for a source file, not a mechanical split
requirement. Split by cohesive internal responsibilities behind the same contract.

**Background work:** Disk I/O, Git, indexing, project search and language services
stay off the UI thread. Results must still belong to the originating document,
revision, request and project when applied; see `application.md`.

**Document ownership:** Documents are shared across panes; cursor, selection,
view and scrolling belong to each pane. See `documents.md` and `projects.md`.

**Persistence:** Preserve unsaved work, external-change protection, atomic saves
and the recovery comparison baseline. A queued request does not prove completed
loading or saving; see `documents.md` and `control.md`.

## Domain Maps

Read the relevant root maps before domain work; supplementary references are
loaded only for their stated trigger (`AGENTS.md` -> Load context for the task).

| Map | Source paths | Covers |
|---|---|---|
| application.md | `src/app.rs`, `src/keybindings.rs`, `src/app_menu.rs`, `src/settings_navigation.rs`, `src/status_messages.rs` | App integration, command dispatch, settings and messages |
| editing.md | `src/editor.rs`, `src/vim.rs`, `src/multicursor*.rs`, `src/editor_input.rs`, `src/indentation.rs` | Text operations, selection sets, input order and undo |
| documents.md | `src/workspace.rs`, `src/recovery.rs`, `src/save_state.rs`, `src/disk_sync.rs` | Documents, disk formats, save conflicts and recovery |
| projects.md | `src/split.rs`, `src/workspace_ui.rs`, `src/explorer_*.rs`, `src/navigation.rs` | Project ownership, panes, explorer and navigation |
| search.md | `src/search.rs`, `src/find.rs`, `src/find_worker.rs`, `src/search_cache.rs` | Buffer find and background project search |
| rendering.md | `src/render.rs`, `src/markdown.rs`, `src/palette.rs`, `src/text_statistics.rs` | Rendering, preview, typography and statistics |
| git.md | `src/git.rs`, `src/git_panel.rs`, `src/git_graph.rs`, `src/git_gutter.rs` | Git subprocesses, status, history and diff presentation |
| reviews.md | `src/change_review.rs`, `src/change_review_feedback.rs`, `src/agent_marks*.rs` | Change reviews and user mark collections |
| language-services.md | `src/lsp.rs`, `src/language_*.rs`, `src/problems.rs` | LSP lifecycle, diagnostics and language intelligence |
| execution.md | `src/tasks.rs`, `src/project_tasks.rs`, `src/project_launch.rs`, `src/task_output_ui.rs` | Task and launch processes, output and stopping |
| control.md | `src/cli*.rs`, `src/control*.rs`, `.agents/skills/vedit/` | Agent CLI, transport, receiver and operating skill |
| platform.md | `src/startup.rs`, `src/profile*.rs`, `src/single_instance.rs`, `src/window_chrome.rs`, `vendor/winit/` | Windows lifecycle, profiles and native backend |
| development.md | `scripts/`, `.cargo/`, `.githooks/`, `tests/`, manifests | Build, packaging, test infrastructure and repository tooling |

## Conventions

**Coordinates:** Byte offsets, Unicode scalar positions and LSP UTF-16 positions
are distinct. Disk loading normalizes text; CLI edits can contain raw CRLF.
Conversion belongs to the relevant text/protocol owner, not a guessed cast.

**Compatibility:** Preserve native and Vim Undo, dirty buffers, agent drafts and
saved layouts. Historical review documents record their original scope and
version; they do not establish present test counts or requirements.

**Temporary work:** Use `.tmp/<task>/` inside the current checkout. Cargo's
`.cargo/config.toml` and PowerShell's `scripts/project-temp.ps1` isolate temporary
paths and stop parent Git discovery. Each checkout builds into its own `target/`;
do not configure a second target directory or share directories via junctions.

## Development

From a Windows checkout with Rustup, MSVC/Windows SDK, Git and Python installed:

```powershell
rustup show active-toolchain
cargo build --locked
git config --local core.hooksPath .githooks
```

`cargo run --release` starts the real editor and can forward files into the live
session. Read `.vorch/workflows/live-testing-workflow.md` before launching an
isolated test instance. Do not stop or restart the user's editor for validation.

For repository worktree operations read `.vorch/workflows/worktree-workflow.md`.
For an authorized push read `.vorch/workflows/push-workflow.md`; creating local
commits is not authorization to push. For a runnable package or installation read
`.vorch/workflows/release-workflow.md` and use the existing release script.

## Testing

Rust's test harness runs inline and owner-specific `*_tests.rs` tests. Headless UI
tests use egui_kittest 0.36.2 with WGPU and references in `tests/snapshots/`.
Ignored tests declare SDK prerequisites or manual performance workloads; they are
not part of the normal suite. See `development/testing.md` before changing tests.

```powershell
cargo test --bin vedit reopen::tests -- --test-threads=8
cargo fmt -- --check
cargo test --all-targets -- --test-threads=8
cargo clippy --all-targets -- -D warnings
git diff --check
python scripts/domain_maps.py
```

The survey requests `WGPU_BACKEND=gl` by default on Windows, matching the existing
visual harness. Use the scoped backend choices and serial fallback in
`docs/development.md` if necessary.
Never update snapshot baselines during the final verification run.

Commit hooks perform static checks only. The development map describes the
staged-file checker, complete push checks and test survey. There is no hosted CI
configuration in this repository; report local verification as local verification.

## Context

- Windows is the shipped target. Native dialogs, Snap and physical DPI changes
  require desktop checks; headless images do not verify them. Before backend or
  dependency changes read `vendor/winit/VEDIT_PATCH.md` via `platform.md`.
- `docs/development.md` is the human developer guide; maps route agents to owners
  and contracts. Existing reviews remain historical evidence under `docs/`.
- Live editor control uses the repository vEdit operating skill (`control.md`);
  source checkout tools and portable application contents are different surfaces.
