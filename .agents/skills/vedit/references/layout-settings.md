# Panes and preferences

Submit structured actions with `request FILE.json`; `schema OP` gives focused examples. JSON files accept UTF-8 with/without BOM and UTF-16 with BOM. Write files in PowerShell with `Set-Content -Encoding utf8`. Request paths resolve against the working directory; review paths resolve against the manifest directory. In the examples, replace IDs, revisions and paths with current task values.

## Arrange panes and preferences

`layout` with `change:{"action":"status"}` reports the active project's `workspace`, split tree and current pane IDs. Each project keeps its own tab order, focused pane, Source/Preview views, cursor, selection and scroll positions. Switching projects restores those views together. `status.active_workspace` identifies the visible project; `status.layout` describes only its panes, while `status.tabs` remains a page of buffers across projects. Split the focused pane to display another tab from this project:

```json
{
  "op": "layout",
  "change": {"action": "split", "orientation": "vertical", "tab_id": 42, "view": "source"}
}
```

`vertical` places panes side by side; `horizontal` stacks them. `pane_id` optionally identifies which pane to split. Each pane owns its tab bar: layout reports its active `tab_id` and ordered local `tab_ids`. The new pane starts with the selected document; opening files adds them to the focused pane. Use returned pane IDs for `focus`, `assign`, `move_tab`, `resize`, `close_tab`, and `close` layout actions. `assign` adds or focuses a tab in that pane without removing it from other panes, and accepts an optional view. The same Markdown buffer can appear in Source and Preview at once; text and Undo are shared while pane navigation and scroll positions are separate.

Layout actions affect only the active project. Pane IDs remain stable and unique across project switches for the app process, but an inactive project's pane ID is refused; it never selects that project or retargets a different pane. A tab from another project is also refused before effects. First select its project with `workspace` or `focus`, then inspect layout and use that project's actual IDs.

To move a tab to the first position in another pane, supply its source and target IDs:

```json
{"op":"layout","change":{"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":0}}
```

Equal source and target pane IDs reorder that pane's tabs. `index` is required and means the final zero-based position after removing any existing target reference; `0` places the tab first, and the remaining target length appends it. Values outside that group, missing panes, and missing source membership are refused before effects. Moving focuses the target and tab, preserves unsaved text and Undo, and carries the source navigation, view, and scroll state unless the target already has its own state for that tab. An empty source split collapses. The result includes the updated groups, tree, and focus; use it to continue with valid pane IDs. After uncertain delivery, inspect layout before retrying because a successful move may have removed the source pane.

To close only one pane's tab, use the actual IDs from layout:

```json
{"op":"layout","change":{"action":"close_tab","pane_id":2,"tab_id":42}}
```

`close_tab` removes that local tab, leaving the shared buffer in any other pane. If it is the final reference to a dirty or saving buffer, it refuses without effects. Closing the last tab automatically collapses an empty split pane; the sole remaining pane stays available. Ordinary `close TAB_ID...` closes each clean, non-saving document in every pane. Layout `close` closes a whole pane and transfers tabs that have no other pane to a survivor, preserving unsaved work; the last pane cannot be closed. Use the returned layout to verify tab membership and continue with current pane IDs.

`settings` without a patch returns the current preferences; `patch` changes supplied fields. Reading preferences are `font_size` (10–32), `editor_font`, `ui_scale` (0.75–2.0), `line_height` (1.0–2.0), `line_numbers` (`absolute`, `relative`, `off`), `highlight_current_line` and `git_gutter` (markers for lines changed since HEAD; default true). Editing preferences are `vim`, `swap_jk`, `autosave`, `autosave_delay_ms` (300–5000), `tab_width` (1–8), `word_wrap`, `smooth_scroll` and `leader_key`. Explorer always includes ignored/generated files in complete lazy directory listings. `show_ignored_files` is deprecated and accepted without effects for compatibility; `search_excludes` separately affects project content search. Omitted/null leaves a field unchanged; an explicit empty exclusion list clears those optional filters. The entire patch is validated before mutation. Changes persist. Do not change unrelated preferences merely to carry out an editor action.

