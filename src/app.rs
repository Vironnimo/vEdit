use crate::{
    design, editor, git, tasks,
    vim::{VimKey, VimMode, VimState, Yank},
    workspace,
};
use eframe::egui::{self, Align, Color32, FontId, Key, Layout, Margin, RichText, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

#[path = "agent_marks_control.rs"]
mod agent_marks_control;
#[path = "agent_marks_ui.rs"]
mod agent_marks_ui;
#[path = "app_menu.rs"]
mod app_menu;
#[path = "change_review.rs"]
mod change_review;
#[path = "context_actions.rs"]
mod context_actions;
#[path = "control.rs"]
mod control;
#[path = "disk_sync.rs"]
mod disk_sync;
#[path = "editor_feedback.rs"]
mod editor_feedback;
#[path = "editor_input.rs"]
mod editor_input;
#[path = "editor_preferences.rs"]
mod editor_preferences;
#[path = "ex_commands.rs"]
mod ex_commands;
#[path = "explorer_git.rs"]
mod explorer_git;
#[path = "explorer_navigation.rs"]
mod explorer_navigation;
#[path = "explorer_tree_ui.rs"]
mod explorer_tree_ui;
#[cfg(test)]
#[path = "explorer_visual_tests.rs"]
mod explorer_visual_tests;
#[path = "find.rs"]
mod find;
#[path = "git_gutter.rs"]
mod git_gutter;
#[path = "git_panel.rs"]
mod git_panel;
#[path = "indentation.rs"]
mod indentation;
#[path = "jump_list.rs"]
mod jump_list;
#[path = "keybindings.rs"]
mod keybindings;
#[path = "language_settings.rs"]
mod language_settings;
#[path = "language_support.rs"]
mod language_support;
#[path = "navigation.rs"]
mod navigation;
#[path = "notifications.rs"]
mod notifications;
#[path = "problems.rs"]
mod problems;
#[path = "project_launch.rs"]
mod project_launch;
#[path = "project_status.rs"]
mod project_status;
#[path = "project_tasks.rs"]
mod project_tasks;
#[path = "reopen.rs"]
mod reopen;
#[path = "save_state.rs"]
mod save_state;
#[path = "settings_navigation.rs"]
mod settings_navigation;
#[path = "sidebar_layout.rs"]
mod sidebar_layout;
#[path = "split.rs"]
mod split;
#[path = "tab_drag.rs"]
mod tab_drag;
#[path = "task_output_ui.rs"]
mod task_output_ui;
#[path = "workspace_ui.rs"]
mod workspace_ui;

use crate::palette::{
    ACCENT, BG, BLUE, BORDER, GREEN, HOVER, MUTED, ORANGE, PANEL, RED, SURFACE, TEXT,
};
const MAX_DOCUMENT: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SidebarView {
    #[default]
    #[serde(alias = "Explorer")]
    Explorer,
    #[serde(alias = "Search")]
    Search,
    #[serde(alias = "SourceControl")]
    SourceControl,
    #[serde(alias = "Review")]
    Review,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    keybindings: keybindings::Bindings,
    vim: bool,
    swap_jk: bool,
    autosave: bool,
    autosave_delay_ms: u64,
    font_size: f32,
    ui_scale: f32,
    line_height: f32,
    line_numbers: crate::cli::LineNumbers,
    highlight_current_line: bool,
    /// Lines added, modified or removed since HEAD beside the line numbers.
    git_gutter: bool,
    show_ignored_files: bool,
    explorer_preview_tabs: bool,
    editor_font: crate::cli::EditorFont,
    tab_width: usize,
    detect_indentation: bool,
    indent_with_tabs: bool,
    language_indent_widths: HashMap<String, usize>,
    language_indent_with_tabs: HashMap<String, bool>,
    word_wrap: bool,
    smooth_scroll: bool,
    leader_key: String,
    search_excludes: Vec<String>,
    language_servers: Vec<language_settings::LanguageServerSettings>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            keybindings: keybindings::Bindings::new(),
            vim: true,
            swap_jk: false,
            autosave: true,
            autosave_delay_ms: 850,
            font_size: 14.0,
            ui_scale: 1.0,
            line_height: 1.35,
            line_numbers: crate::cli::LineNumbers::default(),
            highlight_current_line: true,
            git_gutter: true,
            show_ignored_files: true,
            explorer_preview_tabs: true,
            editor_font: crate::cli::EditorFont::default(),
            tab_width: 4,
            detect_indentation: true,
            indent_with_tabs: false,
            language_indent_widths: indentation::default_widths(),
            language_indent_with_tabs: indentation::default_tab_styles(),
            word_wrap: false,
            smooth_scroll: false,
            leader_key: " ".into(),
            search_excludes: workspace::default_search_excludes(),
            language_servers: language_settings::defaults(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Session {
    projects: Vec<SessionProject>,
    active_project: usize,
    sidebar: Option<SidebarView>,
    sidebars: Option<sidebar_layout::SidebarSession>,
    autosave: Option<bool>,
    settings: Option<Settings>,
    layout: Option<split::SessionLayout>,
    bookmarks: Vec<crate::bookmarks::Bookmark>,
    agent_marks: crate::agent_marks::AgentMarks,
    /// The Find toolbar searches with regular expressions.
    find_regex: bool,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SessionProject {
    launch_selected: Option<String>,
    root: PathBuf,
    tasks: project_tasks::TaskConfiguration,
    open_files: Vec<PathBuf>,
    active_file: Option<PathBuf>,
    tabs: Vec<SessionTab>,
    active_tab_index: Option<usize>,
    layout: Option<split::SessionLayout>,
    git_split_ratio: Option<f32>,
    git_approved_parent: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SessionTab {
    path: Option<PathBuf>,
    title: String,
    recovery_path: Option<PathBuf>,
    cursor: usize,
    view: crate::cli::DocumentView,
    agent_unsaved: bool,
}

struct Project {
    launch: project_launch::LaunchState,
    run: project_tasks::ProjectRun,
    root: PathBuf,
    tasks: project_tasks::TaskConfiguration,
    tabs: Vec<EditorTab>,
    closed_tabs: Vec<reopen::ClosedTab>,
    active_tab: usize,
    /// Parked while another project is active; buffers always stay in this project.
    layout: Option<split::EditorLayout>,
    index: Option<Arc<workspace::WorkspaceIndex>>,
    explorer: Option<workspace::ExplorerTree>,
    indexing: bool,
    index_cancel: Arc<AtomicBool>,
    index_generation: u64,
    /// The latest index build, manual or file-watch driven; older results are dropped.
    index_request: u64,
    index_in_flight: bool,
    /// Files changed while a build ran; build once more when it finishes.
    index_again: bool,
    index_error: Option<String>,
    /// Follows external file creation, deletion and renames below `root`.
    watcher: Option<crate::fs_watch::ProjectWatcher>,
    watcher_starting: bool,
    watcher_error: Option<String>,
    rust_project: bool,
    git_root: Option<PathBuf>,
    git_parent_root: Option<PathBuf>,
    git_approved_parent: Option<PathBuf>,
    git_branch: Option<String>,
    git_branches: Vec<git::CommitRef>,
    git_tracking: Option<git::BranchInfo>,
    git_main_comparison: Result<Option<git::BranchComparison>, String>,
    git_changes: Vec<git::FileStatus>,
    /// Explorer decorations for `git_changes`, rebuilt on first use after a change.
    explorer_git: Option<explorer_git::ExplorerGitMarks>,
    git_stats: git::WorkingTreeStats,
    git_stats_error: Option<String>,
    recent_commits: Vec<git::CommitSummary>,
    git_history_snapshot: Option<git::HistorySnapshot>,
    git_history_loading: bool,
    git_history_has_more: bool,
    git_history_generation: u64,
    git_history_refresh_requested: bool,
    git_history_error: Option<String>,
    git_view: git_panel::GitViewState,
    change_review: change_review::ChangeReviewState,
    git_error: Option<String>,
    git_loading: bool,
    git_status_request: Option<u64>,
    git_refresh_pending: bool,
    last_git_refresh: Instant,
    git_indicator_count: Option<usize>,
    /// Branch, divergence and change totals from the latest full or status refresh.
    git_summary: Option<project_status::GitSummary>,
    /// HEAD and the status fingerprint of the latest full refresh; `None` until
    /// one completed. Gutter markers and change reviews compare them.
    git_head: Option<git::StatusHead>,
    git_fingerprint: Option<u64>,
    git_indicator_request: Option<u64>,
    last_git_indicator_refresh: Instant,
    commit_message: String,
    languages: HashMap<String, language_support::LanguageSession>,
    diagnostics: HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>,
}
impl Project {
    fn new(root: PathBuf) -> Self {
        Self {
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
/// A cached bracket pair: (revision, cursor, between-characters caret) and the
/// pair found for it.
type BracketMatch = ((u64, usize, bool), Option<(usize, usize)>);
struct EditorTab {
    id: u64,
    path: Option<PathBuf>,
    title: String,
    text: String,
    saved_text: String,
    dirty: bool,
    /// Explorer browsing may replace this buffer until it is edited or kept.
    preview_tab: bool,
    agent_unsaved: bool,
    recovery_path: PathBuf,
    recovery_revision: u64,
    recovery_pending: bool,
    recovery_cleanup_pending: Option<u64>,
    recovery_cleanup_retry_at: Option<Instant>,
    recovery_generation: u64,
    revision: u64,
    fingerprint: Option<workspace::FileFingerprint>,
    line_ending: workspace::LineEnding,
    bom: bool,
    loading: bool,
    saving: bool,
    error: Option<String>,
    /// A newer disk version while this buffer has unsaved edits. Only saving
    /// to the original path waits for the user's choice.
    disk_conflict: Option<disk_sync::DiskConflict>,
    /// The last clean reload from disk, shown until the next edit.
    disk_reload: Option<disk_sync::DiskReload>,
    last_edit: Instant,
    vim: VimState,
    selection: Range<usize>,
    pending_selection: Option<Range<usize>>,
    focus: bool,
    scroll_to_cursor: bool,
    center_on_reveal: bool,
    /// Vim `zt` / `zz` / `zb`: place the cursor line at this edge on the next reveal.
    scroll_align: Option<crate::vim::ScrollAlign>,
    /// A Vim status message waiting for the status bar, such as an unsupported command.
    vim_message: Option<String>,
    /// The revision and text that other panes' positions refer to, kept only
    /// while another pane holds one; see `shift_shared_document_positions`.
    pane_baseline: Option<(u64, String)>,
    /// The bracket pair at the caret, computed once per (revision, cursor, caret kind).
    bracket_match: Option<BracketMatch>,
    pending_navigation_feedback: bool,
    feedback: Option<editor_feedback::Highlight>,
    lines: crate::render::LineIndex,
    indentation_cache: indentation::Cache,
    agent_view: crate::cli::DocumentView,
    annotations: Vec<crate::cli::Annotation>,
    annotations_revision: Option<u64>,
    pending_agent_location: Option<crate::cli::Location>,
    agent_location_error: Option<String>,
}
impl EditorTab {
    fn new(path: Option<PathBuf>, title: String) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = (u64::from(std::process::id()) << 32) | NEXT.fetch_add(1, Ordering::Relaxed);
        let recovery_path = recovery_directory().join(format!("{}-{id}.txt", std::process::id()));
        Self {
            id,
            path,
            title,
            text: String::new(),
            saved_text: String::new(),
            dirty: false,
            preview_tab: false,
            agent_unsaved: false,
            recovery_path,
            recovery_revision: 0,
            recovery_pending: false,
            recovery_cleanup_pending: None,
            recovery_cleanup_retry_at: None,
            recovery_generation: 0,
            revision: 0,
            fingerprint: None,
            line_ending: workspace::LineEnding::Lf,
            bom: false,
            loading: false,
            saving: false,
            error: None,
            disk_conflict: None,
            disk_reload: None,
            last_edit: Instant::now(),
            vim: VimState::new(),
            selection: 0..0,
            pending_selection: Some(0..0),
            focus: true,
            scroll_to_cursor: false,
            center_on_reveal: false,
            scroll_align: None,
            vim_message: None,
            pane_baseline: None,
            bracket_match: None,
            pending_navigation_feedback: false,
            feedback: None,
            lines: crate::render::LineIndex::default(),
            indentation_cache: indentation::Cache::default(),
            agent_view: crate::cli::DocumentView::default(),
            annotations: Vec::new(),
            annotations_revision: None,
            pending_agent_location: None,
            agent_location_error: None,
        }
    }
    fn changed(&mut self) {
        self.preview_tab = false;
        self.revision += 1;
        self.dirty = self.text != self.saved_text;
        self.clear_agent_draft_when_saved();
        self.last_edit = Instant::now();
    }
    /// An agent draft ends once the buffer matches its saved text again, e.g.
    /// after Undo. Later edits belong to whoever makes them.
    fn clear_agent_draft_when_saved(&mut self) {
        if !self.dirty {
            self.agent_unsaved = false;
        }
    }
    fn apply(&mut self, edit: editor::EditResult) {
        if edit.text != self.text {
            self.vim.begin_external_edit(&self.text);
        }
        self.text = edit.text;
        self.selection = edit.selection.clone();
        self.pending_selection = Some(edit.selection);
        self.changed();
        self.focus = true;
    }
    /// The bracket at the caret and its partner. The search looks at most
    /// `BRACKET_SCAN` characters either way and skips very large documents.
    fn bracket_pair(&mut self, vim: bool) -> Option<(usize, usize)> {
        const BRACKET_SCAN: usize = 20_000;
        const BRACKET_MAX_DOCUMENT: usize = 2 * 1024 * 1024;
        // A Vim block cursor covers a character; a text caret sits between two.
        let between = !vim || self.vim.mode == VimMode::Insert;
        let key = (self.revision, self.vim.cursor, between);
        if let Some((cached, pair)) = self.bracket_match
            && cached == key
        {
            return pair;
        }
        let pair = if self.loading || self.text.len() > BRACKET_MAX_DOCUMENT {
            None
        } else {
            let offset = self.vim.cursor.saturating_sub(BRACKET_SCAN);
            let window: Vec<char> = self
                .text
                .chars()
                .skip(offset)
                .take(2 * BRACKET_SCAN + 1)
                .collect();
            crate::vim::bracket_pair(&window, offset, self.vim.cursor, between)
        };
        self.bracket_match = Some((key, pair));
        pair
    }
    fn set_cursor(&mut self, range: Range<usize>) {
        self.vim.set_cursor(&self.text, range.start);
        // Explicit navigation may target the boundary after the last character.
        self.vim.cursor = range.start;
        self.selection = range.clone();
        self.pending_selection = Some(range);
        self.focus = true;
        self.scroll_to_cursor = true;
    }
    fn apply_line_move(&mut self, edit: editor::EditResult) {
        if edit.text == self.text {
            return;
        }
        let navigation = self.vim.navigation();
        let start = self
            .vim
            .selection(&self.text)
            .unwrap_or_else(|| self.selection.clone())
            .start;
        let delta = edit.selection.start as isize - start as isize;
        self.apply(edit);
        self.vim.notify_external_text_change();
        self.vim
            .restore_navigation(&self.text, &navigation.shifted(delta));
        self.scroll_to_cursor = true;
    }
}
struct GitSnapshot {
    root: Option<PathBuf>,
    parent_root: Option<PathBuf>,
    branch: Option<String>,
    tracking: Option<git::BranchInfo>,
    main_comparison: Result<Option<git::BranchComparison>, String>,
    files: Vec<git::FileStatus>,
    stats: git::WorkingTreeStats,
    stats_error: Option<String>,
    /// HEAD as the status read reported it; `None` when Git did not say.
    head: Option<git::StatusHead>,
    /// [`git::status_fingerprint`] of this read.
    fingerprint: u64,
    history: Result<GitHistoryRefresh, String>,
}
struct GitHistoryRefresh {
    snapshot: git::HistorySnapshot,
    page: Option<git::HistoryPage>,
}

/// Read one project's repository state on a worker for `refresh_git`.
fn read_git_snapshot(
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
            stats: git::WorkingTreeStats::default(),
            stats_error: None,
            head: None,
            fingerprint: 0,
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
                    &snapshot,
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
        files: status.files,
        stats,
        stats_error,
        head,
        fingerprint,
        history,
    })
}
struct ActiveScreenJump {
    tab_id: u64,
    pane_id: u64,
    revision: u64,
    state: crate::screen_jump::ScreenJump,
    glyphs: Vec<crate::screen_jump::VisibleGlyph>,
}
type BookmarkMarks = Vec<(usize, u64, String)>;
type BookmarkLineCache = HashMap<u64, (u64, BookmarkMarks)>;
enum WorkerResult {
    ExplorerDirectory {
        root: PathBuf,
        path: PathBuf,
        generation: u64,
        result: Result<workspace::ExplorerDirectory, String>,
    },
    Index {
        root: PathBuf,
        generation: u64,
        request: u64,
        result: Result<workspace::WorkspaceIndex, String>,
    },
    FileWatcher {
        root: PathBuf,
        result: Result<crate::fs_watch::ProjectWatcher, String>,
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
        message: String,
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
}
impl WorkerResult {
    /// A periodic disk check that found nothing waits for the next scheduled
    /// frame, which also starts the following check. Every other result is
    /// shown right away.
    fn wakes_window(&self) -> bool {
        !matches!(self, WorkerResult::DiskChanges(changes) if changes.is_empty())
    }
}
#[derive(Clone)]
enum GitAction {
    Initialize,
    StageAll,
    UnstageAll,
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    Commit(String),
    Fetch,
    Pull,
    Push,
    SwitchBranch(String),
}
impl GitAction {
    fn label(&self) -> &'static str {
        match self {
            Self::Initialize => "Initialize repository",
            Self::StageAll | Self::Stage(_) => "Stage changes",
            Self::UnstageAll | Self::Unstage(_) => "Unstage changes",
            Self::Commit(_) => "Create commit",
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
            Self::SwitchBranch(_) => "Switch branch",
        }
    }
    fn run(&self, root: &Path) -> Result<String, String> {
        match self {
            Self::Initialize => git::initialize(root).map(|_| "Repository initialized".into()),
            Self::StageAll => git::stage_all(root).map(|_| "Changes staged".into()),
            Self::UnstageAll => git::unstage_all(root).map(|_| "Changes unstaged".into()),
            Self::Stage(paths) => git::stage_paths(root, paths).map(|_| "Changes staged".into()),
            Self::Unstage(paths) => {
                git::unstage_paths(root, paths).map(|_| "Changes unstaged".into())
            }
            Self::Commit(message) => git::commit(root, message),
            Self::Fetch => git::fetch(root),
            Self::Pull => git::pull(root),
            Self::Push => git::push(root),
            Self::SwitchBranch(target) => git::switch_branch(root, target),
        }
        .map_err(|e| e.to_string())
    }
}
#[derive(Clone, Copy)]
enum CloseAction {
    Tab(u64),
    PaneTab { pane_id: u64, id: u64 },
    Project(usize),
    Exit,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Palette {
    Files,
    Commands,
    Line,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogPurpose {
    Projects,
    Files,
    SaveAs(u64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Picker {
    id: u64,
    purpose: DialogPurpose,
}
struct DialogSelection {
    paths: Vec<PathBuf>,
    overwrite: Option<workspace::FileFingerprint>,
}
struct CompletionPopup {
    tab_id: u64,
    version: i32,
    cursor: usize,
    items: Vec<crate::lsp::CompletionItem>,
    selected: usize,
}
pub struct VEditApp {
    projects: Vec<Project>,
    active_project: usize,
    workspace_ui: workspace_ui::WorkspaceUiState,
    explorer_navigation: explorer_navigation::ExplorerNavigationState,
    sidebar: SidebarView,
    sidebar_visible: bool,
    sidebar_layout: sidebar_layout::SidebarLayout,
    settings: Settings,
    bookmarks: crate::bookmarks::BookmarkStore,
    bookmark_lines: BookmarkLineCache,
    pending_bookmark_jump: Option<(u64, crate::bookmarks::Bookmark)>,
    navigation: navigation::NavigationState,
    screen_jump: Option<ActiveScreenJump>,
    sender: Sender<WorkerResult>,
    receiver: Receiver<WorkerResult>,
    ctx: egui::Context,
    status: Option<(String, bool, Instant)>,
    palette: Option<Palette>,
    palette_query: String,
    palette_selected: usize,
    palette_focus: bool,
    find_visible: bool,
    replace_visible: bool,
    find_text: String,
    replace_text: String,
    find_case: bool,
    find_word: bool,
    /// Find toolbar queries are Rust regular expressions; kept in the session.
    find_regex: bool,
    find_focus: bool,
    find_match: Option<usize>,
    find: find::FindState,
    project_query: String,
    project_search_focus: bool,
    project_search: Option<workspace::SearchResults>,
    project_search_error: Option<String>,
    search_running: bool,
    search_generation: u64,
    search_cancel: Arc<AtomicBool>,
    search_last_edit: Instant,
    search_dirty: bool,
    picker: Option<Picker>,
    context_file_dialog: Option<context_actions::FileActionDialog>,
    context_paste: Option<context_actions::PendingPaste>,
    context_file_jobs: context_actions::FileJobRegistry,
    native_dialog_builder: Option<rfd::FileDialog>,
    settings_open: bool,
    interface_zoom_draft: Option<f32>,
    keybinding_ui: keybindings::State,
    project_tasks_settings_requested: bool,
    project_tasks_interpreter_pending: Option<usize>,
    language_settings_drafts: HashMap<String, language_settings::LanguageSettingsDraft>,
    agent_marks: crate::agent_marks::AgentMarks,
    agent_marks_ui: agent_marks_ui::AgentMarksUi,
    applied_editor_font: crate::cli::EditorFont,
    save_as: Option<u64>,
    save_queue: std::collections::VecDeque<u64>,
    /// Save As destinations confirmed while that document was still saving.
    deferred_save_as: Vec<(u64, PathBuf, Option<workspace::FileFingerprint>)>,
    close_action: Option<CloseAction>,
    close_after_save: bool,
    exit_ready: bool,
    git_busy: Option<PathBuf>,
    git_window_was_active: Option<bool>,
    git_indicator_requests: HashMap<u64, PathBuf>,
    repositories: git_panel::repositories::RepositoryOverview,
    diff: Option<(String, String)>,
    diff_view: crate::git_diff_view::DiffView,
    /// The repository whose paths the diff dialog shows.
    diff_repository: Option<PathBuf>,
    /// Committed versions, gutter markers and the open change peek.
    git_gutter: git_gutter::GitGutterState,
    diff_generation: u64,
    diff_loading: bool,
    diff_error: Option<String>,
    /// The document comparison shown in the diff dialog instead of a Git patch.
    comparison: Option<disk_sync::Comparison>,
    output_visible: bool,
    command: String,
    task_output_cache: task_output_ui::TaskOutputCache,
    register: Option<Yank>,
    yank_register: Option<Yank>,
    editor_focused: bool,
    editor_rect: Option<egui::Rect>,
    editor_cursor_rect: Option<egui::Rect>,
    vim_command: Option<String>,
    session_dirty: bool,
    last_session_save: Instant,
    recovery_session_path: PathBuf,
    recovery_writer: crate::recovery::RecoveryWriter,
    completions: Option<CompletionPopup>,
    completion_request: Option<u64>,
    definition_request: Option<(u64, u64)>,
    definition_origin: Option<(u64, u64, usize, egui::Id)>,
    pending_location: Option<(u64, crate::lsp::Position)>,
    /// Projects follow external file changes; tests opt in explicitly.
    file_watching: bool,
    /// A back/forward destination waiting for its document to load.
    pending_jump: Option<(u64, jump_list::Jump)>,
    /// The next observed move comes from back/forward and is not recorded.
    jump_navigating: bool,
    problem_cache: problems::ProblemCache,
    problems_visible: bool,
    last_disk_check: Instant,
    disk_check_running: bool,
    syntax: crate::render::SyntaxCache,
    text_statistics: crate::text_statistics::Statistics,
    markdown: crate::markdown::MarkdownCache,
    control: Option<crate::cli::Server>,
    /// The working directory of the CLI request being applied, if it sent one.
    control_caller: Option<PathBuf>,
    notifications: notifications::Inbox,
    review: Option<crate::cli::ReviewSummary>,
    tab_strip_active: Option<u64>,
    editor_layout: split::EditorLayout,
    region_focus: split::RegionFocusState,
    layout_project: Option<PathBuf>,
    next_pane_id: u64,
    tab_drag: tab_drag::TabDragState,
    render_pane: u64,
    rendering_panes: bool,
}
impl VEditApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let stored: Session = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        let session: Session = fs::read(recovery_directory().join("session.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(stored);
        let mut app = Self::empty(cc.egui_ctx.clone());
        app.native_dialog_builder = Some(rfd::FileDialog::new().set_parent(cc));
        app.settings = session.settings.unwrap_or_default();
        app.settings.show_ignored_files = true;
        app.settings.normalize_appearance();
        keybindings::migrate_later_defaults(&mut app.settings.keybindings);
        app.ctx.set_zoom_factor(app.settings.ui_scale);
        app.apply_editor_font();
        app.settings.leader_key = crate::cli::normalize_leader_key(&app.settings.leader_key)
            .unwrap_or_else(|_| " ".into());
        app.bookmarks = crate::bookmarks::BookmarkStore::from_items(session.bookmarks);
        app.agent_marks = session.agent_marks;
        app.find_regex = session.find_regex;
        if let Some(autosave) = session.autosave {
            app.settings.autosave = autosave;
        }
        app.restore_session_workspace(session.projects, session.active_project, session.layout);
        app.restore_sidebars(session.sidebars, session.sidebar);
        for index in 0..app.projects.len() {
            app.index_project(index);
        }
        app.session_dirty = false;
        for argument in std::env::args_os().skip(1) {
            app.open_argument(PathBuf::from(argument));
        }
        app.start_control();
        app
    }
    fn restore_session_workspace(
        &mut self,
        projects: Vec<SessionProject>,
        active_project: usize,
        mut layout: Option<split::SessionLayout>,
    ) {
        let layouts: Vec<_> = projects
            .iter()
            .map(|project| project.layout.clone())
            .collect();
        let mapping = self.restore_session_projects(projects);
        let restored_active = mapping.get(active_project).copied().flatten().unwrap_or(0);
        if let Some(layout) = &mut layout {
            layout.remap_projects(&mapping);
        }
        let restored_layouts = layouts
            .into_iter()
            .enumerate()
            .filter_map(|(original, saved)| {
                let project = mapping.get(original).copied().flatten()?;
                let saved = saved
                    .map(|mut saved| {
                        saved.remap_projects(&mapping);
                        saved
                    })
                    .or_else(|| layout.clone());
                Some((project, saved))
            })
            .collect::<Vec<_>>();
        self.restore_project_layouts(restored_layouts, restored_active);
    }
    fn restore_session_projects(&mut self, entries: Vec<SessionProject>) -> Vec<Option<usize>> {
        let mut mapping = vec![None; entries.len()];
        for (original, entry) in entries.into_iter().enumerate() {
            let root = entry.root;
            if !root.is_dir()
                && !entry.tabs.iter().any(|tab| {
                    tab.recovery_path
                        .as_ref()
                        .is_some_and(|path| path.is_file())
                })
            {
                continue;
            }
            let mut project = Project::new(root.clone());
            project.tasks = entry.tasks;
            project.launch.selected = entry.launch_selected;
            project.git_approved_parent = entry.git_approved_parent;
            project.git_view.split_ratio = entry
                .git_split_ratio
                .filter(|ratio| ratio.is_finite())
                .map(|ratio| ratio.clamp(0.0, 1.0));
            let saved_tabs = if entry.tabs.is_empty() {
                entry
                    .open_files
                    .into_iter()
                    .map(|path| SessionTab {
                        title: file_title(&path),
                        path: Some(path),
                        ..Default::default()
                    })
                    .collect()
            } else {
                entry.tabs
            };
            for saved in saved_tabs {
                let mut tab = EditorTab::new(saved.path.clone(), saved.title);
                tab.loading = true;
                tab.vim.cursor = saved.cursor;
                tab.agent_view = saved.view;
                tab.agent_unsaved = saved.agent_unsaved;
                if let Some(recovery) = &saved.recovery_path {
                    tab.recovery_path = recovery.clone();
                }
                self.load_tab(tab.id, saved.path, saved.recovery_path);
                project.tabs.push(tab);
            }
            project.active_tab = entry
                .active_tab_index
                .unwrap_or_else(|| {
                    project
                        .tabs
                        .iter()
                        .position(|t| t.path == entry.active_file)
                        .unwrap_or(0)
                })
                .min(project.tabs.len().saturating_sub(1));
            mapping[original] = Some(self.projects.len());
            self.projects.push(project);
        }
        mapping
    }
    fn open_argument(&mut self, input: PathBuf) {
        let path = fs::canonicalize(&input).unwrap_or(input);
        if path.is_dir() {
            self.add_project(path);
        } else if path.is_file() {
            if let Some(index) = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, project)| path.starts_with(&project.root))
                .max_by_key(|(_, project)| project.root.components().count())
                .map(|(index, _)| index)
            {
                self.switch_project(index);
                self.open_file(path);
            } else if let Some(parent) = path.parent() {
                self.add_project(parent.to_owned());
                self.open_file(path);
            }
        }
    }
    fn empty(ctx: egui::Context) -> Self {
        set_style(&ctx);
        let (sender, receiver) = mpsc::channel();
        Self {
            projects: Vec::new(),
            active_project: 0,
            workspace_ui: workspace_ui::WorkspaceUiState::default(),
            explorer_navigation: explorer_navigation::ExplorerNavigationState::default(),
            sidebar: SidebarView::Explorer,
            sidebar_visible: true,
            sidebar_layout: sidebar_layout::SidebarLayout::default(),
            settings: Settings::default(),
            applied_editor_font: crate::cli::EditorFont::default(),
            bookmarks: crate::bookmarks::BookmarkStore::default(),
            bookmark_lines: HashMap::new(),
            pending_bookmark_jump: None,
            navigation: navigation::NavigationState::default(),
            screen_jump: None,
            sender,
            receiver,
            ctx,
            status: None,
            palette: None,
            palette_query: String::new(),
            palette_selected: 0,
            palette_focus: false,
            find_visible: false,
            replace_visible: false,
            find_text: String::new(),
            replace_text: String::new(),
            find_case: false,
            find_word: false,
            find_regex: false,
            find_focus: false,
            find_match: None,
            find: find::FindState::default(),
            project_query: String::new(),
            project_search_focus: false,
            project_search: None,
            project_search_error: None,
            search_running: false,
            search_generation: 0,
            search_cancel: Arc::new(AtomicBool::new(false)),
            search_last_edit: Instant::now(),
            search_dirty: false,
            picker: None,
            context_file_dialog: None,
            context_paste: None,
            context_file_jobs: context_actions::FileJobRegistry::default(),
            native_dialog_builder: None,
            settings_open: false,
            interface_zoom_draft: None,
            keybinding_ui: keybindings::State::default(),
            project_tasks_settings_requested: false,
            project_tasks_interpreter_pending: None,
            language_settings_drafts: HashMap::new(),
            agent_marks: crate::agent_marks::AgentMarks::default(),
            agent_marks_ui: agent_marks_ui::AgentMarksUi::default(),
            save_as: None,
            save_queue: std::collections::VecDeque::new(),
            deferred_save_as: Vec::new(),
            close_action: None,
            close_after_save: false,
            exit_ready: false,
            git_busy: None,
            git_window_was_active: None,
            git_indicator_requests: HashMap::new(),
            repositories: git_panel::repositories::RepositoryOverview::default(),
            diff: None,
            diff_view: crate::git_diff_view::DiffView::default(),
            diff_repository: None,
            git_gutter: git_gutter::GitGutterState::default(),
            diff_generation: 0,
            diff_loading: false,
            diff_error: None,
            comparison: None,
            output_visible: false,
            command: String::new(),
            task_output_cache: task_output_ui::TaskOutputCache::default(),
            register: None,
            yank_register: None,
            editor_focused: false,
            editor_rect: None,
            editor_cursor_rect: None,
            vim_command: None,
            session_dirty: false,
            last_session_save: Instant::now(),
            recovery_session_path: recovery_directory().join("session.json"),
            recovery_writer: crate::recovery::RecoveryWriter::default(),
            completions: None,
            completion_request: None,
            definition_request: None,
            definition_origin: None,
            pending_location: None,
            file_watching: !cfg!(test),
            pending_jump: None,
            jump_navigating: false,
            problem_cache: Default::default(),
            problems_visible: false,
            last_disk_check: Instant::now(),
            disk_check_running: false,
            syntax: crate::render::SyntaxCache::new(),
            text_statistics: crate::text_statistics::Statistics::default(),
            markdown: crate::markdown::MarkdownCache::default(),
            control: None,
            control_caller: None,
            notifications: notifications::Inbox::default(),
            review: None,
            tab_strip_active: None,
            editor_layout: split::EditorLayout::default(),
            region_focus: split::RegionFocusState::default(),
            layout_project: None,
            next_pane_id: 2,
            tab_drag: tab_drag::TabDragState::default(),
            render_pane: 1,
            rendering_panes: false,
        }
    }
    fn worker(&self, work: impl FnOnce() -> WorkerResult + Send + 'static) {
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = work();
            let wake = result.wakes_window();
            let _ = sender.send(result);
            if wake {
                ctx.request_repaint();
            }
        });
    }
    fn project(&self) -> Option<&Project> {
        self.projects.get(self.active_project)
    }
    fn tab(&self) -> Option<&EditorTab> {
        if self.pane_has_empty_workspace() {
            return None;
        }
        let p = self.project()?;
        p.tabs.get(p.active_tab)
    }
    fn tab_mut(&mut self) -> Option<&mut EditorTab> {
        if self.pane_has_empty_workspace() {
            return None;
        }
        let p = self.projects.get_mut(self.active_project)?;
        p.tabs.get_mut(p.active_tab)
    }
    fn tab_by_id_mut(&mut self, id: u64) -> Option<&mut EditorTab> {
        self.projects
            .iter_mut()
            .flat_map(|p| &mut p.tabs)
            .find(|t| t.id == id)
    }
    fn status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some((text.into(), error, Instant::now()));
    }
    fn add_project(&mut self, path: PathBuf) {
        let root = fs::canonicalize(&path).unwrap_or(path);
        if let Some(index) = self.projects.iter().position(|p| p.root == root) {
            self.switch_project(index);
            return;
        }
        self.projects.push(Project::new(root));
        self.switch_project(self.projects.len() - 1);
        self.index_project(self.active_project);
        self.session_dirty = true;
    }
    fn index_project(&mut self, index: usize) {
        let Some(p) = self.projects.get_mut(index) else {
            return;
        };
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
            generation,
            request,
        });
        self.request_explorer_directory(index, explorer_root);
        self.ensure_file_watcher(index);
    }
    fn load_tab(&self, id: u64, path: Option<PathBuf>, recovery: Option<PathBuf>) {
        self.worker(move || {
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
        });
    }
    fn open_file(&mut self, path: PathBuf) {
        self.open_file_in_mode(path, false);
    }
    fn open_explorer_file(&mut self, path: PathBuf, keep_open: bool) {
        self.open_file_in_mode(path, self.settings.explorer_preview_tabs && !keep_open);
    }
    fn open_file_in_mode(&mut self, path: PathBuf, preview: bool) {
        if self.context_path_busy(&path) {
            self.status("Wait for the file rename to finish before opening it", true);
            return;
        }
        if let Some(index) = self
            .project()
            .and_then(|p| p.tabs.iter().position(|t| t.path.as_ref() == Some(&path)))
        {
            let id = self.projects[self.active_project].tabs[index].id;
            if !preview || !self.current_pane_tab_ids().contains(&id) {
                self.keep_tab_open(id);
            }
            self.activate_tab(self.active_project, index);
            return;
        }
        // Explorer and quick-open paths already have the canonical workspace root.
        let path = if self.project().is_some_and(|p| path.starts_with(&p.root))
            && !path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            path
        } else {
            fs::canonicalize(&path).unwrap_or(path)
        };
        if let Some(index) = self
            .project()
            .and_then(|p| p.tabs.iter().position(|t| t.path.as_ref() == Some(&path)))
        {
            let id = self.projects[self.active_project].tabs[index].id;
            if !preview || !self.current_pane_tab_ids().contains(&id) {
                self.keep_tab_open(id);
            }
            self.activate_tab(self.active_project, index);
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
        self.load_tab(id, Some(path), None);
    }
    fn keep_tab_open(&mut self, id: u64) {
        if let Some(tab) = self.tab_by_id_mut(id) {
            tab.preview_tab = false;
        }
    }
    fn set_explorer_preview_tabs(&mut self, enabled: bool) {
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
    fn activate_tab(&mut self, project_index: usize, tab_index: usize) {
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
    }
    fn set_active_project_context(&mut self, index: usize) {
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
    fn clear_project_search_for_root_change(&mut self, previous_root: Option<PathBuf>) {
        if previous_root.as_deref() == self.project().map(|project| project.root.as_path()) {
            return;
        }
        self.project_search = None;
        self.project_search_error = None;
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_running = false;
        self.search_dirty = self.project().is_some() && !self.project_query.trim().is_empty();
        self.search_last_edit = Instant::now();
    }
    fn new_file(&mut self) {
        if let Some(p) = self.projects.get_mut(self.active_project) {
            p.tabs.push(EditorTab::new(None, "Untitled".into()));
            let index = p.tabs.len() - 1;
            self.activate_tab(self.active_project, index);
        } else {
            self.pick_project();
        }
    }
    fn pick_file(&mut self) {
        let directory = self
            .tab()
            .and_then(|tab| tab.path.as_deref())
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.project().map(|project| project.root.clone()));
        self.start_native_dialog(DialogPurpose::Files, directory, None);
    }
    fn save_tab(&mut self, id: u64, target: Option<PathBuf>) {
        self.save_tab_expected(id, target, None);
    }
    fn save_tab_expected(
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
        tab.saving = true;
        tab.error = None;
        self.worker(move || {
            let result = (|| {
                if expected.is_none()
                    && let Some(parent) = path.parent()
                {
                    fs::create_dir_all(parent).map_err(|e| (e.to_string(), false))?;
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
                Err((_, true)) if own_path => workspace::read_document(&path, MAX_DOCUMENT).ok(),
                _ => None,
            };
            let result = result.map_err(|(error, _)| error);
            let path = if result.is_ok() {
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
        });
    }
    fn save_current(&mut self) {
        if let Some(tab) = self.tab() {
            let id = tab.id;
            self.save_tab(id, None);
        }
    }
    fn save_all(&mut self) {
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
    fn request_close(&mut self, action: CloseAction) {
        if self.close_has_dirty(action) {
            self.close_action = Some(action);
            self.close_after_save = false;
        } else {
            self.perform_close(action);
        }
    }
    fn close_has_dirty(&self, action: CloseAction) -> bool {
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
    fn perform_close(&mut self, action: CloseAction) {
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
                        let _ = self
                            .recovery_writer
                            .remove(&t.recovery_path, self.recovery_writer.stamp());
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
        }
        self.session_dirty = true;
    }
    fn remove_closed_document(&mut self, id: u64) {
        for project in &mut self.projects {
            if let Some(index) = project.tabs.iter().position(|tab| tab.id == id) {
                let tab = project.tabs.remove(index);
                self.agent_marks.detach_document(id);
                self.markdown.forget_document(id);
                self.git_gutter.forget(id);
                let _ = self
                    .recovery_writer
                    .remove(&tab.recovery_path, self.recovery_writer.stamp());
                if project.active_tab > index {
                    project.active_tab -= 1;
                }
                project.active_tab = project.active_tab.min(project.tabs.len().saturating_sub(1));
                break;
            }
        }
    }
    fn control_close_pane_tab(&mut self, pane_id: u64, id: u64) -> Result<(), String> {
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
    fn poll(&mut self) {
        self.poll_launches();
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                WorkerResult::ContextFile(result) => self.context_file_result(result),
                WorkerResult::ContextFilePrepared(result) => self.context_file_prepared(result),
                WorkerResult::ExplorerDirectory {
                    root,
                    path,
                    generation,
                    result,
                } => {
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
                WorkerResult::Index {
                    root,
                    generation,
                    request,
                    result,
                } => {
                    if let Some(index) = self.projects.iter().position(|p| {
                        p.root == root
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
                WorkerResult::FileWatcher { root, result } => {
                    self.file_watcher_started(root, result)
                }
                WorkerResult::Loaded {
                    id,
                    result,
                    recovery,
                } => {
                    self.find.invalidate(id);
                    let ctx = self.ctx.clone();
                    let preserve_scroll = self.has_saved_document_scroll(id);
                    let next_recovery_name = format!(
                        "{}-{id}-after-unreadable-{}.txt",
                        std::process::id(),
                        self.recovery_writer.stamp()
                    );
                    if let Some(tab) = self.tab_by_id_mut(id) {
                        // Only the active load may publish. All load entry
                        // points serialize requests; a late duplicate must not
                        // overwrite a document that has become editable.
                        if !tab.loading || tab.saving {
                            continue;
                        }
                        if tab.dirty {
                            tab.loading = false;
                            tab.error = Some(
                                "The reload was skipped because the buffer has new edits".into(),
                            );
                            continue;
                        }
                        tab.loading = false;
                        tab.lines.invalidate();
                        tab.indentation_cache.invalidate();
                        match result {
                            Ok(doc) => {
                                if tab.text != doc.text {
                                    let replacing = tab.fingerprint.is_some()
                                        || !tab.text.is_empty()
                                        || tab.revision > 0;
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
                                    let disk_version = tab.fingerprint.clone().map(|fingerprint| {
                                        workspace::LoadedDocument {
                                            text: tab.saved_text.clone(),
                                            line_ending: tab.line_ending,
                                            bom: tab.bom,
                                            mixed_line_endings: tab.line_ending
                                                == workspace::LineEnding::Mixed,
                                            fingerprint,
                                        }
                                    });
                                    let disk_matches =
                                        doc.baseline.as_ref().is_some_and(|baseline| {
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
                                    }
                                    tab.recovery_revision = tab.revision;
                                }
                                Err(error) => {
                                    let retained = crate::paths::display(&tab.recovery_path);
                                    tab.recovery_path =
                                        tab.recovery_path.with_file_name(next_recovery_name);
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
                }
                WorkerResult::Saved {
                    id,
                    text,
                    path,
                    result,
                    conflict,
                } => {
                    let mut message = None;
                    let recovery_writer = self.recovery_writer.clone();
                    let mut new_document = false;
                    let mut observed = None;
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
                                    tab.recovery_generation =
                                        tab.recovery_generation.wrapping_add(1);
                                    tab.recovery_revision = 0;
                                }
                                if !tab.dirty {
                                    tab.agent_unsaved = false;
                                }
                                tab.error = None;
                                if !tab.dirty {
                                    let _ = recovery_writer
                                        .remove(&tab.recovery_path, recovery_writer.stamp());
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
                    if let Some((message, error)) = message {
                        self.status(message, error);
                        if error {
                            self.close_after_save = false;
                            self.save_queue.clear();
                        }
                    }
                    self.session_dirty = true;
                    if (new_document
                        || (saved && path.file_name().is_some_and(|name| name == ".gitignore")))
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
                WorkerResult::Recovered {
                    id,
                    revision,
                    generation,
                    result,
                } => {
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
                WorkerResult::RecoveryRemoved {
                    id,
                    revision,
                    generation,
                    result,
                } => {
                    if let Some(tab) = self.tab_by_id_mut(id) {
                        if tab.recovery_cleanup_pending == Some(revision) {
                            tab.recovery_cleanup_pending = None;
                        }
                        if result.is_ok() && generation == tab.recovery_generation {
                            tab.recovery_revision = tab.recovery_revision.max(revision);
                            tab.recovery_cleanup_retry_at = None;
                        } else {
                            tab.recovery_cleanup_retry_at =
                                Some(Instant::now() + Duration::from_millis(850));
                        }
                    }
                    if let Err(error) = result {
                        self.status(error, true);
                    }
                    self.session_dirty = true;
                }
                WorkerResult::GitStatus {
                    root,
                    request_id,
                    result,
                } => {
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
                                p.git_stats = s.stats;
                                p.git_stats_error = s.stats_error;
                                match s.history {
                                    Ok(history) => {
                                        p.git_branches = history.snapshot.refs.clone();
                                        if let Some(page) = history.page {
                                            static NEXT_HISTORY: AtomicU64 = AtomicU64::new(1);
                                            p.git_history_generation =
                                                NEXT_HISTORY.fetch_add(1, Ordering::Relaxed);
                                            p.git_history_loading = false;
                                            p.git_history_has_more = page.has_more;
                                            p.git_view.preserve_history_scroll(
                                                &p.recent_commits,
                                                &page.commits,
                                            );
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
                WorkerResult::GitCommitStats {
                    root,
                    repository,
                    hash,
                    request_id,
                    result,
                } => {
                    let active_root = self.project().map(|project| project.root.clone());
                    if let Some(project) = self
                        .projects
                        .iter_mut()
                        .find(|project| project.root == root)
                    {
                        let key = (repository.clone(), hash);
                        if project.git_view.hover_stats_pending.get(&key) != Some(&request_id) {
                            continue;
                        }
                        project.git_view.hover_stats_pending.remove(&key);
                        let current_repository = project.git_root.as_ref().unwrap_or(&project.root);
                        if current_repository != &repository || active_root.as_ref() != Some(&root)
                        {
                            continue;
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
                WorkerResult::GitHistory {
                    root,
                    generation,
                    offset,
                    result,
                } => {
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
                WorkerResult::GitAction {
                    reply,
                    root,
                    message,
                    result,
                } => {
                    self.git_busy = None;
                    if let Some(reply) = reply {
                        let response = match &result {
                            Ok(branch) => crate::cli::Response::success(
                                serde_json::json!({"repository":crate::paths::display(&root),"branch":branch,"completed":true}),
                            ),
                            Err(error) => {
                                crate::cli::Response::failure("git_switch_failed", error.clone())
                            }
                        };
                        let _ = reply.send(response);
                    }
                    match result {
                        Ok(_) => {
                            self.status(format!("{message} completed"), false);
                            // Commits, fetches and branch switches move other
                            // worktrees' upstream divergence and listings too;
                            // staging stays inside one worktree.
                            if !matches!(message.as_str(), "Stage changes" | "Unstage changes") {
                                self.repositories.request_refresh();
                            }
                            let indices: Vec<_> = self
                                .projects
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| {
                                    p.git_root.as_ref() == Some(&root) || p.root == root
                                })
                                .map(|(index, _)| index)
                                .collect();
                            for i in indices {
                                if message == "Create commit" {
                                    self.projects[i].commit_message.clear();
                                }
                                if message == "Switch branch" {
                                    self.projects[i].git_history_refresh_requested = true;
                                }
                                self.refresh_git(i, true);
                                self.index_project(i);
                            }
                        }
                        Err(e) => self.status(e, true),
                    }
                }
                WorkerResult::Search {
                    root,
                    generation,
                    query,
                    excludes,
                    result,
                } => {
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
    fn git_polling_active(&self) -> bool {
        self.ctx
            .input(|input| input.focused && input.viewport().minimized != Some(true))
    }

    fn refresh_visible_git(&mut self) {
        let active = self.git_polling_active();
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

    fn refresh_git(&mut self, index: usize, force: bool) {
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
        self.worker(move || {
            let result = read_git_snapshot(
                &root,
                approved_parent.as_deref(),
                previous_repository.as_deref(),
                previous_history,
                refresh_history,
                previous_history_error,
                automatic_history_limit,
            );
            WorkerResult::GitStatus {
                root,
                request_id,
                result: result.map(Box::new),
            }
        });
    }
    fn git_action(&mut self, root: PathBuf, action: GitAction) {
        if let Err(error) = self.start_git_action(root, action, None) {
            self.status(error, true);
        }
    }

    fn start_git_action(
        &mut self,
        root: PathBuf,
        action: GitAction,
        reply: Option<Sender<crate::cli::Response>>,
    ) -> Result<(), String> {
        if self.git_busy.is_some() {
            return Err("A Git operation is still running. No new operation was started.".into());
        }
        if matches!(action, GitAction::SwitchBranch(_)) {
            if self.projects.iter().any(|project| {
                project.tabs.iter().any(|tab| {
                    (tab.dirty || tab.saving || tab.loading || tab.agent_unsaved)
                        && tab.path.as_ref().map_or(
                            project.git_root.as_ref() == Some(&root) || project.root == root,
                            |path| context_actions::path_contains(&root, path),
                        )
                })
            }) {
                return Err("Save or close unsaved documents and wait for file operations before switching branches. No branch was switched.".into());
            }
            if self.control_dialog().is_some() {
                return Err(
                    "Finish the open dialog before switching branches. No branch was switched."
                        .into(),
                );
            }
        }
        self.git_busy = Some(root.clone());
        // Status reads use GIT_OPTIONAL_LOCKS=0 and need not hold up an action.
        // Discard their results, which may describe the repository before or
        // halfway through the action. Keep the last complete view until the
        // post-action refresh arrives; late workers cannot overwrite it.
        for project in &mut self.projects {
            if project.git_root.as_ref() == Some(&root) || project.root == root {
                project.git_status_request = None;
                project.git_indicator_request = None;
                project.git_loading = false;
                project.git_refresh_pending = false;
                project.last_git_refresh = Instant::now() - Duration::from_secs(30);
            }
        }
        let message = action.label().to_owned();
        self.status(format!("{message}…"), false);
        self.worker(move || WorkerResult::GitAction {
            result: action.run(&root),
            root,
            message,
            reply,
        });
        Ok(())
    }
    fn request_git_commit(&mut self, index: usize, hash: String) {
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
    fn request_git_hover_stats(&mut self, index: usize, hash: String) {
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
    fn load_more_git_history(&mut self) {
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
            result: git::history_page(&repository, &snapshot, offset, git::HISTORY_PAGE_SIZE)
                .map_err(|error| error.to_string()),
        });
    }
    fn refresh_git_history(&mut self, index: usize) {
        if let Some(project) = self.projects.get_mut(index) {
            project.git_history_refresh_requested = true;
        }
        self.refresh_git(index, true);
    }
    fn request_git_diff(&mut self, root: PathBuf, path: PathBuf, staged: bool, untracked: bool) {
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
    fn request_commit_diff(&mut self, root: PathBuf, hash: String, path: PathBuf) {
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
    fn begin_git_diff(&mut self, title: String) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.diff_loading = true;
        self.diff_error = None;
        self.diff = Some((title, String::new()));
        self.comparison = None;
    }
    fn toggle_bookmark(&mut self) {
        match self.try_toggle_bookmark(self.tab().map(|tab| tab.id)) {
            Ok(crate::bookmarks::ToggleResult::Added(_)) => self.status("Bookmark saved", false),
            Ok(crate::bookmarks::ToggleResult::Removed(_)) => {
                self.status("Bookmark removed", false)
            }
            Err(error) => self.status(error, true),
        }
    }
    fn try_toggle_bookmark(
        &mut self,
        tab_id: Option<u64>,
    ) -> Result<crate::bookmarks::ToggleResult, String> {
        let id = tab_id.ok_or_else(|| "Open a file before saving a bookmark".to_owned())?;
        let tab = self
            .projects
            .iter()
            .flat_map(|p| &p.tabs)
            .find(|tab| tab.id == id)
            .ok_or_else(|| "The document is no longer open".to_owned())?;
        if tab.loading || tab.error.is_some() {
            return Err("Wait until the file has loaded before saving a bookmark".into());
        }
        let path = tab
            .path
            .clone()
            .ok_or_else(|| "Save this file before creating a bookmark".to_owned())?;
        let (line, column) = tab.lines.line_col(&tab.text, tab.revision, tab.vim.cursor);
        let context = tab
            .text
            .lines()
            .nth(line.saturating_sub(1))
            .filter(|line| line.len() <= 4096)
            .unwrap_or("")
            .to_owned();
        let existing = self
            .bookmarks
            .for_path(&path)
            .find(|bookmark| crate::bookmarks::locate(&tab.text, bookmark).line == line)
            .map(|bookmark| bookmark.id);
        let result = if let Some(id) = existing {
            self.bookmarks.remove(id);
            crate::bookmarks::ToggleResult::Removed(id)
        } else {
            self.bookmarks.toggle(&path, line, column, Some(&context))
        };
        self.bookmark_lines.clear();
        self.session_dirty = true;
        Ok(result)
    }
    fn jump_to_bookmark(&mut self, id: u64) {
        if let Err(error) = self.try_jump_to_bookmark(id) {
            self.status(error, true);
        }
    }
    fn try_jump_to_bookmark(&mut self, id: u64) -> Result<u64, String> {
        self.record_jump();
        let bookmark = self
            .bookmarks
            .get(id)
            .cloned()
            .ok_or_else(|| "The bookmark no longer exists".to_owned())?;
        let existing = self.projects.iter().enumerate().find_map(|(project, p)| {
            p.tabs
                .iter()
                .position(|t| t.path.as_ref() == Some(&bookmark.path))
                .map(|tab| (project, tab))
        });
        if let Some((project, tab)) = existing {
            if let Some(error) = &self.projects[project].tabs[tab].error {
                return Err(format!("The bookmarked file is unavailable: {error}"));
            }
            self.activate_tab(project, tab);
        } else {
            let project = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| bookmark.path.starts_with(&p.root))
                .max_by_key(|(_, p)| p.root.components().count())
                .map(|(index, _)| index);
            if let Some(project) = project {
                let active = self.projects[project].active_tab;
                self.activate_tab(project, active);
            } else if let Some(parent) = bookmark.path.parent() {
                self.add_project(parent.to_owned());
            } else {
                return Err("The bookmark has no valid file path".into());
            }
            self.open_file(bookmark.path.clone());
        }
        let tab = self
            .tab_mut()
            .ok_or_else(|| "The bookmarked file could not be opened".to_owned())?;
        let id = tab.id;
        tab.agent_view = crate::cli::DocumentView::Source;
        self.pending_bookmark_jump = Some((id, bookmark));
        self.apply_pending_bookmark_jump();
        self.session_dirty = true;
        Ok(id)
    }
    fn apply_pending_bookmark_jump(&mut self) {
        let Some((id, bookmark)) = self.pending_bookmark_jump.take() else {
            return;
        };
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        if tab.loading {
            self.pending_bookmark_jump = Some((id, bookmark));
            return;
        }
        if let Some(error) = tab.error.clone() {
            self.status(format!("Could not open bookmark: {error}"), true);
            return;
        }
        let location = crate::bookmarks::locate(&tab.text, &bookmark);
        Self::move_navigation_cursor(tab, location.char_index);
        if let Some(stored) = self
            .bookmarks
            .items
            .iter_mut()
            .find(|item| item.id == bookmark.id)
        {
            stored.line = location.line;
            stored.column = location.column;
        }
        self.bookmark_lines.clear();
        self.session_dirty = true;
    }
    fn move_navigation_cursor(tab: &mut EditorTab, cursor: usize) {
        let action = tab.vim.finish_insert(&mut tab.text);
        if action.text_changed {
            tab.changed();
        }
        tab.vim.set_cursor(&tab.text, cursor);
        let selection = tab
            .vim
            .selection(&tab.text)
            .unwrap_or(tab.vim.cursor..tab.vim.cursor);
        tab.selection = selection.clone();
        tab.pending_selection = Some(selection);
        tab.scroll_to_cursor = true;
        tab.focus = true;
    }
    fn start_screen_jump(&mut self) {
        if let Some(tab) = self.tab_mut() {
            let action = tab.vim.finish_insert(&mut tab.text);
            if action.text_changed {
                tab.changed();
            }
        }
        let Some(tab) = self.tab() else { return };
        if tab.loading || tab.error.is_some() || tab.agent_view != crate::cli::DocumentView::Source
        {
            self.status(
                "Open a source document to jump between visible matches",
                true,
            );
            return;
        }
        self.screen_jump = Some(ActiveScreenJump {
            tab_id: tab.id,
            pane_id: self.current_pane_id(),
            revision: tab.revision,
            state: crate::screen_jump::ScreenJump::new(),
            glyphs: Vec::new(),
        });
        self.completions = None;
    }
    fn screen_jump_keys(&mut self) -> bool {
        let Some(mut jump) = self.screen_jump.take() else {
            return false;
        };
        let valid = self.current_pane_id() == jump.pane_id
            && self.tab().is_some_and(|tab| {
                tab.id == jump.tab_id
                    && tab.revision == jump.revision
                    && tab.agent_view == crate::cli::DocumentView::Source
            })
            && self.palette.is_none()
            && !self.settings_open
            && !self.agent_marks_modal_open()
            && self.picker.is_none()
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.vim_command.is_none()
            && self.context_file_dialog.is_none()
            && !egui::Popup::is_any_open(&self.ctx)
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open();
        if !valid || self.ctx.input(|input| input.pointer.any_pressed()) {
            jump.state.cancel();
            return false;
        }
        let events = self
            .ctx
            .input_mut(|input| std::mem::take(&mut input.events));
        // A translated Text event owns its preceding key press, including
        // Shift punctuation and non-US layouts. Comparing character values
        // would process those keys twice and could trigger a jump prematurely.
        let mut keys_with_text = vec![false; events.len()];
        let mut preceding_key = None;
        for (index, event) in events.iter().enumerate() {
            match event {
                egui::Event::Key { pressed: true, .. } => preceding_key = Some(index),
                egui::Event::Text(_)
                | egui::Event::Ime(egui::ImeEvent::Preedit { .. } | egui::ImeEvent::Commit(_)) => {
                    if let Some(key_index) = preceding_key.take() {
                        keys_with_text[key_index] = true;
                    }
                }
                _ => {}
            }
        }
        let mut keep = Vec::new();
        let mut action = crate::screen_jump::JumpAction::None;
        for (event_index, event) in events.into_iter().enumerate() {
            if !matches!(action, crate::screen_jump::JumpAction::None) {
                // A completed jump owns the entire input batch; trailing query
                // characters must never become an editing command.
                if !editor_input::native_input(&event) && !matches!(event, egui::Event::Key { .. })
                {
                    keep.push(event);
                }
                continue;
            }
            match event {
                egui::Event::Text(text)
                | egui::Event::Paste(text)
                | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    for ch in text.chars() {
                        action = jump.state.input_char(ch);
                        jump.state.recompute(&jump.glyphs);
                        if !matches!(action, crate::screen_jump::JumpAction::None) {
                            break;
                        }
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.command || modifiers.ctrl || modifiers.alt {
                        continue;
                    }
                    match key {
                        Key::Escape => action = crate::screen_jump::JumpAction::Cancel,
                        Key::Enter => {
                            action = jump.state.enter();
                            jump.state.recompute(&jump.glyphs);
                        }
                        Key::Backspace => {
                            jump.state.backspace();
                            jump.state.recompute(&jump.glyphs);
                        }
                        _ => {
                            let name = key.name();
                            let ch = if key == Key::Space {
                                Some(' ')
                            } else if name.chars().count() == 1 {
                                name.chars().next().map(|ch| {
                                    if modifiers.shift {
                                        ch
                                    } else {
                                        ch.to_ascii_lowercase()
                                    }
                                })
                            } else {
                                None
                            };
                            if let Some(ch) = ch.filter(|_| !keys_with_text[event_index]) {
                                action = jump.state.input_char(ch);
                                jump.state.recompute(&jump.glyphs);
                            }
                        }
                    }
                }
                egui::Event::Key { .. } => {}
                egui::Event::Copy | egui::Event::Cut | egui::Event::Ime(_) => {}
                other => keep.push(other),
            }
        }
        self.ctx.input_mut(|input| input.events = keep);
        match action {
            crate::screen_jump::JumpAction::Jump(cursor) => {
                if let Some(tab) = self.tab_mut() {
                    Self::move_navigation_cursor(tab, cursor);
                }
                self.session_dirty = true;
            }
            crate::screen_jump::JumpAction::Cancel => {}
            crate::screen_jump::JumpAction::None => self.screen_jump = Some(jump),
        }
        true
    }
    fn search_project(&mut self) {
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
        let Some(p) = self.project() else {
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
    fn invalidate_project_search(&mut self) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_running = false;
        self.search_dirty = self.project().is_some() && !self.project_query.trim().is_empty();
        self.search_last_edit = Instant::now();
        self.project_search = None;
        self.project_search_error = None;
        self.ctx.request_repaint();
    }
    fn autosave(&mut self) {
        let mut recoveries = Vec::new();
        let mut removals = Vec::new();
        let mut saves = Vec::new();
        let mut delay: Option<Duration> = None;
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
                        && t.recovery_revision != t.revision
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
                    && t.path.is_some()
                    && !t.saving
                    && t.error.is_none()
                    && t.disk_conflict.is_none()
                {
                    let remaining = Duration::from_millis(self.settings.autosave_delay_ms)
                        .saturating_sub(t.last_edit.elapsed());
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
            self.worker(move || {
                let result =
                    crate::recovery::encode_document(&text, baseline.as_ref(), ending, bom)
                        .and_then(|bytes| writer.write(&path, stamp, &bytes));
                WorkerResult::Recovered {
                    id,
                    revision,
                    generation,
                    result,
                }
            });
        }
        for (id, revision, generation, path) in removals {
            let writer = self.recovery_writer.clone();
            let stamp = writer.stamp();
            self.worker(move || WorkerResult::RecoveryRemoved {
                id,
                revision,
                generation,
                result: writer.remove(&path, stamp),
            });
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
        if self.session_dirty && self.last_session_save.elapsed() > Duration::from_secs(1) {
            self.persist_session();
            self.session_dirty = false;
            self.last_session_save = Instant::now();
        }
    }
    fn session(&self) -> Session {
        Session {
            projects: self
                .projects
                .iter()
                .map(|p| SessionProject {
                    launch_selected: p.launch.selected.clone(),
                    root: p.root.clone(),
                    tasks: p.tasks.clone(),
                    git_split_ratio: p.git_view.split_ratio,
                    git_approved_parent: p.git_approved_parent.clone(),
                    open_files: Vec::new(),
                    active_file: p.tabs.get(p.active_tab).and_then(|t| t.path.clone()),
                    tabs: p
                        .tabs
                        .iter()
                        .map(|t| SessionTab {
                            path: t.path.clone(),
                            title: t.title.clone(),
                            recovery_path: Some(t.recovery_path.clone()),
                            cursor: t.vim.cursor,
                            view: t.agent_view,
                            agent_unsaved: t.agent_unsaved,
                        })
                        .collect(),
                    active_tab_index: Some(p.active_tab),
                    layout: self.project_session_layout(&p.root),
                })
                .collect(),
            active_project: self.active_project,
            sidebar: Some(self.sidebar),
            sidebars: Some(self.sidebar_session()),
            autosave: Some(self.settings.autosave),
            settings: Some(self.settings.clone()),
            layout: None,
            bookmarks: self.bookmarks.items.clone(),
            agent_marks: self.agent_marks.clone(),
            find_regex: self.find_regex,
        }
    }
    fn persist_session(&self) {
        if let Ok(bytes) = serde_json::to_vec(&self.session()) {
            let path = self.recovery_session_path.clone();
            let writer = self.recovery_writer.clone();
            let stamp = writer.stamp();
            std::thread::spawn(move || {
                let _ = writer.write(&path, stamp, &bytes);
            });
        }
    }
    fn flush_recovery(&self) {
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
    fn pick_project(&mut self) {
        let dir = self
            .project()
            .map(|p| p.root.parent().unwrap_or(&p.root).to_path_buf())
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        self.start_native_dialog(DialogPurpose::Projects, Some(dir), None);
    }
    fn request_save_as(&mut self, id: u64) {
        if self
            .tab_by_id_mut(id)
            .is_some_and(|tab| tab.loading || tab.saving)
        {
            self.status("Wait for this file to finish loading or saving.", false);
            return;
        }
        let Some((directory, title, unnamed)) = self.projects.iter().find_map(|project| {
            project.tabs.iter().find(|tab| tab.id == id).map(|tab| {
                (
                    tab.path
                        .as_deref()
                        .and_then(Path::parent)
                        .unwrap_or(&project.root)
                        .to_path_buf(),
                    tab.title.clone(),
                    tab.path.is_none(),
                )
            })
        }) else {
            return;
        };
        if self.picker.is_some() {
            if self.save_as != Some(id) {
                if unnamed && !self.save_queue.contains(&id) {
                    self.save_queue.push_back(id);
                } else if !unnamed {
                    self.status("Finish the current file dialog before choosing another Save As destination.", false);
                }
            }
            return;
        }
        self.save_as = Some(id);
        self.start_native_dialog(DialogPurpose::SaveAs(id), Some(directory), Some(title));
    }
    fn start_native_dialog(
        &mut self,
        purpose: DialogPurpose,
        directory: Option<PathBuf>,
        file_name: Option<String>,
    ) {
        if self.picker.is_some() {
            return;
        }
        static NEXT_DIALOG: AtomicU64 = AtomicU64::new(1);
        let request = Picker {
            id: NEXT_DIALOG.fetch_add(1, Ordering::Relaxed),
            purpose,
        };
        self.picker = Some(request);
        self.completions = None;
        self.palette = None;
        let Some(mut dialog) = self.native_dialog_builder.clone() else {
            // In-memory test apps inject the dialog completion without opening an OS window.
            return;
        };
        dialog = dialog.set_title(match purpose {
            DialogPurpose::Projects => "Open project folders",
            DialogPurpose::Files => "Open files",
            DialogPurpose::SaveAs(_) => "Save file as",
        });
        if let Some(directory) = directory {
            dialog = dialog.set_directory(crate::paths::display(&directory));
        }
        if let Some(file_name) = file_name {
            dialog = dialog.set_file_name(file_name);
        }
        self.worker(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let paths = match purpose {
                    DialogPurpose::Projects => dialog.pick_folders(),
                    DialogPurpose::Files => dialog.pick_files(),
                    DialogPurpose::SaveAs(_) => dialog.save_file().map(|path| vec![path]),
                };
                let Some(paths) = paths else { return Ok(None) };
                let paths: Vec<_> = paths
                    .into_iter()
                    .map(|path| fs::canonicalize(&path).unwrap_or(path))
                    .collect();
                let overwrite = if matches!(purpose, DialogPurpose::SaveAs(_)) {
                    match fs::symlink_metadata(&paths[0]) {
                        Ok(_) => Some(workspace::fingerprint(&paths[0], MAX_DOCUMENT)?),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error.to_string()),
                    }
                } else {
                    None
                };
                Ok(Some(DialogSelection { paths, overwrite }))
            }))
            .unwrap_or_else(|_| {
                Err("The Windows file dialog could not be opened. Try again.".into())
            });
            WorkerResult::NativeDialog { request, result }
        });
    }
    fn finish_native_dialog(
        &mut self,
        request: Picker,
        result: Result<Option<DialogSelection>, String>,
    ) {
        if self.picker != Some(request) {
            return;
        }
        self.picker = None;
        if matches!(request.purpose, DialogPurpose::SaveAs(_)) {
            self.save_as = None;
        }
        match result {
            Ok(Some(selection)) => {
                match request.purpose {
                    DialogPurpose::Projects => {
                        for path in selection.paths {
                            self.add_project(path);
                        }
                    }
                    DialogPurpose::Files => {
                        for path in selection.paths {
                            self.open_argument(path);
                        }
                    }
                    DialogPurpose::SaveAs(id) => {
                        let Some(path) = selection.paths.into_iter().next() else {
                            return;
                        };
                        if self
                            .projects
                            .iter()
                            .flat_map(|project| &project.tabs)
                            .any(|tab| tab.id != id && tab.path.as_ref() == Some(&path))
                        {
                            self.save_queue.clear();
                            self.close_after_save = false;
                            self.status("This file is already open in another tab. Choose a different name.", true);
                            return;
                        }
                        self.save_tab_expected(id, Some(path), selection.overwrite);
                    }
                }
            }
            Ok(None) => {
                if matches!(request.purpose, DialogPurpose::SaveAs(_)) {
                    self.save_queue.clear();
                    self.close_after_save = false;
                }
            }
            Err(error) => {
                self.save_queue.clear();
                self.close_after_save = false;
                self.status(error, true);
            }
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
    }
    fn open_palette(&mut self, palette: Palette) {
        self.find.break_escape_sequence();
        self.completions = None;
        self.palette = Some(palette);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_focus = true;
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        self.ctx.request_repaint();
    }
    fn close_palette(&mut self) {
        self.palette = None;
        self.palette_focus = false;
        if let Some(focused) = self.ctx.memory(|memory| memory.focused()) {
            self.ctx
                .memory_mut(|memory| memory.surrender_focus(focused));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
            && !tab.loading
            && tab.error.is_none()
        {
            let id = self.document_widget_id(tab.id);
            if self.ctx.read_response(id).is_some() {
                self.ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
        self.ctx.request_repaint();
    }
    fn run_task(&mut self, command: String) {
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.launch.show_output = false;
        }
        if self.project().is_some_and(|p| p.run.task.is_some()) {
            self.status("Stop the current task first", true);
            return;
        }
        let Some(root) = self.project().map(|p| p.root.clone()) else {
            return;
        };
        if self.project_task_unsaved_count() > 0 {
            self.output_visible = true;
            self.problems_visible = false;
            self.status("Save your changes before running a command. Use Save all files in Output, wait for saving to finish, then run again.", true);
            return;
        }
        self.output_visible = true;
        self.problems_visible = false;
        self.command = command.clone();
        let environment = self
            .project_task_configuration()
            .unwrap()
            .environment(&root, self.tab().and_then(|tab| tab.path.as_deref()));
        let run = &mut self.projects[self.active_project].run;
        run.output = format!("> {command}\n{}\n\n", crate::paths::display(&root));
        run.status = "Running".into();
        run.command = command.clone();
        run.task = Some(tasks::spawn_configured(
            root,
            tasks::CommandSpec::Shell(command),
            environment,
        ));
    }
    fn open_terminal(&mut self) {
        let Some(root) = self.project().map(|p| p.root.clone()) else {
            return;
        };
        let environment = self
            .project_task_configuration()
            .unwrap()
            .environment(&root, self.tab().and_then(|tab| tab.path.as_deref()));
        match tasks::open_terminal(&root, environment) {
            Ok(()) => self.status("Terminal opened in this project", false),
            Err(error) => self.status(error, true),
        }
    }
}

impl VEditApp {
    fn shortcuts(&mut self) {
        use keybindings::Action;
        self.resume_editor_input();
        self.resume_shortcut_input();
        keybindings::preserve_altgr_text(&self.ctx);
        let ctx = self.ctx.clone();
        if self.picker.is_some()
            || self.palette.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.vim_search_active()
            || self.context_file_dialog.is_some()
            || self.vim_command.is_some()
            || egui::Popup::is_any_open(&ctx)
        {
            return;
        }
        if self.completion_keys() || self.split_shortcuts(&ctx) {
            return;
        }
        let vim_modal = self.settings.vim
            && self.tab().is_some_and(|tab| {
                tab.vim.mode != VimMode::Insert
                    && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            });
        let vim_insert = self.settings.vim
            && self.tab().is_some_and(|tab| {
                tab.vim.mode == VimMode::Insert
                    && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            });
        let vim_normal = vim_modal
            && self
                .tab()
                .is_some_and(|tab| tab.vim.mode == VimMode::Normal);
        let review_visible = self.change_review_visible();
        let mut ready = Vec::new();
        for &action in Action::ALL {
            if matches!(
                action,
                Action::SwitchProject
                    | Action::PreviousProject
                    | Action::NextProject
                    | Action::FocusExplorer
                    | Action::FocusEditor
                    | Action::FocusGit
                    | Action::FocusOutput
                    | Action::FocusLeft
                    | Action::FocusRight
                    | Action::FocusUp
                    | Action::FocusDown
                    | Action::FocusNextRegion
                    | Action::FocusPreviousRegion
                    | Action::FocusNextPane
                    | Action::FocusPreviousPane
                    | Action::SplitRight
                    | Action::SplitDown
                    | Action::ClosePane
                    | Action::MoveTabLeft
                    | Action::MoveTabRight
                    | Action::MoveTabUp
                    | Action::MoveTabDown
                    | Action::AgentMark
                    | Action::AgentMarks
                    | Action::ToggleComment
                    | Action::DuplicateLine
                    | Action::MoveLineUp
                    | Action::MoveLineDown
                    | Action::Undo
                    | Action::Redo
                    | Action::ZoomIn
                    | Action::ZoomOut
                    | Action::ZoomReset
            ) {
                continue;
            }
            // A change review hides the documents; their commands wait for it to close.
            if review_visible
                && matches!(
                    action,
                    Action::Save
                        | Action::SaveAs
                        | Action::CloseTab
                        | Action::Find
                        | Action::Replace
                        | Action::FindNext
                        | Action::FindPrevious
                        | Action::GoToLine
                        | Action::Definition
                        | Action::Completion
                        | Action::MarkdownPreview
                        | Action::ToggleBookmark
                        | Action::ScreenJump
                        | Action::Reload
                        | Action::CompareWithSaved
                        | Action::NextProblem
                        | Action::PreviousProblem
                        | Action::NavigateBack
                        | Action::NavigateForward
                )
            {
                continue;
            }
            if action == Action::Find
                && vim_modal
                && !self.settings.keybindings.contains_key(action.id())
            {
                continue;
            }
            // Ctrl+O goes back in Vim Normal mode, like Vim's jump list.
            if action == Action::OpenFile
                && vim_normal
                && !self.settings.keybindings.contains_key(action.id())
            {
                continue;
            }
            // Ctrl+B keeps its Vim page-back meaning; explicit custom bindings
            // have no hidden Vim exception, and the safe Ctrl+Alt+E works everywhere.
            if action == Action::ToggleLeftSidebar
                && vim_modal
                && !self.settings.keybindings.contains_key(action.id())
            {
                let available = ctx.input(|input| {
                    input.events.iter().any(|event| {
                        self.shortcut_matches(action, event)
                            && matches!(event, egui::Event::Key { modifiers, .. } if modifiers.alt)
                    })
                });
                if !available {
                    continue;
                }
            }
            ready.push(action);
        }
        let shortcut_context = self.shortcut_input_context();
        let candidate = ctx.input(|input| {
            input.events.iter().enumerate().find_map(|(index, event)| {
                ready
                    .iter()
                    .copied()
                    .find(|action| self.shortcut_boundary(*action, event, shortcut_context))
                    .map(|action| (index, action))
            })
        });
        let Some((index, action)) = candidate else {
            return;
        };
        if self.earlier_shortcut_boundary(index) {
            return;
        }
        // Save and navigation must observe text typed before the shortcut.
        // Render that prefix in its current source, then dispatch next frame.
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
            && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            && ctx.input(|input| input.events[..index].iter().any(editor_input::native_input))
        {
            let suffix = ctx.input_mut(|input| input.events.split_off(index));
            editor_input::defer(&ctx, tab.id, suffix);
            return;
        }
        if ctx.memory(|memory| memory.focused().is_some())
            && ctx.input(|input| input.events[..index].iter().any(editor_input::native_input))
        {
            let suffix = ctx.input_mut(|input| input.events.split_off(index));
            self.defer_widget_shortcut_input(suffix);
            return;
        }
        self.take_shortcut(action);
        let previous_document = self.tab().map(|tab| tab.id);
        let source_owned = self.tab().is_some_and(|tab| {
            ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
        });
        if action == Action::NewFile
            && vim_insert
            && !self.settings.keybindings.contains_key(action.id())
        {
            self.request_completion();
        } else {
            self.run_shortcut_action(action);
        }
        // Continue the suffix only after the resulting source has rendered.
        // This preserves two commands in one OS event batch in input order.
        if self.find_focus {
            // Find takes focus while its toolbar renders, after sidebars. Keep
            // the suffix for that field instead of letting the old field eat it.
            let suffix = ctx.input_mut(|input| std::mem::take(&mut input.events));
            self.defer_widget_shortcut_input(suffix);
        } else if self.palette.is_none()
            && !self.settings_open
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.picker.is_none()
            && !self.navigation_modal_open()
            && !self.agent_marks_modal_open()
            && let Some(tab) = self.tab().filter(|tab| {
                tab.agent_view == crate::cli::DocumentView::Source
                    && (source_owned || Some(tab.id) != previous_document)
            })
        {
            let suffix = ctx.input_mut(|input| std::mem::take(&mut input.events));
            editor_input::defer(&ctx, tab.id, suffix);
        } else if self.palette.is_none()
            && !self.settings_open
            && !self.find_focus
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.picker.is_none()
            && !self.navigation_modal_open()
            && !self.agent_marks_modal_open()
        {
            self.defer_widget_shortcut_tail();
        }
    }
    fn handle_vim(&mut self) {
        self.resume_editor_input();
        self.validate_vim_search();
        if !self.settings.vim
            || egui::Popup::is_any_open(&self.ctx)
            || self
                .tab()
                .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.vim_command.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.context_file_dialog.is_some()
        {
            return;
        }
        let ctx = self.ctx.clone();
        if self.vim_search_active() && ctx.input(|input| input.pointer.any_pressed()) {
            self.cancel_vim_search(false);
        }
        let Some(tab) = self.tab() else {
            return;
        };
        let id = self.document_widget_id(tab.id);
        if !ctx.memory(|memory| memory.has_focus(id)) {
            return;
        }
        if ctx.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.interact_pos().is_some_and(|position| {
                    !self.editor_rect.is_some_and(|rect| rect.contains(position))
                })
        }) {
            return;
        }

        fn apply_action(
            tab: &mut EditorTab,
            action: crate::vim::Action,
            vim_register: &mut Option<Yank>,
        ) {
            if action.text_changed {
                tab.changed();
            }
            if action.yanked.is_some() {
                if !action.text_changed {
                    if action.yanked.as_ref().is_some_and(|yank| yank.linewise) {
                        tab.flash_line_yank(action.yanked_ranges);
                    } else {
                        tab.flash_yank(action.yanked_ranges);
                    }
                }
                *vim_register = action.yanked;
            }
            if action.scroll.is_some() {
                tab.scroll_align = action.scroll;
            }
            if action.message.is_some() {
                tab.vim_message = action.message;
            }
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            tab.selection = selection.clone();
            tab.pending_selection = Some(selection);
            tab.scroll_to_cursor = true;
        }
        fn copy_to_clipboard(tab: &mut EditorTab, system_clipboard: &mut Option<String>) {
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or_else(|| tab.selection.clone());
            let linewise = selection.is_empty() || tab.vim.mode == VimMode::VisualLine;
            let selection = crate::vim::clipboard_range(&tab.text, selection, tab.vim.cursor);
            if !selection.is_empty() {
                *system_clipboard = Some(
                    tab.text
                        .chars()
                        .skip(selection.start)
                        .take(selection.len())
                        .collect(),
                );
                if linewise {
                    tab.flash_line_yank(vec![selection]);
                } else {
                    tab.flash_yank(vec![selection]);
                }
            }
            // Native Copy keeps the mode and selection, but cannot leave an
            // unfinished Vim delete armed for the next navigation key.
            tab.vim.cancel_pending_command();
        }
        fn cut_selection(
            tab: &mut EditorTab,
            system_clipboard: &mut Option<String>,
            vim_register: &mut Option<Yank>,
        ) {
            if tab.loading {
                return;
            }
            if matches!(tab.vim.mode, VimMode::Visual | VimMode::VisualLine) {
                let (action, copied) = tab.vim.cut_visual_to_clipboard(&mut tab.text);
                *system_clipboard = copied;
                apply_action(tab, action, vim_register);
                return;
            }
            let selection =
                crate::vim::clipboard_range(&tab.text, tab.selection.clone(), tab.vim.cursor);
            if selection.is_empty() {
                tab.vim.cancel_pending_command();
                return;
            }
            *system_clipboard = Some(
                tab.text
                    .chars()
                    .skip(selection.start)
                    .take(selection.len())
                    .collect(),
            );
            let edit = editor::replace_selection(&tab.text, selection, "");
            tab.apply(edit);
            tab.set_cursor(tab.selection.start..tab.selection.start);
            tab.vim.notify_external_text_change();
        }
        fn paste_system_text(tab: &mut EditorTab, text: &str) {
            if tab.loading || text.is_empty() {
                return;
            }
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or_else(|| tab.selection.clone());
            let edit = editor::replace_selection(&tab.text, selection, text);
            let cursor = edit.selection.start;
            tab.apply(edit);
            if matches!(tab.vim.mode, VimMode::Visual | VimMode::VisualLine) {
                tab.vim.set_selection(&tab.text, cursor..cursor);
            }
            tab.set_cursor(cursor..cursor);
            tab.vim.notify_external_text_change();
        }
        fn publish_search_yank(
            tab: &mut EditorTab,
            find: &mut find::FindState,
            vim_register: &mut Option<Yank>,
        ) {
            if let Some(yank) = find.take_yank() {
                tab.vim.set_register(yank.clone());
                *vim_register = Some(yank);
            }
        }

        let events = ctx.input(|input| input.events.clone());
        let mut retained = Vec::with_capacity(events.len());
        let mut native_insert = Vec::new();
        let mut prompt: Option<char> = None;
        let mut prompt_text = String::new();
        let mut vim_searched = false;
        let mut definition = false;
        // `[d` / `]d` and Ctrl+O / Ctrl+I: true for previous / back.
        let mut problem = None;
        let mut history = None;
        let mut vim_register = None;
        let mut system_clipboard = None;
        let swap_jk = self.settings.swap_jk;
        let event_time = ctx.input(|input| input.time);
        let native_copy = events
            .iter()
            .any(|event| matches!(event, egui::Event::Copy));
        let native_cut = events.iter().any(|event| matches!(event, egui::Event::Cut));
        let native_paste = events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_)));
        let register = self.register.clone();
        let mut yank_register = self.yank_register.clone();
        let keybindings = self.settings.keybindings.clone();
        let indent = self.tab().map(|tab| tab.indentation(&self.settings));
        let page = self.editor_rect.map_or(24, |rect| {
            (rect.height() / (self.settings.font_size * self.settings.line_height))
                .floor()
                .max(1.0) as isize
        });
        let pane_id = self.current_pane_id();
        let mut find = std::mem::take(&mut self.find);
        if let Some(tab) = self.tab_mut() {
            if let Some(indent) = indent {
                tab.vim.set_indentation(indent.width, indent.tabs);
            }
            if let Some(register) = register {
                tab.vim.set_register(register);
            }
            tab.vim.set_yank_register(yank_register.clone());
            let mut events = events.into_iter().peekable();
            while let Some(event) = events.next() {
                if tab.vim.mode == VimMode::Insert
                    && editor_input::vim_barrier(&event, &keybindings)
                    && native_insert.iter().any(editor_input::native_input)
                {
                    editor_input::defer(
                        &ctx,
                        tab.id,
                        std::iter::once(event).chain(events).collect(),
                    );
                    break;
                }
                find.observe_escape(&event, event_time);
                if find.prompt.is_some() || find.waiting() {
                    match event {
                        egui::Event::Text(text)
                        | egui::Event::Paste(text)
                        | egui::Event::Ime(egui::ImeEvent::Commit(text)) => find.text(tab, &text),
                        egui::Event::Copy => find.key(tab, Key::C, egui::Modifiers::CTRL),
                        egui::Event::Cut | egui::Event::Ime(_) => {}
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => find.key(tab, key, modifiers),
                        other => retained.push(other),
                    }
                    publish_search_yank(tab, &mut find, &mut vim_register);
                    continue;
                }
                if prompt.is_some() {
                    match event {
                        boundary @ egui::Event::Key {
                            key: Key::Enter | Key::Escape,
                            pressed: true,
                            modifiers,
                            ..
                        } if !modifiers.ctrl
                            && !modifiers.command
                            && !modifiers.alt
                            && !modifiers.shift =>
                        {
                            retained.push(boundary);
                            retained.extend(events);
                            break;
                        }
                        egui::Event::Text(text) => prompt_text.push_str(&text),
                        egui::Event::Paste(text) => prompt_text.push_str(&text),
                        egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            prompt_text.push_str(&text)
                        }
                        egui::Event::Copy => {
                            prompt = None;
                            prompt_text.clear();
                            copy_to_clipboard(tab, &mut system_clipboard);
                        }
                        egui::Event::Cut | egui::Event::Ime(_) => {}
                        egui::Event::Key {
                            key: Key::Backspace,
                            pressed: true,
                            modifiers,
                            ..
                        } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => {
                            prompt_text.pop();
                        }
                        egui::Event::Key {
                            pressed: true, key, ..
                        } if !matches!(key, Key::Enter | Key::Escape) => {}
                        other => retained.push(other),
                    }
                    continue;
                }
                if tab.vim.mode != VimMode::Insert {
                    let comment = keybindings::matches(
                        &keybindings,
                        keybindings::Action::ToggleComment,
                        &event,
                    );
                    let duplicate = keybindings::matches(
                        &keybindings,
                        keybindings::Action::DuplicateLine,
                        &event,
                    );
                    let move_up =
                        keybindings::matches(&keybindings, keybindings::Action::MoveLineUp, &event);
                    let move_down = keybindings::matches(
                        &keybindings,
                        keybindings::Action::MoveLineDown,
                        &event,
                    );
                    if comment || duplicate || move_up || move_down {
                        if events
                            .peek()
                            .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                        {
                            events.next();
                        }
                        let selection = tab
                            .vim
                            .selection(&tab.text)
                            .unwrap_or_else(|| tab.selection.clone());
                        let edit = if comment {
                            editor::toggle_line_comment(
                                &tab.text,
                                selection,
                                crate::file_kind::comment_language(tab.path.as_deref()),
                            )
                        } else if duplicate {
                            editor::duplicate_lines(&tab.text, selection)
                        } else {
                            editor::move_lines(&tab.text, selection, move_down)
                        };
                        if move_up || move_down || duplicate {
                            tab.apply_line_move(edit);
                        } else if edit.text != tab.text {
                            tab.apply(edit);
                            tab.vim.notify_external_text_change();
                        }
                        editor_input::defer(&ctx, tab.id, events.collect());
                        break;
                    }
                }
                match &event {
                    event
                        if keybindings::matches(&keybindings, keybindings::Action::Undo, event)
                            || keybindings::matches(
                                &keybindings,
                                keybindings::Action::Redo,
                                event,
                            ) =>
                    {
                        if events
                            .peek()
                            .is_some_and(|next| keybindings::shortcut_text_event(next, event))
                        {
                            events.next();
                        }
                        let redo =
                            keybindings::matches(&keybindings, keybindings::Action::Redo, event);
                        let action = tab.vim.history_edit(&mut tab.text, redo);
                        apply_action(tab, action, &mut vim_register);
                        editor_input::defer(&ctx, tab.id, events.collect());
                        break;
                    }
                    egui::Event::Copy => {
                        copy_to_clipboard(tab, &mut system_clipboard);
                        continue;
                    }
                    egui::Event::Cut => {
                        cut_selection(tab, &mut system_clipboard, &mut vim_register);
                        continue;
                    }
                    egui::Event::Paste(text) => {
                        paste_system_text(tab, text);
                        continue;
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(text))
                        if tab.vim.mode == VimMode::Insert =>
                    {
                        // Composition previews remain separate from source;
                        // committed text uses the same native event chronology.
                        native_insert.push(egui::Event::Text(text.clone()));
                        continue;
                    }
                    egui::Event::Ime(_) => continue,
                    egui::Event::Key {
                        key: Key::C,
                        pressed: true,
                        modifiers,
                        ..
                    } if (modifiers.ctrl || modifiers.command) && !modifiers.alt => {
                        if !native_copy {
                            copy_to_clipboard(tab, &mut system_clipboard);
                        }
                        continue;
                    }
                    egui::Event::Key {
                        key: Key::X,
                        pressed: true,
                        modifiers,
                        ..
                    } if (modifiers.ctrl || modifiers.command) && !modifiers.alt => {
                        if !native_cut {
                            cut_selection(tab, &mut system_clipboard, &mut vim_register);
                        }
                        continue;
                    }
                    egui::Event::Key {
                        key: Key::V,
                        pressed: true,
                        modifiers,
                        ..
                    } if (modifiers.ctrl || modifiers.command) && !modifiers.alt => {
                        if !native_paste {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                        continue;
                    }
                    _ => {}
                }
                if tab.vim.mode == VimMode::Insert {
                    if matches!(
                        &event,
                        egui::Event::Key {
                            key: Key::Escape,
                            pressed: true,
                            ..
                        }
                    ) {
                        let action = tab.vim.handle_key(&mut tab.text, VimKey::Escape);
                        apply_action(tab, action, &mut vim_register);
                    } else {
                        native_insert.push(event);
                    }
                    continue;
                }
                // Vim's Ctrl+A / Ctrl+X: Ctrl+A and Ctrl+X stay Select All and
                // Cut, so Alt+A and Alt+X add to or subtract from a number,
                // unless the user bound those keys to a command.
                let number_step = match &event {
                    egui::Event::Key {
                        key: key @ (Key::A | Key::X),
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt
                        && !modifiers.ctrl
                        && !modifiers.command
                        && !modifiers.shift
                        && tab.vim.mode == VimMode::Normal
                        && !keybindings::Action::ALL
                            .iter()
                            .any(|action| keybindings::matches(&keybindings, *action, &event)) =>
                    {
                        Some(if *key == Key::A {
                            VimKey::Increment
                        } else {
                            VimKey::Decrement
                        })
                    }
                    _ => None,
                };
                if let Some(key) = number_step {
                    if events
                        .peek()
                        .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                    {
                        events.next();
                    }
                    let action = tab.vim.handle_key(&mut tab.text, key);
                    apply_action(tab, action, &mut vim_register);
                    continue;
                }
                match event {
                    egui::Event::Text(text) => {
                        let mut characters = text.chars().peekable();
                        while let Some(ch) = characters.next() {
                            if find.prompt.is_some() || find.waiting() {
                                let remaining: String =
                                    std::iter::once(ch).chain(characters).collect();
                                find.text(tab, &remaining);
                                break;
                            }
                            if tab.vim.mode == VimMode::Insert {
                                let remaining: String =
                                    std::iter::once(ch).chain(characters).collect();
                                native_insert.push(egui::Event::Text(remaining));
                                break;
                            }
                            if !tab.vim.expects_literal_character() {
                                if matches!(ch, '/' | '?') {
                                    vim_searched = true;
                                    find.start(tab, pane_id, ch == '?');
                                    continue;
                                }
                                if ch == ':' {
                                    tab.vim.cancel_pending_command();
                                    prompt = Some(ch);
                                    prompt_text.extend(characters);
                                    break;
                                }
                                if matches!(ch, 'n' | 'N') {
                                    vim_searched = true;
                                    find.repeat(tab, ch == 'N');
                                    publish_search_yank(tab, &mut find, &mut vim_register);
                                    continue;
                                }
                                if matches!(ch, '*' | '#') {
                                    vim_searched = true;
                                    find.word(tab, ch == '#');
                                    publish_search_yank(tab, &mut find, &mut vim_register);
                                    continue;
                                }
                                if ch == 'd'
                                    && tab.vim.mode == VimMode::Normal
                                    && tab.vim.pending_command() == "g"
                                {
                                    tab.vim.cancel_pending_command();
                                    definition = true;
                                    continue;
                                }
                            }
                            if ch == 'd' && tab.vim.mode == VimMode::Normal {
                                let pending = tab.vim.pending_command();
                                if pending == "[" || pending == "]" {
                                    tab.vim.cancel_pending_command();
                                    problem = Some(pending == "[");
                                    continue;
                                }
                            }
                            let ch = if tab.vim.expects_literal_character() {
                                ch
                            } else {
                                crate::vim::map_jk(ch, swap_jk)
                            };
                            let action = tab.vim.handle_key(&mut tab.text, VimKey::Char(ch));
                            apply_action(tab, action, &mut vim_register);
                        }
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let translated = if modifiers.ctrl
                            && !modifiers.alt
                            && !modifiers.shift
                            && matches!(key, Key::O | Key::I)
                            && tab.vim.mode == VimMode::Normal
                        {
                            // Vim's jump list. Ctrl+O stays Open file in
                            // every other mode and when it is rebound.
                            tab.vim.cancel_pending_command();
                            history = Some(key == Key::O);
                            None
                        } else if modifiers.ctrl && !modifiers.alt && key == Key::R {
                            Some(VimKey::Redo)
                        } else if modifiers.ctrl
                            && !modifiers.alt
                            && matches!(key, Key::F | Key::B | Key::D | Key::U)
                        {
                            let amount = if matches!(key, Key::D | Key::U) {
                                (page / 2).max(1)
                            } else {
                                page
                            };
                            let action = tab.vim.scroll_lines(
                                &mut tab.text,
                                if matches!(key, Key::B | Key::U) {
                                    -amount
                                } else {
                                    amount
                                },
                            );
                            apply_action(tab, action, &mut vim_register);
                            None
                        } else if !modifiers.command && !modifiers.ctrl && !modifiers.alt {
                            match key {
                                Key::Escape => Some(VimKey::Escape),
                                Key::Enter => Some(VimKey::Enter),
                                Key::Tab if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Tab)
                                }
                                Key::Backspace if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Backspace)
                                }
                                Key::ArrowLeft | Key::Backspace => Some(VimKey::Char('h')),
                                Key::ArrowDown => Some(VimKey::Char('j')),
                                Key::ArrowUp => Some(VimKey::Char('k')),
                                Key::ArrowRight => Some(VimKey::Char('l')),
                                Key::Home => Some(VimKey::Char('0')),
                                Key::End => Some(VimKey::Char('$')),
                                _ => None,
                            }
                        } else {
                            retained.push(egui::Event::Key {
                                key,
                                pressed: true,
                                repeat: false,
                                physical_key: None,
                                modifiers,
                            });
                            None
                        };
                        if let Some(key) = translated {
                            let action = tab.vim.handle_key(&mut tab.text, key);
                            apply_action(tab, action, &mut vim_register);
                        }
                    }
                    egui::Event::Paste(_) => {}
                    other => retained.push(other),
                }
            }
            yank_register = tab.vim.yank_register().cloned();
        }
        self.find = find;
        if vim_searched {
            self.find_visible = false;
        }
        self.find_message();
        if let Some(message) = self.tab_mut().and_then(|tab| tab.vim_message.take()) {
            self.status(message, true);
        }
        retained.extend(native_insert);
        ctx.input_mut(|input| input.events = retained);
        if let Some(yank) = vim_register {
            self.register = Some(yank);
        }
        self.yank_register = yank_register;
        if let Some(text) = system_clipboard {
            ctx.copy_text(text);
        }
        if let Some(':') = prompt {
            self.begin_ex_command(prompt_text);
        }
        if definition {
            self.request_definition();
        }
        if let Some(backwards) = problem {
            self.go_to_problem(backwards);
        }
        if let Some(backwards) = history {
            self.navigate_history(backwards);
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let options = crate::window_chrome::ChromeOptions {
            interaction_allowed: !self.chrome_blocked(),
            native_titlebar: self.ctx.data(|data| {
                data.get_temp::<bool>(egui::Id::new("native_window_chrome"))
                    .unwrap_or(false)
            }),
            native_maximize_state: self.ctx.data(|data| {
                data.get_temp::<(bool, bool)>(egui::Id::new("native_window_maximize_state"))
                    .unwrap_or_default()
            }),
        };
        let geometry = egui::Panel::top("header")
            .exact_size(crate::window_chrome::HEIGHT)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                crate::window_chrome::draw_top_bar(ui, options, |ui| {
                    self.workspace_header_button(ui);
                })
            })
            .inner;
        if geometry.close_requested {
            self.request_close(CloseAction::Exit);
        }
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("vedit_window_chrome"), geometry);
        });
    }

    fn chrome_blocked(&self) -> bool {
        egui::Popup::is_any_open(&self.ctx)
            || self.workspace_picker_open()
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.close_action.is_some()
            || self.context_file_dialog.is_some()
            || self.save_as.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
    }

    fn switch_project(&mut self, index: usize) {
        if index < self.projects.len() {
            self.set_active_project_context(index);
            self.bind_current_pane();
            self.session_dirty = true;
        }
    }
    fn activity(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("activity")
            .exact_size(49.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(5, 8)),
            )
            .show(ui, |ui| {
                self.main_menu_button(ui);
                ui.add_space(9.0);
                if !self.projects.is_empty() {
                    for (view, icon, hint) in [
                        (
                            SidebarView::Explorer,
                            Icon::Files,
                            format!(
                                "Explorer · {}",
                                self.shortcut_label(keybindings::Action::FocusExplorer)
                            ),
                        ),
                        (
                            SidebarView::Search,
                            Icon::Search,
                            format!(
                                "Search project · {}",
                                self.shortcut_label(keybindings::Action::SearchProject)
                            ),
                        ),
                        (
                            SidebarView::SourceControl,
                            Icon::Branch,
                            format!(
                                "Source control · {}",
                                self.shortcut_label(keybindings::Action::FocusGit)
                            ),
                        ),
                    ] {
                        if icon_button(ui, icon, self.sidebar_view_visible(view), &hint).clicked() {
                            self.toggle_sidebar_view(view);
                        }
                        ui.add_space(6.0);
                    }
                }
                if self.review.is_some() {
                    if icon_button(
                        ui,
                        Icon::Review,
                        self.sidebar_view_visible(SidebarView::Review),
                        "Agent review - important places and notes",
                    )
                    .clicked()
                    {
                        self.toggle_sidebar_view(SidebarView::Review);
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    if icon_button(
                        ui,
                        Icon::Console,
                        self.output_visible,
                        &format!(
                            "Output · {}",
                            self.shortcut_label(keybindings::Action::ToggleOutput)
                        ),
                    )
                    .clicked()
                    {
                        self.output_visible = !self.output_visible;
                    }
                    ui.add_space(8.0);
                    if icon_button(ui, Icon::Settings, false, "Settings").clicked() {
                        self.settings_open = true;
                    }
                });
            });
    }
    fn open_project_search(&mut self) {
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
    fn project_search_ui(&mut self, ui: &mut egui::Ui) {
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

impl VEditApp {
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let problems = self.problem_list();
        let statistics = self.tab().filter(|tab| !tab.loading).map(|tab| {
            self.text_statistics.display(
                ui.ctx(),
                tab.id,
                tab.revision,
                &tab.text,
                tab.lines.char_count(&tab.text, tab.revision),
                tab.selection.clone(),
            )
        });
        if statistics.is_none() {
            self.text_statistics.clear();
        }
        egui::Panel::bottom("status").exact_size(28.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 2)))
            .show(ui, |ui| {
                let compact = ui.available_width() < 820.0;
                let narrow = ui.available_width() < 360.0;
                let compact_statistics = ui.available_width() < 1100.0;
                let show_statistics = ui.available_width() >= 650.0;
                let status_error = self.status.as_ref().is_some_and(|(_, error, _)| *error);
                let right_budget = if narrow { 125.0 } else if !show_statistics { 300.0 } else if compact_statistics { 500.0 } else { 900.0 };
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.spacing_mut().interact_size.y = 20.0;
                ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
                ui.horizontal_centered(|ui| {
                    // Resting modes stay muted; color marks modes that change what keys do.
                    let (mode, color) = if self.screen_jump.is_some() { ("JUMP", ORANGE) } else if self.tab().is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview) { ("PREVIEW", MUTED) } else if self.settings.vim {
                        match self.tab().map(|t| t.vim.mode).unwrap_or(VimMode::Normal) {
                            VimMode::Normal => ("NORMAL", MUTED), VimMode::Insert => ("INSERT", BLUE),
                            VimMode::Visual => ("VISUAL", ORANGE), VimMode::VisualLine => ("V-LINE", ORANGE),
                        }
                    } else { ("EDIT", MUTED) };
                    ui.label(RichText::new(format!(" {mode} ")).monospace().size(11.0).strong().color(color))
                        .on_hover_text("Vim: i to insert, v to select, Shift+V for lines, Esc for Normal");
                    if let Some(tab) = self.tab() {
                        let pending = tab.vim.pending_command();
                        if !pending.is_empty() { ui.label(RichText::new(pending).monospace().size(11.0).color(ORANGE)); }
                    }
                    if !narrow && !(compact && status_error) && let Some(branch) = self.project().and_then(|p| p.git_branch.clone()) {
                        let tracking = self.project().and_then(|p| p.git_tracking.as_ref());
                        let hint = tracking.map(|b| format!("{} - {} ahead, {} behind. Open source control.", b.upstream.as_deref().unwrap_or("Local branch"), b.ahead, b.behind)).unwrap_or_else(|| "Open source control".into());
                        let width = ui.fonts_mut(|fonts| fonts.layout_no_wrap(branch.clone(), FontId::proportional(11.0), TEXT)).size().x + 16.0;
                        let budget = (ui.available_width() - right_budget).clamp(30.0, 180.0);
                        if ui.add_sized([width.min(budget), 20.0], egui::Button::new(RichText::new(&branch).size(11.0).color(MUTED)).truncate().frame_when_inactive(false).corner_radius(3.0)).on_hover_text(&hint).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { self.show_sidebar(SidebarView::SourceControl); }
                    }
                    if let Some(jump) = &self.screen_jump {
                        let hint = jump.state.hint();
                        ui.add(egui::Label::new(RichText::new(&hint).size(11.0).color(ORANGE)).truncate())
                            .on_hover_text("Type 1–3 characters to find visible text, then type its letter label to jump. Home-row letters come first; possible search continuations stay available for typing. Enter locks a shorter query or jumps to the only match. Three characters lock automatically. Backspace corrects a label, unlocks an Enter-confirmed query, or deletes a search character. Esc cancels.");
                    } else if let Some((message, error, time)) = &self.status
                        && (!compact || *error) && (*error || time.elapsed() < Duration::from_secs(6)) {
                        let width = (ui.available_width() - right_budget).clamp(24.0, 440.0);
                        ui.add_sized([width, 20.0], egui::Label::new(RichText::new(message).size(11.0).color(if *error { RED } else { MUTED })).truncate()).on_hover_text(message);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(t) = self.tab() {
                            let (line, col) = t.lines.line_col(&t.text, t.revision, t.vim.cursor);
                            let total = t.lines.line_count(&t.text, t.revision);
                            let position = if narrow { format!("{line}:{col}") } else { format!("Ln {line}/{total}, Col {col}") };
                            let mut position_hint = format!("Line {line}/{total} · Column {col}\nGo to line: {}", self.shortcut_label(keybindings::Action::GoToLine));
                            if !show_statistics && let Some(statistics) = &statistics {
                                position_hint.push_str(&format!("\n\n{}", statistics.tooltip()));
                            }
                            ui.label(RichText::new(position).size(11.0).color(MUTED)).on_hover_text(position_hint);
                            if show_statistics && let Some(statistics) = &statistics {
                                ui.label(RichText::new(statistics.label(compact_statistics)).size(11.0).color(MUTED)).on_hover_text(statistics.tooltip());
                            }
                            let details = format!("UTF-8{} - {} - {}", if t.bom { " with BOM" } else { "" }, t.line_ending, if t.agent_unsaved { "Agent draft: save to resume auto save" } else if self.settings.autosave { "Auto save on" } else { "Manual save" });
                            if !compact_statistics { ui.label(RichText::new(language_name(t.path.as_deref())).size(11.0).color(MUTED)).on_hover_text(details); }
                        }
                        if !compact_statistics { self.indentation_status_ui(ui); }
                        self.save_status_ui(ui, !narrow);
                        let problem_count = problems.counted();
                        if !compact_statistics && problem_count > 0 {
                            let label = if problem_count == 1 { "1 problem".to_owned() } else { format!("{problem_count} problems") };
                            let hint = format!("{}
Show Problems · {}
Next / previous error or warning: {} / {}", problems.summary(), self.shortcut_label(keybindings::Action::Problems), self.shortcut_label(keybindings::Action::NextProblem), self.shortcut_label(keybindings::Action::PreviousProblem));
                            if design::status_item(ui, &label, problems.badge_color(), &hint).clicked() { self.problems_visible = true; self.output_visible = true; }
                        }
                        if !compact_statistics { self.rust_analysis_status_ui(ui); }
                    });
                });
            });
    }
    fn output_ui(&mut self, ui: &mut egui::Ui) {
        if !self.output_visible {
            self.task_output_cache.clear();
            self.clear_hidden_output_focus();
            return;
        }
        let panel = egui::Panel::bottom("output")
            .default_size(240.0)
            .size_range(120.0..=600.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(14, 9)),
            )
            .show(ui, |ui| {
                self.begin_output_region(ui);
                ui.horizontal(|ui| {
                    let output_tab = design::ghost_button(ui, "Output", &format!("Command output · {}", self.shortcut_label(keybindings::Action::ToggleOutput)));
                    if !self.problems_visible && !self.project().is_some_and(|p| p.launch.show_output) {
                        ui.painter().line_segment(
                            [
                                output_tab.rect.left_bottom(),
                                output_tab.rect.right_bottom(),
                            ],
                            Stroke::new(1.5, ACCENT),
                        );
                    }
                    if output_tab.clicked() {
                        self.problems_visible = false;
                        if let Some(project) = self.projects.get_mut(self.active_project) { project.launch.show_output = false; }
                    }
                    self.launch_output_tab(ui);
                    let problems = self.problem_list();
                    let count = problems.counted();
                    let problems_tab = design::ghost_button(
                        ui,
                        &if count > 0 {
                            format!("Problems {count}")
                        } else {
                            "Problems".to_owned()
                        },
                        &format!("{} · {}", problems.summary(), self.shortcut_label(keybindings::Action::Problems)),
                    );
                    if self.problems_visible {
                        ui.painter().line_segment(
                            [
                                problems_tab.rect.left_bottom(),
                                problems_tab.rect.right_bottom(),
                            ],
                            Stroke::new(1.5, ACCENT),
                        );
                    }
                    if problems_tab.clicked() {
                        self.problems_visible = true;
                    }
                    ui.add_space(14.0);
                    let application_visible = self.project().is_some_and(|p| p.launch.show_output) && !self.problems_visible;
                    let run_status = self.project().map_or("Ready", |p| p.run.status.as_str()).to_owned();
                    let running = self.project().is_some_and(|p| p.run.task.is_some());
                    if !application_visible { ui.label(RichText::new(run_status).size(11.0).color(MUTED)); }
                    if running && !application_visible {
                        ui.spinner();
                    }
                    // The rail's Output icon and its shortcut hide the panel.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if design::ghost_button(
                            ui,
                            "Configure tasks",
                            "Edit this project's commands and Python interpreter",
                        )
                        .clicked()
                        {
                            self.open_project_task_settings();
                        }
                        if design::ghost_button(
                            ui,
                            "Terminal",
                            "Open an interactive terminal in this project for prompts, shells, and long-running commands.",
                        )
                        .clicked()
                        {
                            self.open_terminal();
                        }
                        if application_visible { return; }
                        let Some(run) = self.projects.get_mut(self.active_project).map(|p| &mut p.run) else { return; };
                        if let Some(task) = &run.task {
                            if design::ghost_button(ui, "Stop", "Stop the running command")
                                .clicked()
                            {
                                task.cancel();
                            }
                        } else if design::ghost_button(ui, "Clear", "Clear command output")
                            .clicked()
                        {
                            run.output.clear();
                        }
                        if design::ghost_button(ui, "Copy", "Copy all command output").clicked() {
                            ui.ctx().copy_text(run.output.clone());
                        }
                    });
                });
                if self.problems_visible {
                    self.task_output_cache.clear();
                    self.problems_ui(ui);
                    return;
                }
                let unsaved = self.project_task_unsaved_count();
                if unsaved > 0 {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("{unsaved} unsaved document(s). Save before running commands.")).size(11.0).color(ORANGE));
                        if design::ghost_button(ui, "Save all files", "Save edits explicitly, then run again after saving finishes").clicked() {
                            self.save_all();
                        }
                    });
                }
                if self.launch_output_ui(ui) { return; }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(">").monospace().color(ACCENT));
                    let response = ui.add_sized(
                        [ui.available_width() - 66.0, 28.0],
                        egui::TextEdit::singleline(&mut self.command)
                            .id(split::output_command_id())
                            .font(FontId::monospace(12.0))
                            .hint_text("Run a command in this project…"),
                    );
                    let enter = response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    self.register_region_focus_widget(response.id);
                    if ui
                        .add_enabled(
                            !self.project().is_some_and(|p| p.run.task.is_some()) && !self.command.trim().is_empty(),
                            egui::Button::new("Run").frame_when_inactive(false),
                        )
                        .on_hover_text("Commands run without input and stop after 10 minutes. Use Terminal for prompts and long-running commands.")
                        .clicked()
                        || enter
                    {
                        self.run_task(self.command.clone());
                    }
                });
                ui.separator();
                egui::ScrollArea::both()
                    .stick_to_bottom(true)
                    .id_salt("task_output")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let output = self.projects.get(self.active_project).map_or("", |p| p.run.output.as_str());
                        self.task_output_cache.show(ui, output, TEXT);
                    });
            });
        self.finish_output_region(ui, panel.response.rect);
    }
    fn editor_ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                if self.projects.is_empty() {
                    self.welcome(ui);
                } else if self.change_review_visible() {
                    self.change_review_ui(ui);
                } else {
                    self.split_editor_ui(ui);
                }
            });
    }
    fn editor_pane_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        self.tabs_ui(ui);
        ui.separator();
        if self.render_pane_is_focused() && self.find_visible {
            self.find_ui(ui);
        }
        if self.render_pane_is_focused() && self.vim_command.is_some() {
            self.ex_ui(ui);
        }
        if self.render_pane_is_focused() && self.vim_search_active() {
            self.vim_search_ui(ui);
        }
        self.disk_conflict_banner(ui);
        let error = self.tab().and_then(|t| t.error.clone());
        if let Some(error) = error {
            egui::Frame::new()
                .fill(PANEL.lerp_to_gamma(RED, 0.12))
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(error).size(12.0).color(RED));
                        if ui.button("Save as…").clicked()
                            && let Some(id) = self.tab().map(|tab| tab.id)
                        {
                            self.request_save_as(id);
                        }
                        let can_reload = self.tab().is_some_and(|tab| !tab.dirty && !tab.saving && !tab.loading && tab.path.is_some());
                        if ui.add_enabled(can_reload, egui::Button::new("Reload from disk"))
                            .on_disabled_hover_text("Keep your edits with Save as… first. To discard them, close this tab, choose Discard, then reopen the original file.")
                            .clicked() {
                            self.reload_current();
                        }
                        if self.tab().is_some_and(|tab| tab.dirty) {
                            ui.label(RichText::new("Your edits are kept. Save as… preserves them in another file.").size(11.0).color(TEXT));
                        }
                    });
                });
        }
        if self.tab().is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.28).max(40.0));
                brand(ui, 45.0);
                ui.add_space(20.0);
                ui.label(
                    RichText::new("Your workspace is ready.")
                        .size(23.0)
                        .color(TEXT),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("Find a file and make it yours.").color(MUTED));
                ui.add_space(24.0);
                if accent_button(
                    ui,
                    &format!(
                        "Open a file   {}",
                        self.shortcut_label(keybindings::Action::QuickOpen)
                    ),
                )
                .clicked()
                {
                    self.open_palette(Palette::Files);
                }
                ui.add_space(8.0);
                if ui.button("Create a new file").clicked() {
                    self.new_file();
                }
            });
            return;
        }
        if self.tab().is_some_and(|t| t.loading) {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.spinner();
                ui.label("Opening file…");
            });
            return;
        }
        if self
            .tab()
            .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
        {
            self.markdown_preview(ui);
        } else {
            self.text_editor(ui);
        }
    }
    fn set_document_view(&mut self, view: crate::cli::DocumentView) {
        self.cancel_vim_search(false);
        let ctx = self.ctx.clone();
        if let Some(tab) = self.tab_mut() {
            if view == crate::cli::DocumentView::Preview && !is_markdown(tab.path.as_deref()) {
                return;
            }
            let action = tab.vim.finish_insert(&mut tab.text);
            if action.text_changed {
                tab.changed();
            }
            tab.agent_view = view;
            tab.focus = view == crate::cli::DocumentView::Source;
            if !tab.focus {
                ctx.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
            }
        }
        self.completions = None;
        self.completion_request = None;
        self.find_visible = false;
        self.editor_focused = false;
        self.session_dirty = true;
        self.capture_pane_navigation();
    }
    fn markdown_preview(&mut self, ui: &mut egui::Ui) {
        self.editor_focused = false;
        self.editor_rect = None;
        self.editor_cursor_rect = None;
        let keyboard_active = self.render_pane_is_focused() && !self.explorer_keyboard_focused();
        let Some(project) = self.projects.get(self.active_project) else {
            return;
        };
        let Some(tab) = project.tabs.get(project.active_tab) else {
            return;
        };
        let path = tab
            .path
            .clone()
            .unwrap_or_else(|| project.root.join("Untitled.md"));
        if tab.scroll_to_cursor {
            let line = tab
                .lines
                .line_col(&tab.text, tab.revision, tab.vim.cursor)
                .0;
            self.markdown.reveal_line(tab.id, line);
        }
        let response = self.markdown.show(
            ui,
            tab.id,
            tab.revision,
            &tab.text,
            &path,
            self.settings.font_size,
            self.settings.smooth_scroll,
            keyboard_active,
            self.settings.swap_jk,
        );
        if let Some(tab) = self.tab_mut() {
            tab.scroll_to_cursor = false;
        }
        if let Some(path) = response.open_file {
            self.open_argument(path);
        }
        if let Some(line) = response.source_line {
            self.set_document_view(crate::cli::DocumentView::Source);
            if let Some(tab) = self.tab_mut() {
                let at = editor::go_to_line(&tab.text, line);
                tab.set_cursor(at..at);
            }
        }
    }
    fn review_ui(&mut self, ui: &mut egui::Ui) {
        let Some(review) = &self.review else {
            ui.label(RichText::new("No review to show").color(MUTED));
            return;
        };
        let title = review.title.clone();
        let note = review.note.clone();
        let files = review.files.clone();
        ui.label(RichText::new(title).size(17.0).strong().color(TEXT));
        if !note.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(note).size(12.0).color(MUTED));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} files", files.len()))
                    .size(10.5)
                    .color(MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if design::ghost_button(
                    ui,
                    "Clear",
                    "Remove review notes and markings; keep all documents open",
                )
                .clicked()
                {
                    for tab in self.projects.iter_mut().flat_map(|p| &mut p.tabs) {
                        tab.annotations.clear();
                        tab.annotations_revision = None;
                        tab.agent_location_error = None;
                    }
                    self.review = None;
                    self.clear_review_sidebar();
                }
            });
        });
        ui.separator();
        let mut jump = None;
        egui::ScrollArea::vertical()
            .id_salt("agent_review")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for id in files {
                    let Some((project_index, tab_index)) =
                        self.projects.iter().enumerate().find_map(|(pi, p)| {
                            p.tabs.iter().position(|t| t.id == id).map(|ti| (pi, ti))
                        })
                    else {
                        continue;
                    };
                    let project = &self.projects[project_index];
                    let tab = &project.tabs[tab_index];
                    let active =
                        self.active_project == project_index && project.active_tab == tab_index;
                    let path = tab
                        .path
                        .as_ref()
                        .map(|p| crate::paths::display(p.strip_prefix(&project.root).unwrap_or(p)))
                        .unwrap_or_else(|| tab.title.clone());
                    let label = if self.projects.len() > 1 {
                        format!("{} · {path}", file_title(&project.root))
                    } else {
                        path
                    };
                    let tooltip = tab.path.as_ref().map_or_else(
                        || {
                            format!(
                                "{}\nOpen this review document",
                                crate::paths::display(&project.root)
                            )
                        },
                        |path| {
                            format!("{}\nOpen this review document", crate::paths::display(path))
                        },
                    );
                    ui.add_space(5.0);
                    if ui
                        .push_id(id, |ui| {
                            design::left_row(
                                ui,
                                &label,
                                Some((
                                    file_badge(tab.path.as_deref().unwrap_or(Path::new(""))),
                                    file_color(tab.path.as_deref()),
                                )),
                                active,
                                &tooltip,
                            )
                        })
                        .inner
                        .clicked()
                    {
                        jump = Some((project_index, tab_index, None));
                    }
                    if tab
                        .annotations_revision
                        .is_some_and(|revision| revision != tab.revision)
                    {
                        ui.label(
                            RichText::new("Edited since review. Check the marked locations.")
                                .size(10.5)
                                .color(ORANGE),
                        );
                    }
                    if let Some(error) = &tab.agent_location_error {
                        ui.label(RichText::new(error).size(10.5).color(ORANGE));
                    }
                    for annotation in &tab.annotations {
                        let location = annotation
                            .end
                            .as_ref()
                            .map(|end| format!("Lines {}-{}", annotation.start.line, end.line))
                            .unwrap_or_else(|| format!("Line {}", annotation.start.line));
                        let severity = match annotation.severity {
                            crate::cli::AnnotationSeverity::Info => "Info",
                            crate::cli::AnnotationSeverity::Warning => "Warning",
                            crate::cli::AnnotationSeverity::Critical => "Critical",
                        };
                        let detail = format!("{severity} · {location}");
                        let label = if annotation.note.is_empty() {
                            "Marked location"
                        } else {
                            &annotation.note
                        };
                        let response = ui
                            .push_id((id, &annotation.id), |ui| {
                                design::detail_row(
                                    ui,
                                    label,
                                    &detail,
                                    false,
                                    &format!("{label} - {detail}. Click to reveal in source."),
                                )
                            })
                            .inner;
                        let color = annotation_color(annotation.severity);
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                response.rect.min + Vec2::new(0.0, 8.0),
                                Vec2::new(2.0, 32.0),
                            ),
                            1.0,
                            color,
                        );
                        if response.clicked() {
                            jump = Some((project_index, tab_index, Some(annotation.start.clone())));
                        }
                    }
                }
            });
        if let Some((pi, ti, location)) = jump {
            self.activate_tab(pi, ti);
            if let Some(location) = location {
                self.set_document_view(crate::cli::DocumentView::Source);
                if let Some(tab) = self.tab_mut() {
                    tab.pending_agent_location = Some(location);
                    tab.agent_location_error = None;
                }
                self.apply_pending_agent_locations();
            }
        }
    }
    fn tabs_ui(&mut self, ui: &mut egui::Ui) {
        let tabs: Vec<_> = self
            .current_pane_tab_ids()
            .into_iter()
            .filter_map(|id| {
                self.projects.iter().enumerate().find_map(|(pi, project)| {
                    project
                        .tabs
                        .iter()
                        .enumerate()
                        .find(|(_, tab)| tab.id == id)
                        .map(|(ti, tab)| (pi, ti, tab))
                })
            })
            .map(|(pi, ti, t)| {
                (
                    t.id,
                    t.title.clone(),
                    t.dirty,
                    t.loading,
                    t.path.clone(),
                    t.annotations.len(),
                    t.saving,
                    pi,
                    ti,
                    t.preview_tab,
                )
            })
            .collect();
        let tab_labels = crate::file_identity::disambiguate_labels(
            &tabs
                .iter()
                .map(|tab| (tab.1.as_str(), tab.4.as_deref()))
                .collect::<Vec<_>>(),
        );
        let active_id = self.tab().map(|tab| tab.id);
        let mut tab_drop_rects = Vec::new();
        let reveal_active = self.tab_strip_active != active_id;
        self.tab_strip_active = active_id;
        let mut select = None;
        let mut close = None;
        let mut context_action = None;
        let markdown = self
            .tab()
            .is_some_and(|tab| is_markdown(tab.path.as_deref()));
        let view = self.tab().map(|tab| tab.agent_view).unwrap_or_default();
        ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let controls_width = 44.0
            + if markdown { 38.0 } else { 0.0 }
            + match (self.notifications.items_empty(), self.notifications.unread()) {
                (true, _) => 0.0,
                (false, 0) => 28.0,
                _ => 40.0,
            };
        let tab_width = (ui.available_width() - controls_width).max(20.0);
        ui.allocate_ui_with_layout(Vec2::new(tab_width, 28.0), Layout::left_to_right(Align::Min), |ui| {
        let tab_scroll = egui::ScrollArea::horizontal()
            .id_salt("document_tabs")
            .auto_shrink([false, true])
            .scroll_source(egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Never, ..Default::default() })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    for ((id, original_title, dirty, loading, path, marks, saving, project_index, tab_index, preview), title) in tabs.iter().zip(&tab_labels) {
                        let selected = Some(*id) == active_id;
                        let font = FontId::proportional(12.0);
                        let title_galley = design::document_tab_galley(
                            ui, title,
                            font.clone(),
                            if selected { TEXT } else { MUTED },
                            f32::INFINITY, *preview,
                        );
                        let width = (title_galley.size().x + 59.0).clamp(96.0, if title == original_title { 220.0 } else { 290.0 });
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(width, 28.0), egui::Sense::click_and_drag());
                        tab_drop_rects.push((*id, rect));
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title)
                        });
                        if let Some(action) = context_actions::tab_menu(&response, path.is_some(), !loading && !saving, path.is_some() && !loading, &self.settings.keybindings) {
                            context_action = Some((*id, action));
                        }
                        if selected && reveal_active {
                            response.scroll_to_me(None);
                        }
                        let hovered = response.hovered();
                        if hovered || response.context_menu_opened() {
                            ui.painter()
                                .rect_filled(rect, 5.0, HOVER);
                        }
                        if selected {
                            ui.painter().line_segment(
                                [rect.left_bottom() + Vec2::new(8.0, -0.5), rect.right_bottom() + Vec2::new(-8.0, -0.5)],
                                Stroke::new(1.5, ACCENT.gamma_multiply(0.85)),
                            );
                        }
                        let badge = file_badge(path.as_deref().unwrap_or(Path::new("")));
                        ui.painter().text(
                            rect.left_center() + Vec2::new(9.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            badge,
                            FontId::monospace(9.5),
                            file_color(path.as_deref()),
                        );
                        let clip = egui::Rect::from_min_max(
                            rect.min + Vec2::new(30.0, 0.0),
                            rect.max - Vec2::new(29.0, 0.0),
                        );
                        let painter = ui.painter().with_clip_rect(clip);
                        if title != original_title && title_galley.size().x > clip.width() {
                            // Keep the distinguishing folder visible even when
                            // a long shared filename must be shortened.
                            let suffix = title.strip_prefix(original_title).unwrap_or_default();
                            let suffix = design::truncated_galley(ui, suffix, FontId::proportional(10.5), MUTED, clip.width() * 0.7);
                            let filename = design::document_tab_galley(ui, original_title, font, if selected { TEXT } else { MUTED }, (clip.width() - suffix.size().x - 4.0).max(1.0), *preview);
                            painter.galley(egui::pos2(clip.left(), rect.center().y - filename.size().y * 0.5), filename, TEXT);
                            painter.galley(egui::pos2(clip.right() - suffix.size().x, rect.center().y - suffix.size().y * 0.5), suffix, MUTED);
                        } else {
                            painter.galley(
                                egui::pos2(
                                    rect.left() + 30.0,
                                    rect.center().y - title_galley.size().y * 0.5,
                                ),
                                title_galley,
                                TEXT,
                            );
                        }
                        let close_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.right() - 16.0, rect.center().y),
                            Vec2::splat(20.0),
                        );
                        let close_response = ui.interact(
                            close_rect,
                            ui.make_persistent_id(("tab_close", id)),
                            egui::Sense::click(),
                        );
                        if !close_rect.contains(ui.input(|input| input.pointer.press_origin().unwrap_or_default())) {
                            self.start_tab_drag(&response, *id, title);
                        }
                        if hovered || close_response.hovered() || (selected && !*dirty) {
                            if close_response.hovered() {
                                ui.painter().rect_filled(close_rect, 4.0, SURFACE);
                            }
                            let c = close_rect.center();
                            ui.painter().line_segment(
                                [c - Vec2::splat(3.0), c + Vec2::splat(3.0)],
                                Stroke::new(1.2, MUTED),
                            );
                            ui.painter().line_segment(
                                [c + Vec2::new(-3.0, 3.0), c + Vec2::new(3.0, -3.0)],
                                Stroke::new(1.2, MUTED),
                            );
                            if close_response.clicked() {
                                close = Some(*id);
                            }
                            close_response
                                .on_hover_text(format!("Close tab in this pane · {} · Middle-click also closes", self.shortcut_label(keybindings::Action::CloseTab)))
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                        } else if *dirty {
                            ui.painter().circle_filled(close_rect.center(), 3.0, TEXT.gamma_multiply(0.8));
                        }
                        if *marks > 0 {
                            ui.painter().circle_filled(
                                egui::pos2(rect.left() + 22.0, rect.bottom() - 5.0),
                                2.2,
                                BLUE,
                            );
                        }
                        if *loading || *saving {
                            ui.painter().circle_filled(
                                egui::pos2(rect.left() + 24.0, rect.center().y),
                                2.0,
                                MUTED,
                            );
                        }
                        if response.clicked_by(egui::PointerButton::Middle) {
                            close = Some(*id);
                        }
                        if response.clicked()
                            && !self.tab_drag_active()
                            && !close_rect.contains(
                                ui.input(|input| input.pointer.interact_pos().unwrap_or_default()),
                            )
                        {
                            select = Some((*project_index, *tab_index));
                            if response.double_clicked() {
                                self.keep_tab_open(*id);
                            }
                        }
                        let mut tooltip = path.as_ref()
                            .map(|p| crate::paths::display(p))
                            .unwrap_or_else(|| "Untitled document".into());
                        if *dirty {
                            tooltip.push_str("\nUnsaved changes");
                        }
                        if *preview {
                            tooltip.push_str("\nPreview — double-click this tab to keep it open");
                        }
                        if *marks > 0 {
                            tooltip.push_str(&format!("\n{marks} review note{}", if *marks == 1 { "" } else { "s" }));
                        }
                        if *loading {
                            tooltip.push_str("\nLoading…");
                        } else if *saving {
                            tooltip.push_str("\nSaving…");
                        }
                        tooltip.push_str("\nDrag to reorder or move to another editor pane");
                        response
                            .on_hover_text(tooltip)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                    ui.add_space(7.0);
                    if small_button(ui, "+", &format!("New file · {}", self.shortcut_label(keybindings::Action::NewFile))).clicked() {
                        self.new_file();
                    }
                });
            });
        self.register_tab_drop_strip(&tab_scroll, std::mem::take(&mut tab_drop_rects));
        });
        ui.allocate_ui_with_layout(Vec2::new(controls_width, 28.0), Layout::right_to_left(Align::Center), |ui| {
            if self.tab_pointer_interaction_active() { ui.disable(); }
            self.agent_marks_toolbar(ui);
            self.notifications_toolbar(ui);
            if markdown {
                ui.add_space(5.0);
                self.markdown_preview_toggle(ui, view == crate::cli::DocumentView::Preview);
            }
        });
        });
        if let Some((project, index)) = select {
            self.activate_tab(project, index);
        }
        if let Some(id) = close {
            self.request_close(CloseAction::PaneTab {
                pane_id: self.current_pane_id(),
                id,
            });
        }
        if let Some((id, action)) = context_action {
            self.apply_tab_context_action(id, action);
        }
    }
    fn text_editor(&mut self, ui: &mut egui::Ui) {
        if self.render_pane_is_focused() {
            self.resume_editor_input();
        }
        self.sync_agent_marks();
        let explorer_focused = self.explorer_keyboard_focused();
        let search_matches = self.editor_search_matches();
        let search_pane_id = self.current_pane_id();
        let search_cursor = self
            .tab()
            .map_or(0, |tab| self.find.active_cursor(tab, search_pane_id));
        let tab_pointer_interaction = self.tab_pointer_interaction_active();
        let source_interaction = ui.is_enabled()
            && !tab_pointer_interaction
            && !egui::Popup::is_any_open(&self.ctx)
            && self.palette.is_none()
            && self.picker.is_none()
            && !self.settings_open
            && !self.agent_marks_modal_open()
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.vim_command.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && self.context_file_dialog.is_none()
            && !self.find_focus;
        let source_command_pending =
            self.navigation_pending() || self.screen_jump.is_some() || self.vim_search_active();
        let syntax = self.syntax.clone();
        let viewport_width = ui.available_width();
        let diagnostics = self
            .project()
            .and_then(|p| {
                self.tab()
                    .and_then(|t| t.path.as_ref().and_then(|path| p.diagnostics.get(path)))
            })
            .cloned()
            .unwrap_or_default();
        // Paint quieter severities first so an error marker stays on top.
        let mut diagnostics = diagnostics;
        diagnostics.sort_by_key(|diagnostic| std::cmp::Reverse(diagnostic.severity));
        let settings = self.settings.clone();
        let ctx = self.ctx.clone();
        let Some(document_id) = self.tab().map(|tab| tab.id) else {
            return;
        };
        let restore_search_scroll = self
            .find
            .restore_scroll
            .remove(&(search_pane_id, document_id));
        let id = self.document_widget_id(document_id);
        self.register_region_focus_widget(id);
        let mark_overlays = self.agent_mark_overlay_rows(document_id);
        let git_markers = self.git_gutter_markers(document_id);
        let mut git_placements = Vec::new();
        let mut git_click = None;
        let mut git_peek_anchor = (0.0, 0.0);
        let marker_owns_pointer =
            source_interaction && agent_marks_ui::marker_pointer_owns_editor(&ctx, id);
        let source_interaction = source_interaction && !marker_owns_pointer;
        let pane_focused = self.render_pane_is_focused();
        if !source_interaction && !tab_pointer_interaction && !marker_owns_pointer {
            // Give external dialogs their own keyboard/IME input before the
            // source widget is rendered. Tab drags retain editor focus so
            // cancellation can return to the unchanged modal selection.
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        let mut jump = self.screen_jump.take();
        if let Some(tab) = self.tab()
            && self
                .bookmark_lines
                .get(&document_id)
                .is_none_or(|(revision, _)| *revision != tab.revision)
        {
            let marks = tab
                .path
                .as_ref()
                .map(|path| {
                    self.bookmarks
                        .for_path(path)
                        .map(|bookmark| {
                            let position = crate::bookmarks::locate(&tab.text, bookmark);
                            (
                                position.line,
                                bookmark.id,
                                if bookmark.label.is_empty() {
                                    format!("Bookmark · {}:{}", position.line, position.column)
                                } else {
                                    bookmark.label.clone()
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.bookmark_lines
                .insert(document_id, (tab.revision, marks));
        }
        let bookmark_marks = self
            .bookmark_lines
            .get(&document_id)
            .map(|(_, marks)| marks.clone())
            .unwrap_or_default();
        let mut bookmark_click = None;
        let mut agent_mark_remove = None;
        let editor_command = if pane_focused
            && source_interaction
            && !source_command_pending
            && ctx.memory(|memory| memory.has_focus(id))
            && self.tab().is_some_and(|tab| {
                !tab.loading && (!settings.vim || tab.vim.mode == VimMode::Insert)
            }) {
            editor_input::take_command(&ctx, document_id, !settings.vim, &settings.keybindings)
        } else {
            None
        };
        if matches!(
            editor_command,
            Some(editor_input::Command::Undo | editor_input::Command::Redo)
        ) {
            self.context_native_undo(
                document_id,
                editor_command == Some(editor_input::Command::Redo),
            );
        }
        let Some(tab) = self.tab_mut() else { return };
        let mode = tab.vim.mode;
        let modal = settings.vim && mode != VimMode::Insert;
        let source_editable =
            source_interaction && !source_command_pending && !modal && !tab.loading;
        let pending = tab.pending_selection.take();
        if let Some(range) = pending {
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(range.start),
                    egui::text::CCursor::new(range.end),
                )));
            state.store(&ctx, id);
        }
        if !settings.vim && pane_focused {
            context_actions::bind_native_history(&ctx, document_id, id, &tab.text);
        }
        // Opening the context menu must retain the current selection. egui's
        // native pointer handler otherwise collapses it on any button press.
        let context_selection = ctx
            .input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary))
            .then(|| {
                egui::TextEdit::load_state(&ctx, id).and_then(|state| state.cursor.char_range())
            })
            .flatten();
        // Editing shortcuts use the widget's current character selection.
        let mut native_custom_edit = false;
        if pane_focused && source_editable && ctx.memory(|m| m.has_focus(id)) {
            let indentation = tab.indentation(&settings);
            let indent = indentation.unit();
            let edit = match editor_command {
                Some(editor_input::Command::Comment) => Some(editor::toggle_line_comment(
                    &tab.text,
                    tab.selection.clone(),
                    crate::file_kind::comment_language(tab.path.as_deref()),
                )),
                Some(editor_input::Command::Duplicate) => {
                    Some(editor::duplicate_lines(&tab.text, tab.selection.clone()))
                }
                Some(editor_input::Command::MoveLine(down)) => {
                    Some(editor::move_lines(&tab.text, tab.selection.clone(), down))
                }
                Some(editor_input::Command::Indent(outdent)) => {
                    Some(if !outdent && tab.selection.is_empty() {
                        editor::insert_tab(
                            &tab.text,
                            tab.selection.clone(),
                            indentation.width,
                            indentation.tabs,
                        )
                    } else {
                        editor::indent_lines_with_width(
                            &tab.text,
                            tab.selection.clone(),
                            &indent,
                            indentation.width,
                            outdent,
                        )
                    })
                }
                Some(editor_input::Command::Newline) => Some(editor::smart_newline(
                    &tab.text,
                    tab.selection.clone(),
                    &indent,
                )),
                _ => None,
            };
            if let Some(edit) = edit {
                native_custom_edit = !settings.vim && edit.text != tab.text;
                if native_custom_edit {
                    context_actions::native_history_checkpoint(
                        &ctx,
                        document_id,
                        id,
                        &tab.text,
                        tab.selection.clone(),
                    );
                }
                if edit.text != tab.text {
                    if matches!(
                        editor_command,
                        Some(editor_input::Command::MoveLine(_) | editor_input::Command::Duplicate)
                    ) {
                        tab.apply_line_move(edit);
                    } else {
                        tab.apply(edit);
                    }
                }
                let range = tab
                    .pending_selection
                    .take()
                    .unwrap_or_else(|| tab.selection.clone());
                let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(range.start),
                        egui::text::CCursor::new(range.end),
                    )));
                state.store(&ctx, id);
            }
        }
        let language = syntax_language(tab.path.as_deref()).to_owned();
        let font = settings.font_size;
        let gutter_digits = (tab
            .lines
            .line_count(&tab.text, tab.revision)
            .max(1)
            .ilog10()
            + 1) as f32;
        let mark_padding = if mark_overlays.is_empty() { 0.0 } else { 26.0 };
        let mut gutter_font = (font - 2.0).clamp(10.0, 20.0);
        let mut gutter_number_width = ui
            .fonts_mut(|fonts| fonts.glyph_width(&FontId::monospace(gutter_font), '8'))
            * gutter_digits;
        let maximum_number_width = 90.0 - mark_padding;
        if gutter_number_width > maximum_number_width {
            gutter_font *= maximum_number_width / gutter_number_width;
            gutter_number_width = maximum_number_width;
        }
        let gutter_marker_offset = (gutter_number_width + 27.0).max(47.0);
        let gutter_margin = (gutter_number_width + 37.0).max(56.0) + mark_padding;
        let document_id = tab.id;
        let revision = tab.revision;
        let mut focused = false;
        let mut rect = None;
        let mut caret_rect = None;
        let mut context_action = None;
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new(("document_scroll", tab.id)));
        let wheel_offset = (!settings.smooth_scroll)
            .then(|| crate::render::instant_scroll_offset(ui, scroll_id))
            .flatten();
        let selection_drag = source_interaction
            && !source_command_pending
            && pane_focused
            && !tab.loading
            && ctx.is_being_dragged(id)
            && ctx.input(|input| input.focused && input.pointer.primary_down());
        crate::selection_scroll::begin_drag(&ctx, scroll_id, selection_drag);
        let mut scroll = egui::ScrollArea::new([!settings.word_wrap, true])
            .id_salt(("document_scroll", tab.id))
            .animated(settings.smooth_scroll && !selection_drag)
            .scroll_source(egui::scroll_area::ScrollSource {
                mouse_wheel: settings.smooth_scroll,
                ..Default::default()
            })
            .auto_shrink([false, false]);
        if let Some(offset) = wheel_offset {
            scroll = scroll.scroll_offset(offset);
        }
        if let Some(offset) = restore_search_scroll {
            scroll = scroll.scroll_offset(offset);
            tab.scroll_to_cursor = false;
        }
        if selection_drag && let Some(state) = egui::scroll_area::State::load(&ctx, scroll_id) {
            scroll = scroll.scroll_offset(wheel_offset.unwrap_or(state.offset));
        }
        let scroll_output = scroll.show_viewport(ui, |ui, viewport| {
            // egui collapses native selection when another widget owns IME
            // events. Keep this document's range while Find or a toolbar has
            // focus, including a programmatic match selected before rendering.
            let unfocused_selection = (!modal && !ctx.memory(|memory| memory.owns_ime_events(id)))
                .then(|| {
                    egui::TextEdit::load_state(&ctx, id).and_then(|state| state.cursor.char_range())
                })
                .flatten();
            let indent_width = tab.indentation(&settings).width;
            // The last syntax galley handed to the editor, before native
            // selection painting recolors its glyphs.
            let laid_out = std::cell::RefCell::new(None::<std::sync::Arc<egui::Galley>>);
            let mut layouter =
                |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, available_width: f32| {
                    let width = if settings.word_wrap {
                        available_width.max(40.0)
                    } else {
                        f32::INFINITY
                    };
                    let galley = syntax.galley_with_preferences(
                        ui,
                        document_id,
                        revision,
                        buffer.as_str(),
                        &language,
                        font,
                        TEXT,
                        width,
                        settings.line_height,
                        indent_width,
                    );
                    *laid_out.borrow_mut() = Some(galley.clone());
                    galley
                };
            let native_caret = ui.visuals().text_cursor.stroke;
            let native_selection = ui.visuals().selection;
            editor_selection(ui.visuals_mut());
            if modal {
                // Modal navigation paints one block at Vim's active
                // character, rather than a second insertion caret at the
                // native half-open selection endpoint.
                ui.visuals_mut().text_cursor.stroke = Stroke::NONE;
            }
            // Immutable TextBuffer still permits native click/drag selection.
            // Mutable source is exposed only when this editor owns editing.
            let mut readonly_text: &str;
            let editor = if source_editable {
                egui::TextEdit::multiline(&mut tab.text)
            } else {
                readonly_text = tab.text.as_str();
                egui::TextEdit::multiline(&mut readonly_text)
            };
            let mut output = editor
                .id(id)
                .interactive(pane_focused && source_interaction)
                .frame(egui::Frame::new().inner_margin(Margin {
                    left: gutter_margin.ceil().min(127.0) as i8,
                    right: 24,
                    top: 10,
                    bottom: 100,
                }))
                .code_editor()
                .event_filter(egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    tab: true,
                    escape: settings.vim,
                })
                .desired_width(viewport_width)
                .min_size(egui::vec2(0.0, ui.available_height()))
                .desired_rows(1)
                .font(FontId::monospace(font))
                .text_color(TEXT)
                .background_color(BG)
                .layouter(&mut layouter)
                .show(ui);
            ui.visuals_mut().text_cursor.stroke = native_caret;
            ui.visuals_mut().selection = native_selection;
            if let Some(original) = laid_out.take()
                && !std::sync::Arc::ptr_eq(&original, &output.galley)
            {
                // Selected code keeps its syntax colors on the selection wash.
                let mut restored = output.galley.clone();
                crate::render::keep_glyph_colors(&mut restored, &original);
                if crate::render::replace_text_shape(ui, &output, restored.clone()) {
                    output.galley = restored;
                }
            }
            if tab.pending_navigation_feedback && !tab.loading && tab.pending_agent_location.is_none() {
                tab.pending_navigation_feedback = false;
                if tab.agent_location_error.is_none() && tab.error.is_none() {
                    tab.flash_navigation();
                }
            }
            if tab.focus && pane_focused && source_interaction && !explorer_focused {
                if !output.response.has_focus() {
                    output.response.request_focus();
                    ctx.memory_mut(|memory| {
                        memory.set_focus_lock_filter(
                            id,
                            egui::EventFilter {
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                tab: true,
                                escape: settings.vim,
                            },
                        )
                    });
                }
                tab.focus = false;
            }
            focused = output.response.has_focus();
            let native_selection_painted = focused
                && output.state.cursor.char_range().is_some_and(|range| !range.is_empty());
            rect = Some(output.response.rect.intersect(ui.clip_rect()));
            if let Some(range) = context_selection {
                output.state.cursor.set_char_range(Some(range));
                output.state.clone().store(&ctx, id);
            }
            // TextEditOutput.cursor_range predates pointer processing in
            // egui. The stored state contains the actual click/drag result.
            let pointer_selection = source_interaction && (output.response.is_pointer_button_down_on()
                && ui.input(|input| input.pointer.primary_down())
                || output.response.dragged_by(egui::PointerButton::Primary)
                || output.response.clicked()
                || output.response.double_clicked()
                || output.response.triple_clicked());
            crate::selection_scroll::scroll_selection(
                ui, &output.response, scroll_id, viewport.min.to_vec2(), !settings.word_wrap,
                source_interaction && pane_focused && !source_command_pending && !tab.loading,
            );
            let restored_native_selection =
                !pointer_selection && !output.response.changed() && unfocused_selection.is_some();
            if restored_native_selection && let Some(range) = unfocused_selection {
                output.state.cursor.set_char_range(Some(range));
                output.state.clone().store(&ctx, id);
            }
            if let Some(range) = output.state.cursor.char_range() {
                tab.selection =
                    range.as_sorted_char_range().start.0..range.as_sorted_char_range().end.0;
                if settings.vim && pointer_selection {
                    tab.vim.set_mouse_selection(
                        &tab.text,
                        range.primary.index.0,
                        range.secondary.index.0,
                        output.response.triple_clicked()
                            || mode == VimMode::VisualLine && ui.input(|i| i.modifiers.shift),
                    );
                } else if source_interaction && !modal && tab.vim.cursor != range.primary.index.0 {
                    tab.vim.set_cursor(&tab.text, range.primary.index.0);
                    if !settings.vim {
                        // A non-modal caret is an insertion boundary,
                        // including one past the last character of a line.
                        tab.vim.cursor = range.primary.index.0;
                    }
                }
            }
            if output.response.changed() {
                tab.vim.notify_external_text_change();
                tab.changed();
            }
            if modal && !pointer_selection {
                // A menu owns focus while egui may collapse its native range.
                // Build its actions from the actual Vim area, not that
                // temporary widget range; keep live drag anchors untouched.
                tab.selection = tab.vim.selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            }
            let context_menu_was_open = output.response.context_menu_opened();
            context_action = context_actions::editor_menu(
                ui,
                &output.response,
                !tab.loading,
                !tab.selection.is_empty(),
                &settings,
            );
            let retained_selection_painted = !native_selection_painted
                && (context_selection.is_some()
                    || context_menu_was_open
                    || output.response.context_menu_opened())
                && crate::render::paint_retained_text_selection(
                    ui,
                    &{
                        let mut visuals = ui.visuals().clone();
                        editor_selection(&mut visuals);
                        visuals
                    },
                    &output,
                    tab.selection.clone(),
                );
            let cursor = egui::text::CCursor::new(tab.vim.cursor);
            let local = output.galley.pos_from_cursor(cursor);
            let cursor_rect = local.translate(output.galley_pos.to_vec2());
            caret_rect = Some(cursor_rect);
            if settings.highlight_current_line && pane_focused {
                let highlight = egui::Rect::from_x_y_ranges(
                    output.response.rect.x_range(), cursor_rect.y_range(),
                ).intersect(ui.clip_rect());
                ui.painter().rect_filled(highlight, 0.0, ACCENT.linear_multiply(0.055));
            }
            if pane_focused && let Some((bracket, partner)) = tab.bracket_pair(settings.vim) {
                for index in [bracket, partner] {
                    let start = output.galley.pos_from_cursor(egui::text::CCursor::new(index));
                    let end = output.galley.pos_from_cursor(egui::text::CCursor::new(index + 1));
                    let right = if (end.min.y - start.min.y).abs() < 0.5 { end.min.x } else { start.min.x + start.height() * 0.5 };
                    let rect = egui::Rect::from_min_max(start.min, egui::pos2(right, start.max.y));
                    design::paint_bracket_match(ui.painter(), rect.translate(output.galley_pos.to_vec2()));
                }
            }
            // Cursor reveals keep this margin to the viewport edge, like Vim's
            // 'scrolloff'; zt / zb and H / L respect the same margin.
            const REVEAL_MARGIN: f32 = 35.0;
            if tab.scroll_to_cursor {
                let align = match tab.scroll_align.take() {
                    Some(crate::vim::ScrollAlign::Top) => Some(Align::Min),
                    Some(crate::vim::ScrollAlign::Center) => Some(Align::Center),
                    Some(crate::vim::ScrollAlign::Bottom) => Some(Align::Max),
                    None => tab.center_on_reveal.then_some(Align::Center),
                };
                ui.scroll_to_rect(cursor_rect.expand(REVEAL_MARGIN), align);
                tab.scroll_to_cursor = false;
                tab.center_on_reveal = false;
            }
            let clip = ui.clip_rect();
            let rows = &output.galley.rows;
            let first =
                rows.partition_point(|r| output.galley_pos.y + r.pos.y + r.size.y < clip.top());
            let last = rows.partition_point(|r| output.galley_pos.y + r.pos.y < clip.bottom());
            let visible_rows = syntax.visible_rows(document_id, &output.galley, first..last);
            if pane_focused {
                // Vim H / M / L address the visible rows of this pane outside
                // the reveal margin, unless the document starts or ends there.
                let top = |row: usize| rows.get(row).map(|r| output.galley_pos.y + r.pos.y);
                let starts = top(0).is_some_and(|y| y >= clip.top() - 0.5);
                let ends = rows.last().is_some_and(|r| {
                    output.galley_pos.y + r.pos.y + r.size.y <= clip.bottom() + 0.5
                });
                let upper = clip.top() + if starts { 0.0 } else { REVEAL_MARGIN };
                let lower = clip.bottom() - if ends { 0.0 } else { REVEAL_MARGIN };
                let fully_visible = |(row, _): &&(usize, crate::render::VisualRow)| {
                    rows.get(*row).is_some_and(|r| {
                        output.galley_pos.y + r.pos.y >= upper - 0.5
                            && output.galley_pos.y + r.pos.y + r.size.y <= lower + 0.5
                    })
                };
                let shown = visible_rows.iter().find(fully_visible).zip(visible_rows.iter().rev().find(fully_visible));
                tab.vim.set_visible_range(shown.map(|((_, top), (_, bottom))| top.start..bottom.end));
            }
            find::paint_matches(ui, &output.galley, output.galley_pos, &visible_rows, &search_matches, search_cursor);
            agent_mark_remove = agent_marks_ui::paint_agent_mark_overlays(
                ui, id, &output.galley, output.galley_pos, &visible_rows,
                if tab.revision == revision { &mark_overlays } else { &[] },
            );
            if pane_focused && let Some(feedback) = &tab.feedback {
                feedback.paint(ui, &output.galley, output.galley_pos, &visible_rows, tab.revision);
            }
            if let Some(markers) = &git_markers {
                let line_count = tab.lines.line_count(&tab.text, tab.revision);
                git_placements = git_gutter::paint_markers(
                    ui, markers, &output.galley, output.galley_pos, &visible_rows, line_count,
                );
                git_peek_anchor = (output.galley_pos.x, clip.width());
                for placement in &git_placements {
                    let hunk = &markers.hunks[placement.index];
                    let target = placement.rect.expand2(Vec2::new(3.0, 0.0)).intersect(clip);
                    let response = ui
                        .interact(target, id.with(("git_change", placement.index)), egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_ui(|ui| git_gutter::tooltip_ui(ui, markers, hunk));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Git change")
                    });
                    if response.clicked() {
                        git_click = Some(hunk.clone());
                    }
                }
            }
            if pane_focused && let Some(jump) = jump.as_mut()
                && jump.tab_id == tab.id && jump.revision == tab.revision
            {
                jump.glyphs.clear();
                for &(row_index, mapped) in &visible_rows {
                    let row = &rows[row_index];
                    for (offset, glyph) in row.glyphs.iter().enumerate() {
                        let min = output.galley_pos + row.pos.to_vec2() + egui::vec2(glyph.pos.x, 0.0);
                        let rect = egui::Rect::from_min_size(min, egui::vec2(glyph.advance_width.max(1.0), row.size.y));
                        if clip.contains_rect(rect) {
                            jump.glyphs.push(crate::screen_jump::VisibleGlyph { char_index: mapped.start + offset, character: glyph.chr, rect });
                        }
                    }
                }
                jump.state.recompute(&jump.glyphs);
            }
            if !settings.vim && !retained_selection_painted
                && (!focused || restored_native_selection) && !tab.selection.is_empty()
            {
                // Keep search matches and another pane's selection visible
                // while keyboard focus belongs to Find or a different pane.
                for (row, mapped) in &visible_rows {
                    if let Some(selected) = crate::render::range_on_visual_row(
                        &output.galley,
                        *row,
                        *mapped,
                        tab.selection.start,
                        tab.selection.end,
                    ) {
                        ui.painter().rect_filled(
                            selected.translate(output.galley_pos.to_vec2()),
                            0.0,
                            EDITOR_SELECTION.linear_multiply(0.6),
                        );
                    }
                }
            }
            let current_line = tab
                .lines
                .line_col(&tab.text, tab.revision, tab.vim.cursor)
                .0;
            for annotation in &tab.annotations {
                if tab
                    .annotations_revision
                    .is_some_and(|revision| revision != tab.revision)
                {
                    continue;
                }
                let start_column = if annotation.end.is_none() {
                    1
                } else {
                    annotation.start.column
                };
                let Some(start) = tab.lines.position(
                    &tab.text,
                    tab.revision,
                    annotation.start.line,
                    start_column,
                ) else {
                    continue;
                };
                let end = match &annotation.end {
                    Some(end) => tab
                        .lines
                        .position(&tab.text, tab.revision, end.line, end.column)
                        .and_then(|at| {
                            tab.lines
                                .line_end(&tab.text, tab.revision, end.line)
                                .map(|line_end| at.saturating_add(1).min(line_end))
                        }),
                    None => tab
                        .lines
                        .line_end(&tab.text, tab.revision, annotation.start.line),
                };
                let Some(end) = end else { continue };
                let color = annotation_color(annotation.severity);
                for &(row_index, mapped) in &visible_rows {
                    let Some(rect) = crate::render::range_on_visual_row(
                        &output.galley,
                        row_index,
                        mapped,
                        start,
                        end,
                    ) else {
                        continue;
                    };
                    let rect = rect.translate(output.galley_pos.to_vec2()).intersect(clip);
                    ui.painter().rect_filled(
                        rect,
                        2.0,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 25),
                    );
                    ui.painter().line_segment(
                        [
                            egui::pos2(output.galley_pos.x - gutter_marker_offset - 3.0, rect.top() + 2.0),
                            egui::pos2(output.galley_pos.x - gutter_marker_offset - 3.0, rect.bottom() - 2.0),
                        ],
                        Stroke::new(2.0, color),
                    );
                    ui.interact(
                        rect,
                        ui.id()
                            .with(("review_mark", tab.id, &annotation.id, row_index)),
                        egui::Sense::hover(),
                    )
                    .on_hover_text(&annotation.note);
                }
            }
            for diagnostic in &diagnostics {
                let start = crate::lsp::position_to_char(&tab.text, diagnostic.range.start);
                let end =
                    crate::lsp::position_to_char(&tab.text, diagnostic.range.end).max(start + 1);
                let start_row = output
                    .galley
                    .pos_from_cursor(egui::text::CCursor::new(start));
                for &(row_index, mapped) in &visible_rows {
                    if let Some(segment) = crate::render::range_on_visual_row(
                        &output.galley,
                        row_index,
                        mapped,
                        start,
                        end,
                    ) {
                        let severity = problems::Severity::from_lsp(diagnostic.severity);
                        let color = severity.color();
                        // Hints stay a faint underline without a gutter dot.
                        if severity != problems::Severity::Hint
                            && segment.top() == start_row.top()
                        {
                            ui.painter().circle_filled(
                                egui::pos2(
                                    output.galley_pos.x - gutter_marker_offset + 6.0,
                                    output.galley_pos.y + segment.center().y,
                                ),
                                2.8,
                                color,
                            );
                        }
                        ui.painter().line_segment(
                            [
                                output.galley_pos + segment.left_bottom().to_vec2()
                                    - Vec2::new(0.0, 1.0),
                                output.galley_pos + segment.right_bottom().to_vec2()
                                    - Vec2::new(0.0, 1.0),
                            ],
                            Stroke::new(
                                1.0,
                                if severity == problems::Severity::Hint {
                                    color.gamma_multiply(0.6)
                                } else {
                                    color
                                },
                            ),
                        );
                        ui.interact(
                            segment.translate(output.galley_pos.to_vec2()),
                            ui.id().with(("diagnostic", tab.id, start, row_index)),
                            egui::Sense::hover(),
                        )
                        .on_hover_text(format!("{}: {}", severity.label(), diagnostic.message));
                    }
                }
            }
            for &(i, mapped) in &visible_rows {
                if mapped.first_in_line {
                    let row = &rows[i];
                    let y = output.galley_pos.y + row.pos.y;
                    for &(line, id, ref label) in &bookmark_marks {
                        if line != mapped.line { continue; }
                        let center = egui::pos2(output.galley_pos.x - gutter_marker_offset, y + row.size.y * 0.5);
                        let rect = egui::Rect::from_center_size(center, egui::vec2(11.0, 17.0)).intersect(clip);
                        ui.painter().add(egui::Shape::convex_polygon(vec![
                            center + Vec2::new(-3.5, -5.0), center + Vec2::new(3.5, -5.0),
                            center + Vec2::new(3.5, 5.0), center + Vec2::new(0.0, 2.5), center + Vec2::new(-3.5, 5.0),
                        ], ORANGE, Stroke::NONE));
                        let response = ui.interact(rect, ui.id().with(("bookmark_gutter", id)), egui::Sense::click()).on_hover_text(format!("{label}\nClick to jump to this bookmark · Space m toggles a bookmark"));
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Bookmark at line {line}")));
                        if response.clicked() { bookmark_click = Some(id); }
                    }
                    if settings.line_numbers == crate::cli::LineNumbers::Off { continue; }
                    let color = if mapped.line == current_line {
                        TEXT
                    } else {
                        MUTED.linear_multiply(0.7)
                    };
                    let number = ui.painter().layout_no_wrap(
                        if settings.line_numbers == crate::cli::LineNumbers::Relative && mapped.line != current_line {
                            mapped.line.abs_diff(current_line).to_string()
                        } else {
                            mapped.line.to_string()
                        },
                        FontId::monospace(gutter_font),
                        color,
                    );
                    let source_baseline = row.glyphs.first().map(|glyph| glyph.pos.y)
                        .unwrap_or_else(|| {
                            // Use the same line box as source; natural font ascent
                            // alone loses the pixel-rounded leading on empty rows.
                            let mut job = egui::text::LayoutJob::simple("0".into(), FontId::monospace(font), color, f32::INFINITY);
                            job.sections[0].format.line_height = Some(font * settings.line_height);
                            let probe = ui.painter().layout_job(job);
                            probe.rows[0].glyphs[0].pos.y
                        });
                    let number_baseline = number.rows[0].glyphs[0].pos.y;
                    ui.painter().galley(
                        egui::pos2(output.galley_pos.x - 17.0 - number.size().x, y + source_baseline - number_baseline),
                        number, color,
                    );
                }
            }
            ui.painter().line_segment(
                [
                    egui::pos2(output.galley_pos.x - 8.0, clip.top()),
                    egui::pos2(output.galley_pos.x - 8.0, clip.bottom()),
                ],
                Stroke::new(1.0, BORDER),
            );
            if modal && focused {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        cursor_rect.min,
                        Vec2::new(font * 0.6, cursor_rect.height().max(font)),
                    ),
                    1.0,
                    ACCENT.linear_multiply(0.65),
                );
            }
            if modal {
                let range = tab
                    .vim
                    .selection(&tab.text)
                    .unwrap_or(tab.vim.cursor..tab.vim.cursor);
                tab.selection = range.clone();
                // During a drag egui owns its original pointer anchor.
                // Replacing it with a sorted range makes reverse drags jump.
                if !pointer_selection {
                    output
                        .state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(range.start),
                            egui::text::CCursor::new(range.end),
                        )));
                }
                output.state.store(&ctx, id);
            } else if !settings.vim && pane_focused {
                context_actions::publish_native_history(
                    &ctx,
                    document_id,
                    &output.state,
                    output.response.changed() || native_custom_edit,
                );
            }
            if pane_focused && let Some(jump) = &jump && jump.tab_id == tab.id {
                jump.state.paint_with_caret(ui, clip, Some(cursor_rect));
            }
        });
        crate::render::remember_scroll_limits(&ctx, &scroll_output);
        self.find.view_scroll.insert(
            (search_pane_id, document_id),
            (scroll_output.id, scroll_output.state.offset),
        );
        self.editor_focused = focused;
        self.editor_rect = rect;
        self.editor_cursor_rect = caret_rect;
        self.screen_jump = jump;
        if let Some(mark_id) = agent_mark_remove {
            self.agent_marks_remove(mark_id);
            if let Some(tab) = self.tab_mut() {
                tab.focus = true;
            }
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
        if let Some(id) = bookmark_click {
            self.jump_to_bookmark(id);
        }
        if let (Some(hunk), Some(markers)) = (git_click, &git_markers) {
            let peek = git_gutter::GutterPeek {
                pane: search_pane_id,
                document: document_id,
                revision: markers.revision,
                hunk,
            };
            self.git_gutter.peek = (self.git_gutter.peek.as_ref() != Some(&peek)).then_some(peek);
        }
        self.git_peek_ui(
            document_id,
            git_markers.as_ref(),
            &git_placements,
            git_peek_anchor.0,
            git_peek_anchor.1 - 40.0,
        );
        if let Some(action) = context_action {
            self.apply_editor_context_action(document_id, action);
        }
        self.finish_editor_input(document_id);
    }
    fn find_ui(&mut self, ui: &mut egui::Ui) {
        let find_id = ui.make_persistent_id("find_query");
        let replace_id = ui.make_persistent_id("replace_query");
        let interactive = !self.settings_open
            && !self.agent_marks_modal_open()
            && self.palette.is_none()
            && self.picker.is_none()
            && self.close_action.is_none()
            && self.save_as.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && !self.navigation_pending()
            && self.screen_jump.is_none()
            && self.vim_command.is_none()
            && self.find.prompt.is_none()
            && self.context_file_dialog.is_none();
        let mut close = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let controls = |app: &mut Self, ui: &mut egui::Ui| {
            let case = ui
                .toggle_value(&mut app.find_case, "Aa")
                .on_hover_text("Match upper and lower case exactly");
            case.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    app.find_case,
                    "Match case",
                )
            });
            let word = ui
                .toggle_value(&mut app.find_word, "ab")
                .on_hover_text("Match whole words only");
            word.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    app.find_word,
                    "Whole word",
                )
            });
            let regex = ui.toggle_value(&mut app.find_regex, ".*").on_hover_text(
                "Use a regular expression (Rust syntax); Replace inserts groups with $1",
            );
            regex.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    app.find_regex,
                    "Use regular expression",
                )
            });
            if regex.changed() {
                app.session_dirty = true;
            }
            if case.changed() || word.changed() || regex.changed() {
                app.find_match = None;
                if !app.find_text.is_empty() {
                    app.find_next(false);
                }
                if let Some(tab) = app.tab_mut() {
                    tab.focus = false;
                }
            }
            let matches = app.editor_search_matches();
            ui.add_enabled_ui(!matches.is_empty(), |ui| {
                let previous = small_button(
                    ui,
                    "↑",
                    &format!(
                        "Previous match · {}",
                        app.shortcut_label(keybindings::Action::FindPrevious)
                    ),
                );
                previous.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        "Previous match",
                    )
                });
                let next = small_button(
                    ui,
                    "↓",
                    &format!(
                        "Next match · {} or Enter",
                        app.shortcut_label(keybindings::Action::FindNext)
                    ),
                );
                next.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        "Next match",
                    )
                });
                if previous.clicked() || next.clicked() {
                    app.find_next(previous.clicked());
                    if let Some(tab) = app.tab_mut() {
                        tab.focus = false;
                    }
                }
            });
            let current = app
                .tab()
                .and_then(|tab| {
                    let cursor = app.find.active_cursor(tab, app.current_pane_id());
                    let index = matches.partition_point(|range| range.start < cursor);
                    matches
                        .get(index)
                        .filter(|range| range.start == cursor)
                        .map(|_| index)
                })
                .map_or(0, |index| index + 1);
            let searching = app.find_search_running();
            if let Some(error) = app.editor_search_error() {
                let invalid = ui
                    .label(RichText::new("Invalid pattern").size(11.0).color(RED))
                    .on_hover_text(format!("This regular expression cannot search: {error}"));
                invalid.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Invalid pattern")
                });
                return;
            }
            ui.label(
                RichText::new(if searching {
                    "Searching…".into()
                } else {
                    format!("{current} of {}", matches.len())
                })
                .size(11.0)
                .color(
                    if !searching && !app.find_text.is_empty() && matches.is_empty() {
                        ORANGE
                    } else {
                        MUTED
                    },
                ),
            )
            .on_hover_text(if searching {
                "Searching this document in the background; keep typing to refine the query"
            } else if matches.is_empty() {
                "No matching text in this document"
            } else {
                "Current match and total matches · Use the next / previous buttons or their configured shortcuts"
            });
        };
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                if !interactive { ui.disable(); }
                ui.spacing_mut().item_spacing.y = 5.0;
                let compact = ui.available_width() < 600.0;
                ui.horizontal(|ui| {
                    let available = ui.available_width();
                    let toggle = small_button(
                        ui,
                        if self.replace_visible { "▾" } else { "▸" },
                        "Toggle replace",
                    );
                    toggle.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, self.replace_visible, "Show replace"));
                    if toggle.clicked() {
                        self.replace_visible = !self.replace_visible;
                    }
                    let query_width = (available - if compact { 72.0 } else { 320.0 }).clamp(30.0, 360.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.find_text)
                            .id(find_id)
                            .hint_text("Find in this file")
                            .desired_width(query_width),
                    ).on_hover_text(format!("Find text, or a regular expression with .* · Enter / {}: next · {}: previous · Esc: close. Vim Normal: / searches; the default Ctrl+F scrolls a page.", self.shortcut_label(keybindings::Action::FindNext), self.shortcut_label(keybindings::Action::FindPrevious)));
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Find in file"));
                    if self.find_focus {
                        response.request_focus();
                        self.find_focus = false;
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    if response.changed() {
                        self.find_match = None;
                        if !self.find_text.is_empty() { self.find_next(false); }
                        response.request_focus();
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    let previous = (response.has_focus() || response.lost_focus())
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Enter));
                    if previous || ((response.has_focus() || response.lost_focus())
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)))
                    {
                        if !self.find_text.is_empty() { self.find_next(previous); }
                        response.request_focus();
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    if !compact { controls(self, ui); }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let response = small_button(ui, "×", "Close find · Esc");
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close find"));
                        close |= response.clicked();
                    });
                });
                if compact { ui.horizontal_wrapped(|ui| { controls(self, ui); }); }
                if self.replace_visible {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - if compact { 12.0 } else { 174.0 }).clamp(30.0, 360.0);
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.replace_text)
                                .id(replace_id)
                                .hint_text("Replace with")
                                .desired_width(width),
                        ).on_hover_text("Replacement text · Enter: replace this match · Esc: close Find and Replace");
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Replace with"));
                        if (response.has_focus() || response.lost_focus()) && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                            replace_one = true;
                        }
                        if !compact {
                            let enabled = self.tab().is_some_and(|tab| !tab.loading && !tab.saving && tab.error.is_none()) && !self.editor_search_matches().is_empty();
                            ui.add_enabled_ui(enabled, |ui| {
                                replace_one |= design::ghost_button(ui, "Replace", "Replace the current match and move to the next").clicked();
                                replace_all |= design::ghost_button(ui, "Replace all", "Replace every match in this document").clicked();
                            });
                        }
                    });
                    if compact {
                        let enabled = self.tab().is_some_and(|tab| !tab.loading && !tab.saving && tab.error.is_none()) && !self.editor_search_matches().is_empty();
                        ui.add_enabled_ui(enabled, |ui| { ui.horizontal_wrapped(|ui| {
                            replace_one |= design::ghost_button(ui, "Replace", "Replace the current match and move to the next").clicked();
                            replace_all |= design::ghost_button(ui, "Replace all", "Replace every match in this document").clicked();
                        }); });
                    }
                }
            });
        // Escape belongs to the whole toolbar, including Replace and option buttons.
        close |= interactive
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
        if close {
            self.find_visible = false;
            ui.ctx().memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            if let Some(tab) = self.tab_mut() {
                tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
            }
            return;
        }
        let editable = self
            .tab()
            .is_some_and(|tab| !tab.loading && !tab.saving && tab.error.is_none());
        if editable && replace_one {
            let matches = self.editor_search_matches();
            let cursor = self.tab().map_or(0, |tab| {
                self.find.active_cursor(tab, self.current_pane_id())
            });
            if let Some(range) = matches
                .iter()
                .find(|range| range.start >= cursor)
                .or_else(|| matches.first())
                .cloned()
            {
                let options = self.find_options();
                let replacement = if options.regex {
                    // Expand `$1` for this match exactly as Replace all would.
                    let query = self.find_text.clone();
                    let template = self.replace_text.clone();
                    self.tab()
                        .and_then(|tab| {
                            editor::replacements(&tab.text, &query, &template, options)
                                .into_iter()
                                .find(|(found, _)| *found == range)
                        })
                        .map_or(template, |(_, expanded)| expanded)
                } else {
                    self.replace_text.clone()
                };
                // Without Vim, the text widget owns Undo: record the text
                // before this programmatic edit so Ctrl+Z restores it exactly.
                if let Some(id) = self.tab().map(|tab| tab.id) {
                    self.context_native_undo_checkpoint(id);
                }
                if let Some(tab) = self.tab_mut() {
                    let edit = editor::replace_selection(&tab.text, range, &replacement);
                    let cursor = edit.selection.end;
                    tab.apply(edit);
                    tab.vim.cursor = cursor;
                }
                self.find_match = None;
                // Start at the replacement's end so removing a match cannot skip
                // the next match that has just moved into that same position.
                let no_matches = self.editor_search_matches().is_empty();
                if no_matches && !self.find_search_running() {
                    self.status("Replaced the last match", false);
                } else {
                    self.find_next(false);
                }
                if let Some(tab) = self.tab_mut() {
                    tab.focus = false;
                }
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(replace_id));
            }
        }
        if editable && replace_all {
            let query = self.find_text.clone();
            let replacement = self.replace_text.clone();
            let options = self.find_options();
            if let Some(id) = self.tab().map(|tab| tab.id) {
                self.context_native_undo_checkpoint(id);
            }
            if let Some(tab) = self.tab_mut() {
                let (edit, count) = editor::replace_all(
                    &tab.text,
                    tab.selection.clone(),
                    &query,
                    &replacement,
                    options,
                );
                if count > 0 {
                    tab.apply(edit);
                }
                tab.focus = false;
                self.find_match = None;
                self.status(format!("Replaced {count} matches"), false);
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(replace_id));
            }
        }
    }
    fn reload_current(&mut self) {
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
    fn ex_ui(&mut self, ui: &mut egui::Ui) {
        let mut run = None;
        let mut cancel = false;
        let mut changed = false;
        self.ex_input_prefix();
        self.ex_history_key(ui);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(":").monospace().color(ACCENT));
            if let Some(command) = &mut self.vim_command {
                let response = ui.add(
                    egui::TextEdit::singleline(command)
                        .id(egui::Id::new("ex-command"))
                        .font(FontId::monospace(13.0))
                        .hint_text("w · q · bn/bp · split · %s/old/new/g · ↑↓ history")
                        .desired_width(ui.available_width()),
                );
                response.request_focus();
                changed |= response.changed();
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    run = Some(command.clone());
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                    cancel = true;
                }
            }
        });
        if changed {
            self.ex_command_changed();
        }
        let finished = cancel || run.is_some();
        if cancel || run.is_some() {
            self.vim_command = None;
            if let Some(t) = self.tab_mut() {
                t.focus = true;
            }
        }
        if let Some(command) = run {
            self.execute_ex_command(&command);
        }
        if finished {
            self.finish_ex_command();
        }
    }
    fn welcome(&mut self, ui: &mut egui::Ui) {
        let available_height = ui.available_height();
        let compact = available_height < 700.0;
        let space = if compact { 0.6 } else { 1.0 };
        egui::ScrollArea::vertical()
            .id_salt("welcome")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(if compact {
                        14.0
                    } else {
                        (available_height * 0.15).max(35.0)
                    });
                    brand(ui, if compact { 48.0 } else { 58.0 });
                    ui.add_space(22.0 * space);
                    ui.label(
                        RichText::new("A quieter place to build.")
                            .size(if compact { 28.0 } else { 34.0 })
                            .strong()
                            .color(TEXT),
                    );
                    ui.add_space(10.0 * space);
                    ui.label(
                        RichText::new("Your projects. Your motions. Room to focus.")
                            .size(15.0)
                            .color(MUTED),
                    );
                    ui.add_space(if compact { 17.0 } else { 35.0 });
                    if accent_button(ui, "Open a project folder").clicked() {
                        self.pick_project();
                    }
                    ui.add_space(12.0 * space);
                    ui.label(
                        RichText::new(self.shortcut_label(keybindings::Action::OpenProject))
                            .monospace()
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(if compact { 19.0 } else { 46.0 });
                    egui::Frame::new()
                        .fill(PANEL)
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(10.0)
                        .inner_margin(if compact { 12 } else { 24 })
                        .show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(
                                RichText::new("Built around your flow")
                                    .size(11.5)
                                    .color(MUTED),
                            );
                            ui.add_space(if compact { 8.0 } else { 12.0 });
                            for (index, (label, keys)) in [
                                ("Start typing · return to motions", "i / Esc".to_owned()),
                                ("Vim motions & visual selections", "v / V".to_owned()),
                                (
                                    "Find a file",
                                    self.shortcut_label(keybindings::Action::QuickOpen),
                                ),
                                (
                                    "Search the whole project",
                                    self.shortcut_label(keybindings::Action::SearchProject),
                                ),
                                (
                                    "Commands at your fingertips",
                                    self.shortcut_label(keybindings::Action::CommandPalette),
                                ),
                                (
                                    "Editor & Vim settings",
                                    self.shortcut_label(keybindings::Action::Settings),
                                ),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                if index < 2 && !self.settings.vim {
                                    continue;
                                }
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(label).size(12.0).color(TEXT));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(keys).monospace().size(11.0).color(MUTED),
                                        );
                                    });
                                });
                                ui.add_space(if compact { 4.0 } else { 6.0 });
                            }
                        });
                    ui.add_space(if compact { 8.0 } else { 25.0 });
                    ui.label(
                        RichText::new(format!("VEDIT  /  {}", env!("CARGO_PKG_VERSION")))
                            .size(10.0)
                            .color(MUTED.linear_multiply(0.6)),
                    );
                });
            });
    }
}

