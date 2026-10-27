//! Git snapshot, history and diff requests with guarded result publication.
use super::*;

impl VEditApp {
    pub(super) fn finish_git_status(
        &mut self,
        root: PathBuf,
        request_id: u64,
        result: Result<Box<GitSnapshot>, String>,
    ) {
        if let Some(index) = self
            .projects
            .iter()
            .position(|p| p.root == root && p.git_status_request == Some(request_id))
        {
            let p = &mut self.projects[index];
            p.git_loading = false;
            p.git_status_request = None;
            p.last_git_refresh = Instant::now();
            p.last_git_indicator_refresh = Instant::now();
            p.git_indicator_request = None;
            match result {
                Ok(s) => {
                    let s = *s;
                    if p.git_root != s.root {
                        p.clear_git_repository_data();
                    }
                    p.git_summary = project_status::GitSummary::from_snapshot(&s);
                    p.git_parent_root = s.parent_root;
                    p.git_indicator_count =
                        p.git_summary.as_ref().map(|summary| summary.status.changes);
                    let repository = s.root.as_ref().unwrap_or(&p.root);
                    p.git_view
                        .hover_stats_pending
                        .retain(|(root, _), _| root == repository);
                    p.git_view.hover_stats.retain(|_, result| result.is_ok());
                    p.git_view
                        .hover_stats_order
                        .retain(|key| p.git_view.hover_stats.contains_key(key));
                    p.git_root = s.root;
                    p.git_head = s.head;
                    p.git_fingerprint = Some(s.fingerprint);
                    p.git_branch = s.branch;
                    p.git_tracking = s.tracking;
                    p.git_main_comparison = s.main_comparison;
                    p.git_view.update_working_cache(&s.files, &s.stats);
                    if p.git_changes != s.files {
                        p.explorer_git = None;
                    }
                    p.git_changes = s.files;
                    p.git_untracked_collapsed = s.untracked_collapsed;
                    p.git_stats = s.stats;
                    p.git_stats_error = s.stats_error;
                    if let Some((generation, repository, result)) = s.branch_review {
                        p.change_review
                            .apply_branch_check(generation, &repository, result);
                    }
                    match s.history {
                        Ok(history) => {
                            p.git_branches = history.snapshot.refs.clone();
                            if let Some(page) = history.page {
                                static NEXT_HISTORY: AtomicU64 = AtomicU64::new(1);
                                p.git_history_generation =
                                    NEXT_HISTORY.fetch_add(1, Ordering::Relaxed);
                                p.git_history_loading = false;
                                p.git_history_has_more = page.has_more;
                                p.git_view
                                    .preserve_history_scroll(&p.recent_commits, &page.commits);
                                p.recent_commits = page.commits;
                                p.git_view.graph.reset(
                                    history
                                        .snapshot
                                        .refs
                                        .iter()
                                        .find(|reference| reference.current)
                                        .map(|reference| reference.hash.as_str()),
                                );
                                p.git_view.graph.append(&p.recent_commits);
                                p.git_history_error = None;
                                p.git_history_snapshot = Some(history.snapshot);
                            }
                        }
                        Err(error) => p.git_history_error = Some(error),
                    }
                    p.git_error = None;
                }
                Err(e) => {
                    p.git_indicator_count = None;
                    p.git_summary = None;
                    p.git_error = Some(e);
                }
            }
            let refresh = std::mem::take(&mut p.git_refresh_pending);
            if refresh {
                self.refresh_git(index, true);
            }
        }
    }
}

