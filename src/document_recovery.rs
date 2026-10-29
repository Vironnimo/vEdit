//! Recovery snapshots, baseline generations, cleanup and shutdown persistence.
use super::*;

impl VEditApp {
    pub(super) fn finish_recovery_write(
        &mut self,
        id: u64,
        revision: u64,
        generation: u64,
        result: Result<(), String>,
    ) {
        if let Some(tab) = self.tab_by_id_mut(id) {
            tab.recovery_pending = false;
            if result.is_ok() && generation == tab.recovery_generation {
                tab.recovery_revision = tab.recovery_revision.max(revision);
            } else if result.is_err() {
                tab.last_edit = Instant::now();
            }
        }
        if let Err(e) = result {
            self.status(e, true);
        }
        self.session_dirty = true;
    }
}

impl VEditApp {
    pub(super) fn finish_recovery_removal(
        &mut self,
        id: u64,
        revision: u64,
        generation: u64,
        result: Result<(), String>,
    ) {
        if let Some(tab) = self.tab_by_id_mut(id) {
            if tab.recovery_cleanup_pending == Some(revision) {
                tab.recovery_cleanup_pending = None;
            }
            if result.is_ok() && generation == tab.recovery_generation {
                tab.recovery_revision = tab.recovery_revision.max(revision);
                tab.recovery_cleanup_retry_at = None;
                tab.recovery_stale = false;
            } else {
                tab.recovery_cleanup_retry_at = Some(Instant::now() + Duration::from_millis(850));
            }
        }
        if let Err(error) = result {
            self.status(error, true);
        }
        self.session_dirty = true;
    }
}

