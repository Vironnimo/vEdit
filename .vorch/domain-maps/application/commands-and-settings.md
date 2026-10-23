# Commands, settings and notifications

## Commands and input ownership

The `actions!` registry in `src/keybindings.rs` supplies action IDs, labels,
groups, defaults and palette entries. `run_shortcut_action` and
`src/app.rs::command_action` connect them to behavior; source editing commands
also pass through `src/editor_input.rs`. Menus obtain shortcut labels from the
registry so saved overrides appear in hints.

Bindings are saved by stable action ID. An absent override uses defaults, a
custom binding replaces all default aliases, and a disabled action remains
disabled after session restoration. Existing upgrade helpers in `keybindings.rs`
let newly introduced defaults yield to saved custom choices. Do not turn an
explicit disable into an absent setting during migration.

Default shortcuts can have Vim-specific meaning: the default Find chord scrolls
in Normal mode, New File can request completion in Insert, and Open File can
navigate back in Normal. A custom binding chooses the application command.
Preserve these distinctions through the existing context matching rather than
adding an unconditional key test to a widget.

AltGr can produce Ctrl+Alt keyboard events plus text. `capture_shortcut_input`
and matching helpers distinguish that text from a real application chord.
Consumed shortcuts remove only their generated text payload, not subsequent
typing. Deferred widget input carries a focus/owner snapshot and is discarded
when that owner or modal context changes. Source-specific continuation semantics
are in `editing/input-and-history.md`.

Evidence and regression entry points:

- `src/keybindings_tests.rs`: default validity, override/disable round trips,
  migration conflicts, AltGr, generated text, recorder focus and modal ownership.
- `src/editor_input_tests.rs`: remapped save ordering and retiring native Undo
  aliases. A menu-only check does not cover source input dispatch.
- `src/app_menu_tests.rs`: menu access at the minimum window size and the custom
  close button's unsaved-document guard.

## Settings

`Settings` and its defaults live in `src/app.rs`; the visible section router is
`src/settings_navigation.rs`. Keep specialized updates in their owner: for
example preview-tab policy, indentation detection, language configuration and
project tasks have behavior beyond changing a scalar setting.

`src/editor_preferences.rs` holds reading settings. Interface zoom is edited as
a draft while dragging and applied at interaction completion; shortcut zoom
applies immediately. Changing settings must not leak Escape or typing into the
document beneath the dialog. `src/editor_ux_tests.rs` and the inline
`settings_navigation` tests cover dialog focus, short windows and page access.
`src/typography_ui_tests.rs` covers zoom interaction and reading preferences;
the inline `editor_preferences` test covers restored defaults and bounds.

The external-open preference is shared between windows. Only
`set_external_open_new_window` explicitly writes `external-open.json`; background
reads and write results carry revisions. Closing an older window must not
overwrite a newer shared preference. Window/profile ownership is in
`platform.md`.

## Notifications

`src/notifications.rs` owns a bounded, session-only inbox, per-project unread
indicators and taskbar attention throttling. Showing the inbox marks entries
read. Identical unread entries coalesce by their message and action; a repeated
message after reading is a new entry. Receipt does not switch workspace, steal
focus or execute the optional open/review action.

Use the inline inbox tests for coalescing, bounding and flash throttling. CLI
workspace selection, request fields, response text and schema belong to
`control.md`; preserve that contract when editing `control_notify`.