impl VEditApp {
    pub(super) fn finish_git_commit(
        &mut self,
        root: PathBuf,
        hash: String,
        result: Result<git::CommitDetails, String>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.root == root)
            && p.git_root.is_some()
        {
            if let Ok(detail) = &result {
                // Keep recent immutable commits without growing with history.
                if p.git_view.detail_cache.len() >= 16 {
                    p.git_view.detail_cache.clear();
                }
                p.git_view.detail_cache.insert(hash.clone(), detail.clone());
            }
            if p.git_view.selected_commit.as_deref() == Some(&hash) {
                p.git_view.detail_loading = false;
                match result {
                    Ok(detail) => {
                        p.git_view.detail = Some(detail);
                        p.git_view.detail_error = None;
                    }
                    Err(error) => {
                        p.git_view.detail = None;
                        p.git_view.detail_error = Some(error);
                    }
                }
            }
        }
    }
}

impl VEditApp {
    pub(super) fn finish_git_commit_stats(
        &mut self,
        root: PathBuf,
        repository: PathBuf,
        hash: String,
        request_id: u64,
        result: Result<git::CommitStats, String>,
    ) {
        let active_root = self.project().map(|project| project.root.clone());
        if let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.root == root)
        {
            let key = (repository.clone(), hash);
            if project.git_view.hover_stats_pending.get(&key) != Some(&request_id) {
                return;
            }
            project.git_view.hover_stats_pending.remove(&key);
            let current_repository = project.git_root.as_ref().unwrap_or(&project.root);
            if current_repository != &repository || active_root.as_ref() != Some(&root) {
                return;
            }
            while project.git_view.hover_stats.len() >= 128 {
                if let Some(oldest) = project.git_view.hover_stats_order.pop_front() {
                    project.git_view.hover_stats.remove(&oldest);
                } else {
                    project.git_view.hover_stats.clear();
                }
            }
            project.git_view.hover_stats_order.push_back(key.clone());
            project.git_view.hover_stats.insert(key, result);
        }
    }
}

impl VEditApp {
    pub(super) fn finish_git_history(
        &mut self,
        root: PathBuf,
        generation: u64,
        offset: usize,
        result: Result<git::HistoryPage, String>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.root == root)
            && p.git_history_generation == generation
            && p.recent_commits.len() == offset
        {
            p.git_history_loading = false;
            match result {
                Ok(page) => {
                    p.git_history_has_more = page.has_more;
                    p.git_view.graph.append(&page.commits);
                    p.recent_commits.extend(page.commits);
                    p.git_history_error = None;
                }
                Err(error) => p.git_history_error = Some(error),
            }
        }
    }
}

pub(super) struct GitSnapshot {
    pub(super) root: Option<PathBuf>,
    pub(super) parent_root: Option<PathBuf>,
    pub(super) branch: Option<String>,
    pub(super) tracking: Option<git::BranchInfo>,
    pub(super) main_comparison: Result<Option<git::BranchComparison>, String>,
    pub(super) files: Vec<git::FileStatus>,
    pub(super) untracked_collapsed: bool,
    pub(super) stats: git::WorkingTreeStats,
    pub(super) stats_error: Option<String>,
    /// HEAD as the status read reported it; `None` when Git did not say.
    pub(super) head: Option<git::StatusHead>,
    /// [`git::status_fingerprint`] of this read.
    pub(super) fingerprint: u64,
    /// A resolved branch review from this same worker, guarded by generation.
    pub(super) branch_review: Option<(u64, PathBuf, Result<git::ReviewComparison, String>)>,
    pub(super) history: Result<GitHistoryRefresh, String>,
}

pub(super) struct GitHistoryRefresh {
    pub(super) snapshot: git::HistorySnapshot,
    pub(super) page: Option<git::HistoryPage>,
}

