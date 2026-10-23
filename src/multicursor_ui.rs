//! Multicursor input owns complete editing events; TextEdit supplies layout and
//! ordinary pointer selection. View state travels with the existing navigation.
use super::*;
use crate::multicursor::{self as multi, Motion, Replacement, Selection, State};

use crate::cli::MulticursorModifier as MouseModifier;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub modifier: MouseModifier,
    pub spread_paste: bool,
    pub match_case: bool,
    pub whole_word: bool,
    pub limit: usize,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            modifier: MouseModifier::Alt,
            spread_paste: true,
            match_case: true,
            whole_word: true,
            limit: 1000,
        }
    }
}

fn cursor_range(selection: Selection) -> egui::text::CCursorRange {
    egui::text::CCursorRange::two(
        egui::text::CCursor::new(selection.anchor),
        head_cursor(selection),
    )
}

fn head_cursor(selection: Selection) -> egui::text::CCursor {
    let mut cursor = egui::text::CCursor::new(selection.head);
    cursor.prefer_next_row = selection.prefer_next_row;
    cursor
}

#[derive(Clone)]
pub(super) struct Geometry {
    pub revision: u64,
    pub galley: Arc<egui::Galley>,
    pub origin: egui::Pos2,
    pub rect: egui::Rect,
}

fn visual_move(state: &mut State, galley: &egui::Galley, down: bool, extend: bool, steps: usize) {
    for selection in &mut state.selections {
        let mut cursor = head_cursor(*selection);
        let mut x = selection.visual_x.map(|x| x as f32 / 64.0);
        for _ in 0..steps {
            (cursor, x) = if down {
                galley.cursor_down_one_row(&cursor, x)
            } else {
                galley.cursor_up_one_row(&cursor, x)
            };
        }
        selection.head = cursor.index.0;
        selection.prefer_next_row = cursor.prefer_next_row;
        selection.visual_x = x.map(|x| (x * 64.0).round() as i32);
        if !extend {
            selection.anchor = selection.head;
        }
    }
    state.normalize(usize::MAX);
}

impl EditorTab {
    pub(super) fn sync_multicursor(&mut self) {
        if let Some(primary) = self.vim.multi.primary() {
            self.vim.cursor = primary.head;
            self.selection = primary.range();
            self.pending_selection = Some(self.selection.clone());
        }
    }
    fn start_multicursor(&mut self, selection: Selection) {
        if self.vim.multi.active() {
            return;
        }
        if self.vim.finish_insert(&mut self.text).text_changed {
            self.changed();
        }
        self.vim
            .set_selection(&self.text, selection.head..selection.head);
        self.vim.mode = VimMode::Normal;
        self.vim.multi.start(selection);
        self.sync_multicursor();
    }
    pub(super) fn exit_multicursor(&mut self) {
        let Some(primary) = self.vim.multi.primary() else {
            return;
        };
        self.vim.multi.clear();
        self.vim
            .set_selection(&self.text, primary.head..primary.head);
        self.set_cursor(primary.head..primary.head);
    }
}

#[derive(Clone)]
struct NativeRecord {
    text: Arc<str>,
    range: egui::text::CCursorRange,
    state: State,
}
#[derive(Clone, Default)]
struct NativeRecords {
    entries: std::collections::VecDeque<NativeRecord>,
    bytes: usize,
    cursor: Option<usize>,
}
fn native_id(document: u64) -> egui::Id {
    egui::Id::new(("multicursor_native_history", document))
}
pub(super) fn record_native(
    ctx: &egui::Context,
    document: u64,
    text: &str,
    range: egui::text::CCursorRange,
    state: &State,
) {
    ctx.data_mut(|data| {
        let records = data.get_temp_mut_or_default::<NativeRecords>(native_id(document));
        if records
            .cursor
            .and_then(|cursor| records.entries.get(cursor))
            .is_some_and(|r| &*r.text == text && r.range == range && r.state == *state)
        {
            return;
        }
        if let Some(cursor) = records.cursor {
            while records.entries.len() > cursor + 1 {
                records.bytes -= records.entries.pop_back().unwrap().text.len();
            }
        }
        records.bytes += text.len();
        records.entries.push_back(NativeRecord {
            text: Arc::from(text),
            range,
            state: state.clone(),
        });
        while records.entries.len() > 100
            || records.bytes > 16 * 1024 * 1024 && records.entries.len() > 1
        {
            records.bytes -= records.entries.pop_front().unwrap().text.len();
        }
        records.cursor = Some(records.entries.len() - 1);
    });
}
pub(super) fn restore_native(
    ctx: &egui::Context,
    document: u64,
    text: &str,
    range: egui::text::CCursorRange,
    redo: bool,
) -> State {
    ctx.data_mut(|data| {
        let records = data.get_temp_mut_or_default::<NativeRecords>(native_id(document));
        let matches = |index: &usize| {
            let r = &records.entries[*index];
            &*r.text == text && r.range.as_sorted_char_range() == range.as_sorted_char_range()
        };
        // Identical document text can occur several times in undo history with
        // different secondary cursors. Follow history direction, not the newest
        // matching text, and discard this future only on the next actual edit.
        let found = if redo {
            (records.cursor.map_or(0, |cursor| cursor + 1)..records.entries.len()).find(matches)
        } else {
            (0..records.cursor.unwrap_or(records.entries.len()))
                .rev()
                .find(matches)
        };
        records.cursor = found;
        found
            .map(|index| records.entries[index].state.clone())
            .unwrap_or_default()
    })
}

