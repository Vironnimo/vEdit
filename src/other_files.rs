//! A folderless workspace reuses the editor layout and document lifecycle.
use super::*;

// Layouts historically use project roots as stable keys. This key is not a
// filesystem location (NUL is invalid in paths). Folder services
// use `folder_root`/`is_other_files`, and public paths are null for this workspace.
const OTHER_FILES_KEY: &str = "\0vedit-other-files";

pub(super) fn layout_key() -> PathBuf {
    PathBuf::from(OTHER_FILES_KEY)
}

pub(super) struct DroppedPaths {
    target: Option<(u64, u64)>,
    entries: Vec<Result<(PathBuf, bool), String>>,
}

#[cfg(test)]
#[path = "other_files_tests.rs"]
mod tests;

impl Project {
    pub(super) fn other_files() -> Self {
        let mut workspace = Self::new(layout_key());
        workspace.kind = WorkspaceKind::OtherFiles;
        workspace.languages.clear();
        workspace
    }
    pub(super) fn is_other_files(&self) -> bool {
        self.kind == WorkspaceKind::OtherFiles
    }

    pub(super) fn folder_root(&self) -> Option<&Path> {
        (!self.is_other_files()).then_some(self.root.as_path())
    }

    pub(super) fn display_name(&self) -> String {
        if self.is_other_files() {
            "Other files".into()
        } else {
            file_title(&self.root)
        }
    }

    pub(super) fn display_location(&self) -> String {
        self.folder_root()
            .map(crate::paths::display)
            .unwrap_or_else(|| "Files outside open projects".into())
    }
}

impl VEditApp {
    fn drop_target(&self) -> Option<(u64, egui::Rect)> {
        let position = self.ctx.input(|input| input.pointer.hover_pos())?;
        self.tab_drag_pane_rects()
            .into_iter()
            .find(|(_, rect)| rect.contains(position))
    }

    pub(super) fn receive_file_drop(&mut self) {
        let dropped = self
            .ctx
            .input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        if dropped.is_empty() {
            return;
        }
        if self.chrome_blocked() {
            self.status("Finish the current dialog before dropping files.", true);
            return;
        }
        let pane = self
            .drop_target()
            .map(|(pane, _)| pane)
            .unwrap_or_else(|| self.current_pane_id());
        // Selecting the target first also completes the previous pane's edit.
        let _ = self.focus_editor_pane(pane);
        let target = self.project().map(|p| (p.instance_id, pane));
        self.worker(move || {
            let entries = dropped
                .into_iter()
                .map(|file| {
                    let path = file.path();
                    let path = fs::canonicalize(path)
                        .map_err(|error| format!("{}: {error}", crate::paths::display(path)))?;
                    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
                    if metadata.is_file() || metadata.is_dir() {
                        Ok((path, metadata.is_dir()))
                    } else {
                        Err(format!(
                            "{} is not a file or folder",
                            crate::paths::display(&path)
                        ))
                    }
                })
                .collect();
            WorkerResult::DroppedPaths(DroppedPaths { target, entries })
        });
    }

    pub(super) fn finish_dropped_paths(&mut self, batch: DroppedPaths) {
        if batch
            .target
            .as_ref()
            .is_some_and(|(id, _)| !self.projects.iter().any(|p| &p.instance_id == id))
        {
            self.status("Drop cancelled: its workspace was closed.", false);
            return;
        }
        let current = self
            .project()
            .map(|p| (p.instance_id, self.current_pane_id()));
        let background = batch.target != current || self.chrome_blocked();
        let mut errors = Vec::new();
        let mut files = Vec::new();
        // Explicitly dropped folders establish membership before opening files,
        // irrespective of the order in which Windows lists a mixed selection.
        for entry in batch.entries {
            match entry {
                Ok((path, true)) => {
                    if background {
                        self.add_background_project(path);
                    } else {
                        self.add_resolved_project(path);
                    }
                }
                Ok((path, false)) => files.push(path),
                Err(error) => errors.push(error),
            }
        }
        for path in files {
            let index = self.file_workspace(&path);
            if background {
                if !self.projects[index]
                    .tabs
                    .iter()
                    .any(|tab| tab.path.as_ref() == Some(&path))
                {
                    self.open_background_tab(index, path, crate::cli::DocumentView::Source);
                }
                continue;
            }
            self.switch_project(index);
            if let Some((id, pane)) = &batch.target
                && *id == self.projects[index].instance_id
            {
                let _ = self.focus_editor_pane(*pane);
            }
            self.open_resolved_file(path, false, true);
        }
        if !errors.is_empty() {
            self.status(errors.join("\n"), true);
        }
    }

