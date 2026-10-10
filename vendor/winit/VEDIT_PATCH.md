# vEdit's Windows window patches

This is the packaged source of `winit 0.30.13`, copied from crates.io, with
Windows-only mixed-DPI and custom-titlebar fixes. The original Apache-2.0 license is in `LICENSE`; the
original source commit is recorded in `.cargo_vcs_info.json`:
`e9809ef54b18499bb4f2cac945719ecc2a61061b`. vEdit selects this copy through
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

`src/main.rs` includes the pure helper's regression tests in vEdit's normal
`cargo test --bin vedit window_dpi_tests` suite. They cover the physical
rectangle in both directions, negative screen coordinates, repeated transfers,
and the unchanged noninteractive path. Native verification still requires
dragging a built window between monitors with different Windows scaling;
these tests intentionally do not move the user's running window.

When upgrading winit, check whether the upstream interactive `WM_DPICHANGED`
path accepts `suggested_rect` without a monitor nudge. Remove this patch and the
test inclusion only after that fix is present and mixed-DPI dragging is verified.

## Custom Windows titlebar

vEdit hides the native caption and draws its own top bar. Upstream already
retains `WS_SIZEBOX`, `WS_SYSMENU` and the minimize/maximize styles for frameless
windows and clips the maximized client area to the current monitor's `rcWork`.
These native behaviors are retained for Aero Snap, Win+Arrow, system menus,
taskbar-safe maximize/restore and the DPI fix above.

The additional `WM_NCHITTEST` path handles all eight resize edges/corners for
undecorated top-level windows, using `GetSystemMetricsForDpi` when available.
Decorated, child, fullscreen, maximized and fixed-size windows retain their
appropriate behavior. No HWND subclass or replacement window procedure is used.

Two Windows-only inherent window APIs let the app publish its laid-out titlebar
geometry without adding a separate dependency:

- `set_custom_titlebar_regions(&[[i32; 4]], Option<[i32; 4]>)` uses client physical
  pixels `[left, top, right, bottom]` for draggable areas and maximize/restore.
  Publishing empty regions leaves popup/modal controls interactive. Existing
  regions rescale when native DPI changes before the next egui frame.
- `custom_titlebar_maximize_state() -> (bool, bool)` supplies native hover and
  pressed feedback to the egui-painted maximize button.

Caption regions return `HTCAPTION`, delegating drag, double-click, restore from
maximized, Aero Snap and right-click system menus to Windows. The maximize region
returns `HTMAXBUTTON`; non-client hover delegates to `DefWindowProc` to preserve
the Windows 11 Snap Layout menu. Maximize presses capture/release the pointer and
post native `SC_MAXIMIZE`/`SC_RESTORE` only on release inside the button; moving
away or cancellation does not activate it. Ordinary minimize/close buttons remain
client controls handled by the app, including its unsaved-document close flow.

The pure geometry tests in `chrome_hit_test.rs` are included in vEdit's root
regression suite (`window_chrome_native_tests`). They cover all resize edges and
corners on negative monitor coordinates, DPI-dependent border size, maximized and
fixed-size windows, control exclusions, popup/modal clearing, and titlebar
geometry during DPI changes. Native Snap Layout hover still needs a Windows 11
desktop check; tests do not move or close the user's running editor.

Primary native integration references:

- <https://learn.microsoft.com/en-us/windows/win32/dwm/customframe>
- <https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nchittest>
- <https://learn.microsoft.com/windows/apps/desktop/modernize/apply-snap-layout-menu>
