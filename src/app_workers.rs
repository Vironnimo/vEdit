//! Background execution and result routing in the application frame.
use super::*;

pub(super) enum WorkerResult {
    ExternalOpenRead {
        revision: u64,
        enabled: bool,
    },
    ExternalOpenSaved {
        revision: u64,
        result: Result<(), String>,
    },
    RecentOpened(recent::OpenResult),
    DroppedPaths(other_files::DroppedPaths),
    ExplorerDirectory {
        root: PathBuf,
        path: PathBuf,
        generation: u64,
        result: Result<workspace::ExplorerDirectory, String>,
    },
    Index {
        root: PathBuf,
        instance: u64,
        generation: u64,
        request: u64,
        result: Result<workspace::WorkspaceIndex, String>,
    },
    FileWatcher {
        root: PathBuf,
        instance: u64,
        result: Result<crate::fs_watch::ProjectWatcher, String>,
    },
    OutputLink {
        root: PathBuf,
        instance: u64,
        request: u64,
        link: crate::output_links::Link,
        path: Option<PathBuf>,
    },
    Loaded {
        id: u64,
        result: Result<workspace::LoadedDocument, String>,
        recovery: Option<Result<crate::recovery::RecoveredDocument, String>>,
    },
    Saved {
        id: u64,
        text: String,
        path: PathBuf,
        result: Result<workspace::SavedDocument, String>,
        /// The newer disk version that stopped a save to the document's own path.
        conflict: Option<workspace::LoadedDocument>,
    },
    Recovered {
        id: u64,
        revision: u64,
        generation: u64,
        result: Result<(), String>,
    },
    RecoveryRemoved {
        id: u64,
        revision: u64,
        generation: u64,
        result: Result<(), String>,
    },
    GitStatus {
        root: PathBuf,
        request_id: u64,
        result: Result<Box<GitSnapshot>, String>,
    },
    GitIndicator {
        root: PathBuf,
        request_id: u64,
        result: Result<Option<project_status::GitSummary>, String>,
    },
    /// Worktree listing or status for the project picker.
    RepositoryOverview(git_panel::repositories::OverviewResult),
    GitCommit {
        root: PathBuf,
        hash: String,
        result: Result<git::CommitDetails, String>,
    },
    GitCommitStats {
        root: PathBuf,
        repository: PathBuf,
        hash: String,
        request_id: u64,
        result: Result<git::CommitStats, String>,
    },
    GitHistory {
        root: PathBuf,
        generation: u64,
        offset: usize,
        result: Result<git::HistoryPage, String>,
    },
    GitAction {
        reply: Option<Sender<crate::cli::Response>>,
        root: PathBuf,
        kind: GitActionKind,
        result: Result<String, String>,
    },
    Search {
        root: PathBuf,
        generation: u64,
        query: String,
        excludes: Vec<String>,
        result: Result<workspace::SearchResults, String>,
    },
    ContextFile(context_actions::FileActionResult),
    ContextFilePrepared(context_actions::FilePreparationResult),
    NativeDialog {
        request: Picker,
        result: Result<Option<DialogSelection>, String>,
    },
    Diff {
        generation: u64,
        title: String,
        result: Result<String, String>,
    },
    ChangeReview {
        root: PathBuf,
        generation: u64,
        result: Result<change_review::LoadedReview, String>,
    },
    ChangeReviewPatches {
        root: PathBuf,
        generation: u64,
        request: u64,
        patches: Vec<change_review::LoadedPatch>,
    },
    DiskChanges(Vec<disk_sync::DiskCheck>),
    /// The committed version a document's gutter markers compare with.
    GitGutterBase {
        key: git_gutter::BaseKey,
        request: u64,
        result: Result<git::BaseVersion, String>,
    },
    /// Gutter markers of one document revision, compared off the UI thread.
    GitGutterDiff {
        document: u64,
        request: u64,
        revision: u64,
        key: git_gutter::BaseKey,
        base: Arc<str>,
        hunks: Vec<crate::line_diff::Hunk>,
    },
    /// A background job panicked; its own error result follows if it has one.
    Panicked(String),
}

impl WorkerResult {
    /// A periodic disk check that found nothing waits for the next scheduled
    /// frame, which also starts the following check. Every other result is
    /// shown right away.
    fn wakes_window(&self) -> bool {
        !matches!(self, WorkerResult::DiskChanges(changes) if changes.is_empty())
    }
}

