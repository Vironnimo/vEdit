//! Document load requests, guarded publication and explicit reload.
use super::*;

impl VEditApp {
    pub(super) fn finish_document_load(
        &mut self,
        id: u64,
        result: Result<workspace::LoadedDocument, String>,
        recovery: Option<Result<crate::recovery::RecoveredDocument, String>>,
    ) {
        let remember = self.recent_opening.remove(&id);
        self.find.invalidate(id);
        let ctx = self.ctx.clone();
        let preserve_scroll = self.has_saved_document_scroll(id);
        let next_recovery_name = format!(
            "{}-{id}-after-unreadable-{}.txt",
            crate::recovery::session_prefix(),
            self.recovery_writer.stamp()
        );
        if let Some(tab) = self.tab_by_id_mut(id) {
            // Only the active load may publish. All load entry
            // points serialize requests; a late duplicate must not
            // overwrite a document that has become editable.
            if !tab.loading || tab.saving {
                return;
            }
            if tab.dirty {
                tab.loading = false;
                tab.error = Some("The reload was skipped because the buffer has new edits".into());
                return;
            }
            tab.loading = false;
            tab.lines.invalidate();
            tab.indentation_cache.invalidate();
            match result {
                Ok(doc) => {
                    if tab.text != doc.text {
                        let replacing =
                            tab.fingerprint.is_some() || !tab.text.is_empty() || tab.revision > 0;
                        tab.revision = tab.revision.saturating_add(1);
                        if replacing {
                            context_actions::reset_native_history(&ctx, id);
                            let cursor = tab.vim.cursor.min(doc.text.chars().count());
                            tab.vim = VimState::new();
                            tab.vim.set_cursor(&doc.text, cursor);
                            tab.selection = cursor..cursor;
                            tab.pending_selection = Some(cursor..cursor);
                        }
                    }
                    tab.text = doc.text.clone();
                    tab.saved_text = doc.text;
                    tab.dirty = false;
                    tab.line_ending = doc.line_ending;
                    debug_assert_eq!(
                        doc.mixed_line_endings,
                        tab.line_ending == workspace::LineEnding::Mixed
                    );
                    tab.bom = doc.bom;
                    tab.fingerprint = Some(doc.fingerprint);
                    tab.error = None;
                    tab.disk_conflict = None;
                }
                Err(error) => {
                    if tab.path.is_some() {
                        tab.error = Some(error);
                    }
                }
            }
            if let Some(recovery) = recovery {
                match recovery {
                    Ok(doc) => {
                        // The version just read, kept for a conflict
                        // with the recovered edits.
                        let disk_version =
                            tab.fingerprint
                                .clone()
                                .map(|fingerprint| workspace::LoadedDocument {
                                    text: tab.saved_text.clone(),
                                    line_ending: tab.line_ending,
                                    bom: tab.bom,
                                    mixed_line_endings: tab.line_ending
                                        == workspace::LineEnding::Mixed,
                                    fingerprint,
                                });
                        let disk_matches = doc.baseline.as_ref().is_some_and(|baseline| {
                            tab.fingerprint
                                .as_ref()
                                .is_some_and(|disk| disk.same_content(baseline))
                        });
                        if tab.text != doc.text {
                            tab.revision = tab.revision.saturating_add(1);
                        }
                        tab.text = doc.text;
                        tab.dirty = tab.text != tab.saved_text
                            || (tab.path.is_some() && tab.fingerprint.is_none());
                        if tab.dirty {
                            // A fresh disk read is an observation,
                            // not permission to overwrite changes
                            // made since this recovery was recorded.
                            tab.fingerprint = doc.baseline;
                            if !disk_matches && tab.path.is_some() {
                                match (&tab.fingerprint, disk_version) {
                                    // The user decides which version
                                    // continues, as for any disk conflict.
                                    // Without a recorded baseline the disk
                                    // version may be newer, so it is one too.
                                    (_, Some(disk)) => {
                                        tab.disk_conflict =
                                            Some(disk_sync::DiskConflict::new(disk));
                                    }
                                    // The read error is already shown.
                                    (Some(_), None) => {}
                                    (None, None) => {
                                        tab.error = Some(
                                            "Recovered edits have no verified disk baseline. Use Save as… to preserve them; the original file is protected.".into(),
                                        );
                                    }
                                }
                            }
                            if let Some(ending) = doc.line_ending {
                                tab.line_ending = ending;
                            }
                            if let Some(bom) = doc.bom {
                                tab.bom = bom;
                            }
                        } else {
                            // Nothing to recover: the file already
                            // holds this text. Remove the record.
                            tab.recovery_stale = true;
                        }
                        tab.recovery_revision = tab.revision;
                    }
                    Err(error) => {
                        let retained = crate::paths::display(&tab.recovery_path);
                        tab.recovery_path = tab.recovery_path.with_file_name(next_recovery_name);
                        tab.error = Some(format!(
                            "Recovery could not be read: {error}. The unreadable file is retained at {retained}; new edits use a separate recovery file."
                        ));
                    }
                }
            }
            tab.clear_agent_draft_when_saved();
            let range = tab
                .pending_selection
                .clone()
                .filter(|r| r.start != 0 || r.end != 0)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            let length = tab.text.chars().count();
            tab.set_cursor(range.start.min(length)..range.end.min(length));
            // Loading a restored document must not turn its saved
            // reading position into a cursor-reveal request. Pending
            // explicit navigation is applied after the load.
            if preserve_scroll {
                tab.scroll_to_cursor = false;
            }
        }
        if remember {
            self.remember_recent_document(id);
        }
    }
}

impl VEditApp {
    pub(super) fn load_tab(&self, id: u64, path: Option<PathBuf>, recovery: Option<PathBuf>) {
        self.worker_or(
            move || {
                let result = path
                    .as_ref()
                    .map(|p| workspace::read_document(p, MAX_DOCUMENT))
                    .unwrap_or_else(|| Err("Untitled".into()));
                let recovered = recovery
                    .filter(|p| p.is_file())
                    .map(|p| crate::recovery::read_document(&p, MAX_DOCUMENT));
                WorkerResult::Loaded {
                    id,
                    result,
                    recovery: recovered,
                }
            },
            move |error| WorkerResult::Loaded {
                id,
                result: Err(error),
                // The recovery file stays on disk for the next attempt.
                recovery: None,
            },
        );
    }
    pub(super) fn reload_current(&mut self) {
        if let Some(t) = self.tab_mut() {
            if t.loading || t.saving {
                self.status(
                    "Wait for the current file operation to finish before reloading",
                    true,
                );
                return;
            }
            if t.disk_conflict.is_some() {
                let id = t.id;
                self.use_disk_version(id);
                return;
            }
            if t.dirty {
                self.status("Save your changes to another file before reloading", true);
                return;
            }
            t.loading = true;
            t.error = None;
            let id = t.id;
            let path = t.path.clone();
            self.load_tab(id, path, None);
        }
    }
}