use keybindings::COMMANDS;
impl VEditApp {
    fn command_action(&mut self, label: &str) {
        match label {
            "Command palette" => self.open_palette(Palette::Commands),
            "Go to file" => self.open_palette(Palette::Files),
            "Save as..."
            | "Close current tab"
            | "Reopen closed tab"
            | "Previous tab"
            | "Next tab"
            | "Find next"
            | "Find previous"
            | "Keyboard shortcuts"
            | "Zoom in"
            | "Zoom out"
            | "Reset zoom"
            | "Toggle line comment"
            | "Duplicate line / selection"
            | "Undo"
            | "Redo" => {
                if let Some(action) = keybindings::Action::from_label(label) {
                    self.run_shortcut_action(action);
                }
            }
            "Switch project" => self.open_workspace_picker(),
            "Next project" | "Previous project" => {
                if !self.projects.is_empty() {
                    let count = self.projects.len();
                    let next = if label == "Next project" {
                        (self.active_project + 1) % count
                    } else {
                        (self.active_project + count - 1) % count
                    };
                    self.switch_project(next);
                }
            }
            "Add project folder" => self.pick_project(),
            "Open file" => self.pick_file(),
            "New file" => self.new_file(),
            "Save current file" => self.save_current(),
            "Save all files" => self.save_all(),
            "Find in file" => {
                self.find_visible = true;
                self.find_focus = true;
            }
            "Replace in file" => {
                self.find_visible = true;
                self.replace_visible = true;
                self.find_focus = true;
            }
            "Search project" => {
                self.open_project_search();
            }
            "Go to line" => self.open_palette(Palette::Line),
            "Toggle bookmark" => self.toggle_bookmark(),
            "Bookmarks" => self.open_bookmarks(),
            "Mark selection for agent" => {
                self.capture_agent_mark();
            }
            "Agent marks" => self.open_agent_marks(),
            "Agent notifications" => self.open_notifications(),
            "Jump to visible text" => self.start_screen_jump(),
            "Go to definition" => self.request_definition(),
            "Show completions" => self.request_completion(),
            "Restart language analysis" | "Restart Rust analysis" => self.restart_active_language(),
            "Language support" => self.open_settings_section("Languages"),
            "Show problems" => {
                self.output_visible = true;
                self.problems_visible = true;
            }
            "Next problem" => self.go_to_problem(false),
            "Previous problem" => self.go_to_problem(true),
            "Go back" => self.navigate_history(true),
            "Go forward" => self.navigate_history(false),
            "Markdown source / preview" => {
                if self
                    .tab()
                    .is_some_and(|tab| is_markdown(tab.path.as_deref()))
                {
                    let view = if self
                        .tab()
                        .is_some_and(|tab| tab.agent_view == crate::cli::DocumentView::Preview)
                    {
                        crate::cli::DocumentView::Source
                    } else {
                        crate::cli::DocumentView::Preview
                    };
                    self.set_document_view(view);
                } else {
                    self.status("Open a Markdown document to use preview", false);
                }
            }
            "Split right" => self.split_command("right"),
            "Split down" => self.split_command("down"),
            "Focus Explorer" => {
                self.focus_explorer();
                self.defer_region_focus_owner();
            }
            "Focus editor" => {
                self.focus_editor();
                self.defer_region_focus_owner();
            }
            "Focus Git" => {
                self.show_sidebar(SidebarView::SourceControl);
                self.focus_sidebar_region(self.sidebar_side(SidebarView::SourceControl));
                self.defer_region_focus_owner();
            }
            "Focus output" => {
                self.output_visible = true;
                self.focus_output_region();
                self.defer_region_focus_owner();
            }
            "Next editor pane" => self.split_command("next"),
            "Previous editor pane" => self.split_command("previous"),
            "Next region" => self.split_command("region_next"),
            "Previous region" => self.split_command("region_previous"),
            "Focus left" | "Focus pane left" => self.split_command("focus_left"),
            "Focus right" | "Focus pane right" => self.split_command("focus_right"),
            "Focus above" | "Focus pane above" => self.split_command("focus_up"),
            "Focus below" | "Focus pane below" => self.split_command("focus_down"),
            "Move tab left" => self.split_command("move_left"),
            "Move tab right" => self.split_command("move_right"),
            "Move tab up" => self.split_command("move_up"),
            "Move tab down" => self.split_command("move_down"),
            "Close pane" => self.split_command("close"),
            "Show agent review" => {
                self.show_sidebar(SidebarView::Review);
            }
            "Show source control" => {
                self.show_sidebar(SidebarView::SourceControl);
            }
            "Review working tree changes" | "Review branch against main" => {
                let source = if label == "Review working tree changes" {
                    git::ReviewSource::WorkingTree
                } else {
                    git::ReviewSource::Branch { base: None }
                };
                self.show_change_review(self.active_project, source);
            }
            "Show explorer" => {
                self.show_sidebar(SidebarView::Explorer);
            }
            "Toggle left sidebar" => self.toggle_sidebar(sidebar_layout::Side::Left),
            "Toggle right sidebar" => self.toggle_sidebar(sidebar_layout::Side::Right),
            "Explorer left, Git right" => self.explorer_git_layout(),
            "Toggle output" => {
                self.output_visible = !self.output_visible;
                if !self.output_visible {
                    self.clear_hidden_output_focus();
                }
            }
            "Check project" => self.run_project_task(project_tasks::ProjectTaskAction::Check),
            "Run tests" => self.run_project_task(project_tasks::ProjectTaskAction::Test),
            "Build project" => self.run_project_task(project_tasks::ProjectTaskAction::Build),
            "Run active file" => self.run_project_task(project_tasks::ProjectTaskAction::RunFile),
            "Run application" => self.run_selected_application(),
            "Stop application" => self.stop_application(),
            "Configure project tasks" => self.open_project_task_settings(),
            "Open terminal" => self.open_terminal(),
            "Toggle word wrap" => {
                self.settings.word_wrap = !self.settings.word_wrap;
                self.session_dirty = true;
            }
            "Toggle Vim motions" => {
                self.settings.vim = !self.settings.vim;
                self.session_dirty = true;
            }
            "Reload file from disk" => self.reload_current(),
            "Compare with saved" => self.compare_current_with_saved(),
            "Settings" => self.settings_open = true,
            _ => {}
        }
    }
    fn overlays(&mut self) {
        let ctx = self.ctx.clone();
        self.context_file_dialog_ui(&ctx);
        if let Some(palette) = self.palette {
            let mut close = false;
            let mut choose = None;
            let mut line = None;
            let mut retry_index = false;
            let modal = egui::Modal::new(egui::Id::new("palette"))
                .area(egui::Modal::default_area(egui::Id::new("palette"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 125.0]))
                .backdrop_color(Color32::from_black_alpha(30))
                .show(&ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 70.0).clamp(280.0, 590.0));
                let navigate = palette != Palette::Line;
                let down = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                let up = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                let first = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Home));
                let last = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::End));
                let page_down = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::PageDown));
                let page_up = navigate && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::PageUp));
                let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                design::overlay_title(ui, match palette {
                    Palette::Files => "Quick open",
                    Palette::Commands => "Commands",
                    Palette::Line => "Go to line",
                });
                ui.add_space(6.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .id(egui::Id::new("palette_input"))
                        .hint_text(match palette {
                            Palette::Files => "Search by file name or path…",
                            Palette::Commands => "What would you like to do?",
                            Palette::Line => "Line number",
                        })
                        .desired_width(f32::INFINITY)
                        .font(FontId::proportional(15.0)),
                );
                if self.palette_focus && !ui.is_sizing_pass() {
                    response.request_focus();
                    self.palette_focus = false;
                }
                let query_changed = response.changed();
                if query_changed {
                    self.palette_selected = 0;
                }
                ui.add_space(6.0);
                if palette == Palette::Line {
                    if enter {
                        line = self.palette_query.trim().parse::<usize>().ok();
                    }
                } else {
                    let options: Vec<(String, String, Option<PathBuf>)> = if palette
                        == Palette::Commands
                    {
                        COMMANDS
                            .iter()
                            .filter(|(name, _)| {
                                name.to_lowercase()
                                    .contains(&self.palette_query.to_lowercase())
                            })
                            .map(|(name, _)| (name.to_string(), keybindings::Action::from_label(name).map(|action| self.shortcut_label(action)).unwrap_or_default(), None))
                            .collect()
                    } else {
                        self.project()
                            .and_then(|p| {
                                p.index.as_ref().map(|index| {
                                    index
                                        .quick_open(&self.palette_query, 40)
                                        .into_iter()
                                        .map(|path| {
                                            (
                                                file_title(&path),
                                                crate::paths::display(
                                                    path.strip_prefix(&p.root).unwrap_or(&path),
                                                ),
                                                Some(path),
                                            )
                                        })
                                        .collect()
                                })
                            })
                            .unwrap_or_default()
                    };
                    let index_truncated = palette == Palette::Files
                        && self.project().and_then(|project| project.index.as_ref()).is_some_and(|index| index.truncated);
                    let limit_note_height = if index_truncated { 30.0 } else { 0.0 };
                    let results_height = (ctx.content_rect().height() - 285.0 - limit_note_height).clamp(96.0, 340.0);
                    let page_size =
                        (results_height / design::DETAIL_ROW_HEIGHT).floor().max(1.0) as usize;
                    if down && !options.is_empty() {
                        self.palette_selected = (self.palette_selected + 1).min(options.len() - 1);
                    }
                    if up {
                        self.palette_selected = self.palette_selected.saturating_sub(1);
                    }
                    if first {
                        self.palette_selected = 0;
                    }
                    if last {
                        self.palette_selected = options.len().saturating_sub(1);
                    }
                    if page_down {
                        self.palette_selected = self.palette_selected.saturating_add(page_size);
                    }
                    if page_up {
                        self.palette_selected = self.palette_selected.saturating_sub(page_size);
                    }
                    self.palette_selected =
                        self.palette_selected.min(options.len().saturating_sub(1));
                    ui.scope(|ui| {
                    // Reserve a solid scrollbar column, including before hover. The
                    // track also makes keyboard navigation's scroll position visible.
                    ui.spacing_mut().scroll.floating = false;
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::vertical()
                        .id_salt("palette_results")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                        .animated(false)
                        .max_height(results_height)
                        .show(ui, |ui| {
                            for (i, (name, detail, path)) in options.iter().enumerate() {
                                let response = ui
                                    .push_id(i, |ui| {
                                        design::detail_row(
                                            ui,
                                            name,
                                            detail,
                                            i == self.palette_selected,
                                            detail,
                                        )
                                    })
                                    .inner;
                                if response.clicked() || enter && i == self.palette_selected {
                                    choose = Some((name.clone(), path.clone()));
                                }
                                if (down || up || first || last || page_down || page_up || query_changed)
                                    && i == self.palette_selected {
                                    response.scroll_to_me(None);
                                }
                            }
                            if options.is_empty() {
                                ui.add_space(10.0);
                                if palette == Palette::Files {
                                    match self.project() {
                                        Some(project) if project.indexing => {
                                            ui.horizontal(|ui| {
                                                ui.spinner();
                                                ui.label(
                                                    RichText::new("Indexing files…").color(MUTED),
                                                );
                                            });
                                        }
                                        Some(project) if project.index_error.is_some() => {
                                            ui.label(
                                                RichText::new(
                                                    "Could not load this project's files.",
                                                )
                                                .color(ORANGE),
                                            );
                                            if let Some(error) = &project.index_error {
                                                ui.label(
                                                    RichText::new(error).size(11.0).color(MUTED),
                                                );
                                            }
                                            retry_index = ui.button("Retry indexing").clicked();
                                        }
                                        Some(_) => {
                                            ui.label(
                                                RichText::new(
                                                    if index_truncated {
                                                        "No matches in the indexed files"
                                                    } else if self.palette_query.trim().is_empty() {
                                                        "No files found in this project"
                                                    } else {
                                                        "No matches"
                                                    },
                                                )
                                                .color(MUTED),
                                            );
                                        }
                                        None => {
                                            ui.label(
                                                RichText::new("Open a project to find its files.")
                                                    .color(MUTED),
                                            );
                                        }
                                    }
                                } else {
                                    ui.label(RichText::new("No matches").color(MUTED));
                                }
                            }
                        });
                    });
                    if index_truncated {
                        ui.label(
                            RichText::new("Index limit reached. Some files may be missing.")
                                .size(11.0)
                                .color(ORANGE),
                        ).on_hover_text(format!("The index reached its file, depth, or time limit. Open a narrower project folder, or close Quick Open and use Open file ({}) to open directly.", self.shortcut_label(keybindings::Action::OpenFile)));
                    }
                }
            });
            close |= modal.should_close()
                || modal
                    .backdrop_response
                    .clicked_by(egui::PointerButton::Secondary)
                || modal
                    .backdrop_response
                    .clicked_by(egui::PointerButton::Middle);
            if retry_index {
                self.index_project(self.active_project);
            }
            if close || choose.is_some() || line.is_some() {
                self.close_palette();
            }
            if let Some((name, path)) = choose {
                if let Some(path) = path {
                    self.open_file(path);
                } else {
                    self.command_action(&name);
                }
            }
            if let Some(line) = line
                && let Some(t) = self.tab_mut()
            {
                let at = editor::go_to_line(&t.text, line);
                t.set_cursor(at..at);
            }
        }
        if self.settings_open {
            let mut done = false;
            let mut changed = false;
            let modal = egui::Modal::new(egui::Id::new("settings_dialog")).show(&ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 32.0).clamp(220.0, 560.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").size(15.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close = small_button(ui, "×", "Close settings · Esc");
                        close.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                "Close settings",
                            )
                        });
                        done |= close.clicked();
                    });
                });
                ui.add_space(10.0);
                self.settings_navigation_ui(ui);
                egui::ScrollArea::vertical()
                    .id_salt(("settings_contents", self.settings_section()))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .max_height((ctx.content_rect().height() - 200.0).clamp(65.0, 540.0))
                    .show(ui, |ui| {
                        changed |= self.settings_contents_ui(ui);
                    });
                ui.add_space(12.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    // Settings apply immediately, so closing is not a primary action.
                    done |= ui
                        .add(egui::Button::new("Done").min_size(Vec2::new(70.0, 26.0)))
                        .clicked();
                    ui.label(
                        RichText::new(self.settings_save_hint())
                            .size(10.5)
                            .color(MUTED),
                    );
                });
            });
            self.settings_open = !(done || modal.should_close());
            self.session_dirty |= changed;
            self.ctx.set_zoom_factor(self.settings.ui_scale);
            self.apply_editor_font();
            if !self.settings_open {
                self.interface_zoom_draft = None;
                self.apply_pending_project_interpreter();
                self.keybinding_ui = keybindings::State::default();
                ctx.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
                if let Some(tab) = self.tab_mut() {
                    tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
                }
                ctx.request_repaint();
            }
        }
        if let Some(action) = self.close_action
            && self.save_as.is_none()
        {
            let documents: Vec<_> = self
                .projects
                .iter()
                .enumerate()
                .flat_map(|(index, project)| {
                    project.tabs.iter().filter_map(move |tab| {
                        let affected = match action {
                            CloseAction::Tab(id) | CloseAction::PaneTab { id, .. } => tab.id == id,
                            CloseAction::Project(project_index) => index == project_index,
                            CloseAction::Exit => true,
                        };
                        if !affected || !(tab.dirty || tab.saving) {
                            return None;
                        }
                        let relative = tab.path.as_ref().map_or_else(
                            || tab.title.clone(),
                            |path| {
                                crate::paths::display(
                                    path.strip_prefix(&project.root).unwrap_or(path),
                                )
                            },
                        );
                        let full_path = tab.path.as_ref().unwrap_or(&project.root);
                        Some((
                            format!("{} · {relative}", file_title(&project.root)),
                            crate::paths::display(full_path),
                        ))
                    })
                })
                .collect();
            let mut save = false;
            let mut discard = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("unsaved_changes")).show(&ctx, |ui| {
                ui.set_width(390.0);
                ui.label(
                    RichText::new("Keep your changes?")
                        .size(20.0)
                        .strong()
                        .color(TEXT),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!(
                        "{} {} with unsaved changes.",
                        documents.len(),
                        if documents.len() == 1 {
                            "file"
                        } else {
                            "files"
                        },
                    ))
                    .color(MUTED),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("unsaved_documents")
                    .max_height((ctx.content_rect().height() * 0.25).clamp(70.0, 180.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (label, path) in &documents {
                            ui.add(egui::Label::new(RichText::new(label).size(12.0)).truncate())
                                .on_hover_text(path);
                        }
                    });
                if self.close_after_save {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Saving…");
                    });
                }
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled(!self.close_after_save, egui::Button::new("Discard"))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui
                        .add_enabled_ui(!self.close_after_save, |ui| {
                            design::primary_button(ui, "Save & close", Vec2::new(110.0, 28.0))
                        })
                        .inner
                        .clicked()
                    {
                        save = true;
                    }
                });
            });
            if cancel || modal.should_close() {
                self.close_action = None;
                self.close_after_save = false;
                if let Some(tab) = self.tab_mut() {
                    tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
                }
                if let Some(tab) = self.tab()
                    && tab.agent_view == crate::cli::DocumentView::Source
                {
                    ctx.memory_mut(|memory| memory.request_focus(self.document_widget_id(tab.id)));
                }
            }
            if discard {
                self.perform_close(action);
            }
            if save {
                self.close_after_save = true;
                match action {
                    CloseAction::Tab(id) | CloseAction::PaneTab { id, .. } => {
                        self.save_tab(id, None)
                    }
                    CloseAction::Project(index) => {
                        let ids: Vec<_> = self
                            .projects
                            .get(index)
                            .map(|p| p.tabs.iter().filter(|t| t.dirty).map(|t| t.id).collect())
                            .unwrap_or_default();
                        for id in ids {
                            self.save_tab(id, None);
                        }
                    }
                    CloseAction::Exit => self.save_all(),
                }
            }
        }
        self.git_detail_ui();
        if let Some((title, text)) = &self.diff {
            let mut choice = None;
            let mut open = None;
            let modal = crate::git_diff_view::dialog(
                &ctx,
                egui::Id::new("git_patch_window"),
                title,
                "Close diff",
                Vec2::new(950.0, 620.0),
                true,
                |ui| {
                    if let Some(comparison) = &self.comparison {
                        choice = disk_sync::comparison_header(ui, comparison, &self.projects);
                    }
                    if self.diff_loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading changes…");
                        });
                    } else if let Some(error) = &self.diff_error {
                        ui.colored_label(RED, error);
                    } else {
                        open = self.diff_view.show(ui, title, text);
                    }
                },
            );
            if let Some(open) = open {
                self.open_diff_location(open);
            } else if modal.inner || modal.should_close() {
                self.close_diff();
            }
            if let Some(choice) = choice {
                self.close_diff();
                self.resolve_disk_conflict(choice);
            }
        }
    }
    /// Open the file of the diff dialog at the chosen line, as the change
    /// review's o and Enter do.
    fn open_diff_location(&mut self, open: crate::git_diff_view::DiffOpen) {
        self.record_jump();
        let Some(repository) = self.diff_repository.clone() else {
            return;
        };
        if open.deleted {
            self.status(
                format!(
                    "{} is deleted in this diff; there is no file to open",
                    open.path
                ),
                true,
            );
            return;
        }
        self.close_diff();
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.git_view.detail_open = false;
        }
        let path = open
            .path
            .split('/')
            .fold(repository, |path, part| path.join(part));
        self.open_file(path);
        if let Some(line) = open.line
            && let Some(tab) = self.tab_mut()
        {
            tab.pending_agent_location = Some(crate::cli::Location { line, column: 1 });
            tab.agent_location_error = None;
            self.apply_pending_agent_locations();
        }
    }
    fn close_diff(&mut self) {
        let comparison = self.comparison.take().is_some();
        self.diff = None;
        self.diff_repository = None;
        self.diff_loading = false;
        self.diff_error = None;
        self.diff_view.reset_focus();
        if comparison {
            // A document comparison returns to the document it was opened from.
            self.restore_document_focus();
        } else {
            self.restore_git_overlay_focus();
        }
    }
    fn restore_git_overlay_focus(&mut self) {
        self.ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        if self.git_detail_open() {
            if let Some(tab) = self.tab_mut() {
                tab.focus = false;
            }
            if self.restore_git_detail_content_focus() {
                return;
            }
            let modal_id = egui::Id::new(("git-commit-details", self.active_project));
            if let Some(id) = self
                .ctx
                .data(|data| data.get_temp::<egui::Id>(modal_id.with("close_focus")))
            {
                self.ctx.memory_mut(|memory| memory.request_focus(id));
            }
            self.ctx.request_repaint();
            return;
        }
        if self.sidebar_view_visible(SidebarView::SourceControl) {
            self.focus_git_content();
            return;
        }
        self.restore_document_focus();
    }
    fn restore_document_focus(&mut self) {
        self.ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
        {
            let id = self.document_widget_id(tab.id);
            self.ctx.memory_mut(|memory| memory.request_focus(id));
        }
        self.ctx.request_repaint();
    }
    fn apply_editor_font(&mut self) {
        if self.applied_editor_font != self.settings.editor_font {
            self.ctx
                .set_fonts(crate::typography::fonts(self.settings.editor_font));
            self.applied_editor_font = self.settings.editor_font;
            self.ctx.request_repaint();
        }
    }
    fn draw_ui(&mut self, ui: &mut egui::Ui) {
        design::focus_visible(&self.ctx);
        // Observe window focus before editor input routing can consume events.
        self.refresh_visible_git();
        self.begin_region_focus_frame();
        self.capture_shortcut_input();
        self.preference_shortcuts();
        let restore_after_menu = egui::Popup::is_any_open(&self.ctx)
            && self.ctx.input(|input| input.key_pressed(Key::Escape));
        self.poll();
        self.poll_file_watchers();
        self.sync_agent_marks();
        self.poll_control();
        if !self.settings_open {
            self.interface_zoom_draft = None;
        }
        self.ctx.set_zoom_factor(self.settings.ui_scale);
        self.apply_editor_font();
        self.syntax.begin_frame(ui);
        self.apply_pending_bookmark_jump();
        self.apply_pending_agent_locations();
        self.apply_pending_jump();
        self.poll_languages();
        self.resume_region_input();
        self.focus_pane_from_pointer();
        // Editing buffers use LF; Windows clipboard text commonly uses CRLF.
        // Normalize only source input, before either Vim, TextEdit, or a pending
        // context-menu paste consumes it. The file's saved encoding stays intact.
        if self.context_paste.is_some()
            || self.tab().is_some_and(|tab| {
                tab.agent_view == crate::cli::DocumentView::Source
                    && self
                        .ctx
                        .memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            })
        {
            self.ctx.input_mut(|input| {
                for event in &mut input.events {
                    if let egui::Event::Paste(text) = event
                        && text.contains('\r')
                    {
                        *text = text.replace("\r\n", "\n").replace('\r', "\n");
                    }
                }
            });
        }
        self.context_paste_poll();
        self.validate_vim_search();
        self.resume_editor_input();
        self.resume_shortcut_input();
        if self.palette.is_none() && !self.settings_open {
            self.git_peek_escape();
        }
        let dragging_tab = self.tab_drag_keys() || self.sidebar_drag_keys();
        let switching_project = !dragging_tab && self.workspace_keys();
        if !dragging_tab && !switching_project {
            self.focus_region_shortcuts(&self.ctx.clone());
        }
        if dragging_tab || switching_project {
            self.find.break_escape_sequence();
        }
        if !dragging_tab && !switching_project {
            let pending_events = self.ctx.input(|input| input.events.len());
            self.shortcuts();
            if self.ctx.input(|input| input.events.len()) < pending_events {
                self.find.break_escape_sequence();
            }
        }
        if !dragging_tab && !switching_project {
            self.history_pointer_buttons();
        }
        let navigating_explorer =
            !dragging_tab && !switching_project && self.explorer_navigation_keys();
        let navigation_pending =
            !dragging_tab && !switching_project && !navigating_explorer && self.navigation_keys();
        let jumping = !dragging_tab && !switching_project && self.screen_jump_keys();
        if navigating_explorer || navigation_pending || jumping {
            self.find.break_escape_sequence();
        }
        if !dragging_tab
            && !switching_project
            && !navigating_explorer
            && !navigation_pending
            && !jumping
        {
            self.handle_vim();
        }
        if self.search_dirty {
            let elapsed = self.search_last_edit.elapsed();
            if elapsed >= Duration::from_millis(250) {
                self.search_project();
            } else {
                self.ctx
                    .request_repaint_after(Duration::from_millis(250) - elapsed);
            }
        }
        self.header(ui);
        self.launch_toolbar(ui);
        self.status_bar(ui);
        self.activity(ui);
        self.sidebar_ui(ui);
        self.output_ui(ui);
        if restore_after_menu
            && !egui::Popup::is_any_open(&self.ctx)
            && self.palette.is_none()
            && self.picker.is_none()
            && !self.settings_open
            && !self.agent_marks_modal_open()
            && self.close_action.is_none()
            && self.save_as.is_none()
            && self.diff.is_none()
            && !self.git_detail_open()
            && !self.navigation_modal_open()
            && self.screen_jump.is_none()
            && !self.restore_git_popup_focus()
            && let Some(tab) = self.tab_mut()
        {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        self.editor_ui(ui);
        self.finish_workspace_action();
        self.finish_region_render_owner();
        self.finish_region_focus();
        self.overlays();
        self.agent_marks_dialog();
        self.notifications_dialog();
        self.sync_agent_marks();
        self.navigation_ui();
        self.completion_ui();
        self.finish_agent_mark_input_owner();
        self.finish_shortcut_input();
        self.autosave();
        self.sync_languages(false);
        self.watch_disk();
        self.observe_navigation();
        if !self.projects.is_empty() {
            self.ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
}
impl eframe::App for VEditApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(windows)]
        let window = frame.winit_window().cloned();
        #[cfg(windows)]
        self.ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new("native_window_chrome"), window.is_some());
            data.insert_temp(
                egui::Id::new("native_window_maximize_state"),
                window
                    .as_ref()
                    .map(|window| window.custom_titlebar_maximize_state())
                    .unwrap_or_default(),
            );
        });
        #[cfg(not(windows))]
        let _ = frame;
        if self.ctx.input(|i| i.viewport().close_requested()) && !self.exit_ready {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(CloseAction::Exit);
        }
        self.draw_ui(ui);
        #[cfg(windows)]
        if let Some(window) = window {
            let geometry = self.ctx.data(|data| {
                data.get_temp::<crate::window_chrome::ChromeGeometry>(egui::Id::new(
                    "vedit_window_chrome",
                ))
            });
            if !self.chrome_blocked()
                && let Some(geometry) = geometry
            {
                let scale = self.ctx.pixels_per_point();
                let physical = |rect: egui::Rect| {
                    [
                        (rect.left() * scale).floor() as i32,
                        (rect.top() * scale).floor() as i32,
                        (rect.right() * scale).ceil() as i32,
                        (rect.bottom() * scale).ceil() as i32,
                    ]
                };
                let draggable: Vec<_> = geometry.drag_regions.into_iter().map(physical).collect();
                window.set_custom_titlebar_regions(
                    &draggable,
                    geometry.native_maximize.map(physical),
                );
            } else {
                window.set_custom_titlebar_regions(&[], None);
            }
        }
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.flush_recovery();
        eframe::set_value(storage, eframe::APP_KEY, &self.session());
    }
}

