//! Project search requests, stale-result guards and result presentation.
use super::*;

impl VEditApp {
    pub(super) fn finish_project_search(
        &mut self,
        root: PathBuf,
        generation: u64,
        query: String,
        excludes: Vec<String>,
        result: Result<workspace::SearchResults, String>,
    ) {
        if self.project().is_some_and(|p| p.root == root)
            && generation == self.search_generation
            && query == self.project_query
            && excludes == self.settings.search_excludes
        {
            self.search_running = false;
            match result {
                Ok(results) => {
                    self.project_search_error = None;
                    self.project_search = Some(results);
                }
                Err(error) => {
                    self.project_search = None;
                    self.project_search_error = Some(error);
                }
            }
        }
    }
}

impl VEditApp {
    pub(super) fn search_project(&mut self) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_dirty = false;
        self.project_search_error = None;
        if self.project_query.trim().is_empty() {
            self.project_search = None;
            self.search_running = false;
            return;
        }
        let Some(p) = self.project().filter(|p| !p.is_other_files()) else {
            self.search_running = false;
            return;
        };
        let root = p.root.clone();
        let query = self.project_query.clone();
        let excludes = self.settings.search_excludes.clone();
        let generation = self.search_generation;
        let cancel = self.search_cancel.clone();
        self.search_running = true;
        self.worker(move || {
            let result = workspace::search_with_excludes(
                &root,
                &query,
                workspace::SearchOptions {
                    cancel: Some(cancel),
                    ..Default::default()
                },
                &excludes,
            );
            WorkerResult::Search {
                root,
                generation,
                query,
                excludes,
                result,
            }
        });
    }
    pub(super) fn invalidate_project_search(&mut self) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_running = false;
        self.search_dirty = self.active_folder().is_some() && !self.project_query.trim().is_empty();
        self.search_last_edit = Instant::now();
        self.project_search = None;
        self.project_search_error = None;
        self.ctx.request_repaint();
    }
    pub(super) fn open_project_search(&mut self) {
        if self.project().is_some_and(Project::is_other_files) {
            self.status("Open a project folder to search across files.", false);
            return;
        }
        self.intel.references = None;
        self.show_sidebar(SidebarView::Search);
        self.project_search_focus = true;
        self.cancel_vim_search(false);
        self.cancel_navigation();
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        self.ctx.request_repaint();
    }
    pub(super) fn project_search_ui(&mut self, ui: &mut egui::Ui) {
        if self.project().is_some_and(Project::is_other_files) {
            ui.label("Open a project folder to search across files.");
            return;
        }
        if self.intel.references.is_some() {
            self.references_ui(ui);
            return;
        }
        let query_id = egui::Id::new("project_search_query");
        // Closing menus retain their pointer release, and modal layers remain
        // blocked for one frame. Wait until the query can receive focus.
        if self.project_search_focus && !ui.is_sizing_pass() {
            if egui::Popup::is_any_open(ui.ctx())
                || ui.input(|input| input.pointer.any_released())
                || !ui.memory(|memory| memory.allows_interaction(ui.layer_id()))
            {
                ui.ctx().request_repaint();
            } else {
                ui.memory_mut(|memory| memory.request_focus(query_id));
                self.project_search_focus = false;
            }
        }
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.project_query)
                .id(query_id)
                .hint_text("Find text in project…")
                .char_limit(16_384)
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.invalidate_project_search();
        }
        ui.add_space(6.0);
        let exclude_count = self
            .settings
            .search_excludes
            .iter()
            .filter(|value| !value.trim().is_empty())
            .count();
        let mut filters_changed = false;
        egui::CollapsingHeader::new(format!("Exclusions · {exclude_count}"))
            .id_salt("project_search_exclusions")
            .default_open(false)
            .show(ui, |ui| {
                let label = ui.label(RichText::new("Exclude folders or patterns").size(11.0).color(MUTED));
                let mut text = self.settings.search_excludes.join("\n");
                let response = ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .id(egui::Id::new("project_search_excludes"))
                        .desired_width(f32::INFINITY)
                        .desired_rows(5)
                        .char_limit(16_384)
                        .font(FontId::monospace(11.0))
                        .hint_text("node_modules\n**/*.min.js"),
                ).labelled_by(label.id);
                if response.changed() {
                    self.settings.search_excludes = text.split('\n').map(str::to_owned).collect();
                    filters_changed = true;
                }
                ui.label(RichText::new("One per line. Folder names match at any depth; paths start at the project root. Use * or ** for globs.").size(10.5).color(MUTED));
                ui.horizontal(|ui| {
                    if ui.add_enabled(exclude_count > 0, egui::Button::new("Clear exclusions")).clicked() {
                        self.settings.search_excludes.clear();
                        filters_changed = true;
                    }
                    if ui.button("Reset exclusions").clicked() {
                        self.settings.search_excludes = workspace::default_search_excludes();
                        filters_changed = true;
                    }
                });
                ui.label(RichText::new("Saved for all projects. Git-ignored files and repository internals stay excluded.").size(10.5).color(MUTED));
            });
        if filters_changed {
            self.session_dirty = true;
            self.invalidate_project_search();
        }
        if let Some(error) = &self.project_search_error {
            ui.label(RichText::new(error).size(11.0).color(RED));
        }
        if self.search_running {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Searching…").size(11.0).color(MUTED));
            });
        }
        ui.add_space(8.0);
        let mut clicked = None;
        if let Some(results) = &self.project_search {
            ui.label(
                RichText::new(format!(
                    "{} matches · {} files searched",
                    results.matches.len(),
                    results.files_searched
                ))
                .size(11.0)
                .color(MUTED),
            );
            if results.truncated {
                ui.colored_label(ORANGE, "Search limit reached. Refine your query.");
            }
            if results.files_skipped > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} binary or large files skipped",
                        results.files_skipped
                    ))
                    .size(10.0)
                    .color(MUTED),
                );
            }
            if !results.warnings.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} files or folders could not be searched",
                        results.warnings.len()
                    ))
                    .size(10.5)
                    .color(ORANGE),
                )
                .on_hover_text(results.warnings.join("\n"));
            }
            egui::ScrollArea::vertical()
                .id_salt("search_results")
                .show(ui, |ui| {
                    let mut previous = None;
                    for m in &results.matches {
                        if previous.as_ref() != Some(&m.path) {
                            ui.add_space(9.0);
                            let path = self.project().map_or(m.path.as_path(), |project| {
                                m.path.strip_prefix(&project.root).unwrap_or(&m.path)
                            });
                            ui.label(
                                RichText::new(crate::paths::display(path))
                                    .size(12.0)
                                    .strong()
                                    .color(TEXT),
                            );
                            previous = Some(m.path.clone());
                        }
                        let response = ui
                            .push_id((&m.path, m.line, m.column), |ui| {
                                design::left_row(
                                    ui,
                                    m.preview.trim(),
                                    Some((&m.line.to_string(), MUTED)),
                                    false,
                                    &format!(
                                        "{}:{}:{}",
                                        crate::paths::display(&m.path),
                                        m.line,
                                        m.column
                                    ),
                                )
                            })
                            .inner;
                        if response.clicked() {
                            clicked = Some((m.path.clone(), m.line, m.column));
                        }
                    }
                });
        } else if !self.search_running {
            ui.add(egui::Label::new(RichText::new("Search text in this project.\nEdit Exclusions to choose which folders and files to skip.").size(12.0).color(MUTED)).wrap());
        }
        if let Some((path, line, column)) = clicked {
            self.reveal_search_result(path, line, column);
        }
    }
}
