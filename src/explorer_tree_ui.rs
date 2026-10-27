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

    /// Start following external file changes once per project, on a worker:
    /// opening the watch handle is disk I/O. A failed start is retried by the
    /// next manual refresh; until then window activation re-reads instead.
    pub(super) fn ensure_file_watcher(&mut self, index: usize) {
        if !self.file_watching {
            return;
        }
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        if project.is_other_files() || project.watcher.is_some() || project.watcher_starting {
            return;
        }
        project.watcher_starting = true;
        let root = project.root.clone();
        let instance = project.instance_id;
        let ctx = self.ctx.clone();
        self.worker(move || WorkerResult::FileWatcher {
            result: crate::fs_watch::ProjectWatcher::start(&root, move |delay| {
                ctx.request_repaint_after(delay)
            }),
            root,
            instance,
        });
    }

    pub(super) fn file_watcher_started(
        &mut self,
        root: PathBuf,
        instance: u64,
        result: Result<crate::fs_watch::ProjectWatcher, String>,
    ) {
        // Closing and reopening the same root creates a different owner.
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.root == root && project.instance_id == instance)
        else {
            return;
        };
        let project = &mut self.projects[index];
        project.watcher_starting = false;
        match result {
            Ok(watcher) => {
                project.watcher = Some(watcher);
                project.watcher_error = None;
                self.sync_watched_listing(index);
                // Names may have changed between the first listing and the
                // watch. Re-reading the few listed folders is cheap; the index
                // build that started with the project is not repeated.
                self.apply_file_changes(
                    index,
                    crate::fs_watch::Changes {
                        rescan: true,
                        ..Default::default()
                    },
                );
            }
            Err(error) => project.watcher_error = Some(error),
        }
    }

    /// The watcher only reports names in folders the Explorer has listed.
    pub(super) fn sync_watched_listing(&self, index: usize) {
        if let Some(project) = self.projects.get(index)
            && let (Some(watcher), Some(explorer)) = (&project.watcher, &project.explorer)
        {
            watcher.set_listed(explorer.listed().map(Path::to_path_buf));
        }
    }

    pub(super) fn poll_file_watchers(&mut self) {
        let now = Instant::now();
        for index in 0..self.projects.len() {
            let Some(watcher) = &self.projects[index].watcher else {
                continue;
            };
            match watcher.poll(now) {
                crate::fs_watch::Poll::Idle => {}
                crate::fs_watch::Poll::Wait(delay) => self.ctx.request_repaint_after(delay),
                crate::fs_watch::Poll::Ready(changes) => self.apply_file_changes(index, changes),
            }
        }
    }

    /// Re-read affected listed folders in the background, keeping their rows,
    /// the selection, expansion and scroll position until fresh rows arrive.
    /// Open documents are left to disk synchronization.
    pub(super) fn apply_file_changes(&mut self, index: usize, changes: crate::fs_watch::Changes) {
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        if let Some(explorer) = project.explorer.as_mut() {
            explorer.forget(&changes.removed);
            if changes.rescan {
                explorer.invalidate();
            } else {
                explorer.refresh(&changes.directories);
            }
        }
        if changes.index {
            self.refresh_index_quietly(index);
        }
        self.sync_watched_listing(index);
        if index == self.active_project {
            let root = self.projects[index].root.clone();
            self.request_explorer_directory(index, root);
            self.ctx.request_repaint();
        }
    }

    /// Rebuild the Quick Open index without the visible "indexing" state or a
    /// new Explorer generation. Builds never overlap; changes during a build
    /// schedule exactly one more.
    pub(super) fn refresh_index_quietly(&mut self, index: usize) {
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        if project.index_in_flight {
            project.index_again = true;
            return;
        }
        project.index_request += 1;
        project.index_in_flight = true;
        project.index_cancel = Arc::new(AtomicBool::new(false));
        let root = project.root.clone();
        let instance = project.instance_id;
        let generation = project.index_generation;
        let request = project.index_request;
        let cancel = project.index_cancel.clone();
        self.worker(move || WorkerResult::Index {
            result: workspace::build_index(
                &root,
                workspace::IndexOptions {
                    cancel: Some(cancel),
                    ..Default::default()
                },
            ),
            root,
            instance,
            generation,
            request,
        });
    }

    /// Without a working watcher (for example on some network shares), the
    /// window becoming active re-reads listed folders and the index instead.
    pub(super) fn refresh_unwatched_project(&mut self, index: usize) {
        if !self.file_watching
            || self.projects.get(index).is_none_or(|project| {
                project.is_other_files() || project.watcher.is_some() || project.watcher_starting
            })
        {
            return;
        }
        self.ensure_file_watcher(index);
        self.apply_file_changes(
            index,
            crate::fs_watch::Changes {
                rescan: true,
                index: true,
                ..Default::default()
            },
        );
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
        if self.project().is_some_and(Project::is_other_files) {
            self.other_files_ui(ui);
            return;
        }
        let Some(root) = self.project().map(|project| project.root.clone()) else {
            return;
        };
        self.request_explorer_directory(self.active_project, root.clone());
        self.explorer_register_focus(ui, &root);
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
                .on_hover_details(errors.join("\n"));
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

    /// The Git decoration for an Explorer row of the active project.
    fn explorer_git_mark(
        &self,
        path: &Path,
        is_dir: bool,
    ) -> Option<explorer_git::ExplorerGitMark> {
        let project = self.project()?;
        let marks = project
            .explorer_git
            .as_ref()
            .filter(|marks| !marks.is_empty())?;
        let relative = path.strip_prefix(project.git_root.as_ref()?).ok()?;
        marks.get(relative, is_dir)
    }

    fn explorer_tree_rows(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[explorer_navigation::CachedVisibleRow],
        active: &Option<PathBuf>,
    ) {
        let owns_selection = self.explorer_keyboard_focused() || self.explorer_context_menu_open();
        if let Some(project) = self.projects.get_mut(self.active_project)
            && project.explorer_git.is_none()
        {
            project.explorer_git = Some(explorer_git::ExplorerGitMarks::new(&project.git_changes));
        }
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
                let git_mark = self.explorer_git_mark(&entry.path, entry.is_dir);
                if entry.ignored {
                    tooltip.push_str("\nIgnored by Git");
                } else if let Some(mark) = git_mark {
                    tooltip.push('\n');
                    tooltip.push_str(if entry.is_dir {
                        "Contains changes"
                    } else {
                        mark.label()
                    });
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
                        git: git_mark
                            .filter(|_| !entry.ignored)
                            .map(|mark| (mark.letter(), mark.color())),
                        tooltip: &tooltip,
                    },
                );
                self.explorer_row_interaction(&response, &entry.path, entry.is_dir);
                self.file_context_menu(&response, &entry.path, entry.is_dir);
                if response.clicked() && !entry.is_dir {
                    self.open_explorer_file(entry.path.clone(), response.double_clicked());
                    self.focus_explorer();
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reopened_project(root: PathBuf) -> (VEditApp, u64) {
        let old = Project::new(root.clone());
        let mut replacement = Project::new(root);
        assert_ne!(old.instance_id, replacement.instance_id);
        replacement.watcher_starting = true;
        let mut app = VEditApp::empty(egui::Context::default());
        app.projects.push(replacement);
        (app, old.instance_id)
    }

    #[test]
    fn obsolete_watcher_error_does_not_change_reopened_project() {
        let root = std::env::temp_dir().join("repo-audit/watcher-old-error");
        let (mut app, instance) = reopened_project(root.clone());

        app.file_watcher_started(root, instance, Err("old project failed".into()));

        let project = &app.projects[0];
        assert!(project.watcher_starting);
        assert!(project.watcher_error.is_none());
    }

    #[test]
    fn obsolete_watcher_success_does_not_replace_reopened_project_watch() {
        let root = std::env::temp_dir().join(format!(
            "repo-audit/watcher-old-success-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let (mut app, instance) = reopened_project(root.clone());
        let watcher = crate::fs_watch::ProjectWatcher::start(&root, |_| {}).unwrap();

        app.file_watcher_started(root.clone(), instance, Ok(watcher));

        let still_starting = app.projects[0].watcher_starting;
        let has_watcher = app.projects[0].watcher.is_some();
        drop(app);
        fs::remove_dir(&root).unwrap();
        assert!(still_starting);
        assert!(!has_watcher);
    }
}
