# Working in vEdit

These instructions govern the source checkout. In a portable bundle without
`Cargo.toml`, use `skills/vedit/SKILL.md` for live control; the repository maps,
Rust sources and developer scripts are not part of that bundle.

## Load context for the task

At session start, read these core files completely unless their current contents are already in context. Load them once; re-read affected sections when they change:

- `.vorch/PROJECT.md` - project context
- `.vorch/GLOSSARY.md` - project-specific terms

Before working in or discussing a domain, read its root map under `.vorch/domain-maps/` (index in PROJECT.md), plus adjacent maps when ownership or contracts cross domains. Use them to locate owners, contracts, source, and tests. The task's `read:` list is a starting point; add a map only when the task reaches into its domain. Follow the root map's `## References` only for supplementary files whose triggers match the task; do not preload a supplementary folder. Supplementary files never replace the root map or enter the Domain Maps index.

Before changing a domain map or supplementary file, read `.vorch/workflows/domain-map-workflow.md` up to `## Creating or restructuring a map` unless its current contents are already in context; read the rest too when you create, split, merge, rename or restructure a map. Ordinary map reading does not require that workflow.

## Interpret documentation

Distinguish explicit user requirements and engineering contracts from descriptions of current behavior, examples, and proposals. Follow applicable requirements and contracts; a documented implementation or past choice is not automatically a constraint on new work. Existing behavior and design may be revised within the user's requested scope without a documentation exception. When the user asks to rethink or rebuild a system, its documented design, invariants and earlier decisions about it are open for revision; say which ones you would keep and why. A recorded user requirement stays in force until the user agrees to drop it by name.

Source code establishes what is implemented, not what ought to be implemented. Verify behavior claims against source and relevant tests. Correct stale descriptions; when code conflicts with an explicit requirement or contract, address the discrepancy rather than rewriting the requirement to match the code. Keep observations, requirements, and proposed changes identifiable in the docs.

## Communication with the user

### Use established terminology

Use the project's established terms as defined in the glossary and domain maps, such as Document, Pane, Project and Agent mark. Keep these terms untranslated when you talk to the user in another language.

### Present Agent-facing text changes

For runtime Agent-facing text, explain the meaningful changes in the conversation and show the relevant proposed wording verbatim. Include the corresponding current wording when comparison helps the user assess the change.

For larger changes, keep the conversation focused on the passages that materially affect Agent behavior. Make the complete changes available through the source-file diff. Do not create additional files solely to present the review. Show complete wording when the user requests it.

Runtime Agent-facing text includes CLI help, protocol errors and the bundled vEdit operating skill supplied by vEdit to an Agent or Model. Repository governance and development documentation are outside this review rule.

## Agent-facing work

Before writing or reviewing runtime Agent-facing text, load the matching Skill and follow it: `agent-text` for runtime prompts and notices, `agent-cli` for the CLI, `agent-skill-creator` for the operating skill, and `agent-eval` for fresh-agent evaluations.

## Architecture & code

**Few, deep modules:** Prefer clear ownership, small interfaces, and hidden implementation. Extend an existing owner when the capability belongs there. Before introducing a new responsibility, public interface, layer, or abstraction, identify the closest existing owner and explain why the new boundary is warranted. Avoid pass-through layers and abstractions that expose their internals or add little beyond their interface.

Judge complexity by responsibilities, coupling, and what callers must understand, not file count alone. Tests, documentation, components, and internal file splits do not need a new-owner justification unless they introduce an architectural boundary. Split files when it improves clarity and cohesion; do not overload an existing file merely to avoid creating one.

**Technical Decisions** - when making technical decisions, do not give much weight to development cost. Instead, prefer quality, simplicity, robustness, scalability, and long-term maintainability.

## Testing

Tests guard behavior against regressions from concurrent and later work. Every test also costs runtime and maintenance for all later tasks, so keep the suite a curated set of behavior contracts, not a record of every change.

**Write tests for contracts, not for changes.**

