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
#[path = "context_actions.rs"]
mod context_actions;
#[path = "control.rs"]
mod control;
#[path = "editor_feedback.rs"]
mod editor_feedback;
#[path = "explorer_navigation.rs"]
mod explorer_navigation;
#[path = "find.rs"]
mod find;
#[path = "git_panel.rs"]
mod git_panel;
#[path = "navigation.rs"]
mod navigation;
#[path = "split.rs"]
mod split;
#[path = "tab_drag.rs"]
mod tab_drag;
#[path = "workspace_ui.rs"]
mod workspace_ui;

const BG: Color32 = Color32::from_rgb(17, 20, 27);
const PANEL: Color32 = Color32::from_rgb(22, 26, 34);
const SURFACE: Color32 = Color32::from_rgb(29, 34, 44);
const HOVER: Color32 = Color32::from_rgb(39, 47, 60);
const BORDER: Color32 = Color32::from_rgb(43, 50, 63);
const TEXT: Color32 = Color32::from_rgb(224, 230, 240);
const MUTED: Color32 = Color32::from_rgb(139, 150, 172);
const ACCENT: Color32 = Color32::from_rgb(126, 211, 182);
const BLUE: Color32 = Color32::from_rgb(147, 174, 255);
const ORANGE: Color32 = Color32::from_rgb(237, 191, 126);
const RED: Color32 = Color32::from_rgb(240, 136, 148);
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
    vim: bool,
    swap_jk: bool,
    autosave: bool,
    font_size: f32,
    editor_font: crate::cli::EditorFont,
    tab_width: usize,
    word_wrap: bool,
    smooth_scroll: bool,
    leader_key: String,
    search_excludes: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            vim: true,
            swap_jk: false,
            autosave: true,
            font_size: 14.0,
            editor_font: crate::cli::EditorFont::default(),
            tab_width: 4,
            word_wrap: false,
            smooth_scroll: false,
            leader_key: " ".into(),
            search_excludes: workspace::default_search_excludes(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Session {
    projects: Vec<SessionProject>,
    active_project: usize,
    sidebar: Option<SidebarView>,
    autosave: Option<bool>,
    settings: Option<Settings>,
    layout: Option<split::SessionLayout>,
    bookmarks: Vec<crate::bookmarks::Bookmark>,
    agent_marks: crate::agent_marks::AgentMarks,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SessionProject {
    root: PathBuf,
    open_files: Vec<PathBuf>,
    active_file: Option<PathBuf>,
    tabs: Vec<SessionTab>,
    active_tab_index: Option<usize>,
    layout: Option<split::SessionLayout>,
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
    root: PathBuf,
    tabs: Vec<EditorTab>,
    active_tab: usize,
    /// Parked while another project is active; buffers always stay in this project.
    layout: Option<split::EditorLayout>,
    index: Option<Arc<workspace::WorkspaceIndex>>,
    indexing: bool,
    index_cancel: Arc<AtomicBool>,
    index_generation: u64,
    index_error: Option<String>,
    rust_project: bool,
    git_root: Option<PathBuf>,
    git_branch: Option<String>,
    git_tracking: Option<git::BranchInfo>,
    git_changes: Vec<git::FileStatus>,
    git_stats: git::WorkingTreeStats,
    git_stats_error: Option<String>,
    recent_commits: Vec<git::CommitSummary>,
    git_history_snapshot: Option<git::HistorySnapshot>,
    git_history_loading: bool,
    git_history_has_more: bool,
    git_history_generation: u64,
    git_history_update_available: bool,
    git_history_refresh_requested: bool,
    git_history_error: Option<String>,
    git_view: git_panel::GitViewState,
    git_error: Option<String>,
    git_loading: bool,
    git_refresh_pending: bool,
    last_git_refresh: Instant,
    commit_message: String,
    language: Option<crate::lsp::LanguageServer>,
    language_ready: bool,
    language_error: Option<String>,
    language_opened: HashMap<PathBuf, (u64, i32)>,
    diagnostics: HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>,
}
impl Project {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            tabs: Vec::new(),
            active_tab: 0,
            layout: None,
            index: None,
            indexing: false,
            index_cancel: Arc::new(AtomicBool::new(false)),
            index_generation: 0,
            index_error: None,
            rust_project: false,
            git_root: None,
            git_branch: None,
            git_tracking: None,
            git_changes: Vec::new(),
            git_stats: git::WorkingTreeStats::default(),
            git_stats_error: None,
            recent_commits: Vec::new(),
            git_history_snapshot: None,
            git_history_loading: false,
            git_history_has_more: false,
            git_history_generation: 0,
            git_history_update_available: false,
            git_history_refresh_requested: false,
            git_history_error: None,
            git_view: git_panel::GitViewState::default(),
            git_error: None,
            git_loading: false,
            git_refresh_pending: false,
            last_git_refresh: Instant::now() - Duration::from_secs(30),
            commit_message: String::new(),
            language: None,
            language_ready: false,
            language_error: None,
            language_opened: HashMap::new(),
            diagnostics: HashMap::new(),
        }
    }
}
struct EditorTab {
    id: u64,
    path: Option<PathBuf>,
    title: String,
    text: String,
    saved_text: String,
    dirty: bool,
    agent_unsaved: bool,
    recovery_path: PathBuf,
    recovery_revision: u64,
    recovery_pending: bool,
    revision: u64,
    fingerprint: Option<workspace::FileFingerprint>,
    line_ending: workspace::LineEnding,
    bom: bool,
    loading: bool,
    saving: bool,
    error: Option<String>,
    last_edit: Instant,
    vim: VimState,
    selection: Range<usize>,
    pending_selection: Option<Range<usize>>,
    focus: bool,
    scroll_to_cursor: bool,
    center_on_reveal: bool,
    pending_navigation_feedback: bool,
    feedback: Option<editor_feedback::Highlight>,
    lines: crate::render::LineIndex,
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
            agent_unsaved: false,
            recovery_path,
            recovery_revision: 0,
            recovery_pending: false,
            revision: 0,
            fingerprint: None,
            line_ending: workspace::LineEnding::Lf,
            bom: false,
            loading: false,
            saving: false,
            error: None,
            last_edit: Instant::now(),
            vim: VimState::new(),
            selection: 0..0,
            pending_selection: Some(0..0),
            focus: true,
            scroll_to_cursor: false,
            center_on_reveal: false,
            pending_navigation_feedback: false,
            feedback: None,
            lines: crate::render::LineIndex::default(),
            agent_view: crate::cli::DocumentView::default(),
            annotations: Vec::new(),
            annotations_revision: None,
            pending_agent_location: None,
            agent_location_error: None,
        }
    }
    fn changed(&mut self) {
        self.revision += 1;
        self.dirty = self.text != self.saved_text;
        self.last_edit = Instant::now();
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
    fn set_cursor(&mut self, range: Range<usize>) {
        self.vim.set_cursor(&self.text, range.start);
        // Explicit navigation may target the boundary after the last character.
        self.vim.cursor = range.start;
        self.selection = range.clone();
        self.pending_selection = Some(range);
        self.focus = true;
        self.scroll_to_cursor = true;
    }
}
struct GitSnapshot {
    root: Option<PathBuf>,
    branch: Option<String>,
    tracking: Option<git::BranchInfo>,
    files: Vec<git::FileStatus>,
    stats: git::WorkingTreeStats,
    stats_error: Option<String>,
    history: Result<GitHistoryRefresh, String>,
}
struct GitHistoryRefresh {
    snapshot: git::HistorySnapshot,
    page: Option<git::HistoryPage>,
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
    Index {
        root: PathBuf,
        generation: u64,
        result: Result<workspace::WorkspaceIndex, String>,
    },
    Loaded {
        id: u64,
        result: Result<workspace::LoadedDocument, String>,
        recovery: Option<Result<workspace::LoadedDocument, String>>,
    },
    Saved {
        id: u64,
        text: String,
        path: PathBuf,
        result: Result<workspace::FileFingerprint, String>,
    },
    Recovered {
        id: u64,
        revision: u64,
        result: Result<(), String>,
    },
    GitStatus {
        root: PathBuf,
        result: Result<GitSnapshot, String>,
    },
    GitCommit {
        root: PathBuf,
        hash: String,
        result: Result<git::CommitDetails, String>,
    },
    GitHistory {
        root: PathBuf,
        generation: u64,
        offset: usize,
        result: Result<git::HistoryPage, String>,
    },
    GitAction {
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
    DiskChanges(
        Vec<(
            u64,
            workspace::FileFingerprint,
            Result<workspace::LoadedDocument, String>,
        )>,
    ),
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
pub struct MorrowApp {
    projects: Vec<Project>,
    active_project: usize,
    workspace_ui: workspace_ui::WorkspaceUiState,
    explorer_navigation: explorer_navigation::ExplorerNavigationState,
    sidebar: SidebarView,
    sidebar_visible: bool,
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
    agent_marks: crate::agent_marks::AgentMarks,
    agent_marks_ui: agent_marks_ui::AgentMarksUi,
    applied_editor_font: crate::cli::EditorFont,
    save_as: Option<u64>,
    save_queue: std::collections::VecDeque<u64>,
    close_action: Option<CloseAction>,
    close_after_save: bool,
    exit_ready: bool,
    git_busy: Option<PathBuf>,
    diff: Option<(String, String)>,
    diff_view: crate::git_diff_view::DiffView,
    diff_generation: u64,
    diff_loading: bool,
    diff_error: Option<String>,
    output_visible: bool,
    command: String,
    task: Option<tasks::RunningTask>,
    output: String,
    task_status: String,
    register: Option<Yank>,
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
    pending_location: Option<(u64, crate::lsp::Position)>,
    problems_visible: bool,
    last_disk_check: Instant,
    disk_check_running: bool,
    syntax: crate::render::SyntaxCache,
    markdown: crate::markdown::MarkdownCache,
    control: Option<crate::cli::Server>,
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
impl MorrowApp {
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
        app.apply_editor_font();
        app.settings.leader_key = crate::cli::normalize_leader_key(&app.settings.leader_key)
            .unwrap_or_else(|_| " ".into());
        app.bookmarks = crate::bookmarks::BookmarkStore::from_items(session.bookmarks);
        app.agent_marks = session.agent_marks;
        if let Some(autosave) = session.autosave {
            app.settings.autosave = autosave;
        }
        app.sidebar = session.sidebar.unwrap_or_default();
        app.restore_session_workspace(session.projects, session.active_project, session.layout);
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
            if !root.is_dir() {
                continue;
            }
            let mut project = Project::new(root.clone());
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
            agent_marks: crate::agent_marks::AgentMarks::default(),
            agent_marks_ui: agent_marks_ui::AgentMarksUi::default(),
            save_as: None,
            save_queue: std::collections::VecDeque::new(),
            close_action: None,
            close_after_save: false,
            exit_ready: false,
            git_busy: None,
            diff: None,
            diff_view: crate::git_diff_view::DiffView::default(),
            diff_generation: 0,
            diff_loading: false,
            diff_error: None,
            output_visible: false,
            command: String::new(),
            task: None,
            output: String::new(),
            task_status: "Ready".into(),
            register: None,
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
            pending_location: None,
            problems_visible: false,
            last_disk_check: Instant::now(),
            disk_check_running: false,
            syntax: crate::render::SyntaxCache::new(),
            markdown: crate::markdown::MarkdownCache::default(),
            control: None,
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
            let _ = sender.send(result);
            ctx.request_repaint();
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
        self.sidebar_visible = true;
        self.sidebar = SidebarView::Explorer;
        self.session_dirty = true;
    }
    fn index_project(&mut self, index: usize) {
        let Some(p) = self.projects.get_mut(index) else {
            return;
        };
        p.index_cancel.store(true, Ordering::Relaxed);
        p.index_cancel = Arc::new(AtomicBool::new(false));
        p.index_generation += 1;
        p.indexing = true;
        p.index_error = None;
        let root = p.root.clone();
        let generation = p.index_generation;
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
        });
    }
    fn load_tab(&self, id: u64, path: Option<PathBuf>, recovery: Option<PathBuf>) {
        self.worker(move || {
            let result = path
                .as_ref()
                .map(|p| workspace::read_document(p, MAX_DOCUMENT))
                .unwrap_or_else(|| Err("Untitled".into()));
            let recovered = recovery
                .filter(|p| p.is_file())
                .map(|p| workspace::read_document(&p, MAX_DOCUMENT));
            WorkerResult::Loaded {
                id,
                result,
                recovery: recovered,
            }
        });
    }
    fn open_file(&mut self, path: PathBuf) {
        if self.context_path_busy(&path) {
            self.status("Wait for the file rename to finish before opening it", true);
            return;
        }
        if let Some(index) = self
            .project()
            .and_then(|p| p.tabs.iter().position(|t| t.path.as_ref() == Some(&path)))
        {
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
            self.activate_tab(self.active_project, index);
            return;
        }
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return;
        };
        let mut tab = EditorTab::new(Some(path.clone()), file_title(&path));
        tab.loading = true;
        let id = tab.id;
        project.tabs.push(tab);
        let index = project.tabs.len() - 1;
        self.activate_tab(self.active_project, index);
        self.load_tab(id, Some(path), None);
    }
    fn activate_tab(&mut self, project_index: usize, tab_index: usize) {
        if self
            .projects
            .get(project_index)
            .is_none_or(|p| !p.tabs.is_empty() && tab_index >= p.tabs.len())
        {
            return;
        }
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
        let previous_root = self.project().map(|project| project.root.clone());
        self.activate_project_layout(index);
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
        let expected = overwrite.or_else(|| {
            (tab.path.as_ref() == Some(&path))
                .then(|| tab.fingerprint.clone())
                .flatten()
        });
        let text = tab.text.clone();
        let ending = tab.line_ending;
        let bom = tab.bom;
        tab.saving = true;
        tab.error = None;
        self.worker(move || {
            let result = (|| {
                if expected.is_none()
                    && let Some(parent) = path.parent()
                {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                workspace::save_document(&path, &text, ending, bom, expected.as_ref())
                    .map_err(|e| e.to_string())
            })();
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
                    p.index_cancel.store(true, Ordering::Relaxed);
                    for t in p.tabs {
                        self.agent_marks.detach_document(t.id);
                        let _ = self.recovery_writer.remove(&t.recovery_path, t.revision);
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
                        let _ = self.recovery_writer.remove(&t.recovery_path, t.revision);
                    }
                }
                self.flush_recovery();
                self.exit_ready = true;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.close_action = None;
        self.close_after_save = false;
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
                let _ = self
                    .recovery_writer
                    .remove(&tab.recovery_path, tab.revision);
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
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                WorkerResult::ContextFile(result) => self.context_file_result(result),
                WorkerResult::ContextFilePrepared(result) => self.context_file_prepared(result),
                WorkerResult::Index {
                    root,
                    generation,
                    result,
                } => {
                    if let Some(p) = self
                        .projects
                        .iter_mut()
                        .find(|p| p.root == root && p.index_generation == generation)
                    {
                        p.indexing = false;
                        match result {
                            Ok(index) => {
                                p.rust_project = index.files.iter().any(|path| {
                                    path.file_name().is_some_and(|name| name == "Cargo.toml")
                                });
                                p.index = Some(Arc::new(index));
                            }
                            Err(e) => p.index_error = Some(e),
                        }
                    }
                }
                WorkerResult::Loaded {
                    id,
                    result,
                    recovery,
                } => {
                    self.find.invalidate(id);
                    let ctx = self.ctx.clone();
                    if let Some(tab) = self.tab_by_id_mut(id) {
                        tab.loading = false;
                        tab.lines.invalidate();
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
                            }
                            Err(error) => {
                                if tab.path.is_some() {
                                    tab.error = Some(error);
                                }
                            }
                        }
                        if let Some(Ok(doc)) = recovery {
                            if tab.text != doc.text {
                                tab.revision = tab.revision.saturating_add(1);
                            }
                            tab.text = doc.text;
                            tab.dirty = tab.text != tab.saved_text;
                            tab.recovery_revision = tab.revision;
                        }
                        let range = tab
                            .pending_selection
                            .clone()
                            .filter(|r| r.start != 0 || r.end != 0)
                            .unwrap_or(tab.vim.cursor..tab.vim.cursor);
                        let length = tab.text.chars().count();
                        tab.set_cursor(range.start.min(length)..range.end.min(length));
                    }
                }
                WorkerResult::Saved {
                    id,
                    text,
                    path,
                    result,
                } => {
                    let mut message = None;
                    let recovery_writer = self.recovery_writer.clone();
                    let mut new_document = false;
                    if let Some(tab) = self.tab_by_id_mut(id) {
                        tab.saving = false;
                        match result {
                            Ok(fingerprint) => {
                                new_document = tab.path.as_ref() != Some(&path);
                                tab.path = Some(path.clone());
                                tab.title = file_title(&path);
                                tab.saved_text = text;
                                tab.fingerprint = Some(fingerprint);
                                tab.dirty = tab.text != tab.saved_text;
                                if !tab.dirty {
                                    tab.agent_unsaved = false;
                                }
                                tab.error = None;
                                if !tab.dirty {
                                    let _ =
                                        recovery_writer.remove(&tab.recovery_path, tab.revision);
                                }
                                message = Some((format!("Saved {}", tab.title), false));
                            }
                            Err(error) => {
                                tab.error = Some(error.clone());
                                tab.last_edit = Instant::now();
                                message = Some((error, true));
                            }
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
                    if new_document
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
                        if project.tabs.iter().any(|t| t.id == id && t.error.is_none())
                            && let Some(server) = &project.language
                        {
                            server.save(&path);
                        }
                    }
                }
                WorkerResult::Recovered {
                    id,
                    revision,
                    result,
                } => {
                    if let Some(tab) = self.tab_by_id_mut(id) {
                        tab.recovery_pending = false;
                        if result.is_ok() {
                            tab.recovery_revision = tab.recovery_revision.max(revision);
                        } else {
                            tab.last_edit = Instant::now();
                        }
                    }
                    if let Err(e) = result {
                        self.status(e, true);
                    }
                    self.session_dirty = true;
                }
                WorkerResult::GitStatus { root, result } => {
                    if let Some(index) = self.projects.iter().position(|p| p.root == root) {
                        let p = &mut self.projects[index];
                        p.git_loading = false;
                        p.last_git_refresh = Instant::now();
                        match result {
                            Ok(s) => {
                                p.git_root = s.root;
                                p.git_branch = s.branch;
                                p.git_tracking = s.tracking;
                                p.git_changes = s.files;
                                p.git_stats = s.stats;
                                p.git_stats_error = s.stats_error;
                                match s.history {
                                    Ok(history) => {
                                        if let Some(page) = history.page {
                                            static NEXT_HISTORY: AtomicU64 = AtomicU64::new(1);
                                            p.git_history_generation =
                                                NEXT_HISTORY.fetch_add(1, Ordering::Relaxed);
                                            p.git_history_loading = false;
                                            p.git_history_has_more = page.has_more;
                                            p.recent_commits = page.commits;
                                            p.git_view.graph.clear();
                                            p.git_view.graph.append(&p.recent_commits);
                                            p.git_history_error = None;
                                            p.git_history_update_available = false;
                                            p.git_history_snapshot = Some(history.snapshot);
                                        } else {
                                            // Keep browsing and paging the original object
                                            // IDs until the user chooses to refresh history.
                                            p.git_history_update_available =
                                                p.git_history_snapshot.as_ref()
                                                    != Some(&history.snapshot);
                                        }
                                    }
                                    Err(error) => p.git_history_error = Some(error),
                                }
                                p.git_error = None;
                            }
                            Err(e) => p.git_error = Some(e),
                        }
                        let refresh = std::mem::take(&mut p.git_refresh_pending);
                        if refresh {
                            self.refresh_git(index, true);
                        }
                    }
                }
                WorkerResult::GitCommit { root, hash, result } => {
                    if let Some(p) = self.projects.iter_mut().find(|p| p.root == root) {
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
                    root,
                    message,
                    result,
                } => {
                    self.git_busy = None;
                    match result {
                        Ok(_) => {
                            self.status(format!("{message} completed"), false);
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
                WorkerResult::DiskChanges(changes) => {
                    self.disk_check_running = false;
                    for (id, baseline, result) in changes {
                        if let Some(t) = self.tab_by_id_mut(id) {
                            if t.fingerprint.as_ref() != Some(&baseline) || t.saving || t.loading {
                                continue;
                            }
                            match result {
                                Ok(doc) if doc.fingerprint.hash != baseline.hash => {
                                    if t.dirty {
                                        t.error=Some("This file changed outside Morrow. Save your edits as another file, or close and reopen it.".into());
                                    } else {
                                        let cursor = t
                                            .vim
                                            .cursor
                                            .min(doc.text.chars().count().saturating_sub(1));
                                        t.text = doc.text.clone();
                                        t.saved_text = doc.text;
                                        t.fingerprint = Some(doc.fingerprint);
                                        t.line_ending = doc.line_ending;
                                        t.bom = doc.bom;
                                        t.revision += 1;
                                        t.error = None;
                                        t.vim = VimState::new();
                                        t.lines.invalidate();
                                        t.set_cursor(cursor..cursor);
                                    }
                                }
                                Ok(doc) => {
                                    t.fingerprint = Some(doc.fingerprint);
                                    t.error = None;
                                }
                                Err(error) => t.error = Some(error),
                            }
                        }
                    }
                }
            }
        }
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
        let mut finished = false;
        if let Some(task) = &self.task {
            loop {
                let event = match task.receiver.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if !finished {
                            self.task_status = "Task stopped unexpectedly".into();
                        }
                        finished = true;
                        break;
                    }
                };
                match event {
                    tasks::TaskEvent::Output(text) => {
                        if self.output.len() < 1024 * 1024 {
                            self.output.push_str(&text);
                        }
                    }
                    tasks::TaskEvent::Finished { code, cancelled } => {
                        self.task_status = if cancelled {
                            "Cancelled".into()
                        } else if code == Some(0) {
                            "Completed".into()
                        } else {
                            format!(
                                "Exited with code {}",
                                code.map(|c| c.to_string())
                                    .unwrap_or_else(|| "unknown".into())
                            )
                        };
                        finished = true;
                    }
                    tasks::TaskEvent::Error(e) => {
                        self.output.push_str(&format!("\n{e}\n"));
                        self.task_status = "Failed".into();
                    }
                }
            }
        }
        if finished {
            self.task = None;
            self.refresh_git(self.active_project, true);
        }
        if self.task.is_some() {
            self.ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    fn refresh_git(&mut self, index: usize, force: bool) {
        let interval = if self.sidebar_visible && self.sidebar == SidebarView::SourceControl {
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
        if self.git_busy.is_some() || (!force && p.last_git_refresh.elapsed() < interval) {
            return;
        }
        p.git_loading = true;
        let root = p.root.clone();
        let previous_history = p.git_history_snapshot.clone();
        let refresh_history = std::mem::take(&mut p.git_history_refresh_requested);
        let previous_history_error = p.git_history_error.clone().filter(|_| !refresh_history);
        self.worker(move || {
            let result = (|| {
                let repo = git::repository_root(&root).map_err(|e| e.to_string())?;
                if let Some(repo) = repo {
                    let status = git::status(&repo).map_err(|e| e.to_string())?;
                    let tracking = git::branch_info(&repo).ok();
                    let (stats, stats_error) = match git::working_tree_stats(&repo, &status.files) {
                        Ok(stats) => (stats, None),
                        Err(error) => (git::WorkingTreeStats::default(), Some(error.to_string())),
                    };
                    let history = (|| {
                        if let Some(error) = previous_history_error {
                            return Err(error);
                        }
                        let snapshot =
                            git::history_snapshot(&repo).map_err(|error| error.to_string())?;
                        let page = if refresh_history
                            || previous_history
                                .as_ref()
                                .is_none_or(|history| history.tips.is_empty())
                        {
                            Some(
                                git::history_page(&repo, &snapshot, 0, git::HISTORY_PAGE_SIZE)
                                    .map_err(|error| error.to_string())?,
                            )
                        } else {
                            None
                        };
                        Ok(GitHistoryRefresh { snapshot, page })
                    })();
                    Ok(GitSnapshot {
                        root: Some(repo),
                        branch: status.branch,
                        tracking,
                        files: status.files,
                        stats,
                        stats_error,
                        history,
                    })
                } else {
                    Ok(GitSnapshot {
                        root: None,
                        branch: None,
                        tracking: None,
                        files: Vec::new(),
                        stats: git::WorkingTreeStats::default(),
                        stats_error: None,
                        history: Ok(GitHistoryRefresh {
                            snapshot: git::HistorySnapshot::default(),
                            page: Some(git::HistoryPage::default()),
                        }),
                    })
                }
            })();
            WorkerResult::GitStatus { root, result }
        });
    }
    fn git_action(&mut self, root: PathBuf, action: GitAction) {
        if self.git_busy.is_some() {
            return;
        }
        self.git_busy = Some(root.clone());
        let message = action.label().to_owned();
        self.status(format!("{message}…"), false);
        self.worker(move || WorkerResult::GitAction {
            result: action.run(&root),
            root,
            message,
        });
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
        let text_characters: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let egui::Event::Text(text) = event {
                    Some(text.chars())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        let mut keep = Vec::new();
        let mut action = crate::screen_jump::JumpAction::None;
        for event in events {
            if !matches!(action, crate::screen_jump::JumpAction::None) {
                // A completed jump owns the entire input batch; trailing query
                // characters must never become an editing command.
                if !matches!(event, egui::Event::Key { .. } | egui::Event::Text(_)) {
                    keep.push(event);
                }
                continue;
            }
            match event {
                egui::Event::Text(text) => {
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
                            jump.state.enter();
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
                            if let Some(ch) = ch.filter(|ch| !text_characters.contains(ch)) {
                                action = jump.state.input_char(ch);
                                jump.state.recompute(&jump.glyphs);
                            }
                        }
                    }
                }
                egui::Event::Key { .. } => {}
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
        let mut saves = Vec::new();
        let mut delay: Option<Duration> = None;
        for p in &mut self.projects {
            for t in &mut p.tabs {
                if !t.dirty || t.loading {
                    continue;
                }
                let remaining = Duration::from_millis(850).saturating_sub(t.last_edit.elapsed());
                if !remaining.is_zero() {
                    delay = Some(delay.map_or(remaining, |d| d.min(remaining)));
                    continue;
                }
                if t.recovery_revision != t.revision && !t.recovery_pending {
                    t.recovery_pending = true;
                    recoveries.push((t.id, t.revision, t.recovery_path.clone(), t.text.clone()));
                }
                if self.settings.autosave
                    && !t.agent_unsaved
                    && self.save_as != Some(t.id)
                    && t.path.is_some()
                    && !t.saving
                    && t.error.is_none()
                {
                    saves.push(t.id);
                }
            }
        }
        for (id, revision, path, text) in recoveries {
            let writer = self.recovery_writer.clone();
            self.worker(move || {
                let result = writer.write(&path, revision, text.as_bytes());
                WorkerResult::Recovered {
                    id,
                    revision,
                    result,
                }
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
                    root: p.root.clone(),
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
            autosave: Some(self.settings.autosave),
            settings: Some(self.settings.clone()),
            layout: None,
            bookmarks: self.bookmarks.items.clone(),
            agent_marks: self.agent_marks.clone(),
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
                    let _ = self.recovery_writer.write(
                        &tab.recovery_path,
                        tab.revision,
                        tab.text.as_bytes(),
                    );
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
        if self.task.is_some() {
            self.status("Stop the current task first", true);
            return;
        }
        let Some(root) = self.project().map(|p| p.root.clone()) else {
            return;
        };
        self.output_visible = true;
        self.problems_visible = false;
        self.output = format!("> {command}\n{}\n\n", crate::paths::display(&root));
        self.task_status = "Running".into();
        self.command = command.clone();
        self.task = Some(tasks::spawn_task(root, command));
    }
    fn open_terminal(&mut self) {
        let Some(root) = self.project().map(|p| p.root.clone()) else {
            return;
        };
        match tasks::open_terminal(&root) {
            Ok(()) => self.status("Terminal opened in this project", false),
            Err(error) => self.status(error, true),
        }
    }
}

impl MorrowApp {
    fn shortcuts(&mut self) {
        let ctx = self.ctx.clone();
        if self.picker.is_some()
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
        {
            return;
        }
        if self.completion_keys() {
            return;
        }
        let command = egui::Modifiers::COMMAND;
        let shift = command.plus(egui::Modifiers::SHIFT);
        if self.split_shortcuts(&ctx) {
            return;
        }
        if self.tab().is_some_and(|t| is_markdown(t.path.as_deref()))
            && ctx.input_mut(|i| i.consume_key(shift, Key::V))
        {
            let next = if self
                .tab()
                .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
            {
                crate::cli::DocumentView::Source
            } else {
                crate::cli::DocumentView::Preview
            };
            self.set_document_view(next);
        }
        let vim_modal = self.settings.vim
            && self.tab().is_some_and(|t| {
                t.vim.mode != VimMode::Insert
                    && ctx.memory(|m| m.has_focus(self.document_widget_id(t.id)))
            });
        let vim_insert = self.settings.vim
            && self.tab().is_some_and(|t| {
                t.vim.mode == VimMode::Insert
                    && ctx.memory(|m| m.has_focus(self.document_widget_id(t.id)))
            });
        if ctx.input_mut(|i| i.consume_key(command, Key::Space)) {
            self.request_completion();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F12)) {
            self.request_definition();
        }
        // egui allows extra Shift modifiers, so test specific shortcuts first.
        if ctx.input_mut(|i| i.consume_key(shift, Key::P)) {
            self.open_palette(Palette::Commands);
        } else if ctx.input_mut(|i| i.consume_key(command, Key::P)) {
            self.open_palette(Palette::Files);
        }
        if self.palette.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::O)) {
            self.pick_project();
        } else if ctx.input_mut(|i| i.consume_key(command, Key::O)) {
            self.pick_file();
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::S)) {
            self.save_all();
        } else if ctx.input_mut(|i| i.consume_key(command, Key::S)) {
            self.save_current();
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::N)) {
            if vim_insert {
                self.request_completion();
            } else {
                self.new_file();
            }
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::W))
            && let Some(tab) = self.tab()
        {
            self.request_close(CloseAction::PaneTab {
                pane_id: self.current_pane_id(),
                id: tab.id,
            });
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::F)) {
            self.open_project_search();
        } else if !vim_modal && ctx.input_mut(|i| i.consume_key(command, Key::F)) {
            self.find_visible = true;
            self.find_focus = true;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::H)) {
            self.find_visible = true;
            self.replace_visible = true;
            self.find_focus = true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::F3)) {
            self.find_next(true);
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F3)) {
            self.find_next(false);
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::G)) {
            self.open_palette(Palette::Line);
        }
        if !vim_modal && ctx.input_mut(|i| i.consume_key(command, Key::B)) {
            self.sidebar_visible = !self.sidebar_visible;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::Backtick)) {
            self.output_visible = !self.output_visible;
        }
        if ctx.input_mut(|i| i.consume_key(shift, Key::M)) {
            self.output_visible = true;
            self.problems_visible = true;
        }
        if ctx.input_mut(|i| i.consume_key(command, Key::Comma)) {
            self.settings_open = true;
        }
        let previous = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT), Key::Tab)
        });
        let next = !previous && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, Key::Tab));
        if previous || next {
            self.bind_current_pane();
            let ids = self.current_pane_tab_ids();
            if !ids.is_empty() {
                let active = self
                    .tab()
                    .and_then(|tab| ids.iter().position(|id| *id == tab.id))
                    .unwrap_or(0);
                let index = if previous {
                    (active + ids.len() - 1) % ids.len()
                } else {
                    (active + 1) % ids.len()
                };
                if let Some((project, tab)) =
                    self.projects.iter().enumerate().find_map(|(pi, p)| {
                        p.tabs
                            .iter()
                            .position(|tab| tab.id == ids[index])
                            .map(|ti| (pi, ti))
                    })
                {
                    self.activate_tab(project, tab);
                }
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F5)) {
            self.run_task("cargo check".into());
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::F6)) {
            self.run_task("cargo test".into());
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::Z)) {
            self.settings.word_wrap = !self.settings.word_wrap;
        }
    }
    fn handle_vim(&mut self) {
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

        // Native insert events normally reach TextEdit once. If Escape occurs in
        // the same frame, apply the queued edits before ending the transaction.
        fn flush_insert(tab: &mut EditorTab, events: &mut Vec<egui::Event>, width: usize) {
            for event in events.drain(..) {
                let len = tab.text.chars().count();
                let caret = tab.vim.cursor.min(len);
                let mut selection = tab.selection.start.min(len)..tab.selection.end.min(len);
                let indent = " ".repeat(width);
                let edit = match event {
                    egui::Event::Key {
                        key: Key::A,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command || modifiers.ctrl => {
                        tab.selection = 0..len;
                        tab.vim.set_cursor(&tab.text, len);
                        tab.pending_selection = Some(tab.selection.clone());
                        None
                    }
                    egui::Event::Text(text) | egui::Event::Paste(text) => Some(
                        editor::replace_selection(&tab.text, selection.clone(), &text),
                    ),
                    egui::Event::Ime(egui::ImeEvent::Commit(text)) => Some(
                        editor::replace_selection(&tab.text, selection.clone(), &text),
                    ),
                    egui::Event::Key {
                        key: Key::Enter,
                        pressed: true,
                        modifiers,
                        ..
                    } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => {
                        Some(editor::smart_newline(&tab.text, selection.clone(), &indent))
                    }
                    egui::Event::Key {
                        key: Key::Tab,
                        pressed: true,
                        modifiers,
                        ..
                    } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => {
                        Some(if modifiers.shift {
                            editor::indent_lines(&tab.text, selection.clone(), &indent, true)
                        } else if selection.is_empty() {
                            editor::replace_selection(&tab.text, selection.clone(), &indent)
                        } else {
                            editor::indent_lines(&tab.text, selection.clone(), &indent, false)
                        })
                    }
                    egui::Event::Key {
                        key: Key::Backspace | Key::Delete,
                        pressed: true,
                        modifiers,
                        ..
                    } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => {
                        if selection.is_empty() {
                            selection = if matches!(
                                event,
                                egui::Event::Key {
                                    key: Key::Backspace,
                                    ..
                                }
                            ) {
                                caret.saturating_sub(1)..caret
                            } else {
                                caret..caret.saturating_add(1).min(len)
                            };
                        }
                        Some(editor::replace_selection(&tab.text, selection, ""))
                    }
                    egui::Event::Key {
                        key:
                            Key::ArrowLeft
                            | Key::ArrowRight
                            | Key::ArrowUp
                            | Key::ArrowDown
                            | Key::Home
                            | Key::End,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let egui::Event::Key { key, .. } = event else {
                            unreachable!()
                        };
                        let chars: Vec<char> = tab.text.chars().collect();
                        let mut starts = vec![0];
                        for (index, ch) in chars.iter().enumerate() {
                            if *ch == '\n' {
                                starts.push(index + 1);
                            }
                        }
                        let row = starts
                            .partition_point(|start| *start <= caret)
                            .saturating_sub(1);
                        let line_end =
                            |row: usize| starts.get(row + 1).map_or(chars.len(), |next| next - 1);
                        let column = caret - starts[row];
                        let next = match key {
                            Key::ArrowLeft => {
                                if !modifiers.shift && !selection.is_empty() {
                                    selection.start
                                } else {
                                    caret.saturating_sub(1)
                                }
                            }
                            Key::ArrowRight => {
                                if !modifiers.shift && !selection.is_empty() {
                                    selection.end
                                } else {
                                    caret.saturating_add(1).min(len)
                                }
                            }
                            Key::ArrowUp => {
                                let target = row.saturating_sub(1);
                                starts[target] + column.min(line_end(target) - starts[target])
                            }
                            Key::ArrowDown => {
                                let target = (row + 1).min(starts.len() - 1);
                                starts[target] + column.min(line_end(target) - starts[target])
                            }
                            Key::Home => {
                                if modifiers.ctrl || modifiers.command {
                                    0
                                } else {
                                    starts[row]
                                }
                            }
                            Key::End => {
                                if modifiers.ctrl || modifiers.command {
                                    len
                                } else {
                                    line_end(row)
                                }
                            }
                            _ => caret,
                        };
                        let anchor = if caret == selection.start {
                            selection.end
                        } else {
                            selection.start
                        };
                        tab.selection = if modifiers.shift {
                            anchor.min(next)..anchor.max(next)
                        } else {
                            next..next
                        };
                        tab.vim.set_cursor(&tab.text, next);
                        tab.pending_selection = Some(tab.selection.clone());
                        None
                    }
                    _ => None,
                };
                if let Some(edit) = edit {
                    tab.apply(edit);
                    tab.vim.set_cursor(&tab.text, tab.selection.end);
                    tab.vim.notify_external_text_change();
                    tab.scroll_to_cursor = true;
                }
            }
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
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            tab.selection = selection.clone();
            tab.pending_selection = Some(selection);
            tab.scroll_to_cursor = true;
        }
        fn copy_to_clipboard(
            tab: &mut EditorTab,
            native_insert: &mut Vec<egui::Event>,
            width: usize,
            system_clipboard: &mut Option<String>,
        ) {
            flush_insert(tab, native_insert, width);
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
            native_insert: &mut Vec<egui::Event>,
            width: usize,
            system_clipboard: &mut Option<String>,
            vim_register: &mut Option<Yank>,
        ) {
            flush_insert(tab, native_insert, width);
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
        fn paste_system_text(
            tab: &mut EditorTab,
            native_insert: &mut Vec<egui::Event>,
            width: usize,
            text: &str,
        ) {
            flush_insert(tab, native_insert, width);
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
        let width = self.settings.tab_width.clamp(1, 16);
        let page = self.editor_rect.map_or(24, |rect| {
            (rect.height() / (self.settings.font_size + 5.0))
                .floor()
                .max(1.0) as isize
        });
        let pane_id = self.current_pane_id();
        let mut find = std::mem::take(&mut self.find);
        if let Some(tab) = self.tab_mut() {
            tab.vim.set_indent_width(width);
            if let Some(register) = register {
                tab.vim.set_register(register);
            }
            for event in events {
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
                        egui::Event::Text(text) => prompt_text.push_str(&text),
                        egui::Event::Paste(text) => prompt_text.push_str(&text),
                        egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            prompt_text.push_str(&text)
                        }
                        egui::Event::Copy => {
                            prompt = None;
                            prompt_text.clear();
                            copy_to_clipboard(
                                tab,
                                &mut native_insert,
                                width,
                                &mut system_clipboard,
                            );
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
                match &event {
                    egui::Event::Copy => {
                        copy_to_clipboard(tab, &mut native_insert, width, &mut system_clipboard);
                        continue;
                    }
                    egui::Event::Cut => {
                        cut_selection(
                            tab,
                            &mut native_insert,
                            width,
                            &mut system_clipboard,
                            &mut vim_register,
                        );
                        continue;
                    }
                    egui::Event::Paste(text) => {
                        paste_system_text(tab, &mut native_insert, width, text);
                        continue;
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_))
                        if tab.vim.mode == VimMode::Insert =>
                    {
                        native_insert.push(event);
                        flush_insert(tab, &mut native_insert, width);
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
                            copy_to_clipboard(
                                tab,
                                &mut native_insert,
                                width,
                                &mut system_clipboard,
                            );
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
                            cut_selection(
                                tab,
                                &mut native_insert,
                                width,
                                &mut system_clipboard,
                                &mut vim_register,
                            );
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
                        flush_insert(tab, &mut native_insert, width);
                        let action = tab.vim.handle_key(&mut tab.text, VimKey::Escape);
                        apply_action(tab, action, &mut vim_register);
                    } else if matches!(&event, egui::Event::Key { key: Key::Enter | Key::Tab, pressed: true, modifiers, .. }
                        if !modifiers.command && !modifiers.ctrl && !modifiers.alt)
                    {
                        // Preserve text-before-newline order; these two keys
                        // otherwise run as editor shortcuts before TextEdit.
                        native_insert.push(event);
                        flush_insert(tab, &mut native_insert, width);
                    } else {
                        native_insert.push(event);
                    }
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
                        let translated = if modifiers.ctrl && !modifiers.alt && key == Key::R {
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
                                Key::Enter if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Enter)
                                }
                                Key::Tab if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Tab)
                                }
                                Key::Backspace if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Backspace)
                                }
                                Key::ArrowLeft | Key::Backspace => Some(VimKey::Char('h')),
                                Key::ArrowDown | Key::Enter => Some(VimKey::Char('j')),
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
        }
        self.find = find;
        if vim_searched {
            self.find_visible = false;
        }
        self.find_message();
        retained.extend(native_insert);
        ctx.input_mut(|input| input.events = retained);
        if let Some(yank) = vim_register {
            self.register = Some(yank);
        }
        if let Some(text) = system_clipboard {
            ctx.copy_text(text);
        }
        if let Some(':') = prompt {
            self.vim_command = Some(prompt_text);
        }
        if definition {
            self.request_definition();
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(12, 5)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new()
                    .style(|style: &mut egui::Style| {
                        style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                        style.visuals.widgets.inactive.bg_stroke = Stroke::NONE;
                        style.visuals.widgets.hovered.weak_bg_fill = HOVER;
                        style.visuals.widgets.hovered.bg_stroke = Stroke::NONE;
                        style.visuals.widgets.active.bg_stroke = Stroke::NONE;
                        style.spacing.item_spacing.x = 3.0;
                    })
                    .ui(ui, |ui| {
                        brand(ui, 21.0);
                        ui.label(RichText::new("morrow").size(16.0).strong().color(TEXT));
                        ui.add_space(15.0);
                        self.workspace_header_button(ui);
                        ui.add_space(10.0);
                        design::menu(ui, "File", "Files, projects and saving", |ui| {
                            if design::menu_item(
                                ui,
                                "Open project",
                                "Ctrl+Shift+O",
                                "Add a project folder",
                            )
                            .clicked()
                            {
                                self.pick_project();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Open file",
                                "Ctrl+O",
                                "Choose a file from disk",
                            )
                            .clicked()
                            {
                                self.pick_file();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "New file",
                                "Ctrl+N",
                                "Create an untitled draft",
                            )
                            .clicked()
                            {
                                self.new_file();
                                ui.close();
                            }
                            ui.separator();
                            if design::menu_item(ui, "Save", "Ctrl+S", "Save the current document")
                                .clicked()
                            {
                                self.save_current();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Save all",
                                "Ctrl+Shift+S",
                                "Save changes in every project",
                            )
                            .clicked()
                            {
                                self.save_all();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Save as...",
                                "",
                                "Save this document to a new path",
                            )
                            .clicked()
                            {
                                if let Some(id) = self.tab().map(|tab| tab.id) {
                                    self.request_save_as(id);
                                }
                                ui.close();
                            }
                            ui.separator();
                            if design::menu_item(
                                ui,
                                "Settings",
                                "Ctrl+,",
                                "Vim, font and editor preferences",
                            )
                            .clicked()
                            {
                                self.settings_open = true;
                                ui.close();
                            }
                        });
                        design::menu(ui, "View", "Navigate and arrange your workspace", |ui| {
                            if design::menu_item(ui, "For agent", "Ctrl+Alt+Shift+A", "Open selected blocks across all projects, add notes, or restore the last collection").clicked() {
                                self.open_agent_marks();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Bookmarks",
                                "Leader b",
                                "Search saved file and cursor positions",
                            )
                            .clicked()
                            {
                                self.open_bookmarks();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Toggle bookmark",
                                "Leader m",
                                "Mark or unmark the current file and line",
                            )
                            .clicked()
                            {
                                self.toggle_bookmark();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Jump to visible text",
                                "Leader Leader f",
                                "Find a visible match and jump by its label",
                            )
                            .clicked()
                            {
                                self.start_screen_jump();
                                ui.close();
                            }
                            ui.separator();
                            if design::menu_item(
                                ui,
                                "Explorer",
                                "Ctrl+Shift+E",
                                "Focus project files · Arrows or J/K to navigate · F2 to rename",
                            )
                            .clicked()
                            {
                                self.focus_explorer();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Search project",
                                "Ctrl+Shift+F",
                                "Find text across files",
                            )
                            .clicked()
                            {
                                self.open_project_search();
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Source control",
                                "",
                                "Changes, staging and commits",
                            )
                            .clicked()
                            {
                                self.sidebar = SidebarView::SourceControl;
                                self.sidebar_visible = true;
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Problems",
                                "Ctrl+Shift+M",
                                "Rust errors and warnings",
                            )
                            .clicked()
                            {
                                self.output_visible = true;
                                self.problems_visible = true;
                                ui.close();
                            }
                            if design::menu_item(
                                ui,
                                "Output",
                                "Ctrl+`",
                                "Show or hide command output",
                            )
                            .clicked()
                            {
                                self.output_visible = !self.output_visible;
                                self.problems_visible = false;
                                ui.close();
                            }
                            if self
                                .tab()
                                .is_some_and(|tab| is_markdown(tab.path.as_deref()))
                                && design::menu_item(
                                    ui,
                                    "Markdown source / preview",
                                    "Ctrl+Shift+V",
                                    "Read Markdown or edit its source",
                                )
                                .clicked()
                            {
                                let view = if self.tab().is_some_and(|tab| {
                                    tab.agent_view == crate::cli::DocumentView::Preview
                                }) {
                                    crate::cli::DocumentView::Source
                                } else {
                                    crate::cli::DocumentView::Preview
                                };
                                self.set_document_view(view);
                                ui.close();
                            }
                            ui.separator();
                            self.split_menu_ui(ui);
                            ui.separator();
                            if ui
                                .checkbox(&mut self.settings.word_wrap, "Word wrap")
                                .on_hover_text("Wrap long lines - Alt+Z")
                                .changed()
                            {
                                self.session_dirty = true;
                            }
                            if ui
                                .checkbox(&mut self.settings.vim, "Vim motions")
                                .on_hover_text("Normal, Insert, Visual and line Visual modes")
                                .changed()
                            {
                                self.session_dirty = true;
                            }
                        });
                        design::menu(ui, "Run", "Build, test and run commands", |ui| {
                            if design::menu_item(
                                ui,
                                "Open terminal",
                                "",
                                "Open PowerShell in this project",
                            )
                            .clicked()
                            {
                                self.open_terminal();
                                ui.close();
                            }
                            ui.separator();
                            for (name, shortcut, command) in [
                                ("Check", "F5", "cargo check"),
                                ("Test", "Alt+F6", "cargo test"),
                                ("Build", "", "cargo build"),
                                ("Clippy", "", "cargo clippy"),
                                ("Run", "", "cargo run"),
                            ] {
                                if design::menu_item(ui, name, shortcut, command).clicked() {
                                    self.run_task(command.into());
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if design::menu_item(
                                ui,
                                "Custom command",
                                "",
                                "Run a command and view its output",
                            )
                            .clicked()
                            {
                                self.output_visible = true;
                                self.problems_visible = false;
                                ui.close();
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let search = ui.add_sized(
                                [228.0, 28.0],
                                egui::Button::new(
                                    RichText::new("Go to file").size(12.0).color(MUTED),
                                )
                                .shortcut_text(RichText::new("Ctrl P").size(10.5).color(MUTED))
                                .frame_when_inactive(false)
                                .corner_radius(5.0),
                            );
                            if search
                                .on_hover_text("Find a file - Ctrl+P. Commands - Ctrl+Shift+P.")
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.open_palette(Palette::Files);
                            }
                        });
                    });
            });
    }
    fn switch_project(&mut self, index: usize) {
        if index < self.projects.len() {
            self.set_active_project_context(index);
            self.bind_current_pane();
            self.session_dirty = true;
        }
    }
    fn activity(&mut self, ui: &mut egui::Ui) {
        if self.projects.is_empty() {
            return;
        }
        egui::Panel::left("activity")
            .exact_size(49.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(5, 8)),
            )
            .show(ui, |ui| {
                for (view, icon, hint) in [
                    (SidebarView::Explorer, Icon::Files, "Explorer · Ctrl+B"),
                    (
                        SidebarView::Search,
                        Icon::Search,
                        "Search project · Ctrl+Shift+F",
                    ),
                    (SidebarView::SourceControl, Icon::Branch, "Source control"),
                ] {
                    if icon_button(ui, icon, self.sidebar_visible && self.sidebar == view, hint)
                        .clicked()
                    {
                        if self.sidebar == view {
                            self.sidebar_visible = !self.sidebar_visible;
                            if view == SidebarView::Search && self.sidebar_visible {
                                self.open_project_search();
                            }
                        } else if view == SidebarView::Search {
                            self.open_project_search();
                        } else {
                            self.sidebar = view;
                            self.sidebar_visible = true;
                        }
                    }
                    ui.add_space(6.0);
                }
                if self.review.is_some() {
                    if icon_button(
                        ui,
                        Icon::Review,
                        self.sidebar_visible && self.sidebar == SidebarView::Review,
                        "Agent review - important places and notes",
                    )
                    .clicked()
                    {
                        self.sidebar_visible =
                            self.sidebar != SidebarView::Review || !self.sidebar_visible;
                        self.sidebar = SidebarView::Review;
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    if icon_button(ui, Icon::Console, self.output_visible, "Output · Ctrl+`")
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
    fn sidebar_ui(&mut self, ui: &mut egui::Ui) {
        if !self.sidebar_visible || self.projects.is_empty() {
            return;
        }
        egui::Panel::left("sidebar")
            .default_size(272.0)
            .size_range(200.0..=520.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(10, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(match self.sidebar {
                            SidebarView::Explorer => "EXPLORER",
                            SidebarView::Search => "SEARCH",
                            SidebarView::SourceControl => "SOURCE CONTROL",
                            SidebarView::Review => "REVIEW",
                        })
                        .size(10.5)
                        .strong()
                        .color(MUTED),
                    );
                    if self.sidebar == SidebarView::SourceControl {
                        git_panel::refresh_indicator(
                            ui,
                            self.project().is_some_and(|project| project.git_loading),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "↻", "Refresh project").clicked() {
                            self.index_project(self.active_project);
                            if self.sidebar == SidebarView::SourceControl {
                                self.refresh_git_history(self.active_project);
                            } else {
                                self.refresh_git(self.active_project, true);
                            }
                        }
                        if self.sidebar == SidebarView::Explorer
                            && small_button(ui, "+", "New file · Ctrl+N").clicked()
                        {
                            self.new_file();
                        }
                    });
                });
                ui.add_space(10.0);
                match self.sidebar {
                    SidebarView::Explorer => self.explorer(ui),
                    SidebarView::Search => self.project_search_ui(ui),
                    SidebarView::SourceControl => self.git_ui(ui),
                    SidebarView::Review => self.review_ui(ui),
                }
            });
    }
    fn explorer(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else { return };
        let root = p.root.clone();
        let index = p.index.clone();
        let loading = p.indexing;
        let error = p.index_error.clone();
        self.explorer_register_focus(ui, &root);
        self.explorer_project_heading(ui, &root);
        if loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Indexing files…").size(11.0).color(MUTED));
            });
        }
        if let Some(error) = error {
            ui.colored_label(ORANGE, error);
        }
        let Some(index) = index else {
            self.explorer_background_menu(ui, &root, ui.available_rect_before_wrap());
            return;
        };
        if index.truncated {
            ui.label(
                RichText::new("File limit reached. Narrow this workspace.")
                    .size(11.0)
                    .color(ORANGE),
            );
        }
        if !index.warnings.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} folders could not be read",
                    index.warnings.len()
                ))
                .size(10.5)
                .color(ORANGE),
            )
            .on_hover_text(index.warnings.join("\n"));
        }
        let active = self.tab().and_then(|t| t.path.clone());
        self.explorer_initialize(&index);
        let rows = self.explorer_rows(&index);
        let scroll_row = self.explorer_take_scroll_row(&root, &rows);
        egui::ScrollArea::vertical()
            .id_salt(("tree", &root))
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let row_height = 28.0;
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
                if index.entries.is_empty() {
                    ui.label(RichText::new("No files here yet.").color(MUTED));
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
                        self.tree(ui, &rows[first..last], &active);
                    });
                }
                // This rectangle starts below the last row, leaving file and
                // folder menus (and the ScrollArea's scrollbar) in control.
                let blank_top =
                    content_top + content_height.max(if rows.is_empty() { 28.0 } else { 0.0 });
                let blank = egui::Rect::from_min_max(
                    egui::pos2(ui.max_rect().left(), blank_top),
                    ui.max_rect().right_bottom(),
                );
                self.explorer_background_menu(ui, &root, blank);
            });
    }
    fn tree(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[explorer_navigation::VisibleRow<'_>],
        active: &Option<PathBuf>,
    ) {
        for row in rows {
            let entry = row.entry;
            ui.push_id(&entry.path, |ui| {
                let rect = egui::Rect::from_min_size(
                    ui.next_widget_position(),
                    Vec2::new(ui.available_width(), 28.0),
                );
                let row_rect = egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(row.depth as f32 * 14.0, 0.0),
                    rect.right_bottom(),
                );
                ui.allocate_rect(rect, egui::Sense::hover());
                let tooltip = crate::paths::display(&entry.path);
                ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
                    let selected = self.explorer_selected(&entry.path);
                    let active_file = !entry.is_dir && active.as_ref() == Some(&entry.path);
                    let badge = if entry.is_dir {
                        (
                            if self.explorer_expanded(&entry.path) {
                                "v"
                            } else {
                                ">"
                            },
                            MUTED,
                        )
                    } else {
                        (file_badge(&entry.path), file_color(Some(&entry.path)))
                    };
                    let response =
                        design::left_row(ui, &entry.name, Some(badge), selected, &tooltip);
                    if active_file {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                response.rect.left_top() + Vec2::new(1.0, 6.0),
                                Vec2::new(2.0, 16.0),
                            ),
                            1.0,
                            ACCENT,
                        );
                    }
                    self.explorer_row_interaction(&response, &entry.path, entry.is_dir);
                    self.file_context_menu(&response, &entry.path, entry.is_dir);
                    if response.clicked() && !entry.is_dir {
                        self.open_file(entry.path.clone());
                        self.focus_explorer();
                    }
                });
            });
        }
    }
    fn open_project_search(&mut self) {
        self.sidebar = SidebarView::Search;
        self.sidebar_visible = true;
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
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.project_query)
                .id(query_id)
                .hint_text("Find text in project…")
                .char_limit(16_384)
                .desired_width(f32::INFINITY),
        );
        if self.project_search_focus && !ui.is_sizing_pass() {
            response.request_focus();
            self.project_search_focus = false;
        }
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
                        response.on_hover_text(format!(
                            "{}:{}:{}",
                            crate::paths::display(&m.path),
                            m.line,
                            m.column
                        ));
                    }
                });
        } else if !self.search_running {
            ui.label(RichText::new("Search text in this project.\nEdit Exclusions to choose which folders and files to skip.").size(12.0).color(MUTED));
        }
        if let Some((path, line, column)) = clicked {
            self.reveal_search_result(path, line, column);
        }
    }
}

