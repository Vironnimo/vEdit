# Morrow's Windows mixed-DPI window fix

This is the packaged source of `winit 0.30.13`, copied from crates.io, with one
Windows-only geometry fix. The original Apache-2.0 license is in `LICENSE`; the
original source commit is recorded in `.cargo_vcs_info.json`:
`e9809ef54b18499bb4f2cac945719ecc2a61061b`. Morrow selects this copy through
`[patch.crates-io]` in its root Cargo manifest. No new dependency is introduced.

Dragging a titlebar between monitors at 125% and 100% scaling can repeatedly
resize the window until it spans both monitors. In upstream's `WM_DPICHANGED`
handler, the conservative rectangle and monitor nudge can put the window back
on the monitor being left, causing another DPI transition.

While `MARKER_IN_SIZE_MOVE` is set, the patched handler passes Windows'
suggested physical rectangle directly to `SetWindowPos`. It still updates the
stored scale factor and emits `ScaleFactorChanged`, so egui, cursor input and
rendering continue to use the destination monitor's DPI. Outside an interactive
move/resize, upstream's geometry behavior is preserved. The lazy helper prevents
the old monitor-nudge path from running during the interactive transition.

Evidence and upstream reproduction:

- <https://github.com/rust-windowing/winit/issues/4600>
- <https://github.com/rust-windowing/winit/issues/4041>
- <https://learn.microsoft.com/en-us/windows/win32/hidpi/wm-dpichanged>

`src/main.rs` includes the pure helper's regression tests in Morrow's normal
`cargo test --bin morrow window_dpi_tests` suite. They cover the physical
rectangle in both directions, negative screen coordinates, repeated transfers,
and the unchanged noninteractive path. Native verification still requires
dragging a built window between monitors with different Windows scaling;
these tests intentionally do not move the user's running window.

When upgrading winit, check whether the upstream interactive `WM_DPICHANGED`
path accepts `suggested_rect` without a monitor nudge. Remove this patch and the
test inclusion only after that fix is present and mixed-DPI dragging is verified.