impl VEditApp {
    fn multi_selection(&self) -> Option<Selection> {
        let tab = self.tab()?;
        if self.settings.vim && tab.vim.mode != VimMode::Insert {
            let range = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            Some(if tab.vim.cursor == range.start && !range.is_empty() {
                Selection::new(range.end, range.start)
            } else {
                Selection::new(range.start, range.end)
            })
        } else {
            let range = egui::TextEdit::load_state(&self.ctx, self.document_widget_id(tab.id))
                .and_then(|s| s.cursor.char_range());
            Some(range.map_or(
                Selection::new(tab.selection.start, tab.selection.end),
                |r| Selection::new(r.secondary.index.0, r.primary.index.0),
            ))
        }
    }
    pub(super) fn multicursor_action(&mut self, action: keybindings::Action) {
        use keybindings::Action::*;
        if action == ExitMulticursor
            && let Some(tab) = self.tab()
        {
            self.ctx.data_mut(|d| {
                d.remove::<String>(self.document_widget_id(tab.id).with("multi_ime"))
            });
        }
        let Some(selection) = self.multi_selection() else {
            return;
        };
        let preferences = self.settings.multicursor.clone();
        let Some(tab) = self
            .tab_mut()
            .filter(|t| !t.loading && t.agent_view == crate::cli::DocumentView::Source)
        else {
            return;
        };
        if action == ExitMulticursor {
            tab.exit_multicursor();
            return;
        }
        if action == CursorUndo && !tab.vim.multi.active() {
            return;
        }
        tab.start_multicursor(selection);
        let state = &mut tab.vim.multi;
        let complete = match action {
            AddCursorAbove | AddCursorBelow => {
                state.add_vertical(&tab.text, action == AddCursorBelow, preferences.limit)
            }
            CursorsAtLineEnds => state.line_ends(&tab.text, preferences.limit),
            SelectNextOccurrence | SelectAllOccurrences | SkipOccurrence => state.occurrences(
                &tab.text,
                action == SelectAllOccurrences,
                action == SkipOccurrence,
                preferences.match_case,
                preferences.whole_word,
                preferences.limit,
            ),
            CursorUndo => {
                state.undo_selection();
                true
            }
            _ => true,
        };
        tab.sync_multicursor();
        tab.focus = true;
        tab.scroll_to_cursor = true;
        if !complete {
            self.status(format!("Cursor limit reached ({}). Refine the selection or increase the limit in Settings → Editing.", preferences.limit), true);
        }
    }

