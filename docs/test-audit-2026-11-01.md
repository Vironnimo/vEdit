# Test suite audit, 2026-11-01

The complete suite was audited at `558ea15` (v0.6.5), following the repository's
Test Audit Workflow. The user approved T1–T11, both contract gaps and the workflow
corrections. All approved items were implemented. Production behavior, durable
writes, snapshot references and functional-test ignore status are unchanged.

## Measurements

Both surveys used Rust 1.97.1 on Windows, eight isolated test processes, the GL
backend request and the existing 30-second per-case deadline.

| Measurement | Before | After |
| --- | ---: | ---: |
| Passing default tests | 1,849 | 1,846 |
| Ignored tests | 32 | 32 |
| Failed / timed-out tests | 0 / 0 | 0 / 0 |
| Survey test execution | 127.073 s | 95.015 s |
| Survey build | 101.928 s | 5.467 s |
| Complete survey | 230.792 s | 101.704 s |

Test execution decreased by **32.058 seconds (25.2%)** in this comparison. The
before build was fresh and the after build reused artifacts; the large total-time
difference is therefore not a test-speed improvement. Case timings include
process startup and cold process caches. An earlier same-base survey executed in
104.993 seconds, illustrating machine variation; these are local observations,
not a guaranteed percentage for every run.

Four source-test merges removed five runtime registrations because the schema
test ran in both binaries. Two existing Unix-only owner tests now also run on
Windows, producing the net reduction of three registrations. Multicursor coverage
extends an existing test rather than adding another test for the same contract.

Examples of individual process timings:

| Contract | Before | After |
| --- | ---: | ---: |
| Every screen-jump label at width boundaries | 4.770 s | 0.119 s |
| Bottom selection drag across editing modes | 10.024 s | 4.920 s |
| Real worktree grouping and idle process budget | 8.761 s | 4.843 s |

## Implemented items

| Item | Result and retained protection |
| --- | --- |
| T1 | Five explicit-GL harnesses defer renderer creation until actual rendering. Tab-drag, Explorer, Agent mark and worktree-tooltip state/accessibility assertions stay enabled; real snapshots still render. |
| T2 | Screen-jump label boundaries and all 1,515 target selections remain, with seven full model builds instead of about 1,522. Public backspace operations reset selection between labels. |
| T3 | Ordinary drag fixtures use 80 overflowing rows with the original horizontal geometry. The 120G and 180-line reveal contracts retain 250 rows; editing-mode, wrap, smooth-scroll, clipping and history assertions remain. |
| T4 | Pointer completion prepares selected row 110, then really clicks row 109. The separate keyboard test still performs the full navigation past 100. |
| T5 | Document/input fixtures construct their initial Project without unrelated index/Explorer/watcher workers. Real load/save/recovery waits and explicit project-opening integration cases remain. |
| T6 | Hover details create one linked worktree; the idle contract and benchmark retain seven, including the Unicode name and eight-row grouping. The complete warmed minute and per-second process budgets remain in 120 simulated seconds. Local fixture configuration is written together with hooks/signing/line-ending isolation preserved. |
| T7 | The Unicode diff uses 128 distinct wrapped rows, still exceeding its 80-galley bound. Three widths, first/middle/last height equality, offscreen behavior and End navigation remain. |
| T8 | The unsaved-buffer review banner reuses delivered comparison results. Branch/working differences, Show navigation, dirty state and exact draft text are asserted; real Git loading/freshness/entry-point/screenshot tests remain. |
| T9 | The 50 ms setup wait belongs only to the repaint-counting contract. Other Intel fixtures no longer wait for an observation they do not make. |
| T10 | Three disk-search cases and the ignored benchmark use the production search entry point. Unicode/CRLF coordinates, binary skips, multiline/literal/case/word matching, limits and preview assertions remain; only the obsolete test-only index adapter is removed. |
| T11 | The retained Cut case gains full global/local Yank equality, including block width, and clean state after Undo. Paste gains the Normal-mode assertion. The native no-Git watcher case gains the absent-repository and real empty-folder checks. The schema catalog validates every original example once and compares every topic's complete filtered values; default wire assertions remain. |
| G1 | Existing multicursor completion coverage now uses `app.apply_completion`: matching and mismatching context, once-only import with Unicode positions, correct mapped selections, one shared Undo, and overlapping auxiliary edits rejected without Document/revision/history changes. |
| G2 | Existing Explorer/index link contracts run with Windows junctions as well as Unix directory symlinks. Explicit Explorer expansion succeeds while recursive indexing does not traverse the target, including Show ignored files. Cleanup unlinks before fixture recursion and asserts target bytes survive. |
| D1 | The workflow and test guide describe the actual Rust survey, LLVM line/branch measurement, existing ignore/durability policy and managed-worktree lifecycle. Relevant owner maps were updated. |

