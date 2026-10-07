# Developing Morrow

Commands and source paths in this guide require the source checkout. The portable
application includes this guide for reference, but not the Rust sources or scripts.

Use the stable Windows Rust toolchain with the MSVC build prerequisites. Git must
be on PATH for repository functionality and its integration tests. The normal
test suite uses temporary fixtures and headless egui harnesses; it does not need
to control a running Morrow window.

```powershell
cargo build
cargo test --bin morrow reopen::tests -- --test-threads=8
cargo test --bin morrow-cli cli::transport::tests -- --test-threads=8
```

The latter commands are examples of focused test filters. For a complete check:

```powershell
cargo fmt -- --check
cargo test --all-targets -- --test-threads=8
cargo clippy --all-targets -- -D warnings
git diff --check
```

`cargo run --release` starts the actual editor and can forward files to an
existing instance. Use it intentionally, not as an isolated test command.
`morrow-cli` controls that same live instance unless an isolated test profile has
been configured. Never use the live session for destructive test fixtures.

## Visual references

`src/visual_tests.rs` creates headless renders against `tests/snapshots/`.
Behavioral UI tests also live in feature-specific `*_tests.rs` files. WGPU rendering
requires a usable graphics backend even though it does not open a native window.

On this Windows setup, a parallel headless test run produced a native crash in
the NVIDIA OpenGL driver (`nvoglv64.dll`). The full suite passes both serially and
with Direct3D 12 selected explicitly, without changing the references or assertions:

```powershell
$previousGpuBackend = $env:WGPU_BACKEND
try {
    $env:WGPU_BACKEND = 'dx12'
    cargo test --all-targets -- --test-threads=8
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
}
finally {
    if ($null -eq $previousGpuBackend) {
        Remove-Item Env:WGPU_BACKEND -ErrorAction SilentlyContinue
    } else {
        $env:WGPU_BACKEND = $previousGpuBackend
    }
}
```

`--test-threads=1` is another verified fallback. These are test-runtime choices;
the shipped editor continues using its configured Glow renderer. Preserve crash
logs instead of replacing baselines or weakening assertions to obtain a pass.

A deliberate UI change can invalidate snapshots. First run normally, inspect the
reported image differences, then generate candidates separately. For example:

```powershell
$env:MORROW_SNAPSHOT_DIR = Join-Path $PWD 'target/snapshot-candidates'
$env:UPDATE_SNAPSHOTS = '1'
try {
    cargo test --bin morrow app::visual_tests -- --test-threads=8
    if ($LASTEXITCODE -ne 0) { throw 'Candidate rendering failed.' }
}
finally {
    Remove-Item Env:MORROW_SNAPSHOT_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:UPDATE_SNAPSHOTS -ErrorAction SilentlyContinue
}
```

Inspect candidates and replace only references justified by the intended change.
Do not regenerate baselines simply to silence a failure. Rerun the normal suite
against the checked-in references, with both variables unset. Custom chrome,
native dialogs and cross-monitor DPI behavior still require native desktop checks;
headless images cannot establish those results.

## Optional integration checks

The ignored tests document their prerequisites beside `#[ignore]`. Run individual
filters only after the relevant tool is available, for example:

```powershell
cargo test --bin morrow live_rust_analyzer_diagnostics_completion_and_definition -- --ignored --nocapture
cargo test --bin morrow live_python_diagnostics_completion_and_definition -- --ignored --nocapture
```

Rust needs `rust-analyzer` and `rust-src`. Python server tests use Node and the
pinned `tools/basedpyright` package; the virtual-environment test also needs Python.
The Dart test reads `MORROW_TEST_DART_SERVER`. Other ignored tests include task
execution, hidden process-start helpers and manual performance measurements.

## Portable Windows build

```powershell
.\scripts\build-release.ps1
```

The script builds both release binaries and prepares `dist/Morrow/` plus
`Morrow.lnk`. Packaging needs Node and npm on PATH. It installs the pinned Python
language server with `npm ci` when needed, and obtains the license for the bundled
Node version if it is not cached. The bundle includes user/developer documentation,
the repository Morrow skill, visual references and third-party licenses.

A running editor is preserved under a process-specific filename if it locks an
output binary. Its window and documents remain open; it continues using the old
code until the user closes and starts Morrow again. Do not terminate it to force a
build. Verify both packaged binaries and relevant copied files against the build
outputs. `target/`, `dist/`, local backups and the shortcut are ignored by Git.

## Documentation ownership

`README.md` describes current user behavior. `AGENTS.md` provides repository
working instructions. `.agents/skills/morrow/` teaches control of a live editor;
update its references, CLI help and schema together when that interface changes.
`docs/agent-interface.md` separates the current observable contract from historical
evaluations. Versioned review notes retain their original results and scope.
