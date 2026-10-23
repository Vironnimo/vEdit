# Vorch adoption — 23 October 2026

This report records adoption of the local Vorch workflow against vEdit baseline
`721852b` (0.5.6). It is a dated result, not a replacement for current development
instructions or a claim about later application changes.

## Scope and knowledge migration

The repository now separates agent working rules (`AGENTS.md`), project context
(`.vorch/PROJECT.md`), shared vocabulary (`.vorch/GLOSSARY.md`), domain ownership
and contracts (`.vorch/domain-maps/`), and repeatable procedures
(`.vorch/workflows/`). `CLAUDE.md` delegates to the same agent rules. EditorConfig
and Git attributes define text formatting and platform line endings.

The original agent instructions were classified and converted, preserving all
recorded product and delivery requirements. The German user manual, English
product UI, shared documents with pane-local navigation, draft agent edits,
external-change protection, recovery baselines, background worker ownership,
local commits, explicit push authorization, single `target/`, repository-local
temporary files and preservation of the running editor remain requirements.
No prior user requirement or process document was dropped. Generic template
defaults were adapted where those explicit requirements took precedence.

The README opens with the current user guide. Its release notes were moved into
[changelog.md](changelog.md) without deleting their content. The documentation
[index](README.md) distinguishes current guides from historical evidence.
The existing operating skill, historical reviews, Cargo/package locks, patched
`vendor/winit`, licenses, images and existing development scripts were retained.
Release packaging now includes all top-level Markdown documents in `docs/` and
adjusts source-only links for the portable bundle.

## Domain ownership

| Map | Main responsibility |
|---|---|
| application | App state, frame integration and command dispatch |
| editing | Text operations, input order, Vim and undo |
| documents | Disk formats, saves, conflicts and recovery |
| projects | Projects, panes, explorer and navigation |
| search | Buffer find and background project search |
| rendering | Text layout, Markdown, palette and statistics |
| git | Git subprocesses, status, history and diff presentation |
| reviews | Change reviews, feedback and agent mark collections |
| language-services | Language servers, diagnostics and intelligence |
| execution | Tasks, launches, output and process stopping |
| control | CLI, transport, receiver and operating skill |
| platform | Windows lifecycle, profiles and native backend |
| development | Build, test, packaging and repository tools |

The project index maps each domain to its source paths. Domain authors checked
claims against source and tests; two fresh reviewers independently sampled about
120 root-map claims and additional supplementary references. Corrections included
the boundary between tasks and launches and which edits require a revision.
The agent-facing-text rationale covers 14 inspected fragments; it does not claim
to reconstruct every historical wording decision.

The corpus checker reports 13 root maps (59,131 characters), 14 supplementary
files (65,460 characters), a 7,627-character project file and a 2,481-character
glossary. Every file is within its budget; no broken references or remaining
template placeholders were found. `git add --renormalize .` produced no changes.

## Repository tools

- Commit and merge hooks check the actual staged blobs, without rewriting or
  restaging files. Checks cover Rust formatting, Python/JSON/TOML syntax,
  whitespace, map budgets and indexed references, including deletions/renames.
  TOML parsing needs Python 3.11+; Python 3.9/3.10 reports that limitation.
- Rust 1.97.1, Rustfmt and Clippy are pinned. Whole-project compilation, Clippy and
  tests remain separate from the fast commit hook.
- Authorship hooks use the explicitly approved `Julian B. <virojb@web.de>`.
  The initial 149-commit history scan found no prohibited tool attribution.
  No published history or global Git configuration was changed.
- Repository worktrees use `codex/<name>` branches, a shared operation lock,
  isolated landing checkouts and checked cleanup. Tests cover concurrent primary
  commits, dirty primary files, changes to the source during merge, hook failures,
  junctions, nested repositories and Windows paths. Codex-managed worktrees stay
  under the app's lifecycle tools rather than the script's deletion mechanism.
- The push command checks committed `main` in a private checkout, retains logs,
  and can run locally with `--no-push`. It validates the exact commit even with
  inherited Git routing or replace refs. Real publication still requires user
  authorization. There is no hosted CI configuration to report as green.
- Release and isolated live-testing workflows describe the existing packaging,
  native checks, profiles, SDK boundaries and running-editor preservation.

The push mechanism was tested with disposable local repositories and substitute
checks; its configured Rust/static/PowerShell checks were exercised separately.
The entire private-checkout pipeline was not run a second time with another full
Rust build. No remote push, real installation or runnable release was performed.

## Test survey and infrastructure