/// Read one project's repository state on a worker for `refresh_git`.
pub(super) fn read_git_snapshot(
    root: &Path,
    approved_parent: Option<&Path>,
    previous_repository: Option<&Path>,
    previous_history: Option<git::HistorySnapshot>,
    refresh_history: bool,
    previous_history_error: Option<String>,
    automatic_history_limit: Option<usize>,
) -> Result<GitSnapshot, String> {
    let repository = git::project_repository(root, approved_parent).map_err(|e| e.to_string())?;
    let Some(repo) = repository.root else {
        return Ok(GitSnapshot {
            root: None,
            parent_root: repository.parent,
            branch: None,
            tracking: None,
            main_comparison: Ok(None),
            files: Vec::new(),
            untracked_collapsed: false,
            stats: git::WorkingTreeStats::default(),
            stats_error: None,
            head: None,
            fingerprint: 0,
            branch_review: None,
            history: Ok(GitHistoryRefresh {
                snapshot: git::HistorySnapshot::default(),
                page: Some(git::HistoryPage::default()),
            }),
        });
    };
    let same_repository = previous_repository == Some(repo.as_path());
    let previous_history = previous_history.filter(|_| same_repository);
    let previous_history_error = previous_history_error.filter(|_| same_repository);
    let (status, tracking, head) = git::status_with_head(&repo).map_err(|e| e.to_string())?;
    let fingerprint = git::status_fingerprint(&repo, head.as_ref(), &status.files);
    let (stats, stats_error) = match git::working_tree_stats(&repo, &status.files) {
        Ok(stats) => (stats, None),
        Err(error) => (git::WorkingTreeStats::default(), Some(error.to_string())),
    };
    // HEAD comes from the status read and the base branch from the history
    // refs, so an unchanged repository on main needs no further Git process.
    let mut history_head = None;
    let history = (|| {
        if let Some(error) = previous_history_error {
            return Err(error);
        }
        let (snapshot, head) = git::history_snapshot_after_status(
            &repo,
            previous_history.as_ref(),
            status.branch.as_deref(),
            head.as_ref(),
        )
        .map_err(|error| error.to_string())?;
        history_head = Some(head);
        let page = if refresh_history
            || previous_history.is_none()
            || (automatic_history_limit.is_some() && previous_history.as_ref() != Some(&snapshot))
        {
            Some(
                git::history_page(
                    &repo,
                    &snapshot.current_branch(),
                    0,
                    automatic_history_limit.unwrap_or(git::HISTORY_PAGE_SIZE),
                )
                .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        Ok(GitHistoryRefresh { snapshot, page })
    })();
    let main_comparison = match &history_head {
        Some(head) => git::main_comparison_after_history(&repo, head),
        None => git::main_comparison(&repo),
    }
    .map_err(|e| e.to_string());
    Ok(GitSnapshot {
        root: Some(repo),
        parent_root: repository.parent,
        branch: status.branch,
        tracking: Some(tracking),
        main_comparison,
        untracked_collapsed: status.untracked_collapsed,
        files: status.files,
        stats,
        stats_error,
        head,
        fingerprint,
        branch_review: None,
        history,
    })
}

impl VEditApp {
    pub(super) fn git_polling_active(&self) -> bool {
        self.ctx
            .input(|input| input.focused && input.viewport().minimized != Some(true))
    }
    pub(super) fn refresh_visible_git(&mut self) {
        let active = self.git_polling_active();
        if active
            && (self.git_window_was_active != Some(true)
                || self
                    .ctx
                    .input(|input| input.events.contains(&egui::Event::WindowFocused(true))))
            && let Some(server) = &self.control
        {
            server.mark_active();
            self.refresh_external_open_preference();
        }
        // Focus events also cover a minimize/restore cycle during which the
        // native backend did not render an inactive frame.
        let resumed = active
            && self.git_window_was_active.is_some()
            && (self.git_window_was_active == Some(false)
                || self
                    .ctx
                    .input(|input| input.events.contains(&egui::Event::WindowFocused(true))));
        self.git_window_was_active = Some(active);
        if !active {
            return;
        }
        if resumed {
            if let Some(project) = self.projects.get_mut(self.active_project) {
                // Keep the refresh due even if a Git action currently owns the
                // repository. An in-flight read gets one coalesced follow-up.
                project.last_git_refresh = Instant::now() - Duration::from_secs(30);
            }
            self.repositories.request_refresh();
            self.refresh_unwatched_project(self.active_project);
        }
        self.refresh_git(self.active_project, resumed);
        self.refresh_inactive_git_indicators();
        self.refresh_repository_overview();
    }
    pub(super) fn refresh_git(&mut self, index: usize, force: bool) {
        if self.projects.get(index).is_none_or(Project::is_other_files) {
            return;
        }
        static NEXT_STATUS_REQUEST: AtomicU64 = AtomicU64::new(1);
        let window_active = self.git_polling_active();
        if !force && !window_active {
            return;
        }
        // A visible change review notices new changes as quickly as Source control.
        let history_visible = index == self.active_project
            && (self.sidebar_view_visible(SidebarView::SourceControl)
                || self.change_review_visible());
        let interval = if history_visible {
            Duration::from_secs(3)
        } else {
            Duration::from_secs(10)
        };
        let Some(p) = self.projects.get_mut(index) else {
            return;
        };
        if p.git_loading {
            p.git_refresh_pending |= force;
            return;
        }
        if self.git_busy.is_some() {
            return;
        }
        if !force && p.last_git_refresh.elapsed() < interval {
            self.ctx
                .request_repaint_after(interval.saturating_sub(p.last_git_refresh.elapsed()));
            return;
        }
        p.git_loading = true;
        let request_id = NEXT_STATUS_REQUEST.fetch_add(1, Ordering::Relaxed);
        p.git_status_request = Some(request_id);
        // An older lightweight inactive refresh must not supersede this snapshot.
        p.git_indicator_request = None;
        let root = p.root.clone();
        let approved_parent = p.git_approved_parent.clone();
        let previous_repository = p.git_root.clone();
        let previous_history = p.git_history_snapshot.clone();
        // Only reload the graph when its immutable traversal snapshot changes.
        // Retain the depth already loaded by the reader instead of dropping back
        // to the first page. Hidden histories catch up on their next visible poll.
        let automatic_history_limit = (window_active && history_visible)
            .then_some(p.recent_commits.len().max(git::HISTORY_PAGE_SIZE));
        let refresh_history = std::mem::take(&mut p.git_history_refresh_requested)
            || (automatic_history_limit.is_some() && p.git_history_error.is_some());
        let previous_history_error = p.git_history_error.clone().filter(|_| !refresh_history);
        let branch_check = p.change_review.branch_check();
        self.worker(move || {
            let result = read_git_snapshot(
                &root,
                approved_parent.as_deref(),
                previous_repository.as_deref(),
                previous_history,
                refresh_history,
                previous_history_error,
                automatic_history_limit,
            )
            .map(|mut snapshot| {
                if let Some((generation, repository, source)) = branch_check
                    && snapshot.root.as_ref() == Some(&repository)
                {
                    let comparison = git::resolve_review(&repository, &source)
                        .map_err(|error| error.to_string());
                    snapshot.branch_review = Some((generation, repository, comparison));
                }
                snapshot
            });
            WorkerResult::GitStatus {
                root,
                request_id,
                result: result.map(Box::new),
            }
        });
    }
    pub(super) fn request_git_commit(&mut self, index: usize, hash: String) {
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        let view = &mut project.git_view;
        view.detail_open = true;
        if view.selected_commit.as_ref() == Some(&hash) && view.detail_loading {
            return;
        }
        view.selected_commit = Some(hash.clone());
        view.selected_file = None;
        view.detail_error = None;
        if let Some(detail) = view.detail_cache.get(&hash) {
            view.detail = Some(detail.clone());
            view.detail_loading = false;
            return;
        }
        view.detail = None;
        view.detail_loading = true;
        let repository = project
            .git_root
            .clone()
            .unwrap_or_else(|| project.root.clone());
        let root = project.root.clone();
        self.worker(move || WorkerResult::GitCommit {
            root,
            result: git::commit_details(&repository, &hash).map_err(|e| e.to_string()),
            hash,
        });
    }
    pub(super) fn request_git_hover_stats(&mut self, index: usize, hash: String) {
        if index != self.active_project {
            return;
        }
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        let repository = project
            .git_root
            .clone()
            .unwrap_or_else(|| project.root.clone());
        let key = (repository.clone(), hash.clone());
        let view = &mut project.git_view;
        if view.hover_stats.contains_key(&key)
            || view.hover_stats_pending.contains_key(&key)
            || view.hover_stats_pending.len() >= 4
        {
            return;
        }
        static NEXT_HOVER_REQUEST: AtomicU64 = AtomicU64::new(1);
        let request_id = NEXT_HOVER_REQUEST.fetch_add(1, Ordering::Relaxed);
        view.hover_stats_pending.insert(key, request_id);
        let root = project.root.clone();
        self.worker(move || WorkerResult::GitCommitStats {
            result: git::commit_stats(&repository, &hash).map_err(|error| error.to_string()),
            root,
            repository,
            hash,
            request_id,
        });
    }
    pub(super) fn load_more_git_history(&mut self) {
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return;
        };
        if project.git_history_loading
            || !project.git_history_has_more
            || project.git_history_error.is_some()
        {
            return;
        }
        let (Some(repository), Some(snapshot)) = (
            project.git_root.clone(),
            project.git_history_snapshot.clone(),
        ) else {
            return;
        };
        project.git_history_loading = true;
        let root = project.root.clone();
        let generation = project.git_history_generation;
        let offset = project.recent_commits.len();
        self.worker(move || WorkerResult::GitHistory {
            root,
            generation,
            offset,
            result: git::history_page(
                &repository,
                &snapshot.current_branch(),
                offset,
                git::HISTORY_PAGE_SIZE,
            )
            .map_err(|error| error.to_string()),
        });
    }
    pub(super) fn refresh_git_history(&mut self, index: usize) {
        if let Some(project) = self.projects.get_mut(index) {
            project.git_history_refresh_requested = true;
        }
        self.refresh_git(index, true);
    }
    pub(super) fn request_git_diff(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        staged: bool,
        untracked: bool,
    ) {
        let title = format!(
            "{} · {}",
            path.display(),
            if staged {
                "Staged changes"
            } else {
                "Working tree"
            }
        );
        let file = self
            .projects
            .iter()
            .filter(|p| p.git_root.as_ref() == Some(&root))
            .flat_map(|p| &p.git_changes)
            .find(|file| Path::new(&file.path) == path)
            .cloned();
        self.begin_git_diff(title.clone());
        self.diff_repository = Some(root.clone());
        let generation = self.diff_generation;
        self.worker(move || WorkerResult::Diff {
            generation,
            title,
            result: if let Some(file) = file {
                git::status_diff(&root, &file, staged)
            } else if untracked {
                git::untracked_diff(&root, &path)
            } else {
                git::diff(&root, &path, staged)
            }
            .map_err(|e| e.to_string()),
        });
    }
    pub(super) fn request_commit_diff(&mut self, root: PathBuf, hash: String, path: PathBuf) {
        let title = format!("{} · {}", path.display(), &hash[..hash.len().min(8)]);
        let cached = self
            .project()
            .and_then(|p| p.git_view.detail_cache.get(&hash))
            .and_then(|detail| {
                detail
                    .files
                    .iter()
                    .find(|file| Path::new(&file.path) == path)
                    .map(|file| (detail.clone(), file.clone()))
            });
        self.begin_git_diff(title.clone());
        self.diff_repository = Some(root.clone());
        let generation = self.diff_generation;
        self.worker(move || WorkerResult::Diff {
            generation,
            title,
            result: if let Some((detail, file)) = cached {
                git::commit_file_diff(&root, &detail, &file)
            } else {
                git::commit_diff(&root, &hash, &path)
            }
            .map_err(|e| e.to_string()),
        });
    }
    pub(super) fn begin_git_diff(&mut self, title: String) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.diff_loading = true;
        self.diff_error = None;
        self.diff = Some((title, String::new()));
        self.comparison = None;
    }
}
