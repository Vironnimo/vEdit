//! Language event publication, navigation and completion UI.
use super::*;

pub(super) struct CompletionPopup {
    pub(super) tab_id: u64,
    pub(super) version: i32,
    pub(super) cursor: usize,
    pub(super) items: Vec<crate::lsp::CompletionItem>,
    pub(super) selected: usize,
}

impl VEditApp {
    pub(super) fn poll_languages(&mut self) {
        if let Some((request, revision, cursor, widget)) = self.definition_origin
            && self
                .definition_request
                .is_some_and(|(pending, _)| pending == request)
            && self.tab().is_none_or(|tab| {
                tab.revision != revision
                    || tab.vim.cursor != cursor
                    || self.document_widget_id(tab.id) != widget
            })
        {
            self.definition_request = None;
            self.definition_origin = None;
            if self
                .status
                .as_ref()
                .is_some_and(|(text, _, _)| text == "Finding definition…")
            {
                self.status = None;
            }
        }
        let mut events = Vec::new();
        for (index, project) in self.projects.iter().enumerate() {
            for (language, session) in &project.languages {
                if let Some(server) = &session.server {
                    for _ in 0..256 {
                        let Ok(event) = server.receiver.try_recv() else {
                            break;
                        };
                        events.push((index, language.clone(), event));
                    }
                    if events.len() >= 256 {
                        self.ctx.request_repaint();
                    }
                }
            }
        }
        for (index, language, event) in events {
            match event {
                crate::lsp::LspEvent::Ready => {
                    let session = self.projects[index].languages.get_mut(&language).unwrap();
                    session.ready = true;
                    session.error = None;
                }
                crate::lsp::LspEvent::Error(error) => {
                    let project = &mut self.projects[index];
                    let session = project.languages.get_mut(&language).unwrap();
                    session.error = Some(error.clone());
                    if session
                        .server
                        .as_ref()
                        .is_some_and(|server| server.is_stopped())
                    {
                        session.ready = false;
                        session.opened.clear();
                        project.clear_language_diagnostics(&language);
                    }
                    self.status(error, true);
                }
                crate::lsp::LspEvent::RequestFailed { request_id, error } => {
                    // Hover, references and symbols report in their own views.
                    if self.language_intel_failed(request_id, error.clone()) {
                        continue;
                    }
                    if self.active_project == index {
                        if self.completion_request == Some(request_id) {
                            self.completion_request = None;
                            self.completions = None;
                        }
                        if self
                            .definition_request
                            .is_some_and(|(id, _)| id == request_id)
                        {
                            self.definition_request = None;
                            if self
                                .status
                                .as_ref()
                                .is_some_and(|(text, _, _)| text == "Finding definition…")
                            {
                                self.status = None;
                            }
                        }
                    }
                    if let Some(error) = error {
                        self.projects[index]
                            .languages
                            .get_mut(&language)
                            .unwrap()
                            .error = Some(error.clone());
                        self.status(error, true);
                    }
                }
                crate::lsp::LspEvent::Diagnostics {
                    path,
                    version,
                    diagnostics,
                } => {
                    let project = &self.projects[index];
                    let session = &project.languages[&language];
                    let owner = self
                        .settings
                        .language_servers
                        .iter()
                        .find(|config| config.enabled && config.matches(&path));
                    let allowed = owner.map_or_else(
                        || context_actions::path_contains(&project.root, &path),
                        |config| config.language_id == language,
                    );
                    if session.active()
                        && allowed
                        && !session.retired_paths.contains(&path)
                        && version.is_none_or(|version| {
                            session
                                .opened
                                .get(&path)
                                .is_none_or(|(_, current)| version >= *current)
                        })
                    {
                        self.language_response_succeeded(index, &language);
                        let project = &mut self.projects[index];
                        let session = project.languages.get_mut(&language).unwrap();
                        let origin = language_support::DiagnosticsOrigin::new(
                            version,
                            session.opened.get(&path),
                        );
                        session.diagnostic_origins.insert(path.clone(), origin);
                        session.diagnostics.insert(path.clone(), diagnostics);
                        let combined = project
                            .languages
                            .values()
                            .filter_map(|session| session.diagnostics.get(&path))
                            .flatten()
                            .cloned()
                            .collect();
                        project.diagnostics.insert(path, combined);
                    }
                }
                crate::lsp::LspEvent::Completions {
                    request_id,
                    path,
                    version,
                    position,
                    items,
                } => {
                    let session = &self.projects[index].languages[&language];
                    if session.active()
                        && self.completion_request == Some(request_id)
                        && self.active_project == index
                        && let Some(tab) = self.tab()
                        && tab.path.as_ref() == Some(&path)
                        && session
                            .opened
                            .get(&path)
                            .is_some_and(|(revision, current)| {
                                *revision == tab.revision && *current == version
                            })
                        && crate::lsp::char_to_position(&tab.text, tab.vim.cursor) == position
                    {
                        self.completions = (!items.is_empty()).then_some(CompletionPopup {
                            tab_id: tab.id,
                            version,
                            cursor: tab.vim.cursor,
                            items,
                            selected: 0,
                        });
                        self.completion_request = None;
                        self.language_response_succeeded(index, &language);
                    }
                }
                crate::lsp::LspEvent::Definition {
                    request_id,
                    path,
                    line,
                    column,
                } => {
                    if self.active_project == index
                        && self.projects[index].languages[&language].active()
                        && self.definition_request.is_some_and(|(request, tab)| {
                            request == request_id
                                && self.tab().is_some_and(|current| current.id == tab)
                        })
                    {
                        self.language_response_succeeded(index, &language);
                        self.definition_request = None;
                        self.reveal_lsp_position(
                            path,
                            crate::lsp::Position {
                                line,
                                character: column,
                            },
                        );
                    }
                }
                crate::lsp::LspEvent::DefinitionNotFound { request_id } => {
                    if self.active_project == index
                        && self.projects[index].languages[&language].active()
                        && self.definition_request.is_some_and(|(request, tab)| {
                            request == request_id
                                && self.tab().is_some_and(|current| current.id == tab)
                        })
                    {
                        self.language_response_succeeded(index, &language);
                        self.definition_request = None;
                        self.status("No definition found here", false);
                    }
                }
                crate::lsp::LspEvent::Hover {
                    request_id,
                    contents,
                } => {
                    if self.projects[index].languages[&language].active() {
                        self.language_response_succeeded(index, &language);
                        self.hover_result(request_id, contents);
                    }
                }
                crate::lsp::LspEvent::References {
                    request_id,
                    references,
                    truncated,
                } => {
                    if self.projects[index].languages[&language].active() {
                        self.language_response_succeeded(index, &language);
                        self.references_result(request_id, references, truncated);
                    }
                }
                crate::lsp::LspEvent::Symbols {
                    request_id,
                    symbols,
                } => {
                    if self.projects[index].languages[&language].active() {
                        self.language_response_succeeded(index, &language);
                        self.symbols_result(request_id, symbols);
                    }
                }
            }
        }
        self.check_language_intel_requests();
        self.apply_language_reveal();
        self.apply_pending_location();
        if self.completions.as_ref().is_some_and(|popup| {
            self.tab().is_none_or(|t| {
                t.id != popup.tab_id
                    || t.vim.cursor != popup.cursor
                    || self
                        .project()
                        .and_then(|p| {
                            t.path
                                .as_ref()
                                .and_then(|path| p.language_document_version(path))
                        })
                        .is_none_or(|(rev, v)| *rev != t.revision || *v != popup.version)
            })
        }) {
            self.completions = None;
        }
    }
    pub(super) fn request_definition(&mut self) {
        self.sync_languages(true);
        let Some(project) = self.project() else {
            return;
        };
        let Some(tab) = self.tab() else { return };
        let Some(path) = &tab.path else {
            self.language_request_unavailable();
            return;
        };
        if let Some(session) = project.language_for(path)
            && session.active()
            && let Some(server) = &session.server
        {
            let id = server.definition(
                path,
                crate::lsp::char_to_position(&tab.text, tab.vim.cursor),
            );
            if id != 0 {
                let origin = (
                    id,
                    tab.revision,
                    tab.vim.cursor,
                    self.document_widget_id(tab.id),
                );
                let tab_id = tab.id;
                self.definition_origin = Some(origin);
                self.definition_request = Some((id, tab_id));
                self.status("Finding definition…", false);
            } else {
                self.status("Language analysis is busy or unavailable. Try again or restart language analysis.",true);
            }
            return;
        }
        self.language_request_unavailable();
    }
    pub(super) fn request_completion(&mut self) {
        if self.tab().is_some_and(|tab| tab.vim.multi.placing()) && !self.finish_cursor_placement()
        {
            return;
        }
        self.sync_languages(true);
        let Some(project) = self.project() else {
            return;
        };
        let Some(tab) = self.tab() else { return };
        let Some(path) = &tab.path else {
            self.language_request_unavailable();
            return;
        };
        if let Some(session) = project.language_for(path)
            && session.active()
            && let Some(server) = &session.server
        {
            let Some((_, version)) = session.opened.get(path) else {
                self.language_request_unavailable();
                return;
            };
            let id = server.completion(
                path,
                crate::lsp::char_to_position(&tab.text, tab.vim.cursor),
                *version,
            );
            if id != 0 {
                self.completion_request = Some(id);
            } else {
                self.status("Language analysis is busy or unavailable. Try again or restart language analysis.",true);
            }
            return;
        }
        self.language_request_unavailable();
    }
    pub(super) fn apply_completion(&mut self, index: usize) {
        let Some(popup) = self.completions.take() else {
            return;
        };
        let Some(item) = popup.items.get(index).cloned() else {
            return;
        };
        let valid = self.tab().is_some_and(|t| {
            t.id == popup.tab_id
                && t.vim.cursor == popup.cursor
                && popup.cursor <= t.text.chars().count()
                && self
                    .project()
                    .and_then(|p| {
                        t.path
                            .as_ref()
                            .and_then(|path| p.language_document_version(path))
                    })
                    .is_some_and(|(revision, version)| {
                        *revision == t.revision && *version == popup.version
                    })
        });
        if !valid {
            return;
        }
        let Some(t) = self.tab_mut() else { return };
        if t.id != popup.tab_id || t.vim.cursor != popup.cursor {
            return;
        }
        let mut edits = item.additional_text_edits;
        let main = item.text_edit.unwrap_or_else(|| {
            let chars: Vec<_> = t.text.chars().collect();
            let mut start = t.vim.cursor;
            while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
                start -= 1;
            }
            crate::lsp::TextEdit {
                range: crate::lsp::TextRange {
                    start: crate::lsp::char_to_position(&t.text, start),
                    end: crate::lsp::char_to_position(&t.text, t.vim.cursor),
                },
                new_text: item.insert_text,
            }
        });
        let main_start = crate::lsp::position_to_char(&t.text, main.range.start);
        if t.vim.multi.active() {
            let result =
                multicursor_ui::complete(&t.text, &t.vim.multi, &main, &edits, t.vim.cursor);
            match result {
                Ok((text, state)) => self.commit_multi(text, state),
                Err(message) => self.status(message, true),
            }
            return;
        }
        let mut cursor = main_start + main.new_text.chars().count();
        let mut replacements: Vec<_> = edits
            .drain(..)
            .map(|edit| {
                (
                    crate::lsp::position_to_char(&t.text, edit.range.start)
                        ..crate::lsp::position_to_char(&t.text, edit.range.end),
                    edit.new_text,
                    false,
                )
            })
            .collect();
        for (range, text, _) in &replacements {
            if range.end <= main_start {
                cursor = cursor.saturating_sub(range.end - range.start) + text.chars().count();
            }
        }
        replacements.push((
            main_start..crate::lsp::position_to_char(&t.text, main.range.end),
            main.new_text,
            true,
        ));
        replacements.sort_by_key(|b| std::cmp::Reverse(b.0.start));
        t.vim.begin_external_edit(&t.text);
        for (range, text, _) in replacements {
            t.text = editor::replace_selection(&t.text, range, &text).text;
        }
        t.vim.notify_external_text_change();
        t.changed();
        t.set_cursor(cursor..cursor);
    }
    pub(super) fn completion_keys(&mut self) -> bool {
        if self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.status_details.is_some()
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.vim_search_active()
            || self.tab().is_none_or(|t| {
                !self
                    .ctx
                    .memory(|m| m.has_focus(self.document_widget_id(t.id)))
            })
        {
            self.completions = None;
            return false;
        }
        if self
            .completions
            .as_ref()
            .is_none_or(|popup| popup.items.is_empty())
        {
            self.completions = None;
            return false;
        }
        let ctx = self.ctx.clone();
        let boundary = ctx.input(|input| input.events.iter().enumerate().find_map(|(index, event)| {
            matches!(event, egui::Event::Key { key: Key::Escape | Key::ArrowUp | Key::ArrowDown | Key::Enter | Key::Tab,
                pressed: true, modifiers, .. } if !modifiers.ctrl && !modifiers.command && !modifiers.alt && !modifiers.shift).then_some(index)
        }));
        if let Some(index) = boundary {
            if self.earlier_shortcut_boundary(index) {
                return false;
            }
            if ctx.input(|input| input.events[..index].iter().any(editor_input::native_input)) {
                let suffix = ctx.input_mut(|input| input.events.split_off(index));
                if let Some(tab) = self.tab() {
                    editor_input::defer(&ctx, tab.id, suffix);
                }
                return false;
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.completions = None;
            self.defer_shortcut_tail();
            if self.settings.vim {
                return false;
            }
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            return true;
        }
        let plain_key = |key| {
            ctx.input_mut(|input| {
            let before = input.events.len();
            input.events.retain(|event| !matches!(event, egui::Event::Key {
                key: pressed_key, pressed: true, modifiers, ..
            } if *pressed_key == key && !modifiers.ctrl && !modifiers.command && !modifiers.alt && !modifiers.shift));
            before != input.events.len()
        })
        };
        let down = plain_key(Key::ArrowDown);
        let up = plain_key(Key::ArrowUp);
        if let Some(p) = &mut self.completions {
            p.selected = p.selected.min(p.items.len().saturating_sub(1));
            if down {
                p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1));
            }
            if up {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        if down || up {
            ctx.data_mut(|data| {
                data.remove::<(u64, i32, usize, usize)>(egui::Id::new("completion_selected_row"));
            });
            self.defer_shortcut_tail();
        }
        if plain_key(Key::Enter) || plain_key(Key::Tab) {
            let popup = self.completions.as_ref().unwrap();
            let selected = popup.selected;
            let identity = (popup.tab_id, popup.version, popup.cursor, selected);
            let hidden = ctx
                .data(|data| {
                    data.get_temp::<((u64, i32, usize, usize), bool)>(egui::Id::new(
                        "completion_visible_selection",
                    ))
                })
                .is_some_and(|(shown, visible)| shown == identity && !visible);
            if hidden {
                // Reveal an off-screen selection before allowing keyboard acceptance.
                ctx.data_mut(|data| {
                    data.remove::<(u64, i32, usize, usize)>(egui::Id::new(
                        "completion_selected_row",
                    ));
                });
                return true;
            }
            self.apply_completion(selected);
            self.defer_shortcut_tail();
            return true;
        }
        false
    }
    pub(super) fn completion_ui(&mut self) {
        let Some(p) = &self.completions else {
            self.ctx.data_mut(|data| {
                data.remove::<(u64, i32, usize, usize)>(egui::Id::new("completion_selected_row"));
                data.remove::<((u64, i32, usize, usize), bool)>(egui::Id::new(
                    "completion_visible_selection",
                ));
            });
            return;
        };
        let mut selected = None;
        let ctx = self.ctx.clone();
        let bounds = ctx.input(|i| i.content_rect());
        let selection = p.selected.min(p.items.len().saturating_sub(1));
        let identity = (p.tab_id, p.version, p.cursor, selection);
        let reveal_id = egui::Id::new("completion_selected_row");
        let reveal =
            ctx.data(|data| data.get_temp::<(u64, i32, usize, usize)>(reveal_id)) != Some(identity);
        ctx.data_mut(|data| data.insert_temp(reveal_id, identity));
        let caret = self.editor_cursor_rect.unwrap_or_else(|| {
            egui::Rect::from_min_size(
                egui::pos2(bounds.left() + 320.0, bounds.top() + 150.0),
                Vec2::ZERO,
            )
        });
        let row_height = design::DETAIL_ROW_HEIGHT;
        let list_height = (p.items.len().min(8) as f32 * row_height)
            .min((bounds.height() - 180.0).max(row_height));
        let popup_height = list_height + 30.0;
        let width = (bounds.width() - 32.0).clamp(200.0, 490.0);
        let top = if caret.bottom() + popup_height > bounds.bottom() - 40.0 {
            (caret.top() - popup_height).max(bounds.top() + 70.0)
        } else {
            caret.bottom() + 4.0
        };
        let position = egui::pos2(
            caret
                .left()
                .min(bounds.right() - width - 12.0)
                .max(bounds.left() + 12.0),
            top,
        );
        egui::Window::new("Suggestions")
            .id(egui::Id::new("completion"))
            .resizable(false)
            .collapsible(false)
            .title_bar(false)
            .fixed_size([width, popup_height])
            .fixed_pos(position)
            .show(&ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let scroll_id = ui.make_persistent_id("completion_results");
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("completion_results")
                    .animated(false)
                    .max_height(list_height);
                if reveal {
                    let offset = egui::scroll_area::State::load(&ctx, scroll_id)
                        .map_or(0.0, |state| state.offset.y);
                    let top = selection as f32 * row_height;
                    let bottom = top + row_height;
                    let offset = if top < offset {
                        top
                    } else if bottom > offset + list_height {
                        bottom - list_height
                    } else {
                        offset
                    };
                    scroll = scroll.vertical_scroll_offset(offset.max(0.0));
                }
                let output = scroll.show_rows(ui, row_height, p.items.len(), |ui, rows| {
                    for i in rows {
                        let item = &p.items[i];
                        let hint = format!(
                            "{}{}\nEnter / Tab to accept · Esc dismisses",
                            item.label,
                            item.detail
                                .as_ref()
                                .map(|detail| format!("\n{detail}"))
                                .unwrap_or_default()
                        );
                        let response = ui
                            .push_id(i, |ui| {
                                design::detail_row(
                                    ui,
                                    &item.label,
                                    item.detail.as_deref().unwrap_or(""),
                                    i == selection,
                                    &hint,
                                )
                            })
                            .inner;
                        if response.clicked() {
                            selected = Some(i);
                        }
                    }
                });
                let selected_top =
                    output.inner_rect.top() + selection as f32 * row_height - output.state.offset.y;
                let visible = output.inner_rect.intersect(ui.clip_rect());
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new("completion_visible_selection"),
                        (
                            identity,
                            selected_top >= visible.top() - 0.5
                                && selected_top + row_height <= visible.bottom() + 0.5,
                        ),
                    );
                });
                ui.add_space(5.0);
                ui.label(
                    RichText::new(format!("{} of {}", selection + 1, p.items.len()))
                        .size(10.5)
                        .color(MUTED),
                )
                .on_hover_text("↑ ↓ choose · Enter or Tab accepts · Esc closes");
            });
        if let Some(i) = selected {
            self.apply_completion(i);
        }
    }
}
