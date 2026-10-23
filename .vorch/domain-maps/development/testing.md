# Testing vEdit

## Select the owner and check

Tests use Rust's built-in harness. Pure logic tests are usually an inline
`#[cfg(test)] mod tests`; larger feature tests live in app child modules such as
`src/reopen_tests.rs`, connected through `src/app.rs`. Extend the test that owns a
behavior instead of creating a second regression under a different feature.
Module-qualified names distinguish tests without a separate import-mode setting.

Focused examples:

```powershell
cargo test --bin vedit reopen::tests -- --test-threads=8
cargo test --bin vedit-cli cli::transport::tests -- --test-threads=8
cargo test --test project_temp
python -B scripts/test_test_survey.py
```

Editor-state and UI integration delivery retains the complete native checks:

```powershell
cargo fmt -- --check
cargo test --all-targets -- --test-threads=8
cargo clippy --all-targets -- -D warnings
git diff --check
```

The survey is the complete default-test check used by the push tooling and the
Vorch adoption measurement:

```powershell
python scripts/test_survey.py --jobs 8
```

It runs `cargo test --locked --all-targets --no-run --message-format=json`, lists
each test executable with `--list`, separately lists its ignored cases, and runs
each non-ignored name with `--exact` and one harness thread. It rejects a zero-test
"success". Eight subprocesses can run concurrently; `--jobs 1` serializes them.
`--timeout 30` is the default per-case deadline; a timeout fails the case and
stops its owned process tree. A 30-minute build deadline bounds the compile step.
Fix a slow or hanging contract instead of raising deadlines to hide a failure.

Each invocation creates `.tmp/test-survey/<run>/report.json`, `report.md`, a build
log, discovery logs and one output log per executed case. Reports list failures,
timeouts, ignored names and the 20 slowest measured cases. Ignored reasons remain
beside `#[ignore]` in source; stable libtest's listing does not expose them. A build
or discovery error is a failed survey, not an empty successful test run.

Durations are monotonic wall time from process launch to exit, including startup,
private environment setup effects and cold process caches. Overall time includes
compilation and discovery, while build and test execution have separate fields.
These measurements describe the survey; do not label them the duration of one
native `cargo test` invocation. Fresh processes also cannot expose interference
between tests sharing one native test process. Keep direct Cargo verification
when that interaction matters. Rust 1.97.1 rejects `--report-time` without nightly
unstable options, so the runner does not rely on that flag or unstable JSON events.

## Assertions and timing

Assert stable protocol fields, persisted bytes, document revisions, selections,
worker ownership and visible UI behavior. Exact text is appropriate for protocol
tokens and stable user-visible names; avoid locking mutable explanatory prose
when an error code, structured field or state transition proves the contract.
Extend an existing matrix for Unicode scalar, byte and UTF-16 boundaries, and
include raw CRLF when the changed operation accepts CLI text.

Wait for the relevant outcome, not a queue acknowledgement. Existing app helpers
poll worker results with a bounded deadline; they do not replace clocks or sleeps
globally. `FindWorker::manual` and `finish` provide explicit completion for find
tests. `text_statistics` has a `display_at` time input for debounce checks.
`RevisionTracker::wait_idle` bounds recovery work but currently polls with real
sleeps. Prefer the owner's explicit completion or time seam when extending tests.

There is no universal fake clock or no-sleep fixture. Several Git process-tree
regressions intentionally wait seconds to prove a killed child never writes a
completion marker, and GUI helpers poll real workers. Treat long tests as audit
candidates: preserve the process-lifecycle contract when replacing waits with
observable completion. Do not monkey-patch process-wide timing behavior.

## Isolation that exists

- All Cargo children inherit `TEMP`, `TMP`, `TMPDIR` and
  `GIT_CEILING_DIRECTORIES` pointing exactly to this checkout's `.tmp/`. Git in an
  uninitialized fixture cannot discover the enclosing source repository.
- Git's ceiling does not constrain the `ignore` crate. In Rust test builds, the
  three workspace walkers disable ancestor-ignore traversal so the checkout's
  `.tmp/` exclusion cannot hide fixture contents. Local and nested `.gitignore`,
  `.ignore` and `.git/info/exclude` rules remain active. Shipped builds retain
  ancestor-ignore traversal. Unit tests therefore do not verify inheritance from
  outside the fixture root; check that production behavior with a native fixture
  when changing it.
