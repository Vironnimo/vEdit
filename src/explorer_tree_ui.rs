//! Explorer reads complete folders on workers and paints immutable cached rows.
//! Quick Open keeps its independent bounded index and ignore policy.
use super::*;

impl VEditApp {
    pub(super) fn explorer_index(&self) -> Option<Arc<workspace::WorkspaceIndex>> {
        let project = self.project()?;
        project
            .explorer
            .as_ref()
            .map(workspace::ExplorerTree::snapshot)
            .or_else(|| project.index.clone())
    }

    pub(super) fn request_explorer_directory(&mut self, project: usize, path: PathBuf) {
        let Some(project) = self.projects.get_mut(project) else {
            return;
        };
        let Some(explorer) = project.explorer.as_mut() else {
            return;
        };
        if explorer.pending_count() >= 8 {
            return;
        }
        let Some(generation) = explorer.begin(&path) else {
            return;
        };
        let root = project.root.clone();
        self.worker(move || WorkerResult::ExplorerDirectory {
            result: workspace::read_explorer_directory(&root, &path),
            root,
            path,
            generation,
        });
    }

    pub(super) fn retry_explorer(&mut self) {
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return;
        };
        let Some(explorer) = project.explorer.as_mut() else {
            return;
        };
        explorer.invalidate();
        let root = project.root.clone();
        self.request_explorer_directory(self.active_project, root);
        self.ctx.request_repaint();
    }

    pub(super) fn explorer(&mut self, ui: &mut egui::Ui) {
        let Some(root) = self.project().map(|project| project.root.clone()) else {
            return;
        };
        self.request_explorer_directory(self.active_project, root.clone());
        self.explorer_register_focus(ui, &root);
        self.explorer_project_heading(ui, &root);
        let Some(index) = self.explorer_index() else {
            self.explorer_background_menu(ui, &root, ui.available_rect_before_wrap());
            return;
        };
        self.explorer_initialize(&index);
        let rows = self.explorer_rows(&index);
        let requested = &rows.requested_directories;
        for path in requested {
            self.request_explorer_directory(self.active_project, path.clone());
        }
        let (loading, root_listed, errors) = self
            .project()
            .and_then(|project| project.explorer.as_ref())
            .map(|explorer| {
                let errors: Vec<_> = explorer
                    .errors()
                    .filter(|(path, _)| {
                        requested
                            .iter()
                            .any(|requested| requested.as_path() == *path)
                    })
                    .map(|(path, error)| format!("{}: {error}", crate::paths::display(path)))
                    .collect();
                (
                    requested.iter().any(|path| explorer.is_loading(path)),
                    explorer.has_listing(&root),
                    errors,
                )
            })
            .unwrap_or((false, true, Vec::new()));
        if !errors.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if errors.len() == 1 {
                        "Folder unavailable".to_owned()
                    } else {
                        format!("{} folders unavailable", errors.len())
                    })
                    .size(10.5)
                    .color(ORANGE),
                )
                .on_hover_text(errors.join("\n"));
                if small_button(ui, "Retry", "Read these folders again").clicked() {
                    self.retry_explorer();
                }
            });
        } else if loading && !root_listed {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").size(10.5).color(MUTED));
            });
        }
        let active = self.tab().and_then(|tab| tab.path.clone());
        let scroll_row = self.explorer_take_scroll_row(&root, &rows);
        egui::ScrollArea::vertical()
            .id_salt(("tree", &root))
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let row_height = design::EXPLORER_ROW_HEIGHT;
                let content_top = ui.max_rect().top();
                let content_height = row_height * rows.len() as f32;
                ui.set_height(content_height.max(viewport.height()));
                ui.spacing_mut().item_spacing.y = 0.0;
                if let Some(row) = scroll_row {
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(ui.max_rect().left(), content_top + row as f32 * row_height),
                        Vec2::new(ui.available_width(), row_height),
                    );
                    ui.scroll_to_rect_animation(
                        rect,
                        None,
                        if self.settings.smooth_scroll {
                            ui.style().scroll_animation
                        } else {
                            egui::style::ScrollAnimation::none()
                        },
                    );
                }
                if rows.is_empty() && root_listed && errors.is_empty() && !loading {
                    ui.label(RichText::new("Empty folder").size(11.0).color(MUTED));
                }
                let first = (viewport.min.y / row_height).floor().max(0.0) as usize;
                let last = ((viewport.max.y / row_height).ceil() as usize + 1).min(rows.len());
                if first < last {
                    let rect = egui::Rect::from_x_y_ranges(
                        ui.max_rect().x_range(),
                        content_top + first as f32 * row_height
                            ..=content_top + last as f32 * row_height,
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        self.explorer_tree_rows(ui, &rows[first..last], &active);
                    });
                }
                let blank_top = content_top
                    + content_height.max(if rows.is_empty() { row_height } else { 0.0 });
                let blank = egui::Rect::from_min_max(
                    egui::pos2(ui.max_rect().left(), blank_top),
                    ui.max_rect().right_bottom(),
                );
                self.explorer_background_menu(ui, &root, blank);
            });
    }

    fn explorer_tree_rows(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[explorer_navigation::CachedVisibleRow],
        active: &Option<PathBuf>,
    ) {
        let owns_selection = self.explorer_keyboard_focused() || self.explorer_context_menu_open();
        for row in rows {
            let entry = &row.entry;
            ui.push_id(&entry.path, |ui| {
                let selected = self.explorer_selected(&entry.path);
                let active_file = !entry.is_dir && active.as_ref() == Some(&entry.path);
                let emphasized = if owns_selection {
                    selected
                } else {
                    active_file
                };
                let mut tooltip = crate::paths::display(&entry.path);
                if entry.ignored {
                    tooltip.push_str("\nIgnored by Git");
                }
                if let Some(explorer) = self.project().and_then(|project| project.explorer.as_ref())
                {
                    if explorer.is_loading(&entry.path) {
                        tooltip.push_str("\nLoading folder…");
                    } else if let Some(error) = explorer.error(&entry.path) {
                        tooltip.push_str(&format!("\n{error}"));
                    }
                }
                let response = design::explorer_row(
                    ui,
                    design::ExplorerRow {
                        label: &entry.name,
                        depth: row.depth,
                        ancestor_continuations: &row.ancestor_continuations,
                        last_sibling: row.last_sibling,
                        expanded: entry.is_dir.then(|| self.explorer_expanded(&entry.path)),
                        file_color: file_color(Some(&entry.path)),
                        // A context menu keeps the tree's target selected even
                        // while menu items temporarily own keyboard focus.
                        selected: emphasized,
                        active_file,
                        ignored: entry.ignored,
                        tooltip: &tooltip,
                    },
                );
                self.explorer_row_interaction(&response, &entry.path, entry.is_dir);
                self.file_context_menu(&response, &entry.path, entry.is_dir);
                if response.clicked() && !entry.is_dir {
                    self.open_file(entry.path.clone());
                    self.focus_explorer();
                }
            });
        }
    }
}