    pub(super) fn commit_multi(&mut self, text: String, state: State) {
        let Some(id) = self.tab().map(|t| t.id) else {
            return;
        };
        let widget = self.document_widget_id(id);
        let ctx = self.ctx.clone();
        let vim = self.settings.vim;
        let tab = self.tab_mut().unwrap();
        if text != tab.text {
            if !vim {
                let range = cursor_range(tab.vim.multi.primary().unwrap());
                record_native(&ctx, id, &tab.text, range, &tab.vim.multi);
                context_actions::native_history_checkpoint(
                    &ctx,
                    id,
                    widget,
                    &tab.text,
                    tab.selection.clone(),
                );
            }
            tab.vim.begin_external_edit(&tab.text);
            tab.text = text;
            tab.vim.multi = state;
            tab.sync_multicursor();
            tab.changed();
            if !vim {
                let range = cursor_range(tab.vim.multi.primary().unwrap());
                record_native(&ctx, id, &tab.text, range, &tab.vim.multi);
                context_actions::native_history_checkpoint(
                    &ctx,
                    id,
                    widget,
                    &tab.text,
                    tab.selection.clone(),
                );
            }
        } else {
            tab.vim.multi = state;
            tab.sync_multicursor();
        }
        tab.scroll_to_cursor = true;
        ctx.request_repaint();
    }
    pub(super) fn multi_replace(&mut self, text: &str, paste: bool) {
        let tab = self.tab().unwrap();
        let mut state = tab.vim.multi.clone();
        let parts: Vec<_> = text
            .split('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s).to_owned())
            .collect();
        let values = if paste
            && self.settings.multicursor.spread_paste
            && parts.len() == state.selections.len()
        {
            parts
        } else {
            vec![text.to_owned()]
        };
        let output = multi::replace(&tab.text, &mut state, &values);
        self.commit_multi(output, state);
    }
    pub(super) fn multi_clipboard(&mut self, cut: bool) {
        let tab = self.tab().unwrap();
        let (text, ranges) = multi::clipboard(&tab.text, &tab.vim.multi);
        self.ctx.copy_text(text);
        if cut {
            let mut state = tab.vim.multi.clone();
            let output = multi::apply(
                &tab.text,
                &mut state,
                ranges
                    .into_iter()
                    .map(|range| Replacement {
                        range,
                        text: String::new(),
                    })
                    .collect(),
            );
            self.commit_multi(output, state);
        }
    }
    pub(super) fn multi_command(&mut self, command: editor_input::Command) {
        if matches!(
            command,
            editor_input::Command::Undo | editor_input::Command::Redo
        ) {
            let id = self.tab().unwrap().id;
            self.apply_editor_context_action(
                id,
                if command == editor_input::Command::Redo {
                    context_actions::EditorAction::Redo
                } else {
                    context_actions::EditorAction::Undo
                },
            );
            return;
        }
        let tab = self.tab().unwrap();
        let indent = tab.indentation(&self.settings);
        let mut state = tab.vim.multi.clone();
        let output = edit_command(
            &tab.text,
            &mut state,
            command,
            &indent.unit(),
            indent.width,
            indent.tabs,
            crate::file_kind::comment_language(tab.path.as_deref()),
        );
        self.commit_multi(output, state);
    }
    /// Called before global shortcuts. Consume only the ordered source prefix;
    /// native input before activation finishes on the existing continuation path.
    pub(super) fn multicursor_keys(&mut self) {
        use keybindings::Action;
        let Some(tab) = self.tab() else {
            return;
        };
        let id = tab.id;
        if tab.loading
            || tab.agent_view != crate::cli::DocumentView::Source
            || !self
                .ctx
                .memory(|m| m.has_focus(self.document_widget_id(id)))
        {
            return;
        }
        let events = self.ctx.input_mut(|i| std::mem::take(&mut i.events));
        let pass = self.ctx.cumulative_pass_nr();
        let native_copy = events.iter().any(|e| matches!(e, egui::Event::Copy));
        let native_cut = events.iter().any(|e| matches!(e, egui::Event::Cut));
        let native_paste = events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
        let mut remaining = Vec::new();
        let mut events = events.into_iter().peekable();
        while let Some(event) = events.next() {
            let active = self.tab().is_some_and(|t| t.vim.multi.active());
            if let egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            } = &event
            {
                let widget = self.document_widget_id(id);
                let geometry = self
                    .ctx
                    .data(|d| d.get_temp::<Geometry>(widget.with("multi_layout")));
                let additive = match self.settings.multicursor.modifier {
                    MouseModifier::Alt => modifiers.alt && !modifiers.ctrl,
                    MouseModifier::Ctrl => modifiers.ctrl && !modifiers.alt,
                };
                if let Some(geometry) = geometry.filter(|g| g.rect.contains(*pos))
                    && (active || additive)
                {
                    if remaining.iter().any(editor_input::native_input)
                        || self.tab().is_none_or(|t| t.revision != geometry.revision)
                    {
                        self.ctx.data_mut(|d| {
                            d.insert_temp(widget.with("multi_pointer_handled"), pass)
                        });
                        editor_input::defer(
                            &self.ctx,
                            id,
                            std::iter::once(event).chain(events).collect(),
                        );
                        break;
                    }
                    if additive {
                        let selection = self.multi_selection().unwrap();
                        let limit = self.settings.multicursor.limit;
                        let tab = self.tab_mut().unwrap();
                        tab.start_multicursor(selection);
                        if modifiers.shift {
                            tab.vim.multi.checkpoint();
                            self.ctx
                                .data_mut(|d| d.insert_temp(widget.with("multicursor_box"), *pos));
                        } else {
                            let at = geometry.galley.cursor_from_pos(*pos - geometry.origin);
                            let full = !tab.vim.multi.toggle(at.index.0, limit);
                            tab.sync_multicursor();
                            if full {
                                self.status(format!("Cursor limit reached ({limit})"), true);
                            }
                        }
                    } else {
                        self.tab_mut().unwrap().exit_multicursor();
                    }
                    self.ctx
                        .data_mut(|d| d.insert_temp(widget.with("multi_pointer_handled"), pass));
                    let defer = !additive || modifiers.shift;
                    remaining.push(event);
                    if defer {
                        editor_input::defer(&self.ctx, id, events.collect());
                        break;
                    }
                    continue;
                }
            }
            let action = Action::ALL.iter().copied().find(|a| {
                a.multicursor()
                    && self.shortcut_matches(*a, &event)
                    && (*a != Action::CursorUndo || active)
            });
            if let Some(action) = action {
                if remaining.iter().any(editor_input::native_input) {
                    editor_input::defer(
                        &self.ctx,
                        id,
                        std::iter::once(event).chain(events).collect(),
                    );
                    break;
                }
                if events
                    .peek()
                    .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                {
                    events.next();
                }
                self.multicursor_action(action);
                continue;
            }
            if !active {
                remaining.push(event);
                continue;
            }
            if self.completions.is_some()
                && matches!(
                    event,
                    egui::Event::Key {
                        key: Key::ArrowUp | Key::ArrowDown | Key::Enter | Key::Tab | Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            {
                remaining.push(event);
                remaining.extend(events);
                break;
            }
            if let Some(command) = editor_input::command(&event, true, &self.settings.keybindings) {
                if events
                    .peek()
                    .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                {
                    events.next();
                }
                self.multi_command(command);
                continue;
            }
            // Workspace commands are dispatched in order by the normal router.
            if Action::ALL
                .iter()
                .any(|a| !a.multicursor() && self.shortcut_matches(*a, &event))
            {
                remaining.push(event);
                remaining.extend(events);
                break;
            }
            match event {
                egui::Event::Text(text) => self.multi_replace(&text, false),
                egui::Event::Paste(text) => self.multi_replace(&text, true),
                egui::Event::Copy => self.multi_clipboard(false),
                egui::Event::Cut => self.multi_clipboard(true),
                egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    self.ctx.data_mut(|d| {
                        d.remove::<String>(self.document_widget_id(id).with("multi_ime"))
                    });
                    self.multi_replace(&text, false);
                }
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    self.ctx.data_mut(|d| {
                        d.insert_temp(self.document_widget_id(id).with("multi_ime"), text)
                    });
                }
                egui::Event::Ime(_) => {
                    self.ctx.data_mut(|d| {
                        d.remove::<String>(self.document_widget_id(id).with("multi_ime"))
                    });
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if key == Key::Escape {
                        self.ctx.data_mut(|d| {
                            d.remove::<String>(self.document_widget_id(id).with("multi_ime"))
                        });
                        self.tab_mut().unwrap().exit_multicursor();
                        editor_input::defer(&self.ctx, id, events.collect());
                        break;
                    }
                    let command = modifiers.ctrl || modifiers.command;
                    if !modifiers.alt {
                        if command && key == Key::C || command && key == Key::Insert {
                            if !native_copy {
                                self.multi_clipboard(false);
                            }
                            continue;
                        }
                        if command && key == Key::X || modifiers.shift && key == Key::Delete {
                            if !native_cut {
                                self.multi_clipboard(true);
                            }
                            continue;
                        }
                        if command && key == Key::V || modifiers.shift && key == Key::Insert {
                            if !native_paste {
                                self.apply_editor_context_action(
                                    id,
                                    context_actions::EditorAction::Paste,
                                );
                            }
                            continue;
                        }
                    }
                    if command && key == Key::A && !modifiers.alt {
                        let tab = self.tab_mut().unwrap();
                        tab.vim.multi.checkpoint();
                        tab.vim.multi.selections =
                            vec![Selection::new(0, tab.text.chars().count())];
                        tab.sync_multicursor();
                        continue;
                    }
                    if matches!(key, Key::Backspace | Key::Delete) && !modifiers.alt {
                        let tab = self.tab().unwrap();
                        let mut state = tab.vim.multi.clone();
                        let output =
                            multi::delete(&tab.text, &mut state, key == Key::Backspace, command);
                        self.commit_multi(output, state);
                        continue;
                    }
                    let motion = match key {
                        Key::ArrowLeft => Some(Motion::Left),
                        Key::ArrowRight => Some(Motion::Right),
                        Key::ArrowUp => Some(Motion::Up),
                        Key::ArrowDown => Some(Motion::Down),
                        Key::Home => Some(if command {
                            Motion::DocumentStart
                        } else {
                            Motion::Home
                        }),
                        Key::End => Some(if command {
                            Motion::DocumentEnd
                        } else {
                            Motion::End
                        }),
                        Key::PageUp => Some(Motion::Up),
                        Key::PageDown => Some(Motion::Down),
                        _ => None,
                    };
                    if let Some(motion) = motion.filter(|_| !modifiers.alt) {
                        let steps = if matches!(key, Key::PageUp | Key::PageDown) {
                            self.editor_rect.map_or(20, |r| {
                                (r.height() / (self.settings.font_size * self.settings.line_height))
                                    .max(1.0) as usize
                            })
                        } else {
                            1
                        };
                        let geometry = self.ctx.data(|d| {
                            d.get_temp::<Geometry>(self.document_widget_id(id).with("multi_layout"))
                        });
                        if matches!(motion, Motion::Up | Motion::Down)
                            && geometry.as_ref().is_some_and(|g| {
                                self.tab().is_some_and(|t| t.revision != g.revision)
                            })
                        {
                            // Layout must include preceding input before a
                            // vertical move can follow wrapped visual rows.
                            let event = egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers,
                            };
                            editor_input::defer(
                                &self.ctx,
                                id,
                                std::iter::once(event).chain(events).collect(),
                            );
                            break;
                        }
                        let tab = self.tab_mut().unwrap();
                        if let Some(geometry) = geometry.filter(|g| {
                            g.revision == tab.revision
                                && matches!(motion, Motion::Up | Motion::Down)
                        }) {
                            visual_move(
                                &mut tab.vim.multi,
                                &geometry.galley,
                                matches!(motion, Motion::Down),
                                modifiers.shift,
                                steps,
                            );
                        } else {
                            for _ in 0..steps {
                                multi::move_selections(
                                    &mut tab.vim.multi,
                                    &tab.text,
                                    motion,
                                    modifiers.shift,
                                    command,
                                );
                            }
                        }
                        tab.sync_multicursor();
                        tab.scroll_to_cursor = true;
                    }
                }
                other => remaining.push(other),
            }
        }
        self.ctx.input_mut(|i| i.events = remaining);
    }

    pub(super) fn multicursor_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        editor_preferences::preference_heading(ui, "Multicursor");
        let prefs = &mut self.settings.multicursor;
        let before = prefs.clone();
        egui::ComboBox::from_label("Add cursor with")
            .selected_text(match prefs.modifier {
                MouseModifier::Alt => "Alt + click",
                MouseModifier::Ctrl => "Ctrl + click",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.modifier, MouseModifier::Alt, "Alt + click");
                ui.selectable_value(&mut prefs.modifier, MouseModifier::Ctrl, "Ctrl + click");
            });
        ui.label(RichText::new("Hold Shift as well and drag for a rectangular selection. Click an existing cursor again to remove it.").small().color(MUTED));
        ui.checkbox(&mut prefs.spread_paste,"Distribute pasted lines across cursors")
            .on_hover_text("When the number of lines matches the cursor count, paste one line per cursor in document order. Otherwise paste the complete text at every cursor.");
        ui.checkbox(&mut prefs.match_case, "Match case when adding occurrences");
        ui.checkbox(
            &mut prefs.whole_word,
            "Match whole words when adding occurrences",
        );
        ui.add(
            egui::Slider::new(&mut prefs.limit, 100..=10_000)
                .logarithmic(true)
                .text("Maximum cursors"),
        );
        ui.label(RichText::new("Multicursor uses standard editing keys, also with Vim enabled. Esc returns to one cursor. Configure its shortcuts in Keyboard.").small().color(MUTED));
        before != *prefs
    }
}