    pub(super) fn paint_file_drop(&self) {
        if !self.ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            return;
        }
        let rect = self
            .drop_target()
            .map(|(_, rect)| rect)
            .unwrap_or_else(|| self.ctx.content_rect())
            .shrink(5.0);
        let painter = self.ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("file_drop"),
        ));
        painter.rect_filled(rect, 6.0, ACCENT.gamma_multiply(0.08));
        painter.rect_stroke(
            rect,
            6.0,
            Stroke::new(2.0, ACCENT),
            egui::StrokeKind::Inside,
        );
        let text = if self.chrome_blocked() {
            "Finish the current dialog to open files"
        } else {
            "Drop to open files or add project folders"
        };
        let galley = painter.layout_no_wrap(text.into(), FontId::proportional(15.0), TEXT);
        let label =
            egui::Rect::from_center_size(rect.center(), galley.size() + Vec2::new(24.0, 16.0));
        painter.rect_filled(label, 6.0, PANEL);
        painter.galley(label.min + Vec2::new(12.0, 8.0), galley, TEXT);
    }

    pub(super) fn ensure_other_files(&mut self) -> usize {
        if let Some(index) = self.projects.iter().position(Project::is_other_files) {
            return index;
        }
        let mut workspace = Project::other_files();
        workspace.closed_tabs = std::mem::take(&mut self.closed_other_files);
        self.projects.push(workspace);
        self.session_dirty = true;
        self.projects.len() - 1
    }

    pub(super) fn active_folder(&self) -> Option<&Path> {
        self.project().and_then(Project::folder_root)
    }

    pub(super) fn document_directory(&self) -> PathBuf {
        self.active_folder()
            .or_else(|| {
                self.tab()
                    .and_then(|t| t.path.as_deref())
                    .and_then(Path::parent)
            })
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir)
    }

    pub(super) fn file_workspace(&mut self, path: &Path) -> usize {
        // Reuse the exact document even if its folder was opened after the file.
        if let Some(index) = self.projects.iter().position(|project| {
            project.tabs.iter().any(|tab| {
                tab.path
                    .as_deref()
                    .is_some_and(|p| control::same_workspace(p, path))
            })
        }) {
            return index;
        }
        control::containing_project(&self.projects, path)
            .unwrap_or_else(|| self.ensure_other_files())
    }

    pub(super) fn open_routed_file(&mut self, path: PathBuf) {
        // Resolve relative components and aliases before deciding ownership.
        let path = fs::canonicalize(&path).unwrap_or(path);
        let index = self.file_workspace(&path);
        self.switch_project(index);
        self.open_resolved_file(path, false, true);
    }

    pub(super) fn prune_empty_other_files(&mut self) {
        if let Some(index) = self
            .projects
            .iter()
            .position(|p| p.is_other_files() && p.tabs.is_empty())
        {
            self.closed_other_files = std::mem::take(&mut self.projects[index].closed_tabs);
            self.perform_close(CloseAction::Project(index));
        }
    }

    pub(super) fn other_files_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Open files").strong());
        ui.add_space(6.0);
        let Some(project) = self.project() else {
            return;
        };
        let root = project.root.clone();
        let active = project.active_tab;
        let items: Vec<_> = project
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                (
                    index,
                    tab.id,
                    tab.title.clone(),
                    tab.path.as_deref().map(crate::paths::display),
                    tab.dirty,
                )
            })
            .collect();
        self.explorer_register_focus(ui, &root);
        if !items
            .iter()
            .any(|(_, id, _, _, _)| self.explorer_navigation.other_files.selected == Some(*id))
        {
            self.explorer_navigation.other_files.selected =
                items.get(active).map(|(_, id, _, _, _)| *id);
        }
        self.explorer_navigation.other_files.page_rows =
            (ui.available_height() / 44.0).floor().max(1.0) as usize;
        let list_focused = self.explorer_keyboard_focused();
        let mut chosen = None;
        let mut close = None;
        let mut reveal = None;
        egui::ScrollArea::vertical()
            .id_salt("other_files_list")
            .show(ui, |ui| {
                for (index, id, title, path, dirty) in items {
                    let menu_id = egui::Id::new(("other_files_context_menu", id));
                    let label = format!("{title}{}", if dirty { " •" } else { "" });
                    let selected = if list_focused || egui::Popup::is_id_open(&self.ctx, menu_id) {
                        self.explorer_navigation.other_files.selected == Some(id)
                    } else {
                        index == active
                    };
                    let response = ui
                        .push_id(id, |ui| ui.selectable_label(selected, &label))
                        .inner
                        .on_hover_details(path.as_deref().unwrap_or("Unsaved document"));
                    if response.clicked() {
                        self.explorer_navigation.other_files.selected = Some(id);
                        chosen = Some(index);
                    }
                    if response.secondary_clicked() {
                        self.explorer_navigation.other_files.selected = Some(id);
                        self.explorer_navigation.other_files.menu = None;
                    }
                    if self.explorer_navigation.other_files.selected == Some(id)
                        && self.explorer_navigation.other_files.scroll_to_selection
                    {
                        response.scroll_to_me(None);
                        self.explorer_navigation.other_files.scroll_to_selection = false;
                    }
                    let mut popup = egui::Popup::context_menu(&response).id(menu_id);
                    if let Some((target, position)) = self.explorer_navigation.other_files.menu
                        && target == id
                    {
                        let opening = position.is_none();
                        let position = position.unwrap_or_else(|| response.rect.left_bottom());
                        if opening {
                            egui::Popup::open_id(&self.ctx, menu_id);
                            self.explorer_navigation.other_files.menu = Some((id, Some(position)));
                        }
                        if egui::Popup::is_id_open(&self.ctx, menu_id) {
                            popup = popup.at_position(position);
                        } else {
                            self.explorer_navigation.other_files.menu = None;
                            if !self.chrome_blocked() {
                                self.restore_other_files_list_focus();
                            }
                        }
                    }
                    popup.show(|ui| {
                        if ui.button("Close file").clicked() {
                            close = Some(id);
                            ui.close();
                        }
                        if let Some(path) = &path
                            && ui.button("Copy path").clicked()
                        {
                            ui.ctx().copy_text(path.clone());
                            ui.close();
                        }
                        if let Some(path) = &path
                            && ui.button("Reveal in Explorer").clicked()
                        {
                            reveal = Some(PathBuf::from(path));
                            ui.close();
                        }
                    });
                    if let Some(path) = path {
                        ui.add(
                            egui::Label::new(RichText::new(path).small().color(MUTED)).truncate(),
                        );
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(index) = chosen {
            self.activate_tab(self.active_project, index);
        }
        if let Some(id) = close {
            self.request_close(CloseAction::Tab(id));
        }
        if let Some(path) = reveal {
            self.context_reveal(path);
        }
    }
}
