# Window and navigation polish — Morrow 0.4.15

Morrow on Windows now has one titlebar. The native caption is hidden while the
Windows system menu, resize frame, taskbar behavior and minimize/maximize styles
remain available. The project tabs stay in the titlebar. Free areas move the
window; double-clicking them maximizes or restores it. Direct project tabs replace
the project picker button. The three window controls
have keyboard focus, accessible names and hover feedback. Closing uses the
existing unsaved-document guard, including Cancel.

The hamburger button above Explorer opens File, View and Run. It remains visible
on the welcome screen. Existing commands, shortcuts and project actions remain
reachable; Go to file is also available under File. Menus keep their existing
keyboard/Vim input guards.

Project tabs show an orange dot only when Git reports uncommitted changed paths,
including staged and untracked files. Hover shows the count; unsaved editor text
alone does not claim a Git change. The active tab is highlighted, duplicate names
receive a folder suffix, and the strip scrolls horizontally when needed. A compact
plus/list button and Ctrl+Alt+P keep the project list and Add project reachable.
Keyboard project switching reveals the active tab without changing document
contents or each project's layout.

At narrow widths the status bar keeps mode, save state and cursor position
separate, hiding secondary metadata. At 200% interface zoom the project list and
window controls remain reachable even in the minimum-size window.

Inactive projects receive status-only refreshes every 15 seconds with at most two
requests in flight. They read neither patches nor diff statistics nor history.
Removed projects still occupy their running worker slot until completion; unique
request IDs discard stale results after close/reopen or a full Git refresh. Errors
produce an unknown count and retry later instead of claiming a clean repository.
The active project's existing Git refresh updates the same status indicator.

The activity rail is the single Explorer/Search/Source control navigation.
Sidebar headers now contain one quiet active-panel title, the panel menu and
Close. The panel menu can still switch to every other assigned panel, including
inactive ones. Left/right placement, hiding, widths and saved sessions are kept.

Windows uses native hit testing for all eight resize directions and draggable
caption areas. The maximize button exposes HTMAXBUTTON for the Windows Snap
Layout menu. Press/release, capture cancellation and native hover are handled
without issuing a second egui maximize action. Maximized client bounds retain
the monitor work area. The minimum window size is 500 × 360 logical pixels so
smaller Snap zones remain usable.

Titlebar hit regions are published in physical client pixels using the current
egui scale, including interface zoom. During a Windows DPI transition the native
backend scales the previous regions until the next UI frame arrives. Popups and
dialogs clear native drag/maximize regions so their controls stay interactive.
The earlier monitor-move fix remains in place. Dependency changes and source
references are documented in [the winit patch notes](../vendor/winit/VEDIT_PATCH.md).

Regression coverage exercises pointer, keyboard and accessibility activation,
project-switch input ordering and Undo, window controls at small/scaled sizes,
project-picker exclusion, dirty-document close/cancel, nested menu commands at
500 × 360, sidebar switching/moving/hiding, session restoration, and painted
resize boundaries at 100%/125% native scaling. The project list, empty search,
footer and keyboard selection fit the minimum window at 200% interface zoom.
Native helper tests cover negative monitor coordinates, all resize directions,
DPI transitions and maximize press/cancel state. Visual references include the
workspace, project tabs, their compact layout and both levels of the new menu.

Validation: `cargo test --all-targets -- --test-threads=8` passed 1,207 editor
tests and 48 CLI tests, with 12 existing opt-in tests ignored. All 55 visual
references passed comparison. `cargo fmt -- --check`,
`cargo clippy --all-targets -- -D warnings` and `git diff --check` passed.
The release build and portable packaging completed. Packaged editor/CLI binaries
and all 55 reference PNGs matched their source build hashes; documentation,
the bundled skill and dependency patch notes were also checked. The existing
editor session remained open during the build.

Physical cross-monitor dragging and the Windows Snap flyout require a desktop
check after restarting the installed application; headless rendering tests do
not exercise the real shell. The release script preserves any running editor
and its documents. Restart Morrow to activate 0.4.15.