/// Reuse line-edit semantics on disjoint line groups. Every group is evaluated
/// against the original text; overlapping lines are handled exactly once.
fn edit_command(
    text: &str,
    state: &mut State,
    command: editor_input::Command,
    indent: &str,
    width: usize,
    tabs: bool,
    language: &str,
) -> String {
    use editor_input::Command::*;
    let chars: Vec<_> = text.chars().collect();
    let rows = multi::lines(&chars);
    let linewise = !matches!(command, Newline)
        && !(matches!(command, Indent(false))
            && state.selections.iter().all(|s| s.range().is_empty()));
    let mut selections: Vec<_> = state.selections.iter().map(|s| s.range()).collect();
    selections.sort_by_key(|r| r.start);
    if linewise {
        let mut groups: Vec<Range<usize>> = Vec::new();
        for selection in selections {
            let first = multi::row_at(&rows, selection.start);
            if let Some(group) = groups.last_mut() {
                let previous = multi::row_at(
                    &rows,
                    if group.start == group.end {
                        group.end
                    } else {
                        group.end - 1
                    },
                );
                if first <= previous + usize::from(matches!(command, MoveLine(_))) {
                    group.end = group.end.max(selection.end);
                    continue;
                }
            }
            groups.push(selection);
        }
        selections = groups;
    }
    let original = state.selections.clone();
    let mut edits: Vec<(usize, Replacement)> = Vec::new();
    let mut targets: Vec<(usize, usize, usize, Selection)> = Vec::new();
    for (group, range) in selections.iter().enumerate() {
        let first = multi::row_at(&rows, range.start);
        let last = multi::row_at(
            &rows,
            range.end.saturating_sub(usize::from(!range.is_empty())),
        );
        // Only inspect the affected lines and their immediate neighbors. This
        // keeps 1,000 cursors in a large file from copying that file 1,000 times.
        let base = rows[first.saturating_sub(1)].start;
        let end = rows[(last + 1).min(rows.len() - 1)].break_end;
        let local: String = chars[base..end].iter().collect();
        let local_range = range.start - base..range.end - base;
        let edit = match command {
            Newline => editor::smart_newline(&local, local_range.clone(), indent),
            Indent(false) if !linewise => {
                editor::insert_tab(&local, local_range.clone(), width, tabs)
            }
            Indent(outdent) => {
                editor::indent_lines_with_width(&local, local_range.clone(), indent, width, outdent)
            }
            Comment => editor::toggle_line_comment(&local, local_range.clone(), language),
            Duplicate => editor::duplicate_lines(&local, local_range.clone()),
            DeleteLine => editor::delete_lines(&local, local_range.clone()),
            InsertLine(above) => editor::insert_line(&local, local_range.clone(), indent, above),
            MoveLine(down) => editor::move_lines(&local, local_range.clone(), down),
            _ => continue,
        };
        if edit.text == local && edit.selection == local_range {
            continue;
        }
        let shift = edit.selection.start as isize - local_range.start as isize;
        let explicit_target = matches!(
            command,
            Newline | InsertLine(_) | DeleteLine | Duplicate | MoveLine(_)
        ) || matches!(command, Indent(false)) && !linewise;
        if explicit_target {
            for (index, selection) in original.iter().enumerate() {
                let selected = selection.range();
                if selected.start < range.start || selected.end > range.end {
                    continue;
                }
                let next = if matches!(command, Duplicate | MoveLine(_)) {
                    Selection::new(
                        selection.anchor.saturating_add_signed(shift),
                        selection.head.saturating_add_signed(shift),
                    )
                } else {
                    Selection::new(base + edit.selection.start, base + edit.selection.end)
                };
                targets.push((index, group, range.start, next));
            }
        }
        if !linewise {
            // A diff can slide an insertion across identical whitespace. These
            // commands replace the selected range, with smart newline allowed
            // to consume whitespace before a matching closing delimiter.
            let edited: Vec<_> = edit.text.chars().collect();
            let local_chars = &chars[base..end];
            let suffix = local_chars[local_range.end..]
                .iter()
                .rev()
                .zip(edited[local_range.start..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let replacement: String = edited[local_range.start..edited.len() - suffix]
                .iter()
                .collect();
            // Trimming spaces must never swallow a following cursor.
            let limit = selections.get(group + 1).map_or(end, |next| next.start);
            edits.push((
                group,
                Replacement {
                    range: range.start..(end - suffix).min(limit),
                    text: replacement,
                },
            ));
            continue;
        }
        // Diff only this small window. Preserve separate indentation/comment
        // insertions so existing selections keep their original text boundaries.
        let diff = similar::TextDiff::from_chars(&local, &edit.text);
        let mut position = base;
        let mut pending: Option<Replacement> = None;
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Equal => {
                    if let Some(edit) = pending.take() {
                        edits.push((group, edit));
                    }
                    position += change.value().chars().count();
                }
                similar::ChangeTag::Delete => {
                    let edit = pending.get_or_insert_with(|| Replacement {
                        range: position..position,
                        text: String::new(),
                    });
                    position += change.value().chars().count();
                    edit.range.end = position;
                }
                similar::ChangeTag::Insert => pending
                    .get_or_insert_with(|| Replacement {
                        range: position..position,
                        text: String::new(),
                    })
                    .text
                    .push_str(change.value()),
            }
        }
        if let Some(edit) = pending {
            edits.push((group, edit));
        }
    }
    edits.sort_by_key(|(_, e)| (e.range.start, e.range.end));
    edits.dedup_by(|(_, b), (_, a)| a.range == b.range && a.text == b.text);
    let mut mapped: Vec<_> = original
        .iter()
        .map(|s| Selection::new(s.anchor, s.head))
        .collect();
    let mut merged: Vec<Replacement> = Vec::new();
    for (_, edit) in &edits {
        if let Some(previous) = merged.last_mut()
            && previous.range.end > edit.range.start
        {
            // Adjacent deletions at EOF can share the preceding line break.
            assert!(previous.text.is_empty() && edit.text.is_empty());
            previous.range.end = previous.range.end.max(edit.range.end);
        } else {
            merged.push(edit.clone());
        }
    }
    for selection in &mut mapped {
        selection.anchor = multi::map_position(selection.anchor, &merged);
        selection.head = multi::map_position(selection.head, &merged);
    }
    for (index, group, at, mut target) in targets {
        let delta: isize = edits
            .iter()
            .filter(|(owner, e)| *owner != group && e.range.end <= at)
            .map(|(_, e)| e.text.chars().count() as isize - e.range.len() as isize)
            .sum();
        target.anchor = target.anchor.saturating_add_signed(delta);
        target.head = target.head.saturating_add_signed(delta);
        mapped[index] = target;
    }
    let result = multi::apply(text, state, merged);
    state.selections = mapped;
    state.normalize(result.chars().count());
    result
}