- Test behavior at the owning module's public interface: contracts, persisted formats, protocols, CLI output and visible editor behavior. Test an internal helper directly only when it carries its own contract that is impractical to reach through that interface.
- A bug fix extends the existing test that owns the behavior, with a new parameter case or assertion. Add a new test only when no existing test covers that behavior.
- One test per distinct behavior; parameterize only over genuinely different cases.
- Developer scripts and probes get at most a smoke test, unless they protect data or installations (converters, installers, update and worktree tooling).
- Keep tests fast and deterministic: controlled clocks and in-memory fakes instead of real waits, subprocesses, and network. A test that needs more than about a second must justify it. Load and scale variants use Rust's `#[ignore = "manual benchmark: ..."]` convention and run only on demand (PROJECT.md -> Testing).
- Existing Rust persistence tests exercise real syncs. Do not disable durable I/O globally to shorten the suite; keep any future explicit durability seam in the persistence owner and test its contract.
- When a change makes tests obsolete or redundant, delete or merge them in the same change.
- Documentation-only edits need no tests; review content, references, and the diff.

Before writing or changing a test, read `.vorch/domain-maps/development/testing.md`: what to assert, how to wait, and what the shared fixtures isolate.

**Commit and merge hooks run static checks only.** Run the tests covering your change before committing. Changes spanning editor state or UI integration also require `cargo fmt -- --check`, `cargo test --all-targets -- --test-threads=8`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check` before delivery. Complete checks also run for an authorized push. Report failures and their scope; never claim an unrun check passed.

Do not run full suites merely to investigate code. Read the owner and its tests with a concrete question; use a targeted probe to check a disputed behavior. Begin behavioral bug fixes with a targeted regression, extending the existing contract test where possible.

## Dependencies

If a task requires a new dependency, **check first** that no existing dependency already covers the need. Then install it, add it to `Cargo.toml` (or `tools/basedpyright/package.json` for the bundled Python language server), and commit the lock file changes. Do not install packages speculatively - only what the current task requires.

## You maintain the docs & domain maps

Maintain affected documentation as part of the task with small, factual updates:

- `.vorch/PROJECT.md` - architecture, conventions, dev/test setup, domain-maps index, strategic context. Every agent reads it, so a fact whose use is confined to one domain goes into that domain's map instead
- `.vorch/domain-maps/<domain>.md` - a domain's ownership, contracts, or documented behavior changes, including affected supplementary files, or a new domain emerges (a new domain also gets added to the domain-maps index in PROJECT.md)
- `.vorch/GLOSSARY.md` - shared vocabulary under the Terminology rules below
- `.vorch/FLAGGED.md` - git-ignored, never commit it; keep only confirmed, concrete, still-open concerns, with the evidence and affected behavior. During implementation, fix confirmed bugs with a clear, bounded, low-risk solution even outside the original task; defer them to this file only with a concrete reason. Investigate a suspicion before adding it; when reviewing an existing suspicion, check it before removing it. Do not add pending evaluations, missing live checks, speculative future failures, or completion notes. Append at the bottom, or fold into a related entry when one fits; delete an entry once its concern is resolved or verified fixed instead of annotating it.

**Write new developer and agent documentation in English.** Keep the user manual `docs/handbuch.md` in German; the root `README.md` is the English landing page for GitHub. Preserve historical reports in their original language and version context. User-facing chat follows the user's language.

## Terminology

`.vorch/GLOSSARY.md` holds the terms every agent needs in every task: a term belongs there when several domains use it and an agent would otherwise misread it, especially as a concept of its own environment or of another system, such as Document, Pane, Project and Task. A term only one domain uses belongs in that domain map's `## Terms` section, even when the user mentions it; when other domains start using it, move it to the glossary and leave a pointer. Standard vocabulary gets no entry.

An entry, in the glossary or in a map's Terms, says what the term means, not how it works: at most two sentences of definition plus a "Not ..." distinction where a misreading is realistic, and the map or section that owns the concept. Tables, columns, functions, syntax, limits, states and procedures stay out of entries; they belong in the owning map's Data Model, Interfaces or a supplementary file, which the entry names. Give each term one canonical home; use references elsewhere.

Maintain terminology as part of relevant work when a missing, stale, or ambiguous definition could cause mistakes. Make clear, evidence-backed updates directly; no separate Skill or interview is required. Ask the user when competing meanings would materially affect the work, rather than silently establishing a new meaning.