impl VEditApp {
    /// Run `work` on a background thread. A panic is reported in the status
    /// bar instead of silently losing the result.
    pub(super) fn worker(&self, work: impl FnOnce() -> WorkerResult + Send + 'static) {
        self.spawn_worker(work, None);
    }
    /// Like [`Self::worker`], for a job whose pending flag (saving, loading,
    /// a running Git action, a disk check) must clear even if it panics:
    /// `failed` turns the panic message into that job's error result.
    pub(super) fn worker_or(
        &self,
        work: impl FnOnce() -> WorkerResult + Send + 'static,
        failed: impl FnOnce(String) -> WorkerResult + Send + 'static,
    ) {
        self.spawn_worker(work, Some(Box::new(failed)));
    }
    fn spawn_worker(
        &self,
        work: impl FnOnce() -> WorkerResult + Send + 'static,
        failed: Option<Box<dyn FnOnce(String) -> WorkerResult + Send>>,
    ) {
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
                Ok(result) => result,
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|message| (*message).to_owned())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    let message = format!("Internal error in a background task: {message}");
                    let _ = sender.send(WorkerResult::Panicked(message.clone()));
                    match failed {
                        Some(failed) => failed(message),
                        None => {
                            ctx.request_repaint();
                            return;
                        }
                    }
                }
            };
            let wake = result.wakes_window();
            let _ = sender.send(result);
            if wake {
                ctx.request_repaint();
            }
        });
    }
    pub(super) fn poll(&mut self) {
        self.poll_launches();
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                WorkerResult::ExternalOpenRead { revision, enabled } => {
                    if revision == self.external_open_revision && !self.external_open_save_pending {
                        self.settings.external_open_new_window = enabled;
                    }
                }
                WorkerResult::ExternalOpenSaved { revision, result } => {
                    if revision == self.external_open_revision {
                        self.external_open_save_pending = false;
                        if let Err(error) = result {
                            self.status(error, true);
                        }
                    }
                }
                WorkerResult::RecentOpened(opened) => self.finish_recent_open(opened),
                WorkerResult::DroppedPaths(batch) => self.finish_dropped_paths(batch),
                WorkerResult::Panicked(message) => self.status(message, true),
                WorkerResult::ContextFile(result) => self.context_file_result(result),
                WorkerResult::ContextFilePrepared(result) => self.context_file_prepared(result),
                WorkerResult::ExplorerDirectory {
                    root,
                    path,
                    generation,
                    result,
                } => self.finish_explorer_directory(root, path, generation, result),
                WorkerResult::Index {
                    root,
                    instance,
                    generation,
                    request,
                    result,
                } => self.finish_project_index(root, instance, generation, request, result),
                WorkerResult::FileWatcher {
                    root,
                    instance,
                    result,
                } => self.file_watcher_started(root, instance, result),
                WorkerResult::OutputLink {
                    root,
                    instance,
                    request,
                    link,
                    path,
                } => self.output_link_resolved(root, instance, request, link, path),
                WorkerResult::Loaded {
                    id,
                    result,
                    recovery,
                } => self.finish_document_load(id, result, recovery),
                WorkerResult::Saved {
                    id,
                    text,
                    path,
                    result,
                    conflict,
                } => self.finish_document_save(id, text, path, result, conflict),
                WorkerResult::Recovered {
                    id,
                    revision,
                    generation,
                    result,
                } => self.finish_recovery_write(id, revision, generation, result),
                WorkerResult::RecoveryRemoved {
                    id,
                    revision,
                    generation,
                    result,
                } => self.finish_recovery_removal(id, revision, generation, result),
                WorkerResult::GitStatus {
                    root,
                    request_id,
                    result,
                } => self.finish_git_status(root, request_id, result),
                WorkerResult::GitIndicator {
                    root,
                    request_id,
                    result,
                } => {
                    self.finish_git_indicator(root, request_id, result);
                }
                WorkerResult::RepositoryOverview(result) => {
                    self.repositories.finish(result);
                }
                WorkerResult::GitCommit { root, hash, result } => {
                    self.finish_git_commit(root, hash, result)
                }
                WorkerResult::GitCommitStats {
                    root,
                    repository,
                    hash,
                    request_id,
                    result,
                } => self.finish_git_commit_stats(root, repository, hash, request_id, result),
                WorkerResult::GitHistory {
                    root,
                    generation,
                    offset,
                    result,
                } => self.finish_git_history(root, generation, offset, result),
                WorkerResult::GitAction {
                    reply,
                    root,
                    kind,
                    result,
                } => self.finish_git_action(reply, root, kind, result),
                WorkerResult::Search {
                    root,
                    generation,
                    query,
                    excludes,
                    result,
                } => self.finish_project_search(root, generation, query, excludes, result),
                WorkerResult::NativeDialog { request, result } => {
                    self.finish_native_dialog(request, result);
                }
                WorkerResult::Diff {
                    generation,
                    title,
                    result,
                } => {
                    if generation == self.diff_generation && self.diff.is_some() {
                        self.diff_loading = false;
                        match result {
                            Ok(text) => {
                                self.diff = Some((title, text));
                                self.diff_error = None;
                            }
                            Err(error) => self.diff_error = Some(error),
                        }
                    }
                }
                WorkerResult::ChangeReview {
                    root,
                    generation,
                    result,
                } => self.apply_change_review(root, generation, result),
                WorkerResult::ChangeReviewPatches {
                    root,
                    generation,
                    request,
                    patches,
                } => self.apply_change_review_patches(root, generation, request, patches),
                WorkerResult::DiskChanges(checks) => self.apply_disk_checks(checks),
                WorkerResult::GitGutterBase {
                    key,
                    request,
                    result,
                } => self.apply_git_gutter_base(key, request, result),
                WorkerResult::GitGutterDiff {
                    document,
                    request,
                    revision,
                    key,
                    base,
                    hunks,
                } => self.apply_git_gutter_diff(document, request, revision, key, base, hunks),
            }
        }
        self.resume_deferred_save_as();
        if self.save_as.is_none() && self.picker.is_none() {
            while let Some(id) = self.save_queue.pop_front() {
                if self.tab_by_id_mut(id).is_some_and(|t| t.dirty && !t.saving) {
                    self.save_tab(id, None);
                    if self.save_as.is_some() {
                        break;
                    }
                }
            }
        }
        if self.close_after_save
            && let Some(action) = self.close_action
            && !self.close_has_dirty(action)
        {
            self.perform_close(action);
        }
        for index in 0..self.projects.len() {
            if self.projects[index].run.poll() {
                self.refresh_git(index, true);
            }
        }
        if self
            .projects
            .iter()
            .any(|project| project.run.task.is_some())
        {
            self.ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}
