# Commands, settings and notifications

## Commands and input ownership

The `actions!` registry in `src/command_registry.rs` supplies action IDs, labels,
groups, defaults and contexts. `src/application_commands.rs::run_shortcut_action` matches
every action exhaustively. `src/command_palette.rs` carries the selected action,
so a label change cannot change dispatch. Source editing commands use
`Action::editor_command` and the ordering/history owner in `src/editor_input.rs`;
`src/workspace_shortcuts.rs` preserves the frame's shortcut gates. Local Explorer
and Change review owners consume registered commands in their context. Menus
obtain hints through `keybindings.rs` so saved overrides appear in hints.
The private `src/keyboard_settings.rs` child renders alias editing, conflict
reassignment and command filters using that same binding state.

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

`keybindings::vim_keys` owns per-Ctrl-key Editor/Vim preferences and the supported
mode catalogue. Windows clipboard defaults stay Editor; Ctrl+Q starts Visual
Block. Default ownership yields to a custom application binding, while an explicit
Vim preference wins in its supported source modes, including MULTI's modal
handler. Other widgets and preview retain their own input. MULTI's Ctrl+U default
remains Cursor Undo; an explicit Vim preference overrides it. Capture the focused
shortcut context before taking an egui input lock; matching inside that lock must
not read Context again.
Both input continuation owners include the preference map so a setting change
invalidates stale input. The Settings patch merges supplied keys; see `control.md`.

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
- `src/palette_ui_tests.rs`: selected command execution, focus restoration and
  moving lines with one-step Undo in native and Vim modes.

## Settings

`Settings`, defaults and session serialization live in `src/app_session.rs`;
the visible section router is
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

`src/notifications.rs` owns the inbox, per-project unread indicators and taskbar
attention throttling. `Session` persists its messages, IDs, repeat counts and
read state through the existing recovery writer; dialog state and flash timing
are transient. Older sessions default to an empty inbox. Receipt, read-state
changes and clearing the inbox schedule session persistence.

The window header owns the inbox entry point, separate from the pane's user-mark
collection. The overview defaults to the active project's messages when any
exist, with an all-project filter that includes closed projects and unscoped
messages. Filtering and previews do not acknowledge entries. Cards distinguish
text-only messages from file/review attachments; only actual attachments offer
navigation actions. A plain message never implies source annotations. The last
expanded message remains available on reopening within the same session/filter;
filter and expansion state are not persisted.

The user reads an entry by expanding it or opening its action, and can mark it
unread again. Opening the overview, switching projects or receiving a message
while the inbox is open does not acknowledge it. Closed/offline project entries
stay in the inbox. Unread entries are retained until read or explicitly cleared;
only the read history is limited to 100 entries, retaining the entry just opened
while pruning older read entries. An unavailable action leaves its entry unread.
Identical unread entries coalesce by their message and action; a repeated message
after reading is a new entry. Receipt does not switch workspace, steal focus or execute the
optional open/review action.

Use the inline inbox tests for coalescing, retention, read interactions and flash
throttling; `src/agent_projects_tests.rs` covers session round trips. CLI
workspace selection, request fields, response text and schema belong to
`control.md`; preserve that contract when editing `control_notify`.