#[cfg(test)]
fn recovery_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-state")
        .join(std::process::id().to_string())
        .join("recovery")
}

#[cfg(not(test))]
fn recovery_directory() -> PathBuf {
    crate::profile::recovery_directory()
}
fn file_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::paths::display(path))
}
fn extension(path: Option<&Path>) -> &str {
    path.and_then(Path::extension)
        .and_then(|v| v.to_str())
        .unwrap_or("")
}
fn file_badge(path: &Path) -> &'static str {
    crate::file_kind::from_path(Some(path)).badge()
}
fn file_color(path: Option<&Path>) -> Color32 {
    use crate::file_kind::FileKind;
    match crate::file_kind::from_path(path) {
        FileKind::Rust => ORANGE,
        FileKind::Markdown | FileKind::TypeScript | FileKind::Dart | FileKind::Python => BLUE,
        FileKind::Json | FileKind::JavaScript => Color32::from_rgb(208, 195, 137),
        FileKind::Css | FileKind::Scss | FileKind::Less | FileKind::Html | FileKind::Xml => {
            Color32::from_rgb(171, 158, 216)
        }
        _ => MUTED,
    }
}
fn language_name(path: Option<&Path>) -> &'static str {
    crate::file_kind::from_path(path).label()
}
fn is_markdown(path: Option<&Path>) -> bool {
    path.is_some_and(crate::cli::is_markdown)
}
fn annotation_color(severity: crate::cli::AnnotationSeverity) -> Color32 {
    match severity {
        crate::cli::AnnotationSeverity::Info => BLUE,
        crate::cli::AnnotationSeverity::Warning => ORANGE,
        crate::cli::AnnotationSeverity::Critical => RED,
    }
}
fn syntax_language(path: Option<&Path>) -> &str {
    let kind = crate::file_kind::from_path(path);
    if kind == crate::file_kind::FileKind::PlainText && !extension(path).is_empty() {
        // Preserve syntax support for less common extensions in Syntect's set.
        extension(path)
    } else {
        kind.syntax()
    }
}
fn set_style(ctx: &egui::Context) {
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    ctx.set_fonts(crate::typography::fonts(crate::cli::EditorFont::default()));
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = SURFACE;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.weak_text_color = Some(MUTED);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.warn_fg_color = ORANGE;
    style.visuals.error_fg_color = RED;
    style.visuals.code_bg_color = SURFACE;
    style.visuals.window_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.inactive.weak_bg_fill = SURFACE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = HOVER;
    style.visuals.widgets.hovered.weak_bg_fill = HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, crate::palette::BORDER_STRONG);
    style.visuals.widgets.active.bg_fill = HOVER;
    style.visuals.widgets.active.weak_bg_fill = HOVER;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.noninteractive.bg_fill = SURFACE;
    style.visuals.widgets.noninteractive.weak_bg_fill = SURFACE;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.window_corner_radius = egui::CornerRadius::same(8);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(6);
    style.visuals.selection.bg_fill = HOVER;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.spacing.item_spacing = Vec2::new(7.0, 5.0);
    style.spacing.button_padding = Vec2::new(9.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    // Floating scrollbars expand on hover without flashing bright white.
    style.spacing.scroll.interact_handle_opacity = 0.45;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(12.5));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.5));
    style.interaction.tooltip_delay = 0.45;
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::Theme::Dark);
}
/// Code selection: a blue wash that leaves syntax colors readable.
const EDITOR_SELECTION: Color32 = Color32::from_rgba_premultiplied(0x20, 0x30, 0x43, 0x47);