T12's shared GUI/CLI test registration remains deliberately unchanged; after the
catalog merge, 68 cases still run in both binaries. Special registration plumbing
or a new production library solely to avoid those copies was outside the
recommended implementation. Broad cosmetic test-file moves were also deferred.
No approved item remains unimplemented.

## Coverage preservation

Domain test groups ran before and after with matching Rust/LLVM 22.1.6 tools.
The GUI test binary was compiled with `-C instrument-coverage` and
`-Z coverage-options=branch`, using command-scoped `RUSTC_BOOTSTRAP=vedit` solely
for this measurement. Normal final tests, Clippy and the survey used ordinary
build flags. No Cargo dependency or permanent compiler configuration was added.

Covered production-owner locations were compared after mapping unchanged source
lines across the diff. Test bodies, positively test-gated helpers/fields and the
explicitly removed dead search adapter were excluded. Branch figures count
covered true/false sides emitted by the compiler, not an overall suite percentage.
Owner groups overlap and must not be summed.

| Owner group | Covered lines before → after | Covered branch sides before → after | Lost locations |
| --- | ---: | ---: | ---: |
| Projects/navigation | 4,413 → 4,419 | 974 → 978 | 0 |
| Reviews | 3,406 → 3,406 | 670 → 670 | 0 |
| Git repositories/diff | 2,511 → 2,511 | 513 → 513 | 0 |
| Git gutter | 274 → 274 | 62 → 62 | 0 |
| Editing | 5,959 → 5,971 | 1,471 → 1,479 | 0 |
| Documents/application | 2,007 → 2,008 | 379 → 381 | 0 |
| Search | 625 → 625 | 160 → 160 | 0 |
| Language | 666 → 666 | 168 → 168 | 0 |
| Control | 1,605 → 1,605 | 461 → 461 | 0 |

Independent measurement review caught an incorrect gutter module filter and
overbroad exclusions around test-gated fields/imports. Both were corrected before
accepting the comparison. The gutter baseline was reconstructed using its exact
original test file from `558ea15`, then compared with a freshly compiled changed
fixture; its production owner was unchanged throughout. Both captures ran eleven
tests, leaving the existing manual benchmark ignored. All exclusion ranges and
raw line/branch exports were retained. No lost owner location required restoration.

## Validation and review

- Native `cargo test --locked --all-targets -- --test-threads=8`: 1,846 passed,
  32 ignored. The GUI shared-process harness took 51.78 seconds, separate from
  isolated-process survey timing.
- Final complete survey: 1,846 passed, zero failures/timeouts, 32 ignored.
- `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `git diff --check` and `python scripts/domain_maps.py`: passed.
- The rewritten search benchmark was run individually with `--ignored` as a smoke
  check: 900 matches on a 1,849,500-byte Unicode line, all assertions passed.
  Its debug timing is not a release-performance claim.
- Audit-baseline Python tooling: 32 passed in 41.533 seconds; isolated PowerShell
  temp/installer fixtures: 18/26 checks passed. Those unchanged tools were not
  rerun merely because application tests changed.
- Fresh domain reviewers checked every merge and retained assertion, production
  source boundaries, fixture isolation and real screenshot/Git contracts. They
  rejected deleting the native negative watcher test; it remains enabled.

No application behavior or snapshot reference changed. No new functional tests
were ignored, persistence syncs suppressed, deadlines increased or runtime
Agent-facing text changed. Ignored live SDK/CLI/native-window workloads and
physical DPI/Snap checks were not blanket-run. The user's editor was not restarted.

## Commits and local evidence

The implementation uses focused, locally validated commits:

- `5122811` — `test(projects): streamline fixtures and cover windows junctions`
- `b51a5a7` — `test(reviews): reuse ui fixtures and defer renderer setup`
- `86446fc` — `test(git): trim repeated fixture and diff preparation`
- `a6a8229` — `test(editing): streamline fixtures and cover completion imports`
- `a977f2c` — `test(documents): avoid unrelated project index workers`
- `ccb2de5` — `test(search): exercise the production disk search path`
- `b1d8f71` — `test(language): scope repaint settling to its contract`
- `9a2e079` — `test(control): consolidate schema catalog validation`

Workflow corrections and this report are recorded together in the documentation
commit. Local `.tmp/test-audit/` retains the approved plan, domain ledgers,
independent reviews, before/after survey reports, coverage commands/exports and
comparison/exclusion records. These ignored artifacts supplement this historical
report; they are not portable package contents or hosted CI results.