impl VEditApp {
    pub(super) fn autosave(&mut self) {
        let mut recoveries = Vec::new();
        let mut removals = Vec::new();
        let mut saves = Vec::new();
        let mut delay: Option<Duration> = None;
        // Auto save resumes when the pull or branch switch reports its result.
        let checkout = self.git_checkout_root().map(Path::to_path_buf);
        for p in &mut self.projects {
            for t in &mut p.tabs {
                if t.loading {
                    continue;
                }
                if !t.dirty {
                    let retry_delay = t
                        .recovery_cleanup_retry_at
                        .map(|retry| retry.saturating_duration_since(Instant::now()));
                    if !t.saving
                        && t.error.is_none()
                        && (t.recovery_revision != t.revision || t.recovery_stale)
                        && t.recovery_cleanup_pending != Some(t.revision)
                    {
                        if let Some(remaining) =
                            retry_delay.filter(|remaining| !remaining.is_zero())
                        {
                            delay =
                                Some(delay.map_or(remaining, |existing| existing.min(remaining)));
                        } else {
                            t.recovery_cleanup_pending = Some(t.revision);
                            t.recovery_cleanup_retry_at = None;
                            removals.push((
                                t.id,
                                t.revision,
                                t.recovery_generation,
                                t.recovery_path.clone(),
                            ));
                        }
                    }
                    continue;
                }
                if t.recovery_revision != t.revision && !t.recovery_pending {
                    let remaining =
                        Duration::from_millis(850).saturating_sub(t.last_edit.elapsed());
                    if remaining.is_zero() {
                        t.recovery_pending = true;
                        recoveries.push((
                            t.id,
                            t.revision,
                            t.recovery_generation,
                            t.recovery_path.clone(),
                            t.text.clone(),
                            t.fingerprint.clone(),
                            t.line_ending,
                            t.bom,
                        ));
                    } else {
                        delay = Some(delay.map_or(remaining, |d| d.min(remaining)));
                    }
                }
                if self.settings.autosave
                    && !t.agent_unsaved
                    && self.save_as != Some(t.id)
                    && t.path.as_deref().is_some_and(|path| {
                        checkout
                            .as_deref()
                            .is_none_or(|root| !context_actions::path_contains(root, path))
                    })
                    && !t.saving
                    && t.disk_conflict.is_none()
                {
                    // An I/O failure can clear without changing file metadata,
                    // e.g. when another process releases a lock. Keep retrying
                    // through the ordinary fingerprint guard, at a lower rate.
                    let save_delay = Duration::from_millis(self.settings.autosave_delay_ms);
                    let save_delay = if t.error.is_some() {
                        save_delay.max(Duration::from_secs(5))
                    } else {
                        save_delay
                    };
                    let remaining = save_delay.saturating_sub(t.last_edit.elapsed());
                    if remaining.is_zero() {
                        saves.push(t.id);
                    } else {
                        delay = Some(delay.map_or(remaining, |d| d.min(remaining)));
                    }
                }
            }
        }
        for (id, revision, generation, path, text, baseline, ending, bom) in recoveries {
            let writer = self.recovery_writer.clone();
            let stamp = writer.stamp();
            self.worker_or(
                move || {
                    let result =
                        crate::recovery::encode_document(&text, baseline.as_ref(), ending, bom)
                            .and_then(|bytes| writer.write(&path, stamp, &bytes));
                    WorkerResult::Recovered {
                        id,
                        revision,
                        generation,
                        result,
                    }
                },
                move |error| WorkerResult::Recovered {
                    id,
                    revision,
                    generation,
                    result: Err(error),
                },
            );
        }
        for (id, revision, generation, path) in removals {
            self.spawn_recovery_removal(id, revision, generation, path);
        }
        for id in saves {
            let pending = self
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .find(|tab| tab.id == id)
                .and_then(|tab| tab.path.as_deref())
                .is_some_and(|path| self.context_file_path_pending(path));
            if !pending {
                self.save_tab(id, None);
            }
        }
        if let Some(delay) = delay {
            self.ctx.request_repaint_after(delay);
        }
        if self.session_dirty {
            let remaining = Duration::from_secs(1).saturating_sub(self.last_session_save.elapsed());
            if remaining.is_zero() {
                self.persist_session();
                self.session_dirty = false;
                self.last_session_save = Instant::now();
            } else {
                // An inbox can receive messages with no project open. Ensure
                // the session still saves when no other work requests a frame.
                self.ctx.request_repaint_after(remaining);
            }
        }
    }
    fn spawn_recovery_removal(&self, id: u64, revision: u64, generation: u64, path: PathBuf) {
        let writer = self.recovery_writer.clone();
        let stamp = writer.stamp();
        self.worker_or(
            move || WorkerResult::RecoveryRemoved {
                id,
                revision,
                generation,
                result: writer.remove(&path, stamp),
            },
            move |error| WorkerResult::RecoveryRemoved {
                id,
                revision,
                generation,
                result: Err(error),
            },
        );
    }
    /// Remove the recovery file of a document that now matches its file.
    pub(super) fn start_recovery_cleanup(&mut self, id: u64) {
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.dirty
            || tab.loading
            || tab.saving
            || tab.error.is_some()
            || tab.recovery_cleanup_pending == Some(tab.revision)
        {
            return;
        }
        tab.recovery_cleanup_pending = Some(tab.revision);
        tab.recovery_cleanup_retry_at = None;
        let (revision, generation, path) = (
            tab.revision,
            tab.recovery_generation,
            tab.recovery_path.clone(),
        );
        self.spawn_recovery_removal(id, revision, generation, path);
    }
    pub(super) fn flush_recovery(&self) {
        if self.external_open_save_pending {
            let _ = self.recovery_writer.write(
                &self.external_open_directory().join("external-open.json"),
                self.recovery_writer.stamp(),
                &serde_json::to_vec(&self.settings.external_open_new_window).unwrap(),
            );
        }
        for p in &self.projects {
            for tab in &p.tabs {
                if tab.dirty {
                    if let Ok(bytes) = crate::recovery::encode_document(
                        &tab.text,
                        tab.fingerprint.as_ref(),
                        tab.line_ending,
                        tab.bom,
                    ) {
                        let _ = self.recovery_writer.write(
                            &tab.recovery_path,
                            self.recovery_writer.stamp(),
                            &bytes,
                        );
                    }
                } else if !tab.loading && !tab.saving && tab.error.is_none() {
                    let _ = self
                        .recovery_writer
                        .remove(&tab.recovery_path, self.recovery_writer.stamp());
                }
            }
        }
        if let Ok(bytes) = serde_json::to_vec(&self.session()) {
            let _ = self.recovery_writer.write(
                &self.recovery_session_path,
                self.recovery_writer.stamp(),
                &bytes,
            );
        }
    }
}

/// Recovery files that no session references are removed at startup once they
/// are this old, which leaves time to find a file after an unexpected crash.
pub(super) const ORPHANED_RECOVERY_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[cfg(test)]
pub(super) fn recovery_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join("test-state")
        .join(std::process::id().to_string())
        .join("recovery")
}

#[cfg(not(test))]
pub(super) fn recovery_directory() -> PathBuf {
    crate::profile::recovery_directory()
}