impl MorrowApp {
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").exact_size(27.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(12, 2)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (mode, color) = if self.screen_jump.is_some() { ("JUMP", ORANGE) } else if self.tab().is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview) { ("PREVIEW", BLUE) } else if self.settings.vim {
                        match self.tab().map(|t| t.vim.mode).unwrap_or(VimMode::Normal) {
                            VimMode::Normal => ("NORMAL", ACCENT), VimMode::Insert => ("INSERT", BLUE),
                            VimMode::Visual => ("VISUAL", ORANGE), VimMode::VisualLine => ("V-LINE", ORANGE),
                        }
                    } else { ("EDIT", BLUE) };
                    ui.label(RichText::new(format!(" {mode} ")).monospace().size(10.5).strong().color(color))
                        .on_hover_text("Vim: i to insert, v to select, Shift+V for lines, Esc for Normal");
                    if let Some(tab) = self.tab() {
                        let pending = tab.vim.pending_command();
                        if !pending.is_empty() { ui.label(RichText::new(pending).monospace().size(10.5).color(ORANGE)); }
                    }
                    if let Some(branch) = self.project().and_then(|p| p.git_branch.clone()) {
                        let tracking = self.project().and_then(|p| p.git_tracking.as_ref());
                        let hint = tracking.map(|b| format!("{} - {} ahead, {} behind. Open source control.", b.upstream.as_deref().unwrap_or("Local branch"), b.ahead, b.behind)).unwrap_or_else(|| "Open source control".into());
                        if design::ghost_button(ui, &branch, &hint).clicked() { self.sidebar = SidebarView::SourceControl; self.sidebar_visible = true; }
                    }
                    if let Some(jump) = &self.screen_jump {
                        let hint = if jump.state.query.is_empty() {
                            "Type 1–3 characters · Esc cancels".to_owned()
                        } else {
                            let digits = jump.state.targets.first().map_or(1, |target| target.label.len());
                            format!("{} · {} matches · type a {digits}-digit label · Esc cancels", jump.state.query, jump.state.targets.len())
                        };
                        ui.add(egui::Label::new(RichText::new(&hint).size(10.5).color(ORANGE)).truncate())
                            .on_hover_text("Type up to three characters to find visible matches. Digits choose a jump label. For numeric search text, press Enter before selecting a label. Backspace refines the query; Esc cancels.");
                    } else if let Some((message, error, time)) = &self.status
                        && (*error || time.elapsed() < Duration::from_secs(6)) {
                        let width = (ui.available_width() - 350.0).clamp(40.0, 440.0);
                        ui.add_sized([width, 20.0], egui::Label::new(RichText::new(message).size(10.5).color(if *error { RED } else { MUTED })).truncate()).on_hover_text(message);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(t) = self.tab() {
                            let (line, col) = t.lines.line_col(&t.text, t.revision, t.vim.cursor);
                            let total = t.lines.line_count(&t.text, t.revision);
                            ui.label(RichText::new(format!("Ln {line}/{total}, Col {col}")).size(10.5).color(MUTED)).on_hover_text(format!("Current line / total lines · Column {col}\nCtrl+G to go to a line"));
                            let selected = t.selection.end.saturating_sub(t.selection.start);
                            if selected > 0 { ui.label(RichText::new(format!("{selected} selected")).size(10.5).color(ORANGE)).on_hover_text("Selected characters"); }
                            let details = format!("UTF-8{} - {} - {}", if t.bom { " with BOM" } else { "" }, t.line_ending, if t.agent_unsaved { "Agent draft: save to resume auto save" } else if self.settings.autosave { "Auto save on" } else { "Manual save" });
                            ui.label(RichText::new(language_name(t.path.as_deref())).size(10.5).color(MUTED)).on_hover_text(details);
                        }
                        if let Some(project) = self.project() {
                            let problem_count = project.diagnostics.values().map(Vec::len).sum::<usize>();
                            if problem_count > 0 && design::ghost_button(ui, &format!("{problem_count} problems"), "Show errors and warnings - Ctrl+Shift+M").clicked() { self.problems_visible = true; self.output_visible = true; }
                        }
                        if let Some(project) = self.project() && project.language.is_some() {
                            let ready = project.language_ready;
                            let error = project.language_error.clone();
                            let (rect, response) = ui.allocate_exact_size(Vec2::new(16.0, 20.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 2.5, if error.is_some() { ORANGE } else if ready { ACCENT } else { MUTED });
                            response.on_hover_text(error.unwrap_or_else(|| if ready { "Rust analysis ready. Ctrl+Space: suggestions. gd / F12: definition.".into() } else { "Rust analysis is loading in the background".into() }));
                        }
                    });
                });
            });
    }
    fn output_ui(&mut self, ui: &mut egui::Ui) {
        if !self.output_visible {
            return;
        }
        egui::Panel::bottom("output")
            .default_size(240.0)
            .size_range(120.0..=600.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(14, 9)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let output_tab = design::ghost_button(ui, "Output", "Command output - Ctrl+`");
                    if !self.problems_visible {
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
                    }
                    let count = self
                        .project()
                        .map(|p| p.diagnostics.values().map(Vec::len).sum::<usize>())
                        .unwrap_or(0);
                    let problems_tab = design::ghost_button(
                        ui,
                        &format!("Problems {count}"),
                        "Rust errors and warnings - Ctrl+Shift+M",
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
                    ui.label(RichText::new(&self.task_status).size(11.0).color(MUTED));
                    if self.task.is_some() {
                        ui.spinner();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "×", "Hide output").clicked() {
                            self.output_visible = false;
                        }
                        if let Some(task) = &self.task {
                            if design::ghost_button(ui, "Stop", "Stop the running command")
                                .clicked()
                            {
                                task.cancel();
                            }
                        } else if design::ghost_button(ui, "Clear", "Clear command output")
                            .clicked()
                        {
                            self.output.clear();
                        }
                        if design::ghost_button(ui, "Copy", "Copy all command output").clicked() {
                            ui.ctx().copy_text(self.output.clone());
                        }
                    });
                });
                if self.problems_visible {
                    self.problems_ui(ui);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(">").monospace().color(ACCENT));
                    let response = ui.add_sized(
                        [ui.available_width() - 66.0, 28.0],
                        egui::TextEdit::singleline(&mut self.command)
                            .font(FontId::monospace(12.0))
                            .hint_text("Run a command in this project…"),
                    );
                    let enter = response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if ui
                        .add_enabled(
                            self.task.is_none() && !self.command.trim().is_empty(),
                            egui::Button::new("Run"),
                        )
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
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.output)
                                    .monospace()
                                    .size(12.0)
                                    .color(TEXT),
                            )
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
            });
    }
    fn editor_ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                if self.projects.is_empty() {
                    self.welcome(ui);
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
        let error = self.tab().and_then(|t| t.error.clone());
        if let Some(error) = error {
            egui::Frame::new()
                .fill(Color32::from_rgb(57, 34, 38))
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(error).size(12.0).color(RED));
                        if ui.button("Save as…").clicked()
                            && let Some(id) = self.tab().map(|tab| tab.id)
                        {
                            self.request_save_as(id);
                        }
                        if ui.button("Reload from disk").clicked() {
                            self.reload_current();
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
                if accent_button(ui, "Open a file   Ctrl P").clicked() {
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
                    }
                    self.review = None;
                    self.sidebar = SidebarView::Explorer;
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
                        let detail = annotation
                            .end
                            .as_ref()
                            .map(|end| format!("Lines {}-{}", annotation.start.line, end.line))
                            .unwrap_or_else(|| format!("Line {}", annotation.start.line));
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
                )
            })
            .collect();
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
        let controls_width = 44.0 + if markdown { 38.0 } else { 0.0 };
        let tab_width = (ui.available_width() - controls_width).max(20.0);
        ui.allocate_ui_with_layout(Vec2::new(tab_width, 37.0), Layout::left_to_right(Align::Center), |ui| {
        let tab_scroll = egui::ScrollArea::horizontal()
            .id_salt("document_tabs")
            .auto_shrink([false, true])
            .scroll_source(egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Never, ..Default::default() })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    for (id, title, dirty, loading, path, marks, saving, project_index, tab_index) in &tabs {
                        let selected = Some(*id) == active_id;
                        let font = FontId::proportional(12.0);
                        let title_galley = ui.painter().layout_no_wrap(
                            title.clone(),
                            font.clone(),
                            if selected { TEXT } else { MUTED },
                        );
                        let width = (title_galley.size().x + 65.0).clamp(125.0, 230.0);
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(width, 37.0), egui::Sense::click_and_drag());
                        tab_drop_rects.push((*id, rect));
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title)
                        });
                        if let Some(action) = context_actions::tab_menu(&response, path.is_some(), !loading && !saving) {
                            context_action = Some((*id, action));
                        }
                        if selected && reveal_active {
                            response.scroll_to_me(None);
                        }
                        let hovered = response.hovered();
                        if selected || hovered {
                            ui.painter()
                                .rect_filled(rect, 0.0, if hovered { SURFACE } else { BG });
                        }
                        if selected {
                            ui.painter().line_segment(
                                [rect.left_bottom(), rect.right_bottom()],
                                Stroke::new(2.0, ACCENT),
                            );
                        }
                        let badge = file_badge(path.as_deref().unwrap_or(Path::new("")));
                        ui.painter().text(
                            rect.left_center() + Vec2::new(13.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            badge,
                            FontId::monospace(10.5),
                            file_color(path.as_deref()),
                        );
                        let clip = egui::Rect::from_min_max(
                            rect.min + Vec2::new(36.0, 0.0),
                            rect.max - Vec2::new(29.0, 0.0),
                        );
                        ui.painter().with_clip_rect(clip).galley(
                            egui::pos2(
                                rect.left() + 36.0,
                                rect.center().y - title_galley.size().y * 0.5,
                            ),
                            title_galley,
                            TEXT,
                        );
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
                        if selected || hovered || close_response.hovered() {
                            if close_response.hovered() {
                                ui.painter().rect_filled(close_rect, 4.0, HOVER);
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
                                .on_hover_text("Close tab in this pane - Ctrl+W. Middle-click also closes.")
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                        } else if *dirty {
                            ui.painter().circle_filled(close_rect.center(), 2.5, ACCENT);
                        }
                        if *dirty && selected {
                            ui.painter().circle_filled(
                                egui::pos2(rect.right() - 31.0, rect.top() + 9.0),
                                2.0,
                                ACCENT,
                            );
                        }
                        if *marks > 0 {
                            ui.painter().circle_filled(
                                egui::pos2(rect.left() + 27.0, rect.bottom() - 8.0),
                                2.2,
                                BLUE,
                            );
                        }
                        if *loading || *saving {
                            ui.painter().circle_filled(
                                egui::pos2(rect.left() + 26.0, rect.center().y),
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
                        }
                        let mut tooltip = path.as_ref()
                            .map(|p| crate::paths::display(p))
                            .unwrap_or_else(|| "Untitled document".into());
                        if *dirty {
                            tooltip.push_str("\nUnsaved changes");
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
                    if small_button(ui, "+", "New file - Ctrl+N").clicked() {
                        self.new_file();
                    }
                });
            });
        self.register_tab_drop_strip(&tab_scroll, std::mem::take(&mut tab_drop_rects));
        });
        ui.allocate_ui_with_layout(Vec2::new(controls_width, 37.0), Layout::right_to_left(Align::Center), |ui| {
            if self.tab_pointer_interaction_active() { ui.disable(); }
            self.agent_marks_toolbar(ui);
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
        let mark_overlays = self.agent_mark_overlay_rows(document_id);
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
        if !settings.vim
            && pane_focused
            && source_interaction
            && ctx.memory(|memory| memory.has_focus(id))
        {
            let redo = ctx.input_mut(|input| {
                input.consume_key(
                    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
                    Key::Z,
                ) || input.consume_key(egui::Modifiers::COMMAND, Key::Y)
            });
            let undo =
                !redo && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::Z));
            if undo || redo {
                self.context_native_undo(document_id, redo);
            }
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
        if pane_focused && source_editable && ctx.memory(|m| m.has_focus(id)) {
            let mut edit = None;
            let indent = " ".repeat(settings.tab_width);
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Slash)) {
                edit = Some(editor::toggle_line_comment(
                    &tab.text,
                    tab.selection.clone(),
                    language_name(tab.path.as_deref()),
                ));
            } else if ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT),
                    Key::ArrowDown,
                )
            }) {
                edit = Some(editor::duplicate_lines(&tab.text, tab.selection.clone()));
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Tab)) {
                edit = Some(editor::indent_lines(
                    &tab.text,
                    tab.selection.clone(),
                    &indent,
                    true,
                ));
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab)) {
                edit = Some(if tab.selection.is_empty() {
                    editor::replace_selection(&tab.text, tab.selection.clone(), &indent)
                } else {
                    editor::indent_lines(&tab.text, tab.selection.clone(), &indent, false)
                });
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                edit = Some(editor::smart_newline(
                    &tab.text,
                    tab.selection.clone(),
                    &indent,
                ));
            }
            if let Some(edit) = edit {
                if !settings.vim && edit.text != tab.text {
                    context_actions::native_history_checkpoint(
                        &ctx,
                        document_id,
                        id,
                        &tab.text,
                        tab.selection.clone(),
                    );
                }
                tab.apply(edit);
                let range = tab.pending_selection.take().unwrap();
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
            let mut layouter =
                |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, available_width: f32| {
                    let width = if settings.word_wrap {
                        available_width.max(40.0)
                    } else {
                        f32::INFINITY
                    };
                    syntax.galley(
                        ui,
                        document_id,
                        revision,
                        buffer.as_str(),
                        &language,
                        font,
                        TEXT,
                        width,
                    )
                };
            let native_caret = ui.visuals().text_cursor.stroke;
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
                    left: if mark_overlays.is_empty() { 56 } else { 82 },
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
            context_action = context_actions::editor_menu(
                ui,
                &output.response,
                !tab.loading,
                !tab.selection.is_empty(),
            );
            let cursor = egui::text::CCursor::new(tab.vim.cursor);
            let local = output.galley.pos_from_cursor(cursor);
            let cursor_rect = local.translate(output.galley_pos.to_vec2());
            caret_rect = Some(cursor_rect);
            if tab.scroll_to_cursor {
                ui.scroll_to_rect(cursor_rect.expand(35.0), tab.center_on_reveal.then_some(Align::Center));
                tab.scroll_to_cursor = false;
                tab.center_on_reveal = false;
            }
            let clip = ui.clip_rect();
            let rows = &output.galley.rows;
            let first =
                rows.partition_point(|r| output.galley_pos.y + r.pos.y + r.size.y < clip.top());
            let last = rows.partition_point(|r| output.galley_pos.y + r.pos.y < clip.bottom());
            let visible_rows: Vec<_> = crate::render::visual_rows(&output.galley)
                .enumerate()
                .take(last)
                .skip(first)
                .collect();
            find::paint_matches(ui, &output.galley, output.galley_pos, &visible_rows, &search_matches, search_cursor);
            agent_mark_remove = agent_marks_ui::paint_agent_mark_overlays(
                ui, id, &output.galley, output.galley_pos, &visible_rows,
                if tab.revision == revision { &mark_overlays } else { &[] },
            );
            if pane_focused && let Some(feedback) = &tab.feedback {
                feedback.paint(ui, &output.galley, output.galley_pos, &visible_rows, tab.revision);
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
            if !settings.vim && (!focused || restored_native_selection) && !tab.selection.is_empty()
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
                            ui.visuals().selection.bg_fill.linear_multiply(0.35),
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
                            egui::pos2(output.galley_pos.x - 50.0, rect.top() + 2.0),
                            egui::pos2(output.galley_pos.x - 50.0, rect.bottom() - 2.0),
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
                        let color = if diagnostic.severity == 1 {
                            RED
                        } else {
                            ORANGE
                        };
                        if segment.top() == start_row.top() {
                            ui.painter().circle_filled(
                                egui::pos2(
                                    output.galley_pos.x - 41.0,
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
                            Stroke::new(1.0, color),
                        );
                        ui.interact(
                            segment.translate(output.galley_pos.to_vec2()),
                            ui.id().with(("diagnostic", tab.id, start, row_index)),
                            egui::Sense::hover(),
                        )
                        .on_hover_text(&diagnostic.message);
                    }
                }
            }
            for &(i, mapped) in &visible_rows {
                if mapped.first_in_line {
                    let row = &rows[i];
                    let y = output.galley_pos.y + row.pos.y;
                    for &(line, id, ref label) in &bookmark_marks {
                        if line != mapped.line { continue; }
                        let center = egui::pos2(output.galley_pos.x - 47.0, y + row.size.y * 0.5);
                        let rect = egui::Rect::from_center_size(center, egui::vec2(11.0, 17.0)).intersect(clip);
                        ui.painter().add(egui::Shape::convex_polygon(vec![
                            center + Vec2::new(-3.5, -5.0), center + Vec2::new(3.5, -5.0),
                            center + Vec2::new(3.5, 5.0), center + Vec2::new(0.0, 2.5), center + Vec2::new(-3.5, 5.0),
                        ], ORANGE, Stroke::NONE));
                        let response = ui.interact(rect, ui.id().with(("bookmark_gutter", id)), egui::Sense::click()).on_hover_text(format!("{label}\nClick to jump to this bookmark · Space m toggles a bookmark"));
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Bookmark at line {line}")));
                        if response.clicked() { bookmark_click = Some(id); }
                    }
                    let color = if mapped.line == current_line {
                        ACCENT
                    } else {
                        MUTED.linear_multiply(0.7)
                    };
                    let number = ui.painter().layout_no_wrap(
                        mapped.line.to_string(),
                        FontId::monospace((font - 2.0).max(10.0)),
                        color,
                    );
                    let source_baseline = row.glyphs.first().map(|glyph| glyph.pos.y)
                        .unwrap_or_else(|| {
                            // Use the same line box as source; natural font ascent
                            // alone loses the pixel-rounded leading on empty rows.
                            let mut job = egui::text::LayoutJob::simple("0".into(), FontId::monospace(font), color, f32::INFINITY);
                            job.sections[0].format.line_height = Some(font * 1.35);
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
                    output.response.changed(),
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
        if let Some(action) = context_action {
            self.apply_editor_context_action(document_id, action);
        }
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
            if case.changed() || word.changed() {
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
                let previous = small_button(ui, "↑", "Previous match · Shift+F3");
                previous.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        "Previous match",
                    )
                });
                let next = small_button(ui, "↓", "Next match · F3 or Enter");
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
                "Current match and total matches · F3 / Shift+F3 to navigate"
            });
        };
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                if !interactive { ui.disable(); }
                ui.spacing_mut().item_spacing.y = 5.0;
                let compact = ui.available_width() < 570.0;
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
                    let query_width = (available - if compact { 72.0 } else { 290.0 }).clamp(30.0, 360.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.find_text)
                            .id(find_id)
                            .hint_text("Find in this file")
                            .desired_width(query_width),
                    ).on_hover_text("Find literal text · Enter / F3: next · Shift+F3: previous · Esc: close. Vim Normal: / searches; Ctrl+F scrolls a page. Insert: Ctrl+F opens Find.");
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
                    if (response.has_focus() || response.lost_focus())
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
                    {
                        if !self.find_text.is_empty() { self.find_next(false); }
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
            let cursor = self.tab().map_or(0, |tab| tab.vim.cursor);
            if let Some(range) = matches
                .iter()
                .find(|range| range.start >= cursor)
                .or_else(|| matches.first())
                .cloned()
            {
                let replacement = self.replace_text.clone();
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
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(":").monospace().color(ACCENT));
            if let Some(command) = &mut self.vim_command {
                let response = ui.add(
                    egui::TextEdit::singleline(command)
                        .id_salt("ex-command")
                        .font(FontId::monospace(13.0))
                        .hint_text("w · q · wq · wa · e path · line number")
                        .desired_width(ui.available_width()),
                );
                response.request_focus();
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    run = Some(command.clone());
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                    cancel = true;
                }
            }
        });
        if cancel || run.is_some() {
            self.vim_command = None;
            if let Some(t) = self.tab_mut() {
                t.focus = true;
            }
        }
        if let Some(command) = run {
            let command = command.trim();
            match command {
                "w" => self.save_current(),
                "wa" => self.save_all(),
                "q" => {
                    if let Some(t) = self.tab() {
                        self.request_close(CloseAction::PaneTab {
                            pane_id: self.current_pane_id(),
                            id: t.id,
                        });
                    }
                }
                "q!" => {
                    if let Some(t) = self.tab() {
                        self.perform_close(CloseAction::PaneTab {
                            pane_id: self.current_pane_id(),
                            id: t.id,
                        });
                    }
                }
                "wq" | "x" => {
                    if let Some(t) = self.tab() {
                        self.close_action = Some(CloseAction::PaneTab {
                            pane_id: self.current_pane_id(),
                            id: t.id,
                        });
                        self.close_after_save = true;
                        self.save_current();
                    }
                }
                "noh" | "nohlsearch" => {
                    self.find_visible = false;
                    self.find.highlighted = false;
                }
                _ => {
                    if let Ok(line) = command.parse::<usize>() {
                        if let Some(t) = self.tab_mut() {
                            let cursor = editor::go_to_line(&t.text, line);
                            t.set_cursor(cursor..cursor);
                        }
                    } else if let Some(path) = command.strip_prefix("e ") {
                        if let Some(p) = self.project() {
                            self.open_file(p.root.join(path));
                        }
                    } else {
                        self.status(format!("Unknown command: {command}"), true);
                    }
                }
            }
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
                        18.0
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
                    ui.add_space(35.0 * space);
                    if accent_button(ui, "Open a project folder").clicked() {
                        self.pick_project();
                    }
                    ui.add_space(12.0 * space);
                    ui.label(
                        RichText::new("Ctrl + Shift + O")
                            .monospace()
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(46.0 * space);
                    egui::Frame::new()
                        .fill(PANEL)
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(10.0)
                        .inner_margin(if compact { 16 } else { 24 })
                        .show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(
                                RichText::new("BUILT AROUND YOUR FLOW")
                                    .size(10.5)
                                    .strong()
                                    .color(ACCENT),
                            );
                            ui.add_space(12.0);
                            for (label, keys) in [
                                ("Vim motions & visual selections", "v / V"),
                                ("Find a file", "Ctrl P"),
                                ("Search the whole project", "Ctrl Shift F"),
                                ("Commands at your fingertips", "Ctrl Shift P"),
                            ] {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(label).size(12.0).color(TEXT));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(keys).monospace().size(11.0).color(MUTED),
                                        );
                                    });
                                });
                                ui.add_space(6.0);
                            }
                        });
                    ui.add_space(25.0 * space);
                    ui.label(
                        RichText::new(format!("MORROW  /  {}", env!("CARGO_PKG_VERSION")))
                            .size(10.0)
                            .color(MUTED.linear_multiply(0.6)),
                    );
                });
            });
    }
}