pub(super) fn pointer(
    ui: &egui::Ui,
    output: &mut egui::text_edit::TextEditOutput,
    tab: &mut EditorTab,
    preferences: &Preferences,
    before: Selection,
    enabled: bool,
) {
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            output.response.id.with("multi_layout"),
            Geometry {
                revision: tab.revision,
                galley: output.galley.clone(),
                origin: output.galley_pos,
                rect: output.response.rect.intersect(ui.clip_rect()),
            },
        )
    });
    if !enabled {
        return;
    }
    let modifiers = ui.input(|i| i.modifiers);
    let additive = match preferences.modifier {
        MouseModifier::Alt => modifiers.alt && !modifiers.ctrl,
        MouseModifier::Ctrl => modifiers.ctrl && !modifiers.alt,
    };
    let pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    let down = ui.input(|i| i.pointer.primary_down());
    let drag_id = output.response.id.with("multicursor_box");
    let position = ui.input(|i| i.pointer.interact_pos());
    if pressed
        && output.response.contains_pointer()
        && ui
            .ctx()
            .data(|d| d.get_temp::<u64>(output.response.id.with("multi_pointer_handled")))
            != Some(ui.ctx().cumulative_pass_nr())
        && let Some(pos) = position
    {
        if additive {
            tab.start_multicursor(before);
            let at = output
                .galley
                .cursor_from_pos(pos - output.galley_pos)
                .index
                .0;
            if modifiers.shift {
                tab.vim.multi.checkpoint();
                ui.ctx().data_mut(|d| d.insert_temp(drag_id, pos));
            } else if !tab.vim.multi.toggle(at, preferences.limit) {
                tab.vim_message = Some(format!("Cursor limit reached ({})", preferences.limit));
            }
        } else if tab.vim.multi.active() {
            tab.exit_multicursor();
        }
    }
    if down
        && let Some(origin) = ui.ctx().data(|d| d.get_temp::<egui::Pos2>(drag_id))
        && let Some(pos) = position
    {
        let top = origin.y.min(pos.y) - output.galley_pos.y;
        let bottom = origin.y.max(pos.y) - output.galley_pos.y;
        let mut selections = Vec::new();
        for row in &output.galley.rows {
            if row.pos.y + row.size.y < top || row.pos.y > bottom {
                continue;
            }
            let y = row.pos.y + row.size.y * 0.5;
            let anchor = output
                .galley
                .cursor_from_pos(egui::vec2(origin.x - output.galley_pos.x, y))
                .index
                .0;
            let head = output
                .galley
                .cursor_from_pos(egui::vec2(pos.x - output.galley_pos.x, y))
                .index
                .0;
            selections.push(Selection::new(anchor, head));
            if selections.len() > preferences.limit {
                break;
            }
        }
        if selections.len() > preferences.limit {
            tab.vim_message = Some(format!(
                "Cursor limit reached ({}). Refine the rectangle or increase the limit in Settings → Editing.",
                preferences.limit
            ));
            selections.clear();
        }
        if pos.y < origin.y {
            selections.reverse();
        }
        if !selections.is_empty() {
            tab.vim.multi.selections = selections;
        }
        tab.vim.multi.normalize(tab.text.chars().count());
    }
    if !down {
        ui.ctx().data_mut(|d| d.remove::<egui::Pos2>(drag_id));
    }
    if let Some(primary) = tab.vim.multi.primary() {
        tab.sync_multicursor();
        output
            .state
            .cursor
            .set_char_range(Some(cursor_range(primary)));
        output.state.clone().store(ui.ctx(), output.response.id);
    }
}

