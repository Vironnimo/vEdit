//! Command and quick-open presentation with typed action selection.
use super::*;

impl VEditApp {
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if let Some(palette) = self.palette {
            let mut close = false;
            let mut choose = None;
            let mut line = None;
            let mut retry_index = false;
            let modal = egui::Modal::new(egui::Id::new("palette"))
                .area(egui::Modal::default_area(egui::Id::new("palette"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 125.0]))
                .backdrop_color(Color32::from_black_alpha(30))
                .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 70.0).clamp(280.0, 590.0));
                let navigate = palette != Palette::Line;
                let down = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                let up = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                let first = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Home));
                let last = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::End));
                let page_down = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::PageDown));
                let page_up = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::PageUp));
                let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                let symbol_title = (palette == Palette::Files)
                    .then(|| language_intel::symbol_scope(&self.palette_query))
                    .flatten()
                    .map(|(scope, _)| match scope {
                        language_intel::SymbolScope::Document => "Go to symbol",
                        language_intel::SymbolScope::Workspace => "Go to symbol in workspace",
                    });
                design::overlay_title(ui, match palette {
                    Palette::Files if symbol_title.is_some() => symbol_title.unwrap(),
                    Palette::Files => "Quick open",
                    Palette::Recent => "Open recent",
                    Palette::Commands => "Commands",
                    Palette::Line => "Go to line",
                });
                ui.add_space(6.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .id(egui::Id::new("palette_input"))
                        .hint_text(match palette {
                            Palette::Files => "Search by file name or path…",
                            Palette::Recent => "Search recent files and folders…",
                            Palette::Commands => "What would you like to do?",
                            Palette::Line => "Line or line:column",
                        })
                        .desired_width(f32::INFINITY)
                        .font(FontId::proportional(15.0)),
                );
                if self.palette_focus && !ui.is_sizing_pass() {
                    response.request_focus();
                    self.palette_focus = false;
                }
                let query_changed = response.changed();
                if query_changed {
                    self.palette_selected = 0;
                }
                ui.add_space(6.0);
                if palette == Palette::Line {
                    let query = self.palette_query.trim();
                    let (row, column) = query.split_once(':').unwrap_or((query, "1"));
                    let location = row.trim().parse::<usize>().ok()
                        .zip(column.trim().parse::<usize>().ok())
                        .filter(|(row, column)| *row > 0 && *column > 0);
                    if enter {
                        line = location;
                    }
                    if !query.is_empty() && location.is_none() {
                        ui.label(RichText::new("Enter a line or line:column using positive numbers.").color(ORANGE));
                    }
                } else {
                    let query = self.palette_query.clone();
                    let symbols = (palette == Palette::Files)
                        .then(|| language_intel::symbol_scope(&query))
                        .flatten()
                        .map(|(scope, query)| self.symbol_palette(scope, query));
                    let options: Vec<(language_intel::PaletteRow, Option<u64>, Option<keybindings::Action>)> = if let Some(symbols) = &symbols {
                        symbols.rows.iter().cloned().map(|row| (row, None, None)).collect()
                    } else if palette == Palette::Commands
                    {
                        keybindings::Action::ALL
                            .iter().copied()
                            .filter(|action| action.palette_visible())
                            .filter(|action| action.label().to_lowercase().contains(&self.palette_query.to_lowercase()))
                            .map(|action| ((action.label().to_owned(), self.shortcut_label(action), None, None), None, Some(action)))
                            .collect()
                    } else if palette == Palette::Recent {
                        self.matching_recent(&query).into_iter().map(|entry| {
                            let kind = if entry.kind == recent::Kind::Folder { "Folder" } else { "File" };
                            ((format!("{kind} · {}", file_title(&entry.path)), crate::paths::display(&entry.path), Some(entry.path), None), None, None)
                        }).collect()
                    } else {
                        self.quick_open_rows().into_iter().map(|(row, tab)| (row, tab, None)).collect()
                    };
                    let index_truncated = palette == Palette::Files && symbols.is_none()
                        && self.project().and_then(|project| project.index.as_ref()).is_some_and(|index| index.truncated);
                    let limit_note_height = if index_truncated { 30.0 } else { 0.0 };
                    let results_height = (ctx.content_rect().height() - 285.0 - limit_note_height).clamp(96.0, 340.0);
                    let page_size =
                        (results_height / design::DETAIL_ROW_HEIGHT).floor().max(1.0) as usize;
                    if down && !options.is_empty() {
                        self.palette_selected = (self.palette_selected + 1).min(options.len() - 1);
                    }
                    if up {
                        self.palette_selected = self.palette_selected.saturating_sub(1);
                    }
                    if first {
                        self.palette_selected = 0;
                    }
                    if last {
                        self.palette_selected = options.len().saturating_sub(1);
                    }
                    if page_down {
                        self.palette_selected = self.palette_selected.saturating_add(page_size);
                    }
                    if page_up {
                        self.palette_selected = self.palette_selected.saturating_sub(page_size);
                    }
                    self.palette_selected =
                        self.palette_selected.min(options.len().saturating_sub(1));
                    ui.scope(|ui| {
                    // Reserve a solid scrollbar column, including before hover. The
                    // track also makes keyboard navigation's scroll position visible.
                    ui.spacing_mut().scroll.floating = false;
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::vertical()
                        .id_salt("palette_results")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                        .animated(false)
                        .max_height(results_height)
                        // Results can arrive after the palette opened (language
                        // symbols); claim their height so the modal grows with them.
                        .min_scrolled_height(
                            (options.len() as f32 * design::DETAIL_ROW_HEIGHT).min(results_height),
                        )
                        .show(ui, |ui| {
                            for (i, ((name, detail, path, position), tab_id, action)) in options.iter().enumerate() {
                                let response = ui
                                    .push_id(i, |ui| {
                                        design::detail_row(
                                            ui,
                                            name,
                                            detail,
                                            i == self.palette_selected,
                                            detail,
                                        )
                                    })
                                    .inner;
                                if response.clicked() || enter && i == self.palette_selected {
                                    choose = Some((path.clone(), *position, *tab_id, *action));
                                }
                                if (down || up || first || last || page_down || page_up || query_changed)
                                    && i == self.palette_selected {
                                    response.scroll_to_me(None);
                                }
                            }
                            if let Some(symbols) = &symbols {
                                if symbols.loading {
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(RichText::new("Loading symbols…").color(MUTED));
                                    });
                                } else if let Some(message) = &symbols.message {
                                    ui.add_space(10.0);
                                    ui.label(RichText::new(message).color(MUTED));
                                }
                            } else if options.is_empty() || palette == Palette::Files
                                && self.project().is_some_and(|project| project.indexing || project.index_error.is_some()) {
                                ui.add_space(10.0);
                                if palette == Palette::Files {
                                    match self.project() {
                                        Some(project) if project.indexing => {
                                            ui.horizontal(|ui| {
                                                ui.spinner();
                                                ui.label(
                                                    RichText::new("Indexing files…").color(MUTED),
                                                );
                                            });
                                        }
                                        Some(project) if project.index_error.is_some() => {
                                            ui.label(
                                                RichText::new(
                                                    "Could not load this project's files.",
                                                )
                                                .color(ORANGE),
                                            );
                                            if let Some(error) = &project.index_error {
                                                ui.label(
                                                    RichText::new(error).size(11.0).color(MUTED),
                                                );
                                            }
                                            retry_index = ui.button("Retry indexing").clicked();
                                        }
                                        Some(_) => {
                                            ui.label(
                                                RichText::new(
                                                    if index_truncated {
                                                        "No matches in the indexed files"
                                                    } else if self.palette_query.trim().is_empty() {
                                                        "No files found in this project"
                                                    } else {
                                                        "No matches"
                                                    },
                                                )
                                                .color(MUTED),
                                            );
                                        }
                                        None => {
                                            ui.label(
                                                RichText::new("Open a project to find its files.")
                                                    .color(MUTED),
                                            );
                                        }
                                    }
                                } else if palette == Palette::Recent && self.recent.is_empty() {
                                    ui.label(RichText::new("No recent files or folders").color(MUTED));
                                } else {
                                    ui.label(RichText::new("No matches").color(MUTED));
                                }
                            }
                        });
                    });
                    if index_truncated {
                        ui.label(
                            RichText::new("Index limit reached. Some files may be missing.")
                                .size(11.0)
                                .color(ORANGE),
                        ).on_hover_text(format!("The index reached its file, depth, or time limit. Open a narrower project folder, or close Quick Open and use Open file ({}) to open directly.", self.shortcut_label(keybindings::Action::OpenFile)));
                    }
                    if palette == Palette::Recent {
                        ui.add_space(8.0);
                        if ui.add_enabled(!self.recent.is_empty(), egui::Button::new("Clear recent history"))
                            .on_hover_text("Clear this list; open tabs and files on disk are kept.")
                            .clicked() {
                            self.clear_recent();
                            self.palette_selected = 0;
                            choose = None;
                        }
                    }
                }
            });
            close |= modal.should_close()
                || modal
                    .backdrop_response
                    .clicked_by(egui::PointerButton::Secondary)
                || modal
                    .backdrop_response
                    .clicked_by(egui::PointerButton::Middle);
            if retry_index {
                self.index_project(self.active_project);
            }
            if close || choose.is_some() || line.is_some() {
                self.close_palette();
            }
            if let Some((path, position, tab_id, action)) = choose {
                if palette == Palette::Recent {
                    if let Some(entry) = self
                        .recent
                        .iter()
                        .find(|entry| Some(&entry.path) == path.as_ref())
                        .cloned()
                    {
                        self.open_recent(entry);
                    }
                } else if let Some((project, tab)) = tab_id.and_then(|id| self.tab_location(id)) {
                    self.activate_tab(project, tab);
                } else if let (Some(path), Some(position)) = (path.clone(), position) {
                    self.open_language_location(path, position);
                } else if let Some(path) = path {
                    self.open_file(path);
                } else if let Some(action) = action {
                    self.run_shortcut_action(action);
                }
            }
            if let Some((line, column)) = line
                && let Some(t) = self.tab_mut()
            {
                let at = editor::go_to_line(&t.text, line);
                let (length, last) = t
                    .text
                    .chars()
                    .skip(at)
                    .take_while(|ch| *ch != '\n')
                    .fold((0, None), |(length, _), ch| (length + 1, Some(ch)));
                let length = length - usize::from(last == Some('\r'));
                let at = at + (column - 1).min(length);
                t.set_cursor(at..at);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Palette {
    Files,
    Recent,
    Commands,
    Line,
}

impl VEditApp {
    pub(super) fn open_palette(&mut self, palette: Palette) {
        self.find.break_escape_sequence();
        self.completions = None;
        self.palette = Some(palette);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_focus = true;
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        self.ctx.request_repaint();
    }
    fn close_palette(&mut self) {
        self.palette = None;
        self.palette_focus = false;
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
            && !tab.loading
            && tab.error.is_none()
        {
            let id = self.document_widget_id(tab.id);
            if self.ctx.read_response(id).is_some() {
                self.ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
        self.ctx.request_repaint();
    }
    fn quick_open_rows(&self) -> Vec<(language_intel::PaletteRow, Option<u64>)> {
        let Some(project) = self.project() else {
            return Vec::new();
        };
        let query = self.palette_query.trim().replace('\\', "/").to_lowercase();
        let score = |name: &str, detail: &str| {
            if query.is_empty() {
                Some(0)
            } else {
                workspace::path_score(&detail.replace('\\', "/").to_lowercase(), &query)
                    .or_else(|| workspace::path_score(&name.to_lowercase(), &query))
            }
        };
        // Open buffers participate even when their files are ignored, the index
        // is still loading, or this is the folderless Other files area.
        let mut rows = Vec::new();
        for tab in &project.tabs {
            let detail = tab.path.as_ref().map_or_else(
                || "Unsaved document".to_owned(),
                |path| crate::paths::display(path.strip_prefix(&project.root).unwrap_or(path)),
            );
            if let Some(rank) = score(&tab.title, &detail) {
                rows.push((
                    rank,
                    (
                        (tab.title.clone(), detail, tab.path.clone(), None),
                        Some(tab.id),
                    ),
                ));
            }
        }
        if let Some(index) = &project.index {
            for path in index.quick_open(&self.palette_query, 40) {
                if project.tabs.iter().any(|tab| {
                    tab.path
                        .as_deref()
                        .is_some_and(|open| control::same_workspace(open, &path))
                }) {
                    continue;
                }
                let name = file_title(&path);
                let detail =
                    crate::paths::display(path.strip_prefix(&project.root).unwrap_or(&path));
                if let Some(rank) = score(&name, &detail) {
                    rows.push((rank, ((name, detail, Some(path), None), None)));
                }
            }
        }
        // Stable ties retain open-tab order, then the index's existing order.
        rows.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        rows.into_iter().take(40).map(|(_, row)| row).collect()
    }
}