- App, Git, recovery and control fixtures create their own named directories,
  usually using the process ID and a counter or timestamp. App fixtures redirect
  recovery files and disable autosave; Git fixtures configure local test authors
  and isolated hooks. The CLI round-trip fixtures use private control endpoints.
- The survey additionally gives each test process an empty `HOME`/`USERPROFILE`,
  private `APPDATA`/`LOCALAPPDATA`, a private `VEDIT_PROFILE_DIR`, an empty global
  Git config and disabled system Git config. It clears inherited Git repository
  selectors, Git config overrides, `XDG_CONFIG_HOME` and control-endpoint
  overrides. Its build keeps the caller's toolchain environment. `TEMP` remains
  the checkout root `.tmp/` because the Cargo environment test requires it.
- Direct Cargo tests do not receive those extra survey per-case settings. Existing
  fixtures must therefore continue to pass explicit repositories, recovery paths
  and endpoints; a survey pass is not permission to remove fixture isolation.
- Native executable checks use `VEDIT_PROFILE_DIR` for recovery, eframe state and
  instance locks. `VEDIT_CONTROL_DIR` alone isolates only the endpoint. The
  profile-migration script separately redirects `APPDATA` to exercise migration.
- Rust tests often clean fixture directories on drop, but this is not a universal
  cleanup assertion. For example, visual fixtures use a process-wide `OnceLock`
  directory and retain files. Survey logs and private profiles remain for diagnosis.

The runner is not a filesystem/network sandbox, and it does not replace every
possible system configuration variable. It does not detect a child detached from
a successfully exited test or assert that all background threads/tasks closed.
Timeout cleanup targets the launched process tree, not all processes by name.
If Windows process-tree cleanup fails, the report records that failure and falls
back to stopping the owned parent; descendant cleanup is then not established.
Fixture ownership and teardown still belong to the test that creates resources.

Temporary data stays inside `.tmp/` by the repository's explicit requirement,
including worktrees under the Windows user profile. Git's discovery ceiling is
the protection against parent repositories; the setup does not claim the
antivirus/performance benefits of temporary data outside the user profile.
There is no machine-wide worker pool. Avoid overlapping full surveys that compete
for the same CPU/GPU, and do not share `target/` or fixture directories across
checkouts. Process-wide tokenizer/render caches are not centrally prewarmed;
the survey intentionally starts them afresh for each case.

## Optional workloads and durability

Stable libtest has no arbitrary `stress`/`durable` marker registry or strict-marker
mode. Existing manual scale/performance checks use `#[ignore = "manual ..."]`,
and SDK/executable checks use `#[ignore = "requires ..."]`. Both are excluded
from direct default Cargo runs and the survey. Read the reason and run only the
specific ignored test whose prerequisite is available; do not blanket `--ignored`.

`docs/development.md` -> Optional integration checks and CLI integration fixtures
give the executable, Rust-analyzer, basedpyright, Python and Dart setup commands.
Manual benchmarks can require a release build and measure observations without
pass/fail timing assertions. Their ignored status is not evidence they passed.

Persistence currently performs real `sync_all` calls in workspace replacement,
recovery, profile migration and control transport. There is no no-op sync fixture
or durability marker, and this adoption does not alter those tests or production
semantics. A future performance change needs an owner-local seam and dedicated
durability contract tests before it can suppress a sync anywhere.

## Graphics and snapshots

The headless egui harness still creates graphics resources. The survey defaults
to `WGPU_BACKEND=gl` on Windows unless a value is already set, matching
`src/visual_tests.rs::harness`, which explicitly selects OpenGL. The multicursor
harness honors the environment choice; on this host its DX12 device creation
failed while GL passed against the unchanged snapshot. An environment preference
is not proof of every harness's actual renderer. Driver crashes can
require a serial run (`--jobs 1` for the survey or `--test-threads=1` for Cargo).
Use the scoped environment example in `docs/development.md` -> Visual references.

Render fixtures disable cursor blinking and animation, wait for syntax/token
workers, and keep periodic disk, Git and recovery work out of snapshot rendering.
Inspect failures before changing checked-in references. Candidate rendering and
review commands belong to `docs/development.md` -> Visual references. The survey
rejects snapshot-update/output overrides so its final verification compares the
normal references. Headless checks do not establish native dialog, Snap, shell
integration or physical monitor-DPI behavior; use the live-testing workflow.
