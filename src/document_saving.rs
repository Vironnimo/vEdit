//! Saving, saved-result publication and dirty-document close coordination.
use super::*;

impl VEditApp {
    pub(super) fn finish_document_save(
        &mut self,
        id: u64,
        text: String,
        path: PathBuf,
        result: Result<workspace::SavedDocument, String>,
        conflict: Option<workspace::LoadedDocument>,
    ) {
        let mut message = None;
        let mut new_document = false;
        let mut observed = None;
        let mut cleanup = false;
        if let Some(tab) = self.tab_by_id_mut(id) {
            tab.saving = false;
            match result {
                // The file changed after it was loaded: keep the
                // edits and let the user choose a version.
                Err(_) if conflict.is_some() && tab.path.as_ref() == Some(&path) => {
                    tab.last_edit = Instant::now();
                    observed = conflict;
                }
                Ok(saved) => {
                    tab.disk_conflict = None;
                    new_document = tab.path.as_ref() != Some(&path);
                    tab.path = Some(path.clone());
                    tab.title = file_title(&path);
                    tab.saved_text = text;
                    tab.fingerprint = Some(saved.fingerprint);
                    tab.line_ending = saved.line_ending;
                    tab.dirty = tab.text != tab.saved_text;
                    if tab.dirty {
                        // A completed save changes the baseline of
                        // newer edits without changing their text.
                        // Older recovery completions may no longer
                        // claim that this new snapshot is secured.
                        tab.recovery_generation = tab.recovery_generation.wrapping_add(1);
                        tab.recovery_revision = 0;
                    }
                    if !tab.dirty {
                        tab.agent_unsaved = false;
                    }
                    tab.error = None;
                    if !tab.dirty {
                        // Remove the recovery file off the UI
                        // thread; auto save retries a failure.
                        // Older snapshot completions no longer
                        // describe the recovery state.
                        tab.recovery_generation = tab.recovery_generation.wrapping_add(1);
                        tab.recovery_stale = true;
                        cleanup = true;
                    }
                    message = Some((
                        if saved.format_fallback {
                            format!(
                                "Saved {} with LF line endings; the original mixed line endings could not be verified",
                                tab.title
                            )
                        } else {
                            format!("Saved {}", tab.title)
                        },
                        false,
                    ));
                }
                Err(error) => {
                    tab.error = Some(error.clone());
                    tab.last_edit = Instant::now();
                    message = Some((error, true));
                }
            }
        }
        if let Some(disk) = observed {
            self.observe_disk_version(id, disk);
            if let Some(tab) = self.tab_by_id_mut(id) {
                let title = tab.title.clone();
                message = Some(if tab.disk_conflict.is_some() {
                    (
                        format!(
                            "{title} changed on disk, so it was not saved. Compare the versions and choose which one to keep."
                        ),
                        true,
                    )
                } else {
                    (format!("{title} already matches the disk version"), false)
                });
            }
        }
        let saved = message.as_ref().is_some_and(|(_, error)| !error);
        if saved && new_document {
            self.remember_recent_document(id);
        }
        if let Some((message, error)) = message {
            self.status(message, error);
            if error {
                self.close_after_save = false;
                self.save_queue.clear();
            }
        }
        self.session_dirty = true;
        if cleanup {
            self.start_recovery_cleanup(id);
        }
        if (new_document || (saved && path.file_name().is_some_and(|name| name == ".gitignore")))
            && let Some(index) = self
                .projects
                .iter()
                .position(|p| p.tabs.iter().any(|t| t.id == id))
        {
            self.index_project(index);
        }
        if saved {
            let indices: Vec<_> = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| p.tabs.iter().any(|t| t.id == id))
                .map(|(index, _)| index)
                .collect();
            for index in indices {
                self.refresh_git(index, true);
            }
        }
        self.sync_languages(true);
        for project in &self.projects {
            if saved
                && project.tabs.iter().any(|t| t.id == id && t.error.is_none())
                && let Some(server) = project
                    .language_for(&path)
                    .and_then(|session| session.server.as_ref())
            {
                server.save(&path);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum CloseAction {
    Tab(u64),
    PaneTab { pane_id: u64, id: u64 },
    Project(usize),
    Exit,
}

impl VEditApp {
    pub(super) fn save_tab(&mut self, id: u64, target: Option<PathBuf>) {
        self.save_tab_expected(id, target, None);
    }
    pub(super) fn save_tab_expected(
        &mut self,
        id: u64,
        target: Option<PathBuf>,
        overwrite: Option<workspace::FileFingerprint>,
    ) {
        self.keep_tab_open(id);
        if target
            .as_deref()
            .is_some_and(|path| self.context_file_path_pending(path))
            || self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .find(|tab| tab.id == id)
                .and_then(|tab| tab.path.as_deref())
                .is_some_and(|path| self.context_file_path_pending(path))
        {
            self.status(
                "Wait for the filesystem operation to finish before saving",
                true,
            );
            return;
        }
        if let Some(message) = target
            .as_deref()
            .or_else(|| {
                self.projects
                    .iter()
                    .flat_map(|project| &project.tabs)
                    .find(|tab| tab.id == id)
                    .and_then(|tab| tab.path.as_deref())
            })
            .and_then(|path| self.git_checkout_save_error(path))
        {
            self.status(message, true);
            self.close_after_save = false;
            self.save_queue.clear();
            return;
        }
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.loading || tab.saving {
            if let Some(target) = target {
                // A confirmed destination must not be lost to a save in flight.
                let title = tab.title.clone();
                self.deferred_save_as.retain(|(queued, _, _)| *queued != id);
                self.deferred_save_as.push((id, target, overwrite));
                self.status(
                    format!("Saving {title} as a new file after the current save finishes"),
                    false,
                );
            }
            return;
        }
        let Some(path) = target.or_else(|| tab.path.clone()) else {
            if let Some(current) = self.save_as {
                if current != id && !self.save_queue.contains(&id) {
                    self.save_queue.push_back(id);
                }
                return;
            }
            self.request_save_as(id);
            return;
        };
        let own_path = tab.path.as_ref() == Some(&path);
        if own_path && overwrite.is_none() && tab.disk_conflict.is_some() {
            let title = tab.title.clone();
            self.save_queue.clear();
            self.close_after_save = false;
            self.status(
                format!("{title} changed on disk. Choose Keep my version or Use disk version first, or Save as… another file."),
                true,
            );
            return;
        }
        let expected = overwrite.or_else(|| own_path.then(|| tab.fingerprint.clone()).flatten());
        let text = tab.text.clone();
        let ending = tab.line_ending;
        let bom = tab.bom;
        let format_source = tab.path.clone().zip(tab.fingerprint.clone());
        // Keep the path the user opened, for example through a junction, so a
        // later open of the same path finds this document instead of a copy.
        let new_path = tab.path.as_ref() != Some(&path);
        tab.saving = true;
        tab.error = None;
        let failed_path = path.clone();
        self.worker_or(
            move || {
                let result = (|| {
                    if expected.is_none()
                        && let Some(parent) = path.parent()
                    {
                        fs::create_dir_all(parent).map_err(|e| (e.to_string(), false))?;
                    }
                    if let Some(parent) = path.parent() {
                        workspace::remove_stale_save_temporaries(parent);
                    }
                    workspace::save_document(
                        &path,
                        &text,
                        ending,
                        bom,
                        expected.as_ref(),
                        format_source
                            .as_ref()
                            .map(|(path, baseline)| (path.as_path(), baseline)),
                    )
                    .map_err(|error| {
                        let conflict = matches!(error, workspace::SaveError::Conflict(_));
                        (error.to_string(), conflict)
                    })
                })();
                // A conflict on the document's own path shows the newer disk
                // version, so the user can compare and decide.
                let conflict = match &result {
                    Err((_, true)) if own_path => {
                        workspace::read_document(&path, MAX_DOCUMENT).ok()
                    }
                    _ => None,
                };
                let result = result.map_err(|(error, _)| error);
                let path = if result.is_ok() && new_path {
                    fs::canonicalize(&path).unwrap_or(path)
                } else {
                    path
                };
                WorkerResult::Saved {
                    result,
                    id,
                    text,
                    path,
                    conflict,
                }
            },
            move |error| WorkerResult::Saved {
                result: Err(error),
                id,
                text: String::new(),
                path: failed_path,
                conflict: None,
            },
        );
    }
    pub(super) fn save_current(&mut self) {
        if let Some(tab) = self.tab() {
            let id = tab.id;
            self.save_tab(id, None);
        }
    }
    pub(super) fn save_all(&mut self) {
        let ids: Vec<_> = self
            .projects
            .iter()
            .flat_map(|p| &p.tabs)
            .filter(|t| t.dirty && !t.saving)
            .map(|t| t.id)
            .collect();
        for id in ids {
            self.save_tab(id, None);
        }
    }
    pub(super) fn request_close(&mut self, action: CloseAction) {
        if self.close_has_dirty(action) {
            self.close_action = Some(action);
            self.close_after_save = false;
        } else {
            self.perform_close(action);
        }
    }
    pub(super) fn close_has_dirty(&self, action: CloseAction) -> bool {
        match action {
            CloseAction::PaneTab { pane_id, id } => {
                !self.pane_tab_is_shared(pane_id, id)
                    && self
                        .projects
                        .iter()
                        .flat_map(|p| &p.tabs)
                        .any(|t| t.id == id && (t.dirty || t.saving))
            }
            CloseAction::Tab(id) => self
                .projects
                .iter()
                .flat_map(|p| &p.tabs)
                .any(|t| t.id == id && (t.dirty || t.saving)),
            CloseAction::Project(i) => self
                .projects
                .get(i)
                .is_some_and(|p| p.tabs.iter().any(|t| t.dirty || t.saving)),
            CloseAction::Exit => self
                .projects
                .iter()
                .any(|p| p.tabs.iter().any(|t| t.dirty || t.saving)),
        }
    }
    pub(super) fn perform_close(&mut self, action: CloseAction) {
        self.sync_agent_marks();
        self.capture_pane_navigation();
        let closed_tab = self.closed_tab_for_action(action);
        match action {
            CloseAction::PaneTab { pane_id, id } => {
                if self.current_pane_id() == pane_id
                    && self.tab().is_some_and(|tab| tab.id == id)
                    && let Some(tab) = self.tab_mut()
                    && tab.vim.finish_insert(&mut tab.text).text_changed
                {
                    tab.changed();
                }
                match self.detach_pane_tab(pane_id, id) {
                    Ok(true) => self.remove_closed_document(id),
                    Ok(false) => {}
                    Err(error) => {
                        self.status(error, true);
                        return;
                    }
                }
            }
            CloseAction::Tab(id) => {
                self.remove_closed_document(id);
                self.remove_document_from_layout(id);
            }
            CloseAction::Project(index) => {
                if index < self.projects.len() {
                    self.capture_pane_navigation();
                    let previous_root = self.project().map(|project| project.root.clone());
                    let p = self.projects.remove(index);
                    self.explorer_navigation.release_project_rows(&p.root);
                    p.index_cancel.store(true, Ordering::Relaxed);
                    for t in p.tabs {
                        self.agent_marks.detach_document(t.id);
                        self.markdown.forget_document(t.id);
                        self.git_gutter.forget(t.id);
                        self.recovery_writer
                            .remove_in_background(t.recovery_path, self.recovery_writer.stamp());
                    }
                    if self.active_project > index {
                        self.active_project -= 1;
                    }
                    self.active_project = self
                        .active_project
                        .min(self.projects.len().saturating_sub(1));
                    self.activate_project_layout(self.active_project);
                    self.clear_project_search_for_root_change(previous_root);
                }
            }
            CloseAction::Exit => {
                for p in &mut self.projects {
                    for t in &mut p.tabs {
                        t.text.clone_from(&t.saved_text);
                        t.dirty = false;
                        // Discarded drafts must not return as agent drafts in the session.
                        t.agent_unsaved = false;
                        t.disk_conflict = None;
                        let _ = self
                            .recovery_writer
                            .remove(&t.recovery_path, self.recovery_writer.stamp());
                    }
                }
                self.flush_recovery();
                self.exit_ready = true;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.close_action = None;
        self.close_after_save = false;
        self.remember_closed_tab(closed_tab);
        self.control_prune_review();
        if !matches!(action, CloseAction::Exit) {
            self.reconcile_layout_after_close();
            self.prune_empty_other_files();
        }
        self.session_dirty = true;
    }
    pub(super) fn remove_closed_document(&mut self, id: u64) {
        for project in &mut self.projects {
            if let Some(index) = project.tabs.iter().position(|tab| tab.id == id) {
                let tab = project.tabs.remove(index);
                self.agent_marks.detach_document(id);
                self.markdown.forget_document(id);
                self.git_gutter.forget(id);
                self.recovery_writer
                    .remove_in_background(tab.recovery_path, self.recovery_writer.stamp());
                if project.active_tab > index {
                    project.active_tab -= 1;
                }
                project.active_tab = project.active_tab.min(project.tabs.len().saturating_sub(1));
                break;
            }
        }
    }
    pub(super) fn control_close_pane_tab(&mut self, pane_id: u64, id: u64) -> Result<(), String> {
        if !self.pane_contains_tab(pane_id, id) {
            return Err("Tab is not open in this pane; no pane changed".into());
        }
        let action = CloseAction::PaneTab { pane_id, id };
        if self.close_has_dirty(action) {
            return Err("The last view has unsaved changes or a save in progress; save explicitly or close it in the app".into());
        }
        self.perform_close(action);
        Ok(())
    }
}