## Git

- Work directly on `main` - no feature branches by default. When you finish a task, commit it (the user may also ask you to commit mid-way); you don't need to wait to be asked.
- **Small, quick, low-risk changes go directly on `main`; use a worktree for larger or uncertain tasks, or when concurrent work could interfere.** In Codex prefer its managed worktree tools and archive through the app; otherwise use `python scripts/worktree.py create <task-name>` (see PROJECT.md -> Development). Work and commit inside the chosen checkout. Once everything is committed, follow `.vorch/workflows/worktree-workflow.md` for the matching worktree lifecycle; merge only reviewed task changes and preserve concurrent work. For conflicts, follow `.vorch/workflows/worktree-workflow.md`. No user confirmation is needed before merging.
- **Commits carry only the user's authorship.** Never commit as an AI tool (such as `Claude <noreply@anthropic.com>`) and never add `Co-Authored-By`, `Claude-Session`, or "Generated with" lines to commit messages or PRs. The hooks enforce this (`scripts/commit_authorship.py`): a tool identity blocks the commit, attribution lines are stripped from messages, and a push carrying either is refused. If a commit is blocked, commit under the user's identity with repository-local config only, never `--global`.
- **Line endings are handled for you.** `.gitattributes` normalizes text in Git and checks PowerShell/batch files out with CRLF; Git's `CRLF will be replaced by LF` warning is harmless. Never convert files by hand to silence it.
- Conventional format: `<type>(<scope>): <what>` - lowercase, <=72 chars, no trailing period. Types: `feat` `fix` `docs` `refactor` `perf` `test` `chore`. Breaking change -> `!`.
- One logical unit per commit; never batch unrelated changes; commit once the tests covering your change pass.
- **The commit hook checks every commit.** It checks the staged Rust formatting, Python syntax, structured files and domain maps without rewriting the working tree or index (PROJECT.md -> Testing). Review and stage only task-owned changes. When it blocks, fix the reported problems, stage, and commit again; never bypass it with `--no-verify`. It reads staged content; leave another session's unstaged work alone. If a commit of source files prints no `Commit check` report, the hook is not enabled: enable it (PROJECT.md -> Development).
- **Push only through `python scripts/push.py`**, when the user asks to push or release, following `.vorch/workflows/push-workflow.md`; never a plain `git push`.
- **A release request includes publication.** Finish and commit the release changes, push `main`, and publish a GitHub Release with its version tag, complete Windows package and checksum. Follow `.vorch/workflows/release-workflow.md`; a local Release build alone does not complete a release. Install that same package when installation is requested. The release request authorizes these push and publication steps; do not ask for them again.

## Preserve the user's work and deliver

Read the relevant user behavior in `docs/handbuch.md` before changing it. Use the vEdit
operating skill for live-editor interaction; ordinary repository edits use file
tools. Never kill or restart the user's editor for tests or packaging. Fixtures
use isolated profiles and repositories, never the user's current session.

Keep temporary files, logs, screenshots and helper scripts under `.tmp/<task>/`
in this checkout. Cargo sets the environment through `.cargo/config.toml`. For
other tools, dot-source `scripts/project-temp.ps1`, call `Enter-ProjectTemp`, and
restore its context with `Exit-ProjectTemp` in `finally`. Build into the checkout's
`target/`; never set `CARGO_TARGET_DIR` to a second folder.

Inspect rendered differences before replacing snapshot references; the final
suite runs without snapshot-update variables. Do not blanket-run ignored tests.
Native dialogs, Snap and physical DPI transitions need native checks.

Before staging, inspect the diff and separate this task from existing or
concurrent work. Finish validated work with focused local commits, without
waiting for another prompt. Name the commits, actual validation and remaining
limits in the final report; explain any blocked commit and uncommitted changes.
Pushing and publishing require authorization; an explicit release request provides it.

When delivering a runnable update, keep Cargo.toml, Cargo.lock, the user manual, README, relevant
docs and packaged contents consistent. Use `scripts/build-release.ps1`, not a
single copied binary. Historical reports retain their original results.
