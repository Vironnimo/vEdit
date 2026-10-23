//! Source text rendering and native editor interaction.
use super::*;

impl VEditApp {
    pub(super) fn text_editor(&mut self, ui: &mut egui::Ui) {
        if self.render_pane_is_focused() {
            self.resume_editor_input();
        }
        self.sync_agent_marks();
        let explorer_focused = self.explorer_keyboard_focused();
        let search_matches = self.editor_search_matches();
        let search_pane_id = self.current_pane_id();
        let search_cursor = self
            .tab()
            .map_or(0, |tab| self.find.active_cursor(tab, search_pane_id));
        let tab_pointer_interaction = self.tab_pointer_interaction_active();
        let source_interaction = ui.is_enabled()
            && !tab_pointer_interaction
            && !egui::Popup::is_any_open(&self.ctx)
            && self.palette.is_none()
            && self.picker.is_none()
            && !self.settings_open
            && self.status_details.is_none()
            && !self.agent_marks_modal_open()
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.vim_command.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && self.context_file_dialog.is_none()
            && !self.find_focus;
        let source_command_pending =
            self.navigation_pending() || self.screen_jump.is_some() || self.vim_search_active();
        let syntax = self.syntax.clone();
        let viewport_width = ui.available_width();
        let diagnostics = self
            .project()
            .and_then(|p| {
                self.tab()
                    .and_then(|t| t.path.as_ref().and_then(|path| p.diagnostics.get(path)))
            })
            .cloned()
            .unwrap_or_default();
        // With a hover provider, diagnostics join the hover popup instead of
        // a separate tooltip.
        let hover_available = self
            .tab()
            .is_some_and(|tab| self.pointer_hover_available(tab));
        let mut hover_pointer = None;
        // Paint quieter severities first so an error marker stays on top.
        let mut diagnostics = diagnostics;
        diagnostics.sort_by_key(|diagnostic| std::cmp::Reverse(diagnostic.severity));
        let settings = self.settings.clone();
        let ctx = self.ctx.clone();
        let Some(document_id) = self.tab().map(|tab| tab.id) else {
            return;
        };
        let restore_search_scroll = self
            .find
            .restore_scroll
            .remove(&(search_pane_id, document_id));
        let id = self.document_widget_id(document_id);
        self.register_region_focus_widget(id);
        let mark_overlays = self.agent_mark_overlay_rows(document_id);
        let git_markers = self.git_gutter_markers(document_id);
        let mut git_placements = Vec::new();
        let mut git_click = None;
        let mut git_peek_anchor = (0.0, 0.0);
        let marker_owns_pointer =
            source_interaction && agent_marks_ui::marker_pointer_owns_editor(&ctx, id);
        let source_interaction = source_interaction && !marker_owns_pointer;
        let pane_focused = self.render_pane_is_focused();
        if !source_interaction && !tab_pointer_interaction && !marker_owns_pointer {
            // Give external dialogs their own keyboard/IME input before the
            // source widget is rendered. Tab drags retain editor focus so
            // cancellation can return to the unchanged modal selection.
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        let mut jump = self.screen_jump.take();
        if let Some(tab) = self.tab()
            && self
                .bookmark_lines
                .get(&document_id)
                .is_none_or(|(revision, _)| *revision != tab.revision)
        {
            let marks = tab
                .path
                .as_ref()
                .map(|path| {
                    self.bookmarks
                        .for_path(path)
                        .map(|bookmark| {
                            let position = crate::bookmarks::locate(&tab.text, bookmark);
                            (
                                position.line,
                                bookmark.id,
                                if bookmark.label.is_empty() {
                                    format!("Bookmark · {}:{}", position.line, position.column)
                                } else {
                                    bookmark.label.clone()
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.bookmark_lines
                .insert(document_id, (tab.revision, marks));
        }
        let bookmark_marks = self
            .bookmark_lines
            .get(&document_id)
            .map(|(_, marks)| marks.clone())
            .unwrap_or_default();
        let mut bookmark_click = None;
        let mut agent_mark_remove = None;
        let editor_command = if pane_focused
            && source_interaction
            && !source_command_pending
            && ctx.memory(|memory| memory.has_focus(id))
            && self.tab().is_some_and(|tab| {
                !tab.loading
                    && !tab.vim.multi.active()
                    && (!settings.vim || tab.vim.mode == VimMode::Insert)
            }) {
            editor_input::take_command(&ctx, document_id, !settings.vim, &settings.keybindings)
        } else {
            None
        };
        if matches!(
            editor_command,
            Some(editor_input::Command::Undo | editor_input::Command::Redo)
        ) {
            self.context_native_undo(
                document_id,
                editor_command == Some(editor_input::Command::Redo),
            );
        }
        let Some(tab) = self.tab_mut() else { return };
        tab.sync_multicursor();
        let multi_before = tab.vim.multi.primary().unwrap_or_else(|| {
            let range = if settings.vim && tab.vim.mode != VimMode::Insert {
                tab.vim
                    .selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor)
            } else {
                tab.selection.clone()
            };
            crate::multicursor::Selection::new(range.start, range.end)
        });
        let mode = tab.vim.mode;
        let modal = settings.vim && mode != VimMode::Insert && !tab.vim.multi.active();
        let source_editable = source_interaction
            && !source_command_pending
            && !modal
            && !tab.loading
            && !tab.vim.multi.active();
        let pending = tab.pending_selection.take();
        if let Some(range) = pending {
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(range.start),
                    egui::text::CCursor::new(range.end),
                )));
            state.store(&ctx, id);
        }
        if !settings.vim && pane_focused {
            context_actions::bind_native_history(&ctx, document_id, id, &tab.text);
        }
        // Opening the context menu must retain the current selection. egui's
        // native pointer handler otherwise collapses it on any button press.
        let context_selection = ctx
            .input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary))
            .then(|| {
                egui::TextEdit::load_state(&ctx, id).and_then(|state| state.cursor.char_range())
            })
            .flatten();
        // Editing shortcuts use the widget's current character selection.
        let mut native_custom_edit = false;
        if pane_focused && source_editable && ctx.memory(|m| m.has_focus(id)) {
            let indentation = tab.indentation(&settings);
            let indent = indentation.unit();
            let edit = match editor_command {
                Some(editor_input::Command::Comment) => Some(editor::toggle_line_comment(
                    &tab.text,
                    tab.selection.clone(),
                    crate::file_kind::comment_language(tab.path.as_deref()),
                )),
                Some(editor_input::Command::Duplicate) => {
                    Some(editor::duplicate_lines(&tab.text, tab.selection.clone()))
                }
                Some(editor_input::Command::DeleteLine) => {
                    Some(editor::delete_lines(&tab.text, tab.selection.clone()))
                }
                Some(editor_input::Command::InsertLine(above)) => Some(editor::insert_line(
                    &tab.text,
                    tab.selection.clone(),
                    &indent,
                    above,
                )),
                Some(editor_input::Command::MoveLine(down)) => {
                    Some(editor::move_lines(&tab.text, tab.selection.clone(), down))
                }
                Some(editor_input::Command::Indent(outdent)) => {
                    Some(if !outdent && tab.selection.is_empty() {
                        editor::insert_tab(
                            &tab.text,
                            tab.selection.clone(),
                            indentation.width,
                            indentation.tabs,
                        )
                    } else {
                        editor::indent_lines_with_width(
                            &tab.text,
                            tab.selection.clone(),
                            &indent,
                            indentation.width,
                            outdent,
                        )
                    })
                }
                Some(editor_input::Command::Newline) => Some(editor::smart_newline(
                    &tab.text,
                    tab.selection.clone(),
                    &indent,
                )),
                _ => None,
            };
            if let Some(edit) = edit {
                native_custom_edit = !settings.vim && edit.text != tab.text;
                if native_custom_edit {
                    context_actions::native_history_checkpoint(
                        &ctx,
                        document_id,
                        id,
                        &tab.text,
                        tab.selection.clone(),
                    );
                }
                if edit.text != tab.text {
                    if matches!(
                        editor_command,
                        Some(editor_input::Command::MoveLine(_) | editor_input::Command::Duplicate)
                    ) {
                        tab.apply_line_move(edit);
                    } else {
                        tab.apply(edit);
                    }
                }
                let range = tab
                    .pending_selection
                    .take()
                    .unwrap_or_else(|| tab.selection.clone());
                let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(range.start),
                        egui::text::CCursor::new(range.end),
                    )));
                state.store(&ctx, id);
            }
        }
        let language = syntax_language(tab.path.as_deref()).to_owned();
        let font = settings.font_size;
        let gutter_digits = (tab
            .lines
            .line_count(&tab.text, tab.revision)
            .max(1)
            .ilog10()
            + 1) as f32;
        let mark_padding = if mark_overlays.is_empty() { 0.0 } else { 26.0 };
        let mut gutter_font = (font - 2.0).clamp(10.0, 20.0);
        let mut gutter_number_width = ui
            .fonts_mut(|fonts| fonts.glyph_width(&FontId::monospace(gutter_font), '8'))
            * gutter_digits;
        let maximum_number_width = 90.0 - mark_padding;
        if gutter_number_width > maximum_number_width {
            gutter_font *= maximum_number_width / gutter_number_width;
            gutter_number_width = maximum_number_width;
        }
        let gutter_marker_offset = (gutter_number_width + 27.0).max(47.0);
        let gutter_margin = (gutter_number_width + 37.0).max(56.0) + mark_padding;
        let document_id = tab.id;
        let revision = tab.revision;
        let mut focused = false;
        let mut rect = None;
        let mut caret_rect = None;
        let mut context_action = None;
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new(("document_scroll", tab.id)));
        let wheel_offset = (!settings.smooth_scroll)
            .then(|| crate::render::instant_scroll_offset(ui, scroll_id))
            .flatten();
        let selection_drag = source_interaction
            && !source_command_pending
            && pane_focused
            && !tab.loading
            && ctx.is_being_dragged(id)
            && ctx.input(|input| input.focused && input.pointer.primary_down());
        crate::selection_scroll::begin_drag(&ctx, scroll_id, selection_drag);
        let mut scroll = egui::ScrollArea::new([!settings.word_wrap, true])
            .id_salt(("document_scroll", tab.id))
            .animated(settings.smooth_scroll && !selection_drag)
            .scroll_source(egui::scroll_area::ScrollSource {
                mouse_wheel: settings.smooth_scroll,
                ..Default::default()
            })
            .auto_shrink([false, false]);
        if let Some(offset) = wheel_offset {
            scroll = scroll.scroll_offset(offset);
        }
        if let Some(offset) = restore_search_scroll {
            scroll = scroll.scroll_offset(offset);
            tab.scroll_to_cursor = false;
        }
        if selection_drag && let Some(state) = egui::scroll_area::State::load(&ctx, scroll_id) {
            scroll = scroll.scroll_offset(wheel_offset.unwrap_or(state.offset));
        }
        let scroll_output = scroll.show_viewport(ui, |ui, viewport| {
            // egui collapses native selection when another widget owns IME
            // events. Keep this document's range while Find or a toolbar has
            // focus, including a programmatic match selected before rendering.
            let unfocused_selection = (!modal && !ctx.memory(|memory| memory.owns_ime_events(id)))
                .then(|| {
                    egui::TextEdit::load_state(&ctx, id).and_then(|state| state.cursor.char_range())
                })
                .flatten();
            let indent_width = tab.indentation(&settings).width;
            // The last syntax galley handed to the editor, before native
            // selection painting recolors its glyphs.
            let laid_out = std::cell::RefCell::new(None::<std::sync::Arc<egui::Galley>>);
            let mut layouter =
                |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, available_width: f32| {
                    let width = if settings.word_wrap {
                        available_width.max(40.0)
                    } else {
                        f32::INFINITY
                    };
                    let galley = syntax.galley_with_preferences(
                        ui,
                        document_id,
                        revision,
                        buffer.as_str(),
                        &language,
                        font,
                        TEXT,
                        width,
                        settings.line_height,
                        indent_width,
                    );
                    *laid_out.borrow_mut() = Some(galley.clone());
                    galley
                };
            let native_caret = ui.visuals().text_cursor.stroke;
            let native_selection = ui.visuals().selection;
            editor_selection(ui.visuals_mut());
            if modal || tab.vim.multi.active() {
                // Modal navigation paints one block at Vim's active
                // character, rather than a second insertion caret at the
                // native half-open selection endpoint.
                ui.visuals_mut().text_cursor.stroke = Stroke::NONE;
            }
            // Immutable TextBuffer still permits native click/drag selection.
            if tab.vim.multi.active() { ui.visuals_mut().selection.bg_fill = Color32::TRANSPARENT; }
            // Mutable source is exposed only when this editor owns editing.
            let mut readonly_text: &str;
            let editor = if source_editable {
                egui::TextEdit::multiline(&mut tab.text)
            } else {
                readonly_text = tab.text.as_str();
                egui::TextEdit::multiline(&mut readonly_text)
            };
            let mut output = editor
                .id(id)
                .interactive(pane_focused && source_interaction)
                .frame(egui::Frame::new().inner_margin(Margin {
                    left: gutter_margin.ceil().min(127.0) as i8,
                    right: 24,
                    top: 10,
                    bottom: 100,
                }))
                .code_editor()
                .event_filter(egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    tab: true,
                    escape: settings.vim || tab.vim.multi.active(),
                })
                .desired_width(viewport_width)
                .min_size(egui::vec2(0.0, ui.available_height()))
                .desired_rows(1)
                .font(FontId::monospace(font))
                .text_color(TEXT)
                .background_color(BG)
                .layouter(&mut layouter)
                .show(ui);
            ui.visuals_mut().text_cursor.stroke = native_caret;
            ui.visuals_mut().selection = native_selection;
            if let Some(original) = laid_out.take()
                && !std::sync::Arc::ptr_eq(&original, &output.galley)
            {
                // Selected code keeps its syntax colors on the selection wash.
                let mut restored = output.galley.clone();
                crate::render::keep_glyph_colors(&mut restored, &original);
                if crate::render::replace_text_shape(ui, &output, restored.clone()) {
                    output.galley = restored;
                }
            }
            if tab.pending_navigation_feedback && !tab.loading && tab.pending_agent_location.is_none() {
                tab.pending_navigation_feedback = false;
                if tab.agent_location_error.is_none() && tab.error.is_none() {
                    tab.flash_navigation();
                }
            }
            if tab.focus && pane_focused && source_interaction && !explorer_focused {
                if !output.response.has_focus() {
                    output.response.request_focus();
                    ctx.memory_mut(|memory| {
                        memory.set_focus_lock_filter(
                            id,
                            egui::EventFilter {
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                tab: true,
                                escape: settings.vim || tab.vim.multi.active(),
                            },
                        )
                    });
                }
                tab.focus = false;
            }
            focused = output.response.has_focus();
            let native_selection_painted = focused
                && output.state.cursor.char_range().is_some_and(|range| !range.is_empty());
            rect = Some(output.response.rect.intersect(ui.clip_rect()));
            if let Some(range) = context_selection {
                output.state.cursor.set_char_range(Some(range));
                output.state.clone().store(&ctx, id);
            }
            // TextEditOutput.cursor_range predates pointer processing in
            // egui. The stored state contains the actual click/drag result.
            let pointer_selection = source_interaction && (output.response.is_pointer_button_down_on()
                && ui.input(|input| input.pointer.primary_down())
                || output.response.dragged_by(egui::PointerButton::Primary)
                || output.response.clicked()
                || output.response.double_clicked()
                || output.response.triple_clicked());
            crate::selection_scroll::scroll_selection(
                ui, &output.response, scroll_id, viewport.min.to_vec2(), !settings.word_wrap,
                source_interaction && pane_focused && !source_command_pending && !tab.loading,
            );
            let restored_native_selection =
                !pointer_selection && !output.response.changed() && unfocused_selection.is_some();
            if restored_native_selection && let Some(range) = unfocused_selection {
                output.state.cursor.set_char_range(Some(range));
                output.state.clone().store(&ctx, id);
            }
            multicursor_ui::pointer(ui, &mut output, tab, &settings.multicursor, multi_before, pane_focused && source_interaction && !source_command_pending && !tab.loading);
            if let Some(range) = output.state.cursor.char_range().filter(|_| !tab.vim.multi.active()) {
                tab.selection =
                    range.as_sorted_char_range().start.0..range.as_sorted_char_range().end.0;
                if settings.vim && pointer_selection {
                    tab.vim.set_mouse_selection(
                        &tab.text,
                        range.primary.index.0,
                        range.secondary.index.0,
                        output.response.triple_clicked()
                            || mode == VimMode::VisualLine && ui.input(|i| i.modifiers.shift),
                    );
                } else if source_interaction && !modal && tab.vim.cursor != range.primary.index.0 {
                    tab.vim.set_cursor(&tab.text, range.primary.index.0);
                    if !settings.vim {
                        // A non-modal caret is an insertion boundary,
                        // including one past the last character of a line.
                        tab.vim.cursor = range.primary.index.0;
                    }
                }
            }
            if output.response.changed() {
                tab.vim.notify_external_text_change();
                tab.changed();
            }
            if modal && !pointer_selection && !tab.vim.multi.active() {
                // A menu owns focus while egui may collapse its native range.
                // Build its actions from the actual Vim area, not that
                // temporary widget range; keep live drag anchors untouched.
                tab.selection = tab.vim.selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            }
            let context_menu_was_open = output.response.context_menu_opened();
            context_action = context_actions::editor_menu(
                ui,
                &output.response,
                !tab.loading,
                !tab.selection.is_empty() || tab.vim.multi.active(),
                &settings,
            );
            let retained_selection_painted = !native_selection_painted
                && (context_selection.is_some()
                    || context_menu_was_open
                    || output.response.context_menu_opened())
                && crate::render::paint_retained_text_selection(
                    ui,
                    &{
                        let mut visuals = ui.visuals().clone();
                        editor_selection(&mut visuals);
                        visuals
                    },
                    &output,
                    tab.selection.clone(),
                );
            let mut cursor = egui::text::CCursor::new(tab.vim.cursor);
            if let Some(primary) = tab.vim.multi.primary() { cursor.prefer_next_row = primary.prefer_next_row; }
            multicursor_ui::paint(ui, &output, &tab.vim.multi, focused);
            let local = output.galley.pos_from_cursor(cursor);
            let cursor_rect = local.translate(output.galley_pos.to_vec2());
            caret_rect = Some(cursor_rect);
            if settings.highlight_current_line && pane_focused {
                let highlight = egui::Rect::from_x_y_ranges(
                    output.response.rect.x_range(), cursor_rect.y_range(),
                ).intersect(ui.clip_rect());
                ui.painter().rect_filled(highlight, 0.0, ACCENT.linear_multiply(0.055));
            }
            if pane_focused && let Some((bracket, partner)) = tab.bracket_pair(settings.vim) {
                for index in [bracket, partner] {
                    let start = output.galley.pos_from_cursor(egui::text::CCursor::new(index));
                    let end = output.galley.pos_from_cursor(egui::text::CCursor::new(index + 1));
                    let right = if (end.min.y - start.min.y).abs() < 0.5 { end.min.x } else { start.min.x + start.height() * 0.5 };
                    let rect = egui::Rect::from_min_max(start.min, egui::pos2(right, start.max.y));
                    design::paint_bracket_match(ui.painter(), rect.translate(output.galley_pos.to_vec2()));
                }
            }
            // Cursor reveals keep this margin to the viewport edge, like Vim's
            // 'scrolloff'; zt / zb and H / L respect the same margin.
            const REVEAL_MARGIN: f32 = 35.0;
            if tab.scroll_to_cursor {
                let align = match tab.scroll_align.take() {
                    Some(crate::vim::ScrollAlign::Top) => Some(Align::Min),
                    Some(crate::vim::ScrollAlign::Center) => Some(Align::Center),
                    Some(crate::vim::ScrollAlign::Bottom) => Some(Align::Max),
                    None => tab.center_on_reveal.then_some(Align::Center),
                };
                ui.scroll_to_rect(cursor_rect.expand(REVEAL_MARGIN), align);
                tab.scroll_to_cursor = false;
                tab.center_on_reveal = false;
            }
            let clip = ui.clip_rect();
            let rows = &output.galley.rows;
            let first =
                rows.partition_point(|r| output.galley_pos.y + r.pos.y + r.size.y < clip.top());
            let last = rows.partition_point(|r| output.galley_pos.y + r.pos.y < clip.bottom());
            let visible_rows = syntax.visible_rows(document_id, &output.galley, first..last);
            if pane_focused {
                // Vim H / M / L address the visible rows of this pane outside
                // the reveal margin, unless the document starts or ends there.
                let top = |row: usize| rows.get(row).map(|r| output.galley_pos.y + r.pos.y);
                let starts = top(0).is_some_and(|y| y >= clip.top() - 0.5);
                let ends = rows.last().is_some_and(|r| {
                    output.galley_pos.y + r.pos.y + r.size.y <= clip.bottom() + 0.5
                });
                let upper = clip.top() + if starts { 0.0 } else { REVEAL_MARGIN };
                let lower = clip.bottom() - if ends { 0.0 } else { REVEAL_MARGIN };
                let fully_visible = |(row, _): &&(usize, crate::render::VisualRow)| {
                    rows.get(*row).is_some_and(|r| {
                        output.galley_pos.y + r.pos.y >= upper - 0.5
                            && output.galley_pos.y + r.pos.y + r.size.y <= lower + 0.5
                    })
                };
                let shown = visible_rows.iter().find(fully_visible).zip(visible_rows.iter().rev().find(fully_visible));
                tab.vim.set_visible_range(shown.map(|((_, top), (_, bottom))| top.start..bottom.end));
            }
            find::paint_matches(ui, &output.galley, output.galley_pos, &visible_rows, &search_matches, search_cursor);
            agent_mark_remove = agent_marks_ui::paint_agent_mark_overlays(
                ui, id, &output.galley, output.galley_pos, &visible_rows,
                if tab.revision == revision { &mark_overlays } else { &[] },
            );
            if pane_focused && let Some(feedback) = &tab.feedback {
                feedback.paint(ui, &output.galley, output.galley_pos, &visible_rows, tab.revision);
            }
            if let Some(markers) = &git_markers {
                let line_count = tab.lines.line_count(&tab.text, tab.revision);
                git_placements = git_gutter::paint_markers(
                    ui, markers, &output.galley, output.galley_pos, &visible_rows, line_count,
                );
                git_peek_anchor = (output.galley_pos.x, clip.width());
                for placement in &git_placements {
                    let hunk = &markers.hunks[placement.index];
                    let target = placement.rect.expand2(Vec2::new(3.0, 0.0)).intersect(clip);
                    let response = ui
                        .interact(target, id.with(("git_change", placement.index)), egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_details_ui(|ui| git_gutter::tooltip_ui(ui, markers, hunk));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Git change")
                    });
                    if response.clicked() {
                        git_click = Some(hunk.clone());
                    }
                }
            }
            if pane_focused && let Some(jump) = jump.as_mut()
                && jump.tab_id == tab.id && jump.revision == tab.revision
            {
                jump.glyphs.clear();
                for &(row_index, mapped) in &visible_rows {
                    let row = &rows[row_index];
                    for (offset, glyph) in row.glyphs.iter().enumerate() {
                        let min = output.galley_pos + row.pos.to_vec2() + egui::vec2(glyph.pos.x, 0.0);
                        let rect = egui::Rect::from_min_size(min, egui::vec2(glyph.advance_width.max(1.0), row.size.y));
                        if clip.contains_rect(rect) {
                            jump.glyphs.push(crate::screen_jump::VisibleGlyph { char_index: mapped.start + offset, character: glyph.chr, rect });
                        }
                    }
                }
                jump.state.recompute(&jump.glyphs);
            }
            if !settings.vim && !retained_selection_painted
                && (!focused || restored_native_selection) && !tab.selection.is_empty()
            {
                // Keep search matches and another pane's selection visible
                // while keyboard focus belongs to Find or a different pane.
                for (row, mapped) in &visible_rows {
                    if let Some(selected) = crate::render::range_on_visual_row(
                        &output.galley,
                        *row,
                        *mapped,
                        tab.selection.start,
                        tab.selection.end,
                    ) {
                        ui.painter().rect_filled(
                            selected.translate(output.galley_pos.to_vec2()),
                            0.0,
                            EDITOR_SELECTION.linear_multiply(0.6),
                        );
                    }
                }
            }
            let current_line = tab
                .lines
                .line_col(&tab.text, tab.revision, tab.vim.cursor)
                .0;
            for annotation in &tab.annotations {
                if tab
                    .annotations_revision
                    .is_some_and(|revision| revision != tab.revision)
                {
                    continue;
                }
                let start_column = if annotation.end.is_none() {
                    1
                } else {
                    annotation.start.column
                };
                let Some(start) = tab.lines.position(
                    &tab.text,
                    tab.revision,
                    annotation.start.line,
                    start_column,
                ) else {
                    continue;
                };
                let end = match &annotation.end {
                    Some(end) => tab
                        .lines
                        .position(&tab.text, tab.revision, end.line, end.column)
                        .and_then(|at| {
                            tab.lines
                                .line_end(&tab.text, tab.revision, end.line)
                                .map(|line_end| at.saturating_add(1).min(line_end))
                        }),
                    None => tab
                        .lines
                        .line_end(&tab.text, tab.revision, annotation.start.line),
                };
                let Some(end) = end else { continue };
                let color = annotation_color(annotation.severity);
                for &(row_index, mapped) in &visible_rows {
                    let Some(rect) = crate::render::range_on_visual_row(
                        &output.galley,
                        row_index,
                        mapped,
                        start,
                        end,
                    ) else {
                        continue;
                    };
                    let rect = rect.translate(output.galley_pos.to_vec2()).intersect(clip);
                    ui.painter().rect_filled(
                        rect,
                        2.0,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 25),
                    );
                    ui.painter().line_segment(
                        [
                            egui::pos2(output.galley_pos.x - gutter_marker_offset - 3.0, rect.top() + 2.0),
                            egui::pos2(output.galley_pos.x - gutter_marker_offset - 3.0, rect.bottom() - 2.0),
                        ],
                        Stroke::new(2.0, color),
                    );
                    ui.interact(
                        rect,
                        ui.id()
                            .with(("review_mark", tab.id, &annotation.id, row_index)),
                        egui::Sense::hover(),
                    )
                    .on_hover_details(&annotation.note);
                }
            }
            for diagnostic in &diagnostics {
                let start = crate::lsp::position_to_char(&tab.text, diagnostic.range.start);
                let end =
                    crate::lsp::position_to_char(&tab.text, diagnostic.range.end).max(start + 1);
                let start_row = output
                    .galley
                    .pos_from_cursor(egui::text::CCursor::new(start));
                for &(row_index, mapped) in &visible_rows {
                    if let Some(segment) = crate::render::range_on_visual_row(
                        &output.galley,
                        row_index,
                        mapped,
                        start,
                        end,
                    ) {
                        let severity = problems::Severity::from_lsp(diagnostic.severity);
                        let color = severity.color();
                        // Hints stay a faint underline without a gutter dot.
                        if severity != problems::Severity::Hint
                            && segment.top() == start_row.top()
                        {
                            ui.painter().circle_filled(
                                egui::pos2(
                                    output.galley_pos.x - gutter_marker_offset + 6.0,
                                    output.galley_pos.y + segment.center().y,
                                ),
                                2.8,
                                color,
                            );
                        }
                        ui.painter().line_segment(
                            [
                                output.galley_pos + segment.left_bottom().to_vec2()
                                    - Vec2::new(0.0, 1.0),
                                output.galley_pos + segment.right_bottom().to_vec2()
                                    - Vec2::new(0.0, 1.0),
                            ],
                            Stroke::new(
                                1.0,
                                if severity == problems::Severity::Hint {
                                    color.gamma_multiply(0.6)
                                } else {
                                    color
                                },
                            ),
                        );
                        if !hover_available {
                            ui.interact(
                                segment.translate(output.galley_pos.to_vec2()),
                                ui.id().with(("diagnostic", tab.id, start, row_index)),
                                egui::Sense::hover(),
                            )
                            .on_hover_details(format!("{}: {}", severity.label(), diagnostic.message));
                        }
                    }
                }
            }
            if hover_available
                && source_interaction
                && output.response.contains_pointer()
                && !ctx.input(|input| input.pointer.any_down())
                && let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
                && clip.contains(pointer)
                && let Some((index, word, rect)) = language_intel::pointer_word(
                    &output.galley,
                    &visible_rows,
                    pointer - output.galley_pos.to_vec2(),
                )
            {
                hover_pointer = Some(language_intel::PointerTarget {
                    tab_id: tab.id,
                    pane_id: search_pane_id,
                    revision: tab.revision,
                    index,
                    word,
                    rect: rect.translate(output.galley_pos.to_vec2()),
                    pointer,
                });
            }
            for &(i, mapped) in &visible_rows {
                if mapped.first_in_line {
                    let row = &rows[i];
                    let y = output.galley_pos.y + row.pos.y;
                    for &(line, id, ref label) in &bookmark_marks {
                        if line != mapped.line { continue; }
                        let center = egui::pos2(output.galley_pos.x - gutter_marker_offset, y + row.size.y * 0.5);
                        let rect = egui::Rect::from_center_size(center, egui::vec2(11.0, 17.0)).intersect(clip);
                        ui.painter().add(egui::Shape::convex_polygon(vec![
                            center + Vec2::new(-3.5, -5.0), center + Vec2::new(3.5, -5.0),
                            center + Vec2::new(3.5, 5.0), center + Vec2::new(0.0, 2.5), center + Vec2::new(-3.5, 5.0),
                        ], ORANGE, Stroke::NONE));
                        let response = ui.interact(rect, ui.id().with(("bookmark_gutter", id)), egui::Sense::click()).on_hover_details(format!("{label}\nClick to jump to this bookmark · Space m toggles a bookmark"));
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Bookmark at line {line}")));
                        if response.clicked() { bookmark_click = Some(id); }
                    }
                    if settings.line_numbers == crate::cli::LineNumbers::Off { continue; }
                    let color = if mapped.line == current_line {
                        TEXT
                    } else {
                        MUTED.linear_multiply(0.7)
                    };
                    let number = ui.painter().layout_no_wrap(
                        if settings.line_numbers == crate::cli::LineNumbers::Relative && mapped.line != current_line {
                            mapped.line.abs_diff(current_line).to_string()
                        } else {
                            mapped.line.to_string()
                        },
                        FontId::monospace(gutter_font),
                        color,
                    );
                    let source_baseline = row.glyphs.first().map(|glyph| glyph.pos.y)
                        .unwrap_or_else(|| {
                            // Use the same line box as source; natural font ascent
                            // alone loses the pixel-rounded leading on empty rows.
                            let mut job = egui::text::LayoutJob::simple("0".into(), FontId::monospace(font), color, f32::INFINITY);
                            job.sections[0].format.line_height = Some(font * settings.line_height);
                            let probe = ui.painter().layout_job(job);
                            probe.rows[0].glyphs[0].pos.y
                        });
                    let number_baseline = number.rows[0].glyphs[0].pos.y;
                    ui.painter().galley(
                        egui::pos2(output.galley_pos.x - 17.0 - number.size().x, y + source_baseline - number_baseline),
                        number, color,
                    );
                }
            }
            ui.painter().line_segment(
                [
                    egui::pos2(output.galley_pos.x - 8.0, clip.top()),
                    egui::pos2(output.galley_pos.x - 8.0, clip.bottom()),
                ],
                Stroke::new(1.0, BORDER),
            );
            if modal && focused && !tab.vim.multi.active() {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        cursor_rect.min,
                        Vec2::new(font * 0.6, cursor_rect.height().max(font)),
                    ),
                    1.0,
                    ACCENT.linear_multiply(0.65),
                );
            }
            if modal && !tab.vim.multi.active() {
                let range = tab
                    .vim
                    .selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor);
                tab.selection = range.clone();
                // During a drag egui owns its original pointer anchor.
                // Replacing it with a sorted range makes reverse drags jump.
                if !pointer_selection {
                    output
                        .state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(range.start),
                            egui::text::CCursor::new(range.end),
                        )));
                }
                output.state.store(&ctx, id);
            } else if !settings.vim && pane_focused {
                if output.response.changed() {
                    multicursor_ui::record_native(&ctx, document_id, &tab.text, output.state.cursor.char_range().unwrap_or_default(), &tab.vim.multi);
                }
                context_actions::publish_native_history(
                    &ctx,
                    document_id,
                    &output.state,
                    output.response.changed() || native_custom_edit,
                );
            }
            if pane_focused && let Some(jump) = &jump && jump.tab_id == tab.id {
                jump.state.paint_with_caret(ui, clip, Some(cursor_rect));
            }
        });
        crate::render::remember_scroll_limits(&ctx, &scroll_output);
        self.find.view_scroll.insert(
            (search_pane_id, document_id),
            (scroll_output.id, scroll_output.state.offset),
        );
        self.editor_focused = focused;
        self.editor_rect = rect;
        self.editor_cursor_rect = caret_rect;
        self.intel.interactive |= source_interaction;
        if hover_pointer.is_some() {
            self.intel.pointer = hover_pointer;
        }
        if pane_focused {
            self.intel.caret = caret_rect;
        }
        self.screen_jump = jump;
        if let Some(mark_id) = agent_mark_remove {
            self.agent_marks_remove(mark_id);
            if let Some(tab) = self.tab_mut() {
                tab.focus = true;
            }
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
        if let Some(id) = bookmark_click {
            self.jump_to_bookmark(id);
        }
        if let (Some(hunk), Some(markers)) = (git_click, &git_markers) {
            let peek = git_gutter::GutterPeek {
                pane: search_pane_id,
                document: document_id,
                revision: markers.revision,
                hunk,
            };
            self.git_gutter.peek = (self.git_gutter.peek.as_ref() != Some(&peek)).then_some(peek);
        }
        self.git_peek_ui(
            document_id,
            git_markers.as_ref(),
            &git_placements,
            git_peek_anchor.0,
            git_peek_anchor.1 - 40.0,
        );
        if let Some(action) = context_action {
            self.apply_editor_context_action(document_id, action);
        }
        self.finish_editor_input(document_id);
    }
}