pub(super) fn paint(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    state: &State,
    focused: bool,
) {
    if !state.active() {
        ui.ctx()
            .data_mut(|d| d.remove::<String>(output.response.id.with("multi_ime")));
        return;
    }
    let mut visuals = ui.visuals().clone();
    editor_selection(&mut visuals);
    let mut galley = output.galley.clone();
    for selection in &state.selections {
        let range = selection.range();
        let top = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(range.start))
            .top()
            + output.galley_pos.y;
        let bottom = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(range.end))
            .bottom()
            + output.galley_pos.y;
        if bottom < ui.clip_rect().top() || top > ui.clip_rect().bottom() {
            continue;
        }
        egui::text_selection::visuals::paint_text_selection(
            &mut galley,
            &visuals,
            &cursor_range(*selection),
            None,
        );
    }
    crate::render::keep_glyph_colors(&mut galley, &output.galley);
    crate::render::replace_text_shape(ui, output, galley);
    for (index, selection) in state.selections.iter().enumerate() {
        let rect = output
            .galley
            .pos_from_cursor(head_cursor(*selection))
            .translate(output.galley_pos.to_vec2());
        if ui.clip_rect().intersects(rect.expand(1.0)) {
            let primary = index + 1 == state.selections.len();
            let color = if primary { ACCENT } else { TEXT };
            ui.painter().line_segment(
                [rect.left_top(), rect.left_bottom()],
                Stroke::new(
                    if primary { 2.0 } else { 1.4 },
                    color.linear_multiply(if focused { 1.0 } else { 0.5 }),
                ),
            );
            if primary && focused {
                let ime_rect = ui.clip_rect();
                ui.output_mut(|output| {
                    output.ime = Some(egui::output::IMEOutput {
                        purpose: egui::IMEPurpose::Normal,
                        rect: ime_rect,
                        cursor_rect: rect,
                        should_interrupt_composition: false,
                    })
                });
                if let Some(preedit) = ui
                    .ctx()
                    .data(|d| d.get_temp::<String>(output.response.id.with("multi_ime")))
                    .filter(|s| !s.is_empty())
                {
                    let galley = ui.painter().layout_no_wrap(
                        preedit,
                        FontId::monospace(rect.height() / 1.35),
                        TEXT,
                    );
                    let bounds = egui::Rect::from_min_size(rect.min, galley.size());
                    ui.painter().rect_filled(bounds, 0.0, BG);
                    ui.painter().galley(rect.min, galley, TEXT);
                    ui.painter().line_segment(
                        [bounds.left_bottom(), bounds.right_bottom()],
                        Stroke::new(1.0, ACCENT),
                    );
                }
            }
        }
    }
}