/// Selected code keeps its own color on the blue wash, instead of egui's
/// accent-colored selection glyphs.
fn editor_selection(visuals: &mut egui::Visuals) {
    visuals.selection.bg_fill = EDITOR_SELECTION;
    visuals.selection.stroke.color = TEXT;
}
fn brand(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let c = rect.center();
    let points = |r: f32| {
        vec![
            c + Vec2::new(0.0, -r),
            c + Vec2::new(r, 0.0),
            c + Vec2::new(0.0, r),
            c + Vec2::new(-r, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points(size * 0.44),
        Color32::TRANSPARENT,
        Stroke::new((size / 18.0).max(1.3), ACCENT),
    ));
    ui.painter().add(egui::Shape::convex_polygon(
        points(size * 0.13),
        ACCENT,
        Stroke::NONE,
    ));
}
fn small_button(ui: &mut egui::Ui, label: &str, hint: &str) -> egui::Response {
    design::ghost_button(ui, label, hint)
}
fn accent_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    design::primary_button(ui, label, Vec2::new(210.0, 38.0))
}
#[derive(Clone, Copy)]
enum Icon {
    Menu,
    Files,
    Search,
    Branch,
    Console,
    Settings,
    Review,
}
fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool, hint: &str) -> egui::Response {
    let response = ui.add_sized(
        Vec2::splat(35.0),
        egui::Button::new("").frame_when_inactive(false),
    );
    let rect = response.rect;
    let label = match icon {
        Icon::Menu => "Main menu",
        Icon::Files => "Explorer",
        Icon::Search => "Search project",
        Icon::Branch => "Source control",
        Icon::Console => "Output",
        Icon::Settings => "Settings",
        Icon::Review => "Agent review",
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    let c = rect.center();
    let painter = ui.painter();
    let color = if selected || response.hovered() {
        TEXT
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.5, color);
    if response.hovered() {
        painter.rect_filled(rect, 5.0, HOVER);
    }
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() - 4.0, rect.top() + 7.0),
                egui::pos2(rect.left() - 2.0, rect.bottom() - 7.0),
            ),
            1.0,
            ACCENT,
        );
    }
    if design::keyboard_focus(&response) {
        painter.rect_stroke(
            rect.shrink(1.0),
            5.0,
            Stroke::new(1.5, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let line = |a: Vec2, b: Vec2| {
        painter.line_segment([c + a, c + b], stroke);
    };
    match icon {
        Icon::Menu => {
            for y in [-6.0, 0.0, 6.0] {
                line(Vec2::new(-7.0, y), Vec2::new(7.0, y));
            }
        }
        Icon::Files => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c + Vec2::new(2.0, 2.0), Vec2::new(12.0, 16.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(Vec2::new(-8.0, 4.0), Vec2::new(-8.0, -9.0));
            line(Vec2::new(-8.0, -9.0), Vec2::new(2.0, -9.0));
        }
        Icon::Search => {
            painter.circle_stroke(c - Vec2::splat(2.0), 6.0, stroke);
            line(Vec2::new(2.5, 2.5), Vec2::new(8.0, 8.0));
        }
        Icon::Branch => {
            for p in [
                Vec2::new(-5.0, -7.0),
                Vec2::new(-5.0, 7.0),
                Vec2::new(6.0, -6.0),
            ] {
                painter.circle_stroke(c + p, 2.3, stroke);
            }
            line(Vec2::new(-5.0, -4.7), Vec2::new(-5.0, 4.7));
            line(Vec2::new(-5.0, 2.0), Vec2::new(6.0, -2.0));
            line(Vec2::new(6.0, -2.0), Vec2::new(6.0, -3.7));
        }
        Icon::Console => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(21.0, 16.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(Vec2::new(-6.0, -3.0), Vec2::new(-2.0, 0.0));
            line(Vec2::new(-2.0, 0.0), Vec2::new(-6.0, 3.0));
            line(Vec2::new(1.0, 4.0), Vec2::new(6.0, 4.0));
        }
        Icon::Settings => {
            painter.circle_stroke(c, 6.5, stroke);
            painter.circle_stroke(c, 2.0, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                let v = Vec2::angled(angle);
                line(v * 6.5, v * 9.0);
            }
        }
        Icon::Review => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(16.0, 21.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            for y in [-5.0, 1.0, 7.0] {
                line(Vec2::new(-4.0, y), Vec2::new(-2.0, y + 2.0));
                line(Vec2::new(-2.0, y + 2.0), Vec2::new(1.0, y - 2.0));
                line(Vec2::new(3.0, y), Vec2::new(5.0, y));
            }
        }
    }
    response
        .on_hover_text(hint)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
#[path = "explorer_watch_tests.rs"]
mod explorer_watch_tests;
#[cfg(test)]
#[path = "navigation_history_tests.rs"]
mod navigation_history_tests;
#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "app_menu_tests.rs"]
mod app_menu_tests;

#[cfg(test)]
#[path = "interaction_tests.rs"]
mod interaction_tests;

#[cfg(test)]
#[path = "native_input_tests.rs"]
mod native_input_tests;

#[cfg(test)]
#[path = "vim_clipboard_tests.rs"]
mod vim_clipboard_tests;

#[cfg(test)]
#[path = "windows_undo_tests.rs"]
mod windows_undo_tests;

#[cfg(test)]
#[path = "recovery_audit_tests.rs"]
mod recovery_audit_tests;

#[cfg(test)]
#[path = "editor_input_tests.rs"]
mod editor_input_tests;

#[cfg(test)]
#[path = "user_review_tests.rs"]
mod user_review_tests;

#[cfg(test)]
#[path = "focus_feedback_tests.rs"]
mod focus_feedback_tests;

#[cfg(test)]
#[path = "selection_scroll_tests.rs"]
mod selection_scroll_tests;

#[cfg(test)]
#[path = "typography_ui_tests.rs"]
mod typography_ui_tests;

impl VEditApp {
    fn poll_languages(&mut self) {
        if let Some((request, revision, cursor, widget)) = self.definition_origin
            && self
                .definition_request
                .is_some_and(|(pending, _)| pending == request)
            && self.tab().is_none_or(|tab| {
                tab.revision != revision
                    || tab.vim.cursor != cursor
                    || self.document_widget_id(tab.id) != widget
            })
        {
            self.definition_request = None;
            self.definition_origin = None;
            if self
                .status
                .as_ref()
                .is_some_and(|(text, _, _)| text == "Finding definition…")
            {
                self.status = None;
            }
        }
        let mut events = Vec::new();
        for (index, project) in self.projects.iter().enumerate() {
            for (language, session) in &project.languages {
                if let Some(server) = &session.server {
                    for _ in 0..256 {
                        let Ok(event) = server.receiver.try_recv() else {
                            break;
                        };
                        events.push((index, language.clone(), event));
                    }
                    if events.len() >= 256 {
                        self.ctx.request_repaint();
                    }
                }
            }
        }
        for (index, language, event) in events {
            match event {
                crate::lsp::LspEvent::Ready => {
                    let session = self.projects[index].languages.get_mut(&language).unwrap();
                    session.ready = true;
                    session.error = None;
                }
                crate::lsp::LspEvent::Error(error) => {
                    let project = &mut self.projects[index];
                    let session = project.languages.get_mut(&language).unwrap();
                    session.error = Some(error.clone());
                    if session
                        .server
                        .as_ref()
                        .is_some_and(|server| server.is_stopped())
                    {
                        session.ready = false;
                        session.opened.clear();
                        project.clear_language_diagnostics(&language);
                    }
                    self.status(error, true);
                }
                crate::lsp::LspEvent::RequestFailed { request_id, error } => {
                    if self.active_project == index {
                        if self.completion_request == Some(request_id) {
                            self.completion_request = None;
                            self.completions = None;
                        }
                        if self
                            .definition_request
                            .is_some_and(|(id, _)| id == request_id)
                        {
                            self.definition_request = None;
                            if self
                                .status
                                .as_ref()
                                .is_some_and(|(text, _, _)| text == "Finding definition…")
                            {
                                self.status = None;
                            }
                        }
                    }
                    if let Some(error) = error {
                        self.projects[index]
                            .languages
                            .get_mut(&language)
                            .unwrap()
                            .error = Some(error.clone());
                        self.status(error, true);
                    }
                }
                crate::lsp::LspEvent::Diagnostics {
                    path,
                    version,
                    diagnostics,
                } => {
                    let project = &self.projects[index];
                    let session = &project.languages[&language];
                    let owner = self
                        .settings
                        .language_servers
                        .iter()
                        .find(|config| config.enabled && config.matches(&path));
                    let allowed = owner.map_or_else(
                        || context_actions::path_contains(&project.root, &path),
                        |config| config.language_id == language,
                    );
                    if session.active()
                        && allowed
                        && !session.retired_paths.contains(&path)
                        && version.is_none_or(|version| {
                            session
                                .opened
                                .get(&path)
                                .is_none_or(|(_, current)| version >= *current)
                        })
                    {
                        self.language_response_succeeded(index, &language);
                        let project = &mut self.projects[index];
                        let session = project.languages.get_mut(&language).unwrap();
                        let origin = language_support::DiagnosticsOrigin::new(
                            version,
                            session.opened.get(&path),
                        );
                        session.diagnostic_origins.insert(path.clone(), origin);
                        session.diagnostics.insert(path.clone(), diagnostics);
                        let combined = project
                            .languages
                            .values()
                            .filter_map(|session| session.diagnostics.get(&path))
                            .flatten()
                            .cloned()
                            .collect();
                        project.diagnostics.insert(path, combined);
                    }
                }
                crate::lsp::LspEvent::Completions {
                    request_id,
                    path,
                    version,
                    position,
                    items,
                } => {
                    let session = &self.projects[index].languages[&language];
                    if session.active()
                        && self.completion_request == Some(request_id)
                        && self.active_project == index
                        && let Some(tab) = self.tab()
                        && tab.path.as_ref() == Some(&path)
                        && session
                            .opened
                            .get(&path)
                            .is_some_and(|(revision, current)| {
                                *revision == tab.revision && *current == version
                            })
                        && crate::lsp::char_to_position(&tab.text, tab.vim.cursor) == position
                    {
                        self.completions = (!items.is_empty()).then_some(CompletionPopup {
                            tab_id: tab.id,
                            version,
                            cursor: tab.vim.cursor,
                            items,
                            selected: 0,
                        });
                        self.completion_request = None;
                        self.language_response_succeeded(index, &language);
                    }
                }
                crate::lsp::LspEvent::Definition {
                    request_id,
                    path,
                    line,
                    column,
                } => {
                    if self.active_project == index
                        && self.projects[index].languages[&language].active()
                        && self.definition_request.is_some_and(|(request, tab)| {
                            request == request_id
                                && self.tab().is_some_and(|current| current.id == tab)
                        })
                    {
                        self.language_response_succeeded(index, &language);
                        self.definition_request = None;
                        self.reveal_lsp_position(
                            path,
                            crate::lsp::Position {
                                line,
                                character: column,
                            },
                        );
                    }
                }
                crate::lsp::LspEvent::DefinitionNotFound { request_id } => {
                    if self.active_project == index
                        && self.projects[index].languages[&language].active()
                        && self.definition_request.is_some_and(|(request, tab)| {
                            request == request_id
                                && self.tab().is_some_and(|current| current.id == tab)
                        })
                    {
                        self.language_response_succeeded(index, &language);
                        self.definition_request = None;
                        self.status("No definition found here", false);
                    }
                }
            }
        }
        self.apply_pending_location();
        if self.completions.as_ref().is_some_and(|popup| {
            self.tab().is_none_or(|t| {
                t.id != popup.tab_id
                    || t.vim.cursor != popup.cursor
                    || self
                        .project()
                        .and_then(|p| {
                            t.path
                                .as_ref()
                                .and_then(|path| p.language_document_version(path))
                        })
                        .is_none_or(|(rev, v)| *rev != t.revision || *v != popup.version)
            })
        }) {
            self.completions = None;
        }
    }
    fn request_definition(&mut self) {
        self.sync_languages(true);
        let Some(project) = self.project() else {
            return;
        };
        let Some(tab) = self.tab() else { return };
        let Some(path) = &tab.path else {
            self.language_request_unavailable();
            return;
        };
        if let Some(session) = project.language_for(path)
            && session.active()
            && let Some(server) = &session.server
        {
            let id = server.definition(
                path,
                crate::lsp::char_to_position(&tab.text, tab.vim.cursor),
            );
            if id != 0 {
                let origin = (
                    id,
                    tab.revision,
                    tab.vim.cursor,
                    self.document_widget_id(tab.id),
                );
                let tab_id = tab.id;
                self.definition_origin = Some(origin);
                self.definition_request = Some((id, tab_id));
                self.status("Finding definition…", false);
            } else {
                self.status("Language analysis is busy or unavailable. Try again or restart language analysis.",true);
            }
            return;
        }
        self.language_request_unavailable();
    }
    fn request_completion(&mut self) {
        self.sync_languages(true);
        let Some(project) = self.project() else {
            return;
        };
        let Some(tab) = self.tab() else { return };
        let Some(path) = &tab.path else {
            self.language_request_unavailable();
            return;
        };
        if let Some(session) = project.language_for(path)
            && session.active()
            && let Some(server) = &session.server
        {
            let Some((_, version)) = session.opened.get(path) else {
                self.language_request_unavailable();
                return;
            };
            let id = server.completion(
                path,
                crate::lsp::char_to_position(&tab.text, tab.vim.cursor),
                *version,
            );
            if id != 0 {
                self.completion_request = Some(id);
            } else {
                self.status("Language analysis is busy or unavailable. Try again or restart language analysis.",true);
            }
            return;
        }
        self.language_request_unavailable();
    }
    fn apply_completion(&mut self, index: usize) {
        let Some(popup) = self.completions.take() else {
            return;
        };
        let Some(item) = popup.items.get(index).cloned() else {
            return;
        };
        let valid = self.tab().is_some_and(|t| {
            t.id == popup.tab_id
                && t.vim.cursor == popup.cursor
                && popup.cursor <= t.text.chars().count()
                && self
                    .project()
                    .and_then(|p| {
                        t.path
                            .as_ref()
                            .and_then(|path| p.language_document_version(path))
                    })
                    .is_some_and(|(revision, version)| {
                        *revision == t.revision && *version == popup.version
                    })
        });
        if !valid {
            return;
        }
        let Some(t) = self.tab_mut() else { return };
        if t.id != popup.tab_id || t.vim.cursor != popup.cursor {
            return;
        }
        let mut edits = item.additional_text_edits;
        let main = item.text_edit.unwrap_or_else(|| {
            let chars: Vec<_> = t.text.chars().collect();
            let mut start = t.vim.cursor;
            while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
                start -= 1;
            }
            crate::lsp::TextEdit {
                range: crate::lsp::TextRange {
                    start: crate::lsp::char_to_position(&t.text, start),
                    end: crate::lsp::char_to_position(&t.text, t.vim.cursor),
                },
                new_text: item.insert_text,
            }
        });
        let main_start = crate::lsp::position_to_char(&t.text, main.range.start);
        let mut cursor = main_start + main.new_text.chars().count();
        let mut replacements: Vec<_> = edits
            .drain(..)
            .map(|edit| {
                (
                    crate::lsp::position_to_char(&t.text, edit.range.start)
                        ..crate::lsp::position_to_char(&t.text, edit.range.end),
                    edit.new_text,
                    false,
                )
            })
            .collect();
        for (range, text, _) in &replacements {
            if range.end <= main_start {
                cursor = cursor.saturating_sub(range.end - range.start) + text.chars().count();
            }
        }
        replacements.push((
            main_start..crate::lsp::position_to_char(&t.text, main.range.end),
            main.new_text,
            true,
        ));
        replacements.sort_by_key(|b| std::cmp::Reverse(b.0.start));
        t.vim.begin_external_edit(&t.text);
        for (range, text, _) in replacements {
            t.text = editor::replace_selection(&t.text, range, &text).text;
        }
        t.vim.notify_external_text_change();
        t.changed();
        t.set_cursor(cursor..cursor);
    }
    fn completion_keys(&mut self) -> bool {
        if self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.vim_search_active()
            || self.tab().is_none_or(|t| {
                !self
                    .ctx
                    .memory(|m| m.has_focus(self.document_widget_id(t.id)))
            })
        {
            self.completions = None;
            return false;
        }
        if self
            .completions
            .as_ref()
            .is_none_or(|popup| popup.items.is_empty())
        {
            self.completions = None;
            return false;
        }
        let ctx = self.ctx.clone();
        let boundary = ctx.input(|input| input.events.iter().enumerate().find_map(|(index, event)| {
            matches!(event, egui::Event::Key { key: Key::Escape | Key::ArrowUp | Key::ArrowDown | Key::Enter | Key::Tab,
                pressed: true, modifiers, .. } if !modifiers.ctrl && !modifiers.command && !modifiers.alt && !modifiers.shift).then_some(index)
        }));
        if let Some(index) = boundary {
            if self.earlier_shortcut_boundary(index) {
                return false;
            }
            if ctx.input(|input| input.events[..index].iter().any(editor_input::native_input)) {
                let suffix = ctx.input_mut(|input| input.events.split_off(index));
                if let Some(tab) = self.tab() {
                    editor_input::defer(&ctx, tab.id, suffix);
                }
                return false;
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.completions = None;
            self.defer_shortcut_tail();
            if self.settings.vim {
                return false;
            }
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            return true;
        }
        let plain_key = |key| {
            ctx.input_mut(|input| {
            let before = input.events.len();
            input.events.retain(|event| !matches!(event, egui::Event::Key {
                key: pressed_key, pressed: true, modifiers, ..
            } if *pressed_key == key && !modifiers.ctrl && !modifiers.command && !modifiers.alt && !modifiers.shift));
            before != input.events.len()
        })
        };
        let down = plain_key(Key::ArrowDown);
        let up = plain_key(Key::ArrowUp);
        if let Some(p) = &mut self.completions {
            p.selected = p.selected.min(p.items.len().saturating_sub(1));
            if down {
                p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1));
            }
            if up {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        if down || up {
            ctx.data_mut(|data| {
                data.remove::<(u64, i32, usize, usize)>(egui::Id::new("completion_selected_row"));
            });
            self.defer_shortcut_tail();
        }
        if plain_key(Key::Enter) || plain_key(Key::Tab) {
            let popup = self.completions.as_ref().unwrap();
            let selected = popup.selected;
            let identity = (popup.tab_id, popup.version, popup.cursor, selected);
            let hidden = ctx
                .data(|data| {
                    data.get_temp::<((u64, i32, usize, usize), bool)>(egui::Id::new(
                        "completion_visible_selection",
                    ))
                })
                .is_some_and(|(shown, visible)| shown == identity && !visible);
            if hidden {
                // Reveal an off-screen selection before allowing keyboard acceptance.
                ctx.data_mut(|data| {
                    data.remove::<(u64, i32, usize, usize)>(egui::Id::new(
                        "completion_selected_row",
                    ));
                });
                return true;
            }
            self.apply_completion(selected);
            self.defer_shortcut_tail();
            return true;
        }
        false
    }
    fn completion_ui(&mut self) {
        let Some(p) = &self.completions else {
            self.ctx.data_mut(|data| {
                data.remove::<(u64, i32, usize, usize)>(egui::Id::new("completion_selected_row"));
                data.remove::<((u64, i32, usize, usize), bool)>(egui::Id::new(
                    "completion_visible_selection",
                ));
            });
            return;
        };
        let mut selected = None;
        let ctx = self.ctx.clone();
        let bounds = ctx.input(|i| i.content_rect());
        let selection = p.selected.min(p.items.len().saturating_sub(1));
        let identity = (p.tab_id, p.version, p.cursor, selection);
        let reveal_id = egui::Id::new("completion_selected_row");
        let reveal =
            ctx.data(|data| data.get_temp::<(u64, i32, usize, usize)>(reveal_id)) != Some(identity);
        ctx.data_mut(|data| data.insert_temp(reveal_id, identity));
        let caret = self.editor_cursor_rect.unwrap_or_else(|| {
            egui::Rect::from_min_size(
                egui::pos2(bounds.left() + 320.0, bounds.top() + 150.0),
                Vec2::ZERO,
            )
        });
        let row_height = design::DETAIL_ROW_HEIGHT;
        let list_height = (p.items.len().min(8) as f32 * row_height)
            .min((bounds.height() - 180.0).max(row_height));
        let popup_height = list_height + 30.0;
        let width = (bounds.width() - 32.0).clamp(200.0, 490.0);
        let top = if caret.bottom() + popup_height > bounds.bottom() - 40.0 {
            (caret.top() - popup_height).max(bounds.top() + 70.0)
        } else {
            caret.bottom() + 4.0
        };
        let position = egui::pos2(
            caret
                .left()
                .min(bounds.right() - width - 12.0)
                .max(bounds.left() + 12.0),
            top,
        );
        egui::Window::new("Suggestions")
            .id(egui::Id::new("completion"))
            .resizable(false)
            .collapsible(false)
            .title_bar(false)
            .fixed_size([width, popup_height])
            .fixed_pos(position)
            .show(&ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let scroll_id = ui.make_persistent_id("completion_results");
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("completion_results")
                    .animated(false)
                    .max_height(list_height);
                if reveal {
                    let offset = egui::scroll_area::State::load(&ctx, scroll_id)
                        .map_or(0.0, |state| state.offset.y);
                    let top = selection as f32 * row_height;
                    let bottom = top + row_height;
                    let offset = if top < offset {
                        top
                    } else if bottom > offset + list_height {
                        bottom - list_height
                    } else {
                        offset
                    };
                    scroll = scroll.vertical_scroll_offset(offset.max(0.0));
                }
                let output = scroll.show_rows(ui, row_height, p.items.len(), |ui, rows| {
                    for i in rows {
                        let item = &p.items[i];
                        let hint = format!(
                            "{}{}\nEnter / Tab to accept · Esc dismisses",
                            item.label,
                            item.detail
                                .as_ref()
                                .map(|detail| format!("\n{detail}"))
                                .unwrap_or_default()
                        );
                        let response = ui
                            .push_id(i, |ui| {
                                design::detail_row(
                                    ui,
                                    &item.label,
                                    item.detail.as_deref().unwrap_or(""),
                                    i == selection,
                                    &hint,
                                )
                            })
                            .inner;
                        if response.clicked() {
                            selected = Some(i);
                        }
                    }
                });
                let selected_top =
                    output.inner_rect.top() + selection as f32 * row_height - output.state.offset.y;
                let visible = output.inner_rect.intersect(ui.clip_rect());
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new("completion_visible_selection"),
                        (
                            identity,
                            selected_top >= visible.top() - 0.5
                                && selected_top + row_height <= visible.bottom() + 0.5,
                        ),
                    );
                });
                ui.add_space(5.0);
                ui.label(
                    RichText::new(format!("{} of {}", selection + 1, p.items.len()))
                        .size(10.5)
                        .color(MUTED),
                )
                .on_hover_text("↑ ↓ choose · Enter or Tab accepts · Esc closes");
            });
        if let Some(i) = selected {
            self.apply_completion(i);
        }
    }
}