For example, `{"op":"settings","patch":{"font_size":18,"line_height":1.5,"line_numbers":"relative","ui_scale":1.1}}` changes reading preferences while retaining Vim and save settings. The result contains the applied settings; reindexing can still be pending in `status.workspaces`. Zoom affects the entire interface, while font size and line height control source layout. Recovery keeps its independent 850 ms delay; changing autosave timing never releases agent drafts for automatic saving.

`keybindings` is a complete override map: stable action IDs map to shortcut strings or `null` to disable an action; omitted IDs use defaults. Inspect the effective `keybinding_actions` catalog and existing map before editing it. For example, `{"op":"settings","patch":{"keybindings":{"save":"F8","toggle_left_sidebar":null}}}` replaces all overrides, moves Save to F8, disables the left-sidebar shortcut, and restores defaults for every other action. `{}` resets all overrides. Conflicts, unknown actions and reserved native keys reject the whole patch. The GUI has searchable recording, disable and reset controls in Settings → Keyboard.

Indentation uses `detect_indentation`, `indent_with_tabs` and language-ID maps `language_indent_widths` (1–8) / `language_indent_with_tabs` (booleans). Each supplied map replaces that kind of override; `{}` clears it, omitted/null preserves it. For example, `{"op":"settings","patch":{"detect_indentation":false,"language_indent_widths":{"python":4,"dart":2},"language_indent_with_tabs":{"python":false,"dart":false}}}` forces four-space Python and two-space Dart indentation for subsequent edits. Existing text remains unchanged. With detection enabled, existing file style takes priority.

`panel right_sidebar --hide` hides only the right side, preserving its tabs; `panel left_sidebar` shows the left without pulling a panel from the right. Content commands such as `panel source_control` switch the active content on its assigned side. Each side displays one active panel; the other assigned panels remain available as tabs. Hiding a content panel removes only its tab. `status.panels.sidebars.left.panels` and `.right.panels` expose ordered available tabs; each side also reports its active panel (`null` when empty), visibility and width. `collapsed_panels` is an empty compatibility array. Either side may be empty. Sidebar assignment is a user layout preference; showing a Git panel does not alter source text.

`settings font jetbrains-mono`, `consolas`, or `cascadia-mono` selects a regular editor font. JetBrains Mono is bundled and the default; unavailable Windows fonts fall back to it. The JSON form is `{"op":"settings","patch":{"editor_font":"consolas"}}`; values are `jetbrains_mono`, `consolas`, and `cascadia_mono`, with equivalent hyphen spellings accepted. The response returns the applied choice.

`settings swap-jk true` exchanges j/k navigation; `false` restores the default. Counts, Vim operators and Visual motions follow the exchanged directions, as do Explorer navigation and Markdown scrolling. Inserted text and literal `f`/`t`/`r` targets keep their letters; arrows and page motions retain their direction. The JSON form is `{"op":"settings","patch":{"swap_jk":true}}`.

`settings leader-key space`, `comma`, `backslash`, or a single printable Unicode character changes the Vim leader key. The JSON equivalent for Space is `{"op":"settings","patch":{"leader_key":" "}}`. Named aliases are normalized to the actual character in the applied settings response; controls and multiple characters are refused.

`settings` also returns `multicursor`: `modifier` (`alt` or `ctrl`), `spread_paste`, `match_case`, `whole_word`, and `limit` (100–10000). Its patch is partial: `{"op":"settings","patch":{"multicursor":{"modifier":"ctrl","spread_paste":false}}}` changes those 2 preferences and retains the rest. These preferences control the user's interactive selections. Buffer edit requests still apply to their explicit ranges, independent of visible cursors.