/// Completion comes from the primary location. Repeat it only where its source
/// text agrees; language-server auxiliary edits (e.g. imports) are applied once.
pub(super) fn complete(
    text: &str,
    state: &State,
    main: &crate::lsp::TextEdit,
    additional: &[crate::lsp::TextEdit],
    cursor: usize,
) -> Result<(String, State), &'static str> {
    let chars: Vec<_> = text.chars().collect();
    let start = crate::lsp::position_to_char(text, main.range.start);
    let end = crate::lsp::position_to_char(text, main.range.end);
    if start > cursor || end < cursor || end > chars.len() {
        return Err("Completion does not match the cursor selection; no text changed.");
    }
    let before = cursor - start;
    let after = end - cursor;
    let mut state = state.clone();
    let mut edits = Vec::new();
    for selection in &mut state.selections {
        let Some(start_at) = selection.head.checked_sub(before) else {
            return Err("Completion needs the same text at every cursor; no text changed.");
        };
        let end_at = selection.head + after;
        if chars.get(start_at..end_at) != chars.get(start..end) {
            return Err("Completion needs the same text at every cursor; no text changed.");
        }
        *selection = Selection::new(start_at, end_at);
        edits.push(Replacement {
            range: start_at..end_at,
            text: main.new_text.clone(),
        });
    }
    edits.extend(additional.iter().map(|edit| Replacement {
        range: crate::lsp::position_to_char(text, edit.range.start)
            ..crate::lsp::position_to_char(text, edit.range.end),
        text: edit.new_text.clone(),
    }));
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    edits.dedup_by(|b, a| a.range == b.range && a.text == b.text);
    if edits
        .windows(2)
        .any(|e| e[0].range.end > e[1].range.start || e[0].range.start == e[1].range.start)
    {
        return Err("Completion edits overlap; no text changed. Press Esc to use a single cursor.");
    }
    let output = multi::apply(text, &mut state, edits);
    for selection in &mut state.selections {
        selection.anchor = selection.head;
    }
    Ok((output, state))
}