The stable-Rust survey builds every target, discovers cases, gives each runnable
test a fresh process and private user/application/Git configuration, and enforces
a 30-second per-case deadline with owned process-tree cleanup. Temporary data
stays below this checkout's `.tmp/`; build output stays in `target/`.
The testing guide documents what isolation does and does not establish.
Existing ignored SDK/manual cases remain ignored. No assertions or snapshot
baselines were removed or regenerated, and durable disk synchronization remains
real.

Both full attempts are retained:

| Run | Passed | Failed | Timed out | Ignored | Wall time |
|---|---:|---:|---:|---:|---:|
| Initial survey | 1,666 | 73 | 1 | 27 | 365.969 s |
| Corrected survey | 1,740 | 0 | 0 | 27 | 214.950 s |

The initial run exposed the `ignore` crate inheriting the source checkout's
`.tmp/` exclusion into temporary fixtures. Git's discovery ceiling does not
constrain that crate. All three workspace walkers now disable ancestor-ignore
traversal in test builds only. Production behavior is unchanged; local/nested
ignore files remain tested. Focused workspace regressions passed (34 passed,
2 ignored), followed by the successful complete survey. Tests compiled with this
seam do not verify ignore inheritance from outside the fixture root; changes to
that production behavior need an explicit native fixture check.

The initial `polished_screenshots` case exceeded 30 seconds amid the failing
fixtures. It passed in 11.735 seconds in the corrected run with the original
deadline and snapshot references. The original timeout remains recorded; the
fixture failure was reproduced, while the timeout was not independently isolated
from cold rendering and concurrent load.

The successful survey discovered 1,767 tests: 1,740 runnable and 27 ignored.
Build time was 9.430 seconds and test execution 204.217 seconds. Per-case timings
include process startup and fresh caches under eight-worker contention. They are
not timings from one shared-process Cargo test run. The 27 ignored entries are
manual performance workloads or live SDK/executable integration cases; their
names are retained in the raw report and reasons remain next to `#[ignore]`.

### Slowest 20 cases

| Seconds | Status | Target | Test |
|---:|---|---|---|
| 22.219 | passed | vedit | app::git_history_tests::restoring_the_window_refreshes_immediately_even_after_a_recent_read |
| 21.666 | passed | vedit | app::git_panel::repositories::tests::real_worktrees_group_under_one_repository_at_a_bounded_idle_cost |
| 21.008 | passed | vedit | app::git_history_tests::primary_push_button_pushes_to_an_isolated_local_remote |
| 20.499 | passed | vedit | app::git_history_tests::source_control_history_follows_the_worktree_branch_and_upstream |
| 15.685 | passed | vedit | app::change_review::tests::source_control_entry_points_open_commit_branch_and_working_tree_reviews |
| 14.148 | passed | vedit | app::git_history_tests::unchanged_repository_refresh_on_main_launches_three_git_processes |
| 14.121 | passed | vedit | app::git_history_tests::visible_history_automatically_shows_external_commits |
| 13.354 | passed | vedit | app::git_history_tests::resume_coalesces_an_in_flight_read_and_waits_for_a_git_action |
| 13.016 | passed | vedit | app::git_panel::repositories::tests::hovered_worktree_details_count_both_comparisons_and_untracked_files |
| 12.606 | passed | vedit | app::git_history_tests::focused_history_handles_upstream_changes_detached_head_and_unborn_branches |
| 11.872 | passed | vedit | git::tests::refresh_after_status_matches_separate_head_and_main_reads |
| 11.735 | passed | vedit | app::visual_tests::polished_screenshots |
| 11.735 | passed | vedit | app::git_ui_tests::commit_and_push_button_text_is_centered_in_every_state |
| 11.681 | passed | vedit | app::change_review::tests::change_review_screenshots |
| 11.021 | passed | vedit | app::git_history_tests::git_cli_pages_branches_and_history_and_waits_for_safe_switch_completion |
| 10.805 | passed | vedit | app::change_review::tests::keys_scroll_jump_mark_seen_and_open_the_line_on_screen |
| 10.010 | passed | vedit | app::change_review::tests::feedback_tests::changes_after_loading_show_a_calm_refresh_hint_that_keeps_the_position |
| 9.964 | passed | vedit | app::selection_scroll_tests::stationary_bottom_drag_scrolls_and_extends_selection_in_every_editing_mode |
| 9.948 | passed | vedit | app::project_status::tests::real_repository_reports_untracked_staged_modified_and_clean_status |
| 9.824 | passed | vedit | git_diff_view::tests::large_wrapped_unicode_diff_keeps_offscreen_galleys_out_of_font_cache |

Other final validation passed:

- `cargo fmt -- --check` and `cargo clippy --all-targets -- -D warnings`.
- All 29 Python tool regressions, 50.933 seconds.
- All 18 PowerShell temporary-directory checks and 26 isolated installer checks.
- Domain-map validation, placeholder scan, staged and working-tree whitespace.
- Windows PowerShell 5.1 and PowerShell 7 parsing of the release script; execution
  of its actual documentation-copy block, including portable link substitutions.