const COMMANDS: &[(&str, &str)] = &[
    ("Switch project", "Ctrl Alt P"),
    ("Add project folder", "Ctrl Shift O"),
    ("Open file", "Ctrl O"),
    ("New file", "Ctrl N"),
    ("Save current file", "Ctrl S"),
    ("Save all files", "Ctrl Shift S"),
    ("Find in file", "Ctrl F in Insert"),
    ("Replace in file", "Ctrl H"),
    ("Search project", "Ctrl Shift F"),
    ("Go to line", "Ctrl G"),
    ("Go to definition", "gd / F12"),
    ("Show completions", "Ctrl Space"),
    ("Show problems", "Ctrl Shift M"),
    ("Mark selection for agent", "Ctrl Shift A"),
    ("Agent marks", "Ctrl Alt Shift A"),
    ("Markdown source / preview", "Ctrl Shift V"),
    ("Split right", "Ctrl \\"),
    ("Split down", "Ctrl Shift \\"),
    ("Focus Explorer", "Ctrl Shift E"),
    ("Focus editor", "Ctrl 1"),
    ("Next region", "F6"),
    ("Previous region", "Shift F6"),
    ("Focus pane left", "Ctrl Alt Left"),
    ("Focus pane right", "Ctrl Alt Right"),
    ("Focus pane above", "Ctrl Alt Up"),
    ("Focus pane below", "Ctrl Alt Down"),
    ("Close pane", "Ctrl Alt W"),
    ("Show agent review", ""),
    ("Show source control", ""),
    ("Show explorer", ""),
    ("Toggle output", "Ctrl `"),
    ("Check project", "F5"),
    ("Run tests", "Alt F6"),
    ("Build project", ""),
    ("Open terminal", ""),
    ("Toggle word wrap", "Alt Z"),
    ("Toggle Vim motions", ""),
    ("Toggle bookmark", "Leader m"),
    ("Bookmarks", "Leader b"),
    ("Jump to visible text", "Leader Leader f"),
    ("Reload file from disk", ""),
    ("Settings", "Ctrl ,"),
];
impl MorrowApp {
    fn command_action(&mut self, label: &str) {
        match label {
            "Switch project" => self.open_workspace_picker(),
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
            "Jump to visible text" => self.start_screen_jump(),
            "Go to definition" => self.request_definition(),
            "Show completions" => self.request_completion(),
            "Show problems" => {
                self.output_visible = true;
                self.problems_visible = true;
            }
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
            "Focus Explorer" => self.focus_explorer(),
            "Focus editor" => self.focus_editor(),
            "Next region" => self.cycle_focus_region(false),
            "Previous region" => self.cycle_focus_region(true),
            "Focus pane left" => self.split_command("focus_left"),
            "Focus pane right" => self.split_command("focus_right"),
            "Focus pane above" => self.split_command("focus_up"),
            "Focus pane below" => self.split_command("focus_down"),
            "Close pane" => self.split_command("close"),
            "Show agent review" => {
                self.sidebar = SidebarView::Review;
                self.sidebar_visible = true;
            }
            "Show source control" => {
                self.sidebar = SidebarView::SourceControl;
                self.sidebar_visible = true;
            }
            "Show explorer" => {
                self.sidebar = SidebarView::Explorer;
                self.sidebar_visible = true;
            }
            "Toggle output" => self.output_visible = !self.output_visible,
            "Check project" => self.run_task("cargo check".into()),
            "Run tests" => self.run_task("cargo test".into()),
            "Build project" => self.run_task("cargo build".into()),
            "Open terminal" => self.open_terminal(),
            "Toggle word wrap" => self.settings.word_wrap = !self.settings.word_wrap,
            "Toggle Vim motions" => {
                self.settings.vim = !self.settings.vim;
                self.session_dirty = true;
            }
            "Reload file from disk" => self.reload_current(),
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
                .backdrop_color(Color32::from_black_alpha(70))
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
                ui.horizontal(|ui| {
                ui.label(
                    RichText::new(match palette {
                        Palette::Files => "QUICK OPEN",
                        Palette::Commands => "COMMANDS",
                        Palette::Line => "GO TO LINE",
                    })
                    .size(10.5)
                    .strong()
                    .color(ACCENT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let response = design::ghost_button(ui, "×", "Close picker (Esc)");
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button,
                        ui.is_enabled(), "Close picker"));
                    close = response.clicked();
                });
                });
                ui.add_space(8.0);
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
                ui.add_space(9.0);
                ui.separator();
                if palette == Palette::Line {
                    if enter {
                        line = self.palette_query.trim().parse::<usize>().ok();
                    }
                    ui.label(
                        RichText::new("Enter to jump · Escape to close")
                            .size(11.0)
                            .color(MUTED),
                    );
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
                            .map(|(name, shortcut)| (name.to_string(), shortcut.to_string(), None))
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
                    let results_height = (ctx.content_rect().height() - 285.0).clamp(96.0, 340.0);
                    let page_size = (results_height / 48.0).floor().max(1.0) as usize;
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
                                                    if self.palette_query.trim().is_empty() {
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
                    ui.separator();
                    ui.label(
                        RichText::new("↑ ↓ to navigate    Page ↑ ↓ to scroll    Enter to open    Esc to close")
                            .size(10.5)
                            .color(MUTED),
                    );
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
                ui.set_width((ctx.content_rect().width() - 48.0).clamp(280.0, 420.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").size(20.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close = small_button(ui, "×", "Close settings · Esc");
                        close.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close settings"));
                        done |= close.clicked();
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .id_salt("settings_contents")
                    .max_height((ctx.content_rect().height() - 190.0).clamp(120.0, 540.0))
                    .show(ui, |ui| {
                    ui.label(
                        RichText::new("MAKE IT YOURS")
                            .size(10.5)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.add_space(12.0);
                    changed |= ui.checkbox(&mut self.settings.vim, "Vim motions (default)")
                        .on_hover_text("Use Normal, Insert, Visual and Visual Line modes. Changes apply immediately.")
                        .changed();
                    changed |= ui.checkbox(&mut self.settings.swap_jk, "Swap J and K")
                        .on_hover_text("J moves up and K moves down in Vim navigation, including selections and operators. Inserted text and literal search targets stay unchanged.")
                        .changed();
                    changed |= egui::ComboBox::from_label("Leader key")
                        .selected_text(if self.settings.leader_key == " " { "Space" } else { &self.settings.leader_key })
                        .show_ui(ui, |ui| {
                            let mut changed = ui.selectable_value(&mut self.settings.leader_key, " ".into(), "Space (default)").changed();
                            changed |= ui.selectable_value(&mut self.settings.leader_key, ",".into(), "Comma").changed();
                            changed |= ui.selectable_value(&mut self.settings.leader_key, "\\".into(), "Backslash").changed();
                            changed
                        }).inner.unwrap_or(false);
                    ui.label(RichText::new("Leader m: bookmark · Leader b: bookmarks · Leader Leader f: jump").size(10.5).color(MUTED));
                    ui.label(
                        RichText::new(
                            "v selects characters · V selects lines · Esc returns to Normal",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                    ui.add_space(10.0);
                    changed |= ui.checkbox(
                        &mut self.settings.autosave,
                        "Save files automatically after a pause",
                    ).on_hover_text("Save edits after you stop typing. Agent drafts remain manual until explicitly saved.").changed();
                    ui.label(
                        RichText::new("Recovery copies are kept even with auto save off.")
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(10.0);
                    changed |= ui.checkbox(&mut self.settings.word_wrap, "Wrap long lines").on_hover_text("Fit long source lines to the editor pane · Alt+Z").changed();
                    changed |= ui.checkbox(&mut self.settings.smooth_scroll, "Smooth scrolling")
                        .on_hover_text("Animate source and Markdown scrolling; turn off for immediate movement.").changed();
                    ui.add_space(10.0);
                    let font_choice = egui::ComboBox::from_label("Editor font")
                        .selected_text(self.settings.editor_font.label())
                        .show_ui(ui, |ui| {
                            let mut changed = false;
                            for font in [crate::cli::EditorFont::JetBrainsMono, crate::cli::EditorFont::Consolas, crate::cli::EditorFont::CascadiaMono] {
                                changed |= ui.selectable_value(&mut self.settings.editor_font, font, font.label()).changed();
                            }
                            changed
                        });
                    changed |= font_choice.inner.unwrap_or(false);
                    font_choice.response.on_hover_text("Regular code fonts. JetBrains Mono is included; unavailable Windows fonts use it as a fallback. Changes apply immediately.");
                    changed |= ui.add(
                        egui::Slider::new(&mut self.settings.font_size, 10.0..=24.0)
                            .text("Editor font size"),
                    ).changed();
                    changed |= ui.add(
                        egui::Slider::new(&mut self.settings.tab_width, 1..=8).text("Indent width"),
                    ).changed();
                    ui.add_space(16.0);
                    ui.separator();
                    ui.label(
                        RichText::new("VIM REFERENCE")
                            .size(10.0)
                            .strong()
                            .color(MUTED),
                    );
                    ui.add_space(8.0);
                    for text in [
                        "h j k l · w b e · 0 ^ $ · gg G · %",
                        "i a I A · o O · r · x · dd · cw · cc · D C",
                        "v V · y d c p · > < · u · Ctrl R",
                        "Counts: 3w · 2dd · d2w     Find: f F t T ; ,",
                        "Search: / ? · n N · * #     Pages: Ctrl F B D U",
                        "Commands: :w :wa :q :wq :e path :42",
                    ] {
                        ui.label(RichText::new(text).monospace().size(11.5).color(TEXT));
                    }
                    ui.add_space(4.0);
                    });
                ui.add_space(12.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Changes apply immediately · Esc closes").size(10.5).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        done |= design::primary_button(ui, "Done", Vec2::new(70.0, 28.0)).clicked();
                    });
                });
            });
            self.settings_open = !(done || modal.should_close());
            self.session_dirty |= changed;
            self.apply_editor_font();
            if !self.settings_open {
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
            let modal = crate::git_diff_view::dialog(
                &ctx,
                egui::Id::new("git_patch_window"),
                title,
                "Close diff",
                Vec2::new(950.0, 620.0),
                true,
                |ui| {
                    if self.diff_loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading changes…");
                        });
                    } else if let Some(error) = &self.diff_error {
                        ui.colored_label(RED, error);
                    } else {
                        self.diff_view.show(ui, title, text);
                    }
                },
            );
            if modal.inner || modal.should_close() {
                self.close_diff();
            }
        }
    }
    fn close_diff(&mut self) {
        self.diff = None;
        self.diff_loading = false;
        self.diff_error = None;
        self.diff_view.reset_focus();
        self.restore_git_overlay_focus();
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
        let restore_after_menu = egui::Popup::is_any_open(&self.ctx)
            && self.ctx.input(|input| input.key_pressed(Key::Escape));
        self.poll();
        self.sync_agent_marks();
        self.poll_control();
        self.apply_editor_font();
        self.syntax.begin_frame(ui);
        self.apply_pending_bookmark_jump();
        self.apply_pending_agent_locations();
        self.poll_languages();
        self.resume_region_input();
        self.focus_pane_from_pointer();
        self.context_paste_poll();
        self.validate_vim_search();
        let dragging_tab = self.tab_drag_keys();
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
        self.refresh_git(self.active_project, false);
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
            && let Some(tab) = self.tab_mut()
        {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        self.editor_ui(ui);
        self.finish_workspace_action();
        self.finish_region_focus();
        self.overlays();
        self.agent_marks_dialog();
        self.sync_agent_marks();
        self.navigation_ui();
        self.completion_ui();
        self.finish_agent_mark_input_owner();
        self.autosave();
        self.sync_languages(false);
        self.watch_disk();
        if !self.projects.is_empty() {
            self.ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
}
impl eframe::App for MorrowApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.ctx.input(|i| i.viewport().close_requested()) && !self.exit_ready {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(CloseAction::Exit);
        }
        self.draw_ui(ui);
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
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Morrow")
        .join("recovery")
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
    match extension(Some(path)) {
        "rs" => "rs",
        "toml" => "≡",
        "json" => "{}",
        "md" => "MD",
        "js" | "jsx" => "JS",
        "ts" | "tsx" => "TS",
        "py" => "Py",
        "html" => "<>",
        "css" => "#",
        _ => "·",
    }
}
fn file_color(path: Option<&Path>) -> Color32 {
    match extension(path) {
        "rs" => ORANGE,
        "md" | "ts" | "tsx" => BLUE,
        "json" | "js" | "jsx" => Color32::from_rgb(208, 195, 137),
        "toml" | "yaml" | "yml" => MUTED,
        "css" | "html" => Color32::from_rgb(171, 158, 216),
        _ => MUTED,
    }
}
fn language_name(path: Option<&Path>) -> &'static str {
    match extension(path) {
        "rs" => "Rust",
        "toml" => "TOML",
        "json" => "JSON",
        "md" | "markdown" | "mdown" => "Markdown",
        "js" | "jsx" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "py" => "Python",
        "html" => "HTML",
        "css" => "CSS",
        "c" | "h" => "C",
        "cpp" | "hpp" => "C++",
        "go" => "Go",
        "yaml" | "yml" => "YAML",
        "sh" => "Shell",
        _ => "Plain text",
    }
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
    match extension(path) {
        "tsx" | "ts" => "js",
        "jsx" => "js",
        "yml" => "yaml",
        "h" | "hpp" => "cpp",
        "" => "txt",
        ext => ext,
    }
}
fn set_style(ctx: &egui::Context) {
    ctx.set_fonts(crate::typography::fonts(crate::cli::EditorFont::default()));
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = SURFACE;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.inactive.weak_bg_fill = SURFACE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = HOVER;
    style.visuals.widgets.hovered.weak_bg_fill = HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(62, 78, 87));
    style.visuals.widgets.active.bg_fill = HOVER;
    style.visuals.widgets.active.weak_bg_fill = HOVER;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.window_corner_radius = egui::CornerRadius::same(8);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(6);
    style.visuals.selection.bg_fill = Color32::from_rgb(54, 76, 78);
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.spacing.item_spacing = Vec2::new(7.0, 5.0);
    style.spacing.button_padding = Vec2::new(9.0, 5.0);
    style.spacing.interact_size.y = 28.0;
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
    Files,
    Search,
    Branch,
    Console,
    Settings,
    Review,
}
fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(35.0), egui::Sense::click());
    let c = rect.center();
    let painter = ui.painter();
    let color = if selected {
        ACCENT
    } else if response.hovered() {
        TEXT
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.5, color);
    if selected || response.hovered() {
        painter.rect_filled(rect, 5.0, if selected { SURFACE } else { HOVER });
    }
    let line = |a: Vec2, b: Vec2| {
        painter.line_segment([c + a, c + b], stroke);
    };
    match icon {
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
#[path = "app_tests.rs"]
mod tests;

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
#[path = "focus_feedback_tests.rs"]
mod focus_feedback_tests;

#[cfg(test)]
#[path = "selection_scroll_tests.rs"]
mod selection_scroll_tests;

#[cfg(test)]
#[path = "typography_ui_tests.rs"]
mod typography_ui_tests;

impl MorrowApp {
    fn sync_languages(&mut self, force: bool) {
        for p in &mut self.projects {
            if p.language.is_none() && p.rust_project {
                p.language = Some(crate::lsp::LanguageServer::start(
                    p.root.clone(),
                    Some(self.ctx.clone()),
                ));
            }
            if !p.language_ready {
                continue;
            }
            let Some(server) = &p.language else { continue };
            let closed: Vec<_> = p
                .language_opened
                .keys()
                .filter(|path| !p.tabs.iter().any(|t| t.path.as_ref() == Some(path)))
                .cloned()
                .collect();
            for path in closed {
                if server.close(&path) {
                    p.language_opened.remove(&path);
                }
            }
            for t in &p.tabs {
                if t.loading || extension(t.path.as_deref()) != "rs" {
                    continue;
                }
                let Some(path) = &t.path else { continue };
                if let Some((revision, version)) = p.language_opened.get_mut(path) {
                    if *revision != t.revision
                        && (force || t.last_edit.elapsed() > Duration::from_millis(160))
                        && server.change(path, &t.text, version.saturating_add(1))
                    {
                        *revision = t.revision;
                        *version = version.saturating_add(1);
                    }
                } else if server.open(path, &t.text) {
                    p.language_opened.insert(path.clone(), (t.revision, 1));
                }
            }
        }
    }
    fn poll_languages(&mut self) {
        let mut events = Vec::new();
        for (i, p) in self.projects.iter().enumerate() {
            if let Some(server) = &p.language {
                while let Ok(event) = server.receiver.try_recv() {
                    events.push((i, event));
                }
            }
        }
        for (i, event) in events {
            match event {
                crate::lsp::LspEvent::Ready => {
                    self.projects[i].language_ready = true;
                    self.projects[i].language_error = None;
                }
                crate::lsp::LspEvent::Error(error) => {
                    self.projects[i].language_error = Some(error.clone());
                    if self.projects[i]
                        .language
                        .as_ref()
                        .is_some_and(|s| s.is_stopped())
                    {
                        self.projects[i].language_ready = false;
                    }
                    self.status(error, true);
                }
                crate::lsp::LspEvent::Diagnostics {
                    path,
                    version,
                    diagnostics,
                } => {
                    if version.is_none_or(|v| {
                        self.projects[i]
                            .language_opened
                            .get(&path)
                            .is_none_or(|(_, current)| v >= *current)
                    }) {
                        self.projects[i].diagnostics.insert(path, diagnostics);
                    }
                }
                crate::lsp::LspEvent::Completions {
                    request_id,
                    path,
                    version,
                    position,
                    items,
                } => {
                    if self.completion_request == Some(request_id)
                        && self.active_project == i
                        && let Some(t) = self.tab()
                        && t.path.as_ref() == Some(&path)
                        && self.projects[i]
                            .language_opened
                            .get(&path)
                            .is_some_and(|(rev, v)| *rev == t.revision && *v == version)
                        && crate::lsp::char_to_position(&t.text, t.vim.cursor) == position
                        && !items.is_empty()
                    {
                        self.completions = Some(CompletionPopup {
                            tab_id: t.id,
                            version,
                            cursor: t.vim.cursor,
                            items,
                            selected: 0,
                        });
                    }
                }
                crate::lsp::LspEvent::Definition {
                    request_id,
                    path,
                    line,
                    column,
                } => {
                    if self.definition_request.is_some_and(|(request, tab)| {
                        request == request_id && self.tab().is_some_and(|t| t.id == tab)
                    }) && self.active_project == i
                    {
                        self.open_file(path);
                        if let Some(t) = self.tab() {
                            self.pending_location = Some((
                                t.id,
                                crate::lsp::Position {
                                    line,
                                    character: column,
                                },
                            ));
                        }
                        self.definition_request = None;
                    }
                }
                crate::lsp::LspEvent::DefinitionNotFound { request_id } => {
                    if self
                        .definition_request
                        .is_some_and(|(request, _)| request == request_id)
                    {
                        self.definition_request = None;
                        self.status("No definition found here", false);
                    }
                }
            }
        }
        if let Some((id, position)) = self.pending_location
            && let Some(t) = self.tab_by_id_mut(id)
            && !t.loading
        {
            let cursor = crate::lsp::position_to_char(&t.text, position);
            t.set_cursor(cursor..cursor);
            self.pending_location = None;
        }
        if self.completions.as_ref().is_some_and(|popup| {
            self.tab().is_none_or(|t| {
                t.id != popup.tab_id
                    || t.vim.cursor != popup.cursor
                    || self
                        .project()
                        .and_then(|p| t.path.as_ref().and_then(|path| p.language_opened.get(path)))
                        .is_none_or(|(rev, v)| *rev != t.revision || *v != popup.version)
            })
        }) {
            self.completions = None;
        }
    }
    fn request_definition(&mut self) {
        self.sync_languages(true);
        let Some(p) = self.project() else { return };
        let Some(t) = self.tab() else { return };
        let Some(path) = &t.path else { return };
        if let Some(server) = &p.language
            && p.language_ready
        {
            let id = server.definition(path, crate::lsp::char_to_position(&t.text, t.vim.cursor));
            if id != 0 {
                self.definition_request = Some((id, t.id));
                self.status("Finding definition…", false);
            }
            return;
        }
        self.status(
            "Rust analysis is still loading, or this file has no language server",
            false,
        );
    }
    fn request_completion(&mut self) {
        self.sync_languages(true);
        let Some(p) = self.project() else { return };
        let Some(t) = self.tab() else { return };
        let Some(path) = &t.path else { return };
        if let Some(server) = &p.language
            && p.language_ready
        {
            let version = p.language_opened.get(path).map(|(_, v)| *v).unwrap_or(1);
            let id = server.completion(
                path,
                crate::lsp::char_to_position(&t.text, t.vim.cursor),
                version,
            );
            if id != 0 {
                self.completion_request = Some(id);
            }
            return;
        }
        self.status(
            "Rust analysis is still loading, or this file has no language server",
            false,
        );
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
                    .and_then(|p| t.path.as_ref().and_then(|path| p.language_opened.get(path)))
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
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.completions = None;
            if self.settings.vim {
                return false;
            }
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            return true;
        }
        let down = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
        let up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
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
        }
        if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, Key::Enter)
                || i.consume_key(egui::Modifiers::NONE, Key::Tab)
        }) {
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
        let row_height = 48.0;
        let list_height = (p.items.len().min(6) as f32 * row_height)
            .min((bounds.height() - 180.0).max(row_height));
        let popup_height = list_height + 65.0;
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
                ui.label(RichText::new("SUGGESTIONS").size(10.0).color(ACCENT));
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
                ui.add_space(7.0);
                ui.label(
                    RichText::new(format!(
                        "{} of {} · ↑ ↓ choose · Enter / Tab accepts · Esc closes",
                        selection + 1,
                        p.items.len()
                    ))
                    .size(10.5)
                    .color(MUTED),
                );
            });
        if let Some(i) = selected {
            self.apply_completion(i);
        }
    }
    fn problems_ui(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else { return };
        let root = p.root.clone();
        let rows: Vec<_> = p
            .diagnostics
            .iter()
            .flat_map(|(path, list)| list.iter().map(move |d| (path.clone(), d.clone())))
            .take(2000)
            .collect();
        let error = p.language_error.clone();
        let active = p.language_ready;
        let loading = p.language.is_some();
        let mut jump = None;
        if let Some(error) = error {
            ui.colored_label(ORANGE, error);
        } else if loading && !active {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Rust analysis is loading…");
            });
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("problems")
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(
                        RichText::new(if active {
                            "No problems reported"
                        } else {
                            "Rust projects use rust-analyzer for diagnostics and code suggestions."
                        })
                        .size(12.0)
                        .color(MUTED),
                    );
                }
                for (path, d) in rows {
                    let label = format!(
                        "{}  {}:{}:{}   {}",
                        if d.severity == 1 { "Error" } else { "Warning" },
                        crate::paths::display(path.strip_prefix(&root).unwrap_or(&path)),
                        d.range.start.line + 1,
                        d.range.start.character + 1,
                        d.message.lines().next().unwrap_or("")
                    );
                    let response = ui.add(
                        egui::Label::new(
                            RichText::new(label).size(12.0).color(if d.severity == 1 {
                                RED
                            } else {
                                ORANGE
                            }),
                        )
                        .sense(egui::Sense::click()),
                    );
                    let tooltip = format!("{}\n{}", crate::paths::display(&path), d.message);
                    if response.clicked() {
                        jump = Some((path, d.range.start));
                    }
                    response.on_hover_text(tooltip);
                }
            });
        if let Some((path, position)) = jump {
            self.open_file(path);
            if let Some(t) = self.tab() {
                self.pending_location = Some((t.id, position));
            }
        }
    }
    fn watch_disk(&mut self) {
        if self.disk_check_running || self.last_disk_check.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_disk_check = Instant::now();
        let documents: Vec<_> = self
            .projects
            .iter()
            .flat_map(|p| &p.tabs)
            .filter(|t| !t.loading && !t.saving)
            .filter_map(|t| Some((t.id, t.path.clone()?, t.fingerprint.clone()?)))
            .collect();
        if documents.is_empty() {
            return;
        }
        self.disk_check_running = true;
        self.worker(move || {
            let mut changes = Vec::new();
            for (id, path, baseline) in documents {
                match fs::metadata(&path) {
                    Ok(meta)
                        if meta.len() == baseline.len
                            && meta.modified().ok() == baseline.modified => {}
                    _ => {
                        changes.push((id, baseline, workspace::read_document(&path, MAX_DOCUMENT)))
                    }
                }
            }
            WorkerResult::DiskChanges(changes)
        });
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
