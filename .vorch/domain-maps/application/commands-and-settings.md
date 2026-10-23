# Commands, settings and notifications

## Commands and input ownership

The `actions!` registry in `src/keybindings.rs` supplies action IDs, labels,
groups, defaults, contexts and palette entries. Palette label lookup and keyboard
dispatch both reach the exhaustive typed `src/app.rs::execute_action`. Source
editing commands use `Action::editor_command` and the ordering/history owner in
`src/editor_input.rs`. Local Explorer and Change review owners consume registered
commands in their context. Menus obtain shortcut labels from the same registry.

Bindings are saved by stable action ID. An absent override uses defaults, a
custom binding replaces the default alias set, and a disabled action remains
disabled after session restoration. `cli::ShortcutBinding` reads legacy strings
and alias arrays; one alias writes as a string, several as an array, and the outer
null disables the command. Adding/removing an alias in Keyboard settings preserves
the rest of the set. Reassign removes only the conflicting alias from other
commands and validates the candidate before changing preferences.

Conflicts depend on overlapping scopes. Workspace commands overlap all scopes;
ordinary source newline and finishing cursor placement use disjoint contexts.
Clipboard, basic navigation and Vim grammar remain native input. `restore_bindings`
keeps valid saved aliases, reports invalid/conflicting ones and lets default
aliases yield to explicit overrides. Unknown saved IDs remain preserved but cannot
match input or block editing known actions; new CLI patches still reject unknown
IDs. Never turn an explicit disable into an absent setting during migration.

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

Status messages and the notification inbox are separate surfaces.
`src/status_messages.rs` captures the selected full message when details or its
context menu opens; later status updates do not replace that snapshot. The
read-only modal supports selection and copying, suppresses source input and
restores the prior focus when closed. `src/status_message_tests.rs` covers this
behavior; `control.md` describes the observation/mutation boundary.

`src/notifications.rs` owns a bounded, session-only inbox, per-project unread
indicators and taskbar attention throttling. Showing the inbox marks entries
read. Identical unread entries coalesce by their message and action; a repeated
message after reading is a new entry. Receipt does not switch workspace, steal
focus or execute the optional open/review action.

Use the inline inbox tests for coalescing, bounding and flash throttling. CLI
workspace selection, request fields, response text and schema belong to
`control.md`; preserve that contract when editing `control_notify`.
