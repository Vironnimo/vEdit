# Native Windows window behavior

`vendor/winit/VEDIT_PATCH.md` is the canonical patch rationale and upgrade/removal
gate. The vendored dependency is winit 0.30.13, selected by `[patch.crates-io]`.
Read that document and the touched native source before replacing the dependency.

## First visible frame

`vendor/eframe/VEDIT_PATCH.md` owns the Glow startup presentation fix. The native
window stays hidden until the first successful buffer swap; drawing into the
back buffer alone does not make pixels available to the visible window.
`vendor/winit/VEDIT_PATCH.md` separately covers temporary DWM cloaking while
hidden native state is applied. Maximized restoration otherwise briefly shows
the window before eframe has created its graphics context; preserve the real
maximized geometry so the first frame is painted at its final size.
`cargo test --test startup_window -- --ignored --nocapture` checks the actual
front-buffer pixels synchronously at the first Windows show notification and
rejects uncloaked show requests before app construction, for normal and
maximized startup. It uses private storage and closes its own window; it requires
a Windows desktop and OpenGL. Keep this native check when upgrading eframe.

## Ownership boundaries

`window_chrome::draw_top_bar` reserves distinct project controls, drag areas and
window buttons. Geometry remains in logical client coordinates. It checks popup
state after drawing project widgets so a popup opened in that frame immediately
removes native drag/maximize regions. `app::chrome_blocked` adds application-level
modal/close guards; `eframe::App::ui` republishes the final geometry or clears it.

The patched native APIs `set_custom_titlebar_regions` and
`custom_titlebar_maximize_state` bridge that layout to winit. Publishing uses
physical client rectangles, flooring left/top and ceiling right/bottom after the
current scale conversion. Native hit testing scales existing regions across DPI
changes before the next egui frame. Empty regions preserve popup/modal input.

Windows owns caption dragging, double-click maximize/restore, system menus and
Snap behavior. Resize hit testing covers all eight edges/corners for eligible
undecorated windows; maximize uses native non-client interaction to retain the
Windows 11 Snap Layout hover. Pointer maximize is not dispatched twice through
egui; keyboard/AccessKit activation remains available. Close continues through
the app's dirty-document guard. Headless/non-native chrome uses viewport commands
instead of the native hit regions.

## DPI and file-drop contracts

During interactive move/resize, `dpi_change::outer_rect_for_dpi_change` uses the
Windows-suggested physical rectangle directly and avoids evaluating the ordinary
monitor-nudge path. Outside that operation the original geometry behavior remains.
Native scale-factor state and events still update. Do not apply egui UI zoom to
the Windows screen rectangle.

OLE owns pointer input during Explorer drags, so the patched drop handler converts
the callback's screen `POINTL` to physical client coordinates and emits
`CursorMoved` before hover/drop and during `DragOver`. Its COM signatures accept
`POINTL` by value. App drop overlays and split targeting depend on this forwarding;
ordinary `WM_MOUSEMOVE` cannot replace it reliably.

## Verification boundary

`vendor/egui-winit/VEDIT_PATCH.md` documents the separate egui-winit 0.36.2
keyboard patch. Its internal clipboard translator preserves the originating Key
immediately before Copy/Cut/Paste, including a lone Key when paste is empty.
Vim forwarding depends on that provenance; an eframe raw-input hook runs too
late to reconstruct it. `native_clipboard_input_tests` includes the exact
production helper and covers event order, clipboard alternatives and empty paste.
Editor input routing remains owned by `editing.md`; desktop key delivery is not
verified by these pure tests.

`src/main.rs` includes the exact vendored pure helpers in normal tests:
`cargo test --bin vedit window_dpi_tests` and
`cargo test --bin vedit window_chrome_native_tests`. They cover negative screen
coordinates, repeated mixed-DPI transfers, resize geometry and native region
state. `cargo test --bin vedit window_chrome::tests` covers headless chrome layout,
popup blocking, accessibility activation and close delegation.

Actual monitor dragging, Windows 11 Snap Layout hover and Explorer-drop targeting
still require desktop checks. The tests intentionally do not manipulate a user's
running editor window. Removal of the patch requires equivalent upstream behavior
and native verification, not only passing pure helper tests.
