//! Project state, file opening, index publication and active workspace changes.
use super::*;

impl VEditApp {
    pub(super) fn finish_project_index(
        &mut self,
        root: PathBuf,
        instance: u64,
        generation: u64,
        request: u64,
        result: Result<workspace::WorkspaceIndex, String>,
    ) {
        if let Some(index) = self.projects.iter().position(|p| {
            p.root == root
                && p.instance_id == instance
                && p.index_generation == generation
                && p.index_request == request
        }) {
            let p = &mut self.projects[index];
            p.indexing = false;
            p.index_in_flight = false;
            match result {
                Ok(index) => {
                    p.index_error = None;
                    p.rust_project = index.has_rust_manifest;
                    p.index = Some(Arc::new(index));
                }
                Err(e) => p.index_error = Some(e),
            }
            if std::mem::take(&mut p.index_again) {
                self.refresh_index_quietly(index);
            }
        }
    }
}

impl VEditApp {
    pub(super) fn finish_explorer_directory(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        generation: u64,
        result: Result<workspace::ExplorerDirectory, String>,
    ) {
        if let Some(index) = self
            .projects
            .iter()
            .position(|project| project.root == root)
            && let Some(explorer) = self.projects[index].explorer.as_mut()
            && explorer.complete(&path, generation, result)
        {
            self.sync_watched_listing(index);
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkspaceKind {
    #[default]
    Project,
    OtherFiles,
}

pub(super) struct Project {
    pub(super) instance_id: u64,
    pub(super) kind: WorkspaceKind,
    pub(super) launch: project_launch::LaunchState,
    pub(super) run: project_tasks::ProjectRun,
    pub(super) root: PathBuf,
    pub(super) tasks: project_tasks::TaskConfiguration,
    pub(super) tabs: Vec<EditorTab>,
    pub(super) closed_tabs: Vec<reopen::ClosedTab>,
    pub(super) active_tab: usize,
    /// Parked while another project is active; buffers always stay in this project.
    pub(super) layout: Option<split::EditorLayout>,
    pub(super) index: Option<Arc<workspace::WorkspaceIndex>>,
    pub(super) explorer: Option<workspace::ExplorerTree>,
    pub(super) indexing: bool,
    pub(super) index_cancel: Arc<AtomicBool>,
    pub(super) index_generation: u64,
    /// The latest index build, manual or file-watch driven; older results are dropped.
    pub(super) index_request: u64,
    pub(super) index_in_flight: bool,
    /// Files changed while a build ran; build once more when it finishes.
    pub(super) index_again: bool,
    pub(super) index_error: Option<String>,
    /// Follows external file creation, deletion and renames below `root`.
    pub(super) watcher: Option<crate::fs_watch::ProjectWatcher>,
    pub(super) watcher_starting: bool,
    pub(super) watcher_error: Option<String>,
    pub(super) rust_project: bool,
    pub(super) git_root: Option<PathBuf>,
    pub(super) git_parent_root: Option<PathBuf>,
    pub(super) git_approved_parent: Option<PathBuf>,
    pub(super) git_branch: Option<String>,
    pub(super) git_branches: Vec<git::CommitRef>,
    pub(super) git_tracking: Option<git::BranchInfo>,
    pub(super) git_main_comparison: Result<Option<git::BranchComparison>, String>,
    pub(super) git_changes: Vec<git::FileStatus>,
    /// `git_changes` lists untracked folders because there were too many files.
    pub(super) git_untracked_collapsed: bool,
    /// Explorer decorations for `git_changes`, rebuilt on first use after a change.
    pub(super) explorer_git: Option<explorer_git::ExplorerGitMarks>,
    pub(super) git_stats: git::WorkingTreeStats,
    pub(super) git_stats_error: Option<String>,
    pub(super) recent_commits: Vec<git::CommitSummary>,
    pub(super) git_history_snapshot: Option<git::HistorySnapshot>,
    pub(super) git_history_loading: bool,
    pub(super) git_history_has_more: bool,
    pub(super) git_history_generation: u64,
    pub(super) git_history_refresh_requested: bool,
    pub(super) git_history_error: Option<String>,
    pub(super) git_view: git_panel::GitViewState,
    pub(super) change_review: change_review::ChangeReviewState,
    pub(super) git_error: Option<String>,
    pub(super) git_loading: bool,
    pub(super) git_status_request: Option<u64>,
    pub(super) git_refresh_pending: bool,
    pub(super) last_git_refresh: Instant,
    pub(super) git_indicator_count: Option<usize>,
    /// Branch, divergence and change totals from the latest full or status refresh.
    pub(super) git_summary: Option<project_status::GitSummary>,
    /// HEAD and the status fingerprint of the latest full refresh; `None` until
    /// one completed. Gutter markers and change reviews compare them.
    pub(super) git_head: Option<git::StatusHead>,
    pub(super) git_fingerprint: Option<u64>,
    pub(super) git_indicator_request: Option<u64>,
    pub(super) last_git_indicator_refresh: Instant,
    pub(super) commit_message: String,
    pub(super) languages: HashMap<String, language_support::LanguageSession>,
    pub(super) diagnostics: HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>,
}

impl Project {
    pub(super) fn new(root: PathBuf) -> Self {
        static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
        Self {
            instance_id: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            kind: WorkspaceKind::Project,
            launch: project_launch::LaunchState::default(),
            run: project_tasks::ProjectRun::default(),
            root,
            tasks: project_tasks::TaskConfiguration::default(),
            tabs: Vec::new(),
            closed_tabs: Vec::new(),
            active_tab: 0,
            layout: None,
            index: None,
            explorer: None,
            indexing: false,
            index_cancel: Arc::new(AtomicBool::new(false)),
            index_generation: 0,
            index_request: 0,
            index_in_flight: false,
            index_again: false,
            index_error: None,
            watcher: None,
            watcher_starting: false,
            watcher_error: None,
            rust_project: false,
            git_root: None,
            git_parent_root: None,
            git_approved_parent: None,
            git_branch: None,
            git_branches: Vec::new(),
            git_tracking: None,
            git_main_comparison: Ok(None),
            git_changes: Vec::new(),
            git_untracked_collapsed: false,
            explorer_git: None,
            git_stats: git::WorkingTreeStats::default(),
            git_stats_error: None,
            recent_commits: Vec::new(),
            git_history_snapshot: None,
            git_history_loading: false,
            git_history_has_more: false,
            git_history_generation: 0,
            git_history_refresh_requested: false,
            git_history_error: None,
            git_view: git_panel::GitViewState::default(),
            change_review: change_review::ChangeReviewState::default(),
            git_error: None,
            git_loading: false,
            git_status_request: None,
            git_refresh_pending: false,
            last_git_refresh: Instant::now() - Duration::from_secs(30),
            git_indicator_count: None,
            git_summary: None,
            git_head: None,
            git_fingerprint: None,
            git_indicator_request: None,
            last_git_indicator_refresh: Instant::now() - Duration::from_secs(30),
            commit_message: String::new(),
            languages: HashMap::from([(
                "rust".into(),
                language_support::LanguageSession::new(language_settings::defaults().remove(0)),
            )]),
            diagnostics: HashMap::new(),
        }
    }
}

impl VEditApp {
    pub(super) fn open_argument(&mut self, input: PathBuf) {
        let path = fs::canonicalize(&input).unwrap_or(input);
        if path.is_dir() {
            self.add_project(path);
        } else if path.is_file() {
            self.open_routed_file(path);
        }
    }
    pub(super) fn add_project(&mut self, path: PathBuf) {
        let root = fs::canonicalize(&path).unwrap_or(path);
        self.add_resolved_project(root);
    }
    pub(super) fn add_resolved_project(&mut self, root: PathBuf) {
        self.remember_recent(root.clone(), recent::Kind::Folder);
        if let Some(index) = self.projects.iter().position(|p| p.root == root) {
            self.switch_project(index);
            return;
        }
        self.projects.push(Project::new(root));
        self.switch_project(self.projects.len() - 1);
        self.index_project(self.active_project);
        self.session_dirty = true;
    }
    pub(super) fn index_project(&mut self, index: usize) {
        let Some(p) = self.projects.get_mut(index) else {
            return;
        };
        if p.is_other_files() {
            return;
        }
        p.index_cancel.store(true, Ordering::Relaxed);
        p.index_cancel = Arc::new(AtomicBool::new(false));
        p.index_generation += 1;
        p.index_request += 1;
        p.index_in_flight = true;
        p.index_again = false;
        p.indexing = true;
        p.index_error = None;
        if let Some(explorer) = &mut p.explorer {
            explorer.invalidate();
        } else {
            p.explorer = Some(workspace::ExplorerTree::new(p.root.clone()));
        }
        let root = p.root.clone();
        let instance = p.instance_id;
        let explorer_root = root.clone();
        let generation = p.index_generation;
        let request = p.index_request;
        let cancel = p.index_cancel.clone();
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
        self.request_explorer_directory(index, explorer_root);
        self.ensure_file_watcher(index);
    }
    pub(super) fn open_file(&mut self, path: PathBuf) {
        self.open_file_in_mode(path, false);
    }
    pub(super) fn open_explorer_file(&mut self, path: PathBuf, keep_open: bool) {
        self.open_file_in_mode(path, self.settings.explorer_preview_tabs && !keep_open);
    }
    fn open_file_in_mode(&mut self, path: PathBuf, preview: bool) {
        self.open_resolved_file(path, preview, false);
    }
    pub(super) fn focus_open_file(&mut self, path: &Path, preview: bool) -> bool {
        let existing = self.projects.iter().enumerate().find_map(|(project, p)| {
            p.tabs
                .iter()
                .position(|tab| {
                    tab.path
                        .as_deref()
                        .is_some_and(|open| control::same_workspace(open, path))
                })
                .map(|tab| (project, tab))
        });
        let Some((project, tab)) = existing else {
            return false;
        };
        let id = self.projects[project].tabs[tab].id;
        if !preview || !self.current_pane_tab_ids().contains(&id) {
            self.keep_tab_open(id);
        }
        self.activate_tab(project, tab);
        true
    }
    pub(super) fn open_resolved_file(&mut self, path: PathBuf, preview: bool, resolved: bool) {
        if self.context_path_busy(&path) {
            self.status("Wait for the file rename to finish before opening it", true);
            return;
        }
        if self.focus_open_file(&path, preview) {
            return;
        }
        // Explorer and quick-open paths already have the canonical workspace root.
        let path = if resolved
            || (self.project().is_some_and(|p| path.starts_with(&p.root))
                && !path
                    .components()
                    .any(|part| part == std::path::Component::ParentDir))
        {
            path
        } else {
            fs::canonicalize(&path).unwrap_or(path)
        };
        if self.focus_open_file(&path, preview) {
            return;
        }
        let previous_preview = preview
            .then(|| {
                self.current_pane_tab_ids().into_iter().find(|id| {
                    self.project().is_some_and(|project| {
                        project
                            .tabs
                            .iter()
                            .any(|tab| tab.id == *id && tab.preview_tab)
                    })
                })
            })
            .flatten();
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return;
        };
        let mut tab = EditorTab::new(Some(path.clone()), file_title(&path));
        tab.preview_tab = preview;
        tab.loading = true;
        let id = tab.id;
        project.tabs.push(tab);
        let index = project.tabs.len() - 1;
        self.activate_tab(self.active_project, index);
        // Switching first finishes any pending Vim edit and keeps the pane alive.
        if let Some(previous) = previous_preview {
            self.replace_pane_preview(previous, id);
        }
        self.recent_opening.insert(id);
        self.load_tab(id, Some(path), None);
    }
    pub(super) fn keep_tab_open(&mut self, id: u64) {
        if let Some(tab) = self.tab_by_id_mut(id) {
            tab.preview_tab = false;
        }
    }
    pub(super) fn set_explorer_preview_tabs(&mut self, enabled: bool) {
        self.settings.explorer_preview_tabs = enabled;
        if !enabled {
            for tab in self
                .projects
                .iter_mut()
                .flat_map(|project| &mut project.tabs)
            {
                tab.preview_tab = false;
            }
        }
        self.session_dirty = true;
    }
    pub(super) fn activate_tab(&mut self, project_index: usize, tab_index: usize) {
        if self
            .projects
            .get(project_index)
            .is_none_or(|p| !p.tabs.is_empty() && tab_index >= p.tabs.len())
        {
            return;
        }
        // Showing a document ends a change review of that project.
        self.park_change_review(project_index);
        let different = self.active_project != project_index
            || self.project().is_some_and(|p| p.active_tab != tab_index);
        if different {
            self.cancel_vim_search(false);
            if let Some(tab) = self.tab_mut() {
                let action = tab.vim.finish_insert(&mut tab.text);
                if action.text_changed {
                    tab.changed();
                }
            }
            self.completions = None;
            self.completion_request = None;
        }
        self.capture_pane_navigation();
        let switching_project = self.active_project != project_index;
        self.set_active_project_context(project_index);
        if switching_project {
            self.focus_existing_project_tab(project_index, tab_index);
        }
        self.clear_empty_workspace();
        let project = &mut self.projects[project_index];
        project.active_tab = tab_index;
        let preview = project
            .tabs
            .get(tab_index)
            .is_some_and(|tab| tab.agent_view == crate::cli::DocumentView::Preview);
        if let Some(tab) = project.tabs.get_mut(tab_index) {
            tab.focus = !preview;
        }
        if preview {
            let focused = self.ctx.memory(|memory| memory.focused());
            if let Some(id) = focused
                && self
                    .projects
                    .iter()
                    .flat_map(|project| &project.tabs)
                    .any(|tab| id == self.document_widget_id(tab.id))
            {
                self.ctx.memory_mut(|memory| memory.surrender_focus(id));
            }
        }
        self.session_dirty = true;
        self.bind_current_pane();
        if let Some(id) = self.tab().map(|tab| tab.id) {
            self.remember_recent_document(id);
        }
    }
    pub(super) fn set_active_project_context(&mut self, index: usize) {
        let previous_index = self.active_project;
        let previous_root = self.project().map(|project| project.root.clone());
        self.activate_project_layout(index);
        if previous_root.as_deref() != self.project().map(|project| project.root.as_path())
            && let Some(previous) = self.projects.get_mut(previous_index)
        {
            previous.git_view.hover_stats_pending.clear();
        }
        self.clear_project_search_for_root_change(previous_root);
    }
    pub(super) fn clear_project_search_for_root_change(&mut self, previous_root: Option<PathBuf>) {
        if previous_root.as_deref() == self.project().map(|project| project.root.as_path()) {
            return;
        }
        self.project_search = None;
        self.project_search_error = None;
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_running = false;
        self.search_dirty = self.active_folder().is_some() && !self.project_query.trim().is_empty();
        self.search_last_edit = Instant::now();
    }
    pub(super) fn new_file(&mut self) {
        if self.projects.is_empty() {
            let index = self.ensure_other_files();
            self.switch_project(index);
        }
        if let Some(p) = self.projects.get_mut(self.active_project) {
            p.tabs.push(EditorTab::new(None, "Untitled".into()));
            let index = p.tabs.len() - 1;
            self.activate_tab(self.active_project, index);
        }
    }
    pub(super) fn switch_project(&mut self, index: usize) {
        if index < self.projects.len() {
            self.set_active_project_context(index);
            self.bind_current_pane();
            self.session_dirty = true;
        }
    }
}