- Local commits through the real hooks, printing the `Commit check` report.

No additional native shared-process full suite was run during adoption. The
survey does not expose interference between tests in one process. Headless
rendering does not verify Windows dialogs, Snap or physical DPI transitions.
The initial survey requested DX12, but existing harness code explicitly selects
OpenGL; the environment setting alone does not establish the actual renderer.

## Fresh-agent orientation trial

The user approved a short trial using the `agent-eval` skill. Three fresh Codex
agents used the harness's small-model default, `gpt-6-luna`, with one run per
goal. Each saw an isolated source copy with its own Git root and normal agent
configuration, without this conversation or expected answers. Requests were in
German and explicitly asked for analysis without implementation.

| Goal | Result | Tool calls / planned ideal |
|---|---|---:|
| Independent cursors in two panes | pass | 24 / 4 |
| Preserve changes on save conflicts | pass | 6 / 4 |
| Portable build while editor stays open | pass | 4 / 4 |

Judging used the call transcripts and end-state file hashes: all three identified
the correct owners, contracts and relevant checks, and none changed files or
started the editor. The first read the maps but continued through redundant source
searches and repeated reads (20 extra calls). The second made two additional
source-verification calls. The third followed the release route directly.
An explicit task-level stopping cue after finding the owner, contract and test
entry would be a candidate improvement for a later trial; more general reading
instructions alone did not prevent over-reading in the first run.

This measures orientation, not implementation quality or reliable performance
across repeated runs. The fixture snapshots predate the final small tooling
repairs. No operating-skill loading behavior was under test. To honor the
repository's explicit temporary-file policy, the eval location guard was scoped
to `.tmp/vorch-eval/`; every run still had its own copy and Git metadata. Raw
transcripts and test reports remain local and are not committed with user data.

## Architecture observations and follow-up recommendation

Ownership could be named for all 13 domains. The flat Rust directory is not a
collection of independent modules: app child modules and shared state make the
declarations and existing owner boundaries important. No confirmed ownerless
domain or unnecessary pass-through layer was established by this adoption.

Several files exceed the approximately 1,000-line review signal. Whole-file line
counts, including inline tests, were: `app.rs` 9,667; `vim.rs` 6,234; `git.rs`
5,727; `cli.rs` 4,046; `git_panel.rs` 3,633; `lsp.rs` 3,471; `workspace.rs` 2,757;
`change_review.rs` 1,972; `tasks.rs` 1,057. These are audit leads, not evidence that
a mechanical split would improve the design. A focused deep-module audit of app
integration and worker-result ownership is recommended before architectural
refactoring; keep cohesive owners and their observable contracts.

A focused test audit is also recommended for Git/process lifecycle cases and
render setup. Git tests contain real waits of 2.3, 3.5 and up to 6.5 seconds to
prove a stopped child cannot later write a marker. Replace those only with an
equally strong observable completion contract. GPU/process cold starts and the
slowest tests should be investigated before changing timeouts or assertions.
Neither audit nor wholesale application refactoring was part of adoption.

## Integration with concurrent vEdit 0.6.0 work

Commit `c2b4575` subsequently added multicursor editing on `main`. It was merged
into the adoption branch; the new manual section remains in README and its
release paragraph joins the preserved changelog. The editing maps were updated,
and a fresh reviewer confirmed ten new claim groups against source. Project
ownership and the project index were adjusted. These are integration updates;
the original measurements above retain their original baseline and counts.

The integration survey with DX12 requested discovered 1,800 tests and finished in
199.623 seconds: 1,771 passed, one failed, 28 ignored, no timeout. The sole failure
was WGPU `Device(Lost)` while creating the multicursor test renderer, before any
image comparison. It reproduced with a single test/thread and DX12. The exact
same test passed with `WGPU_BACKEND=gl` and the existing reference image (2.49 s).
The new harness honors the backend environment while the existing visual harness
pins GL. The survey and push runner now default to GL and preserve an explicitly
chosen caller backend; the developer guide retains DX12 as a fallback for GL
driver failures. No application behavior, test assertion or reference image was
changed for this repair.

## Delivery boundary

The adoption was committed on `codex/vorch-adoption` in a managed checkout
because the primary checkout had concurrent application work. At report creation
the primary still had staged/unstaged changes overlapping README and development
documentation; those changes were neither stashed nor included in adoption
commits. Shared hook activation is deferred until the hook files are available in
the primary checkout. The final delivery message records whether integration
subsequently completed. No unresolved confirmed application bug was established
by the map review; observations above are follow-up candidates, not bug claims.