#[cfg(test)]
#[path = "visual_tests.rs"]
mod visual_tests;

#[cfg(test)]
#[path = "git_history_tests.rs"]
mod git_history_tests;
#[cfg(test)]
#[path = "git_ui_tests.rs"]
mod git_ui_tests;

#[cfg(test)]
#[path = "git_modal_tests.rs"]
mod git_modal_tests;

#[cfg(test)]
#[path = "navigation_tests.rs"]
mod navigation_tests;

#[cfg(test)]
#[path = "palette_ui_tests.rs"]
mod palette_ui_tests;

#[cfg(test)]
#[path = "intel_tests.rs"]
mod intel_tests;

#[cfg(test)]
#[path = "project_search_ui_tests.rs"]
mod project_search_ui_tests;

#[cfg(test)]
#[path = "workspace_ux_tests.rs"]
mod workspace_ux_tests;

#[cfg(test)]
#[path = "explorer_filter_tests.rs"]
mod explorer_filter_tests;

#[cfg(test)]
#[path = "editor_ux_tests.rs"]
mod editor_ux_tests;

#[cfg(test)]
#[path = "workspace_context_tests.rs"]
mod workspace_context_tests;

#[cfg(test)]
#[path = "project_layout_tests.rs"]
mod project_layout_tests;

#[cfg(test)]
#[path = "agent_marks_receiver_tests.rs"]
mod agent_marks_receiver_tests;

#[cfg(test)]
#[path = "agent_marks_input_tests.rs"]
mod agent_marks_input_tests;

#[cfg(test)]
pub(crate) fn builtin_language_smoke_config(
    language: &str,
    root: &Path,
) -> crate::lsp::ServerConfig {
    let mut config = language_settings::defaults()
        .into_iter()
        .find(|config| config.language_id == language)
        .expect("known built-in test language");
    if language == "dart"
        && let Ok(program) = std::env::var("VEDIT_TEST_DART_SERVER")
    {
        config.program = program;
    }
    config.resolve(
        root,
        &project_tasks::TaskConfiguration::default().python_program(root),
    )
}

#[cfg(test)]
#[path = "disk_sync_tests.rs"]
mod disk_sync_tests;
