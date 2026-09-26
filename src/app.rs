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

#[path = "context_actions.rs"]
mod context_actions;
#[path = "control.rs"]
mod control;
#[path = "split.rs"]
mod split;

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
    autosave: bool,
    font_size: f32,
    tab_width: usize,
    word_wrap: bool,
    smooth_scroll: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            vim: true,
            autosave: true,
            font_size: 14.0,
            tab_width: 4,
            word_wrap: false,
            smooth_scroll: false,
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
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SessionProject {
    root: PathBuf,
    open_files: Vec<PathBuf>,
    active_file: Option<PathBuf>,
    tabs: Vec<SessionTab>,
    active_tab_index: Option<usize>,
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
    recent_commits: Vec<git::CommitSummary>,
    git_error: Option<String>,
    git_loading: bool,
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
            recent_commits: Vec::new(),
            git_error: None,
            git_loading: false,
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
    commits: Vec<git::CommitSummary>,
}
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
    GitAction {
        root: PathBuf,
        message: String,
        result: Result<String, String>,
    },
    Search {
        root: PathBuf,
        generation: u64,
        result: Result<workspace::SearchResults, String>,
    },
    ContextFile(context_actions::FileActionResult),
    ContextFilePrepared(context_actions::FilePreparationResult),
    NativeDialog {
        request: Picker,
        result: Result<Option<DialogSelection>, String>,
    },
    Diff {
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
    sidebar: SidebarView,
    sidebar_visible: bool,
    settings: Settings,
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
    project_query: String,
    project_search: Option<workspace::SearchResults>,
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
    save_as: Option<u64>,
    save_queue: std::collections::VecDeque<u64>,
    close_action: Option<CloseAction>,
    close_after_save: bool,
    exit_ready: bool,
    git_busy: Option<PathBuf>,
    diff: Option<(String, String)>,
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
        if let Some(autosave) = session.autosave {
            app.settings.autosave = autosave;
        }
        app.sidebar = session.sidebar.unwrap_or_default();
        for entry in session.projects {
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
                app.load_tab(tab.id, saved.path, saved.recovery_path);
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
            app.projects.push(project);
        }
        app.active_project = session
            .active_project
            .min(app.projects.len().saturating_sub(1));
        app.restore_editor_layout(session.layout);
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
            sidebar: SidebarView::Explorer,
            sidebar_visible: true,
            settings: Settings::default(),
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
            project_query: String::new(),
            project_search: None,
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
            save_as: None,
            save_queue: std::collections::VecDeque::new(),
            close_action: None,
            close_after_save: false,
            exit_ready: false,
            git_busy: None,
            diff: None,
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
        let p = self.project()?;
        p.tabs.get(p.active_tab)
    }
    fn tab_mut(&mut self) -> Option<&mut EditorTab> {
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
        if self.active_project != project_index {
            self.project_search = None;
            self.search_cancel.store(true, Ordering::Relaxed);
            self.search_generation += 1;
            self.search_running = false;
            self.search_dirty = !self.project_query.is_empty();
        }
        self.active_project = project_index;
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
                    let p = self.projects.remove(index);
                    p.index_cancel.store(true, Ordering::Relaxed);
                    for t in p.tabs {
                        let _ = self.recovery_writer.remove(&t.recovery_path, t.revision);
                    }
                    if self.active_project > index {
                        self.active_project -= 1;
                    }
                    self.active_project = self
                        .active_project
                        .min(self.projects.len().saturating_sub(1));
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
                    if let Some(p) = self.projects.iter_mut().find(|p| p.root == root) {
                        p.git_loading = false;
                        p.last_git_refresh = Instant::now();
                        match result {
                            Ok(s) => {
                                p.git_root = s.root;
                                p.git_branch = s.branch;
                                p.git_tracking = s.tracking;
                                p.git_changes = s.files;
                                p.recent_commits = s.commits;
                                p.git_error = None;
                            }
                            Err(e) => p.git_error = Some(e),
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
                            if let Some(i) = self
                                .projects
                                .iter()
                                .position(|p| p.git_root.as_ref() == Some(&root) || p.root == root)
                            {
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
                    result,
                } => {
                    if self.project().is_some_and(|p| p.root == root)
                        && generation == self.search_generation
                    {
                        self.search_running = false;
                        match result {
                            Ok(results) => self.project_search = Some(results),
                            Err(e) => self.status(e, true),
                        }
                    }
                }
                WorkerResult::NativeDialog { request, result } => {
                    self.finish_native_dialog(request, result);
                }
                WorkerResult::Diff { title, result } => match result {
                    Ok(text) => self.diff = Some((title, text)),
                    Err(e) => self.status(e, true),
                },
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
        let Some(p) = self.projects.get_mut(index) else {
            return;
        };
        if p.git_loading
            || self.git_busy.is_some()
            || (!force && p.last_git_refresh.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        p.git_loading = true;
        let root = p.root.clone();
        self.worker(move || {
            let result = (|| {
                let repo = git::repository_root(&root).map_err(|e| e.to_string())?;
                if let Some(repo) = repo {
                    let status = git::status(&repo).map_err(|e| e.to_string())?;
                    let tracking = git::branch_info(&repo).ok();
                    let commits = git::recent_log(&repo, 8).unwrap_or_default();
                    Ok(GitSnapshot {
                        root: Some(repo),
                        branch: status.branch,
                        tracking,
                        files: status.files,
                        commits,
                    })
                } else {
                    Ok(GitSnapshot {
                        root: None,
                        branch: None,
                        tracking: None,
                        files: Vec::new(),
                        commits: Vec::new(),
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
    fn search_project(&mut self) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        self.search_generation += 1;
        self.search_dirty = false;
        if self.project_query.trim().is_empty() {
            self.project_search = None;
            self.search_running = false;
            return;
        }
        let Some(p) = self.project() else { return };
        let Some(index) = p.index.clone() else {
            self.search_dirty = true;
            return;
        };
        let root = p.root.clone();
        let query = self.project_query.clone();
        let generation = self.search_generation;
        let cancel = self.search_cancel.clone();
        self.search_running = true;
        self.worker(move || WorkerResult::Search {
            root,
            generation,
            result: workspace::search_project(
                &index,
                &query,
                workspace::SearchOptions {
                    cancel: Some(cancel),
                    ..Default::default()
                },
            ),
        });
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
                })
                .collect(),
            active_project: self.active_project,
            sidebar: Some(self.sidebar),
            autosave: Some(self.settings.autosave),
            settings: Some(self.settings.clone()),
            layout: Some(self.editor_session_layout()),
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
        self.completions = None;
        self.palette = Some(palette);
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_focus = true;
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
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
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
            self.sidebar = SidebarView::Search;
            self.sidebar_visible = true;
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
        if !self.settings.vim
            || egui::Popup::is_any_open(&self.ctx)
            || self
                .tab()
                .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.vim_command.is_some()
            || self.diff.is_some()
            || self.context_file_dialog.is_some()
        {
            return;
        }
        let ctx = self.ctx.clone();
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
            clipboard: &mut Option<Yank>,
        ) {
            if action.text_changed {
                tab.changed();
            }
            if action.yanked.is_some() {
                *clipboard = action.yanked;
            }
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            tab.selection = selection.clone();
            tab.pending_selection = Some(selection);
            tab.scroll_to_cursor = true;
        }

        let events = ctx.input(|input| input.events.clone());
        let mut retained = Vec::with_capacity(events.len());
        let mut native_insert = Vec::new();
        let mut prompt: Option<char> = None;
        let mut prompt_text = String::new();
        let mut search_direction = None;
        let mut search_word = None;
        let mut definition = false;
        let mut clipboard = None;
        let register = self.register.clone();
        let width = self.settings.tab_width.clamp(1, 16);
        let page = self.editor_rect.map_or(24, |rect| {
            (rect.height() / (self.settings.font_size + 5.0))
                .floor()
                .max(1.0) as isize
        });
        if let Some(tab) = self.tab_mut() {
            tab.vim.set_indent_width(width);
            if let Some(register) = register {
                tab.vim.set_register(register);
            }
            for event in events {
                if prompt.is_some() {
                    match event {
                        egui::Event::Text(text) => prompt_text.push_str(&text),
                        egui::Event::Paste(text) => prompt_text.push_str(&text),
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
                        apply_action(tab, action, &mut clipboard);
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
                            if tab.vim.mode == VimMode::Insert {
                                let remaining: String =
                                    std::iter::once(ch).chain(characters).collect();
                                native_insert.push(egui::Event::Text(remaining));
                                break;
                            }
                            if !tab.vim.expects_literal_character() {
                                if matches!(ch, ':' | '/' | '?') {
                                    tab.vim.cancel_pending_command();
                                    prompt = Some(ch);
                                    prompt_text.extend(characters);
                                    break;
                                }
                                if matches!(ch, 'n' | 'N') {
                                    tab.vim.cancel_pending_command();
                                    search_direction = Some(ch == 'N');
                                    continue;
                                }
                                if matches!(ch, '*' | '#') {
                                    tab.vim.cancel_pending_command();
                                    let chars: Vec<char> = tab.text.chars().collect();
                                    let mut start = tab.vim.cursor.min(chars.len());
                                    let mut end = start;
                                    while start > 0
                                        && (chars[start - 1].is_alphanumeric()
                                            || chars[start - 1] == '_')
                                    {
                                        start -= 1;
                                    }
                                    while end < chars.len()
                                        && (chars[end].is_alphanumeric() || chars[end] == '_')
                                    {
                                        end += 1;
                                    }
                                    if start < end {
                                        search_word = Some((
                                            chars[start..end].iter().collect::<String>(),
                                            ch == '#',
                                        ));
                                    }
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
                            let action = tab.vim.handle_key(&mut tab.text, VimKey::Char(ch));
                            apply_action(tab, action, &mut clipboard);
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
                            apply_action(tab, action, &mut clipboard);
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
                            apply_action(tab, action, &mut clipboard);
                        }
                    }
                    egui::Event::Paste(_) => {}
                    other => retained.push(other),
                }
            }
        }
        retained.extend(native_insert);
        ctx.input_mut(|input| input.events = retained);
        if let Some(yank) = clipboard {
            ctx.copy_text(yank.text.clone());
            self.register = Some(yank);
        }
        match prompt {
            Some(':') => self.vim_command = Some(prompt_text),
            Some('/' | '?') => {
                self.find_text = prompt_text;
                self.find_match = None;
                self.find_visible = true;
                self.find_focus = true;
            }
            _ => {}
        }
        if let Some((word, backwards)) = search_word {
            self.find_text = word;
            self.find_word = true;
            self.find_match = None;
            self.find_next(backwards);
        }
        if let Some(backwards) = search_direction {
            self.find_next(backwards);
        }
        if definition {
            self.request_definition();
        }
    }

    fn find_options(&self) -> editor::SearchOptions {
        editor::SearchOptions {
            case_sensitive: self.find_case,
            whole_word: self.find_word,
        }
    }
    fn find_next(&mut self, backwards: bool) {
        let options = self.find_options();
        let query = self.find_text.clone();
        let current = self.find_match;
        let Some(tab) = self.tab_mut() else { return };
        let matches = editor::find_matches(&tab.text, &query, options);
        if matches.is_empty() {
            self.find_match = None;
            return;
        }
        let at = current.unwrap_or(tab.vim.cursor);
        let found = if backwards {
            matches
                .iter()
                .rev()
                .find(|r| r.start < at)
                .or_else(|| matches.last())
        } else {
            matches
                .iter()
                .find(|r| r.start > at || current.is_none() && r.start == at)
                .or_else(|| matches.first())
        }
        .unwrap()
        .clone();
        tab.set_cursor(found.clone());
        self.find_match = Some(found.start);
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
                            if design::menu_item(ui, "Explorer", "", "Browse project files")
                                .clicked()
                            {
                                self.sidebar = SidebarView::Explorer;
                                self.sidebar_visible = true;
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
                                self.sidebar = SidebarView::Search;
                                self.sidebar_visible = true;
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
                            if let Some(project) = self.project() {
                                let name = file_title(&project.root);
                                let root = crate::paths::display(&project.root);
                                let projects: Vec<_> = self
                                    .projects
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| {
                                        (i, file_title(&p.root), crate::paths::display(&p.root))
                                    })
                                    .collect();
                                design::menu(ui, &format!("{name}  ▾"), &root, |ui| {
                                    ui.label(RichText::new("WORKSPACES").size(10.0).color(MUTED));
                                    for (i, name, path) in projects {
                                        if design::menu_item(
                                            ui,
                                            &name,
                                            if i == self.active_project {
                                                "active"
                                            } else {
                                                ""
                                            },
                                            &path,
                                        )
                                        .clicked()
                                        {
                                            self.switch_project(i);
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    if design::menu_item(
                                        ui,
                                        "Add project",
                                        "Ctrl+Shift+O",
                                        "Keep another project in this window",
                                    )
                                    .clicked()
                                    {
                                        self.pick_project();
                                        ui.close();
                                    }
                                    if design::menu_item(
                                        ui,
                                        "Close project",
                                        "",
                                        "Close this workspace; unsaved edits are confirmed",
                                    )
                                    .clicked()
                                    {
                                        self.request_close(CloseAction::Project(
                                            self.active_project,
                                        ));
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
            });
    }
    fn switch_project(&mut self, index: usize) {
        if let Some(project) = self.projects.get(index) {
            self.activate_tab(index, project.active_tab);
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
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "↻", "Refresh project").clicked() {
                            self.index_project(self.active_project);
                            self.refresh_git(self.active_project, true);
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
        if loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Indexing files…").size(11.0).color(MUTED));
            });
        }
        if let Some(error) = error {
            ui.colored_label(ORANGE, error);
        }
        let Some(index) = index else { return };
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
        let mut root_state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id(("explorer_root", &root)),
            true,
        );
        let root_response = design::left_row(
            ui,
            &file_title(&root),
            Some((if root_state.is_open() { "v" } else { ">" }, MUTED)),
            false,
            &crate::paths::display(&root),
        );
        self.file_context_menu(&root_response, &root, true);
        if root_response.clicked() {
            root_state.toggle(ui);
        }
        let show_root = root_state.is_open();
        root_state.store(ui.ctx());
        if !show_root {
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(("tree", &root))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if index.entries.is_empty() {
                    ui.label(RichText::new("No files here yet.").color(MUTED));
                }
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.spacing_mut().indent = 14.0;
                self.tree(ui, &index, &index.root, &active, 0);
            });
    }
    fn tree(
        &mut self,
        ui: &mut egui::Ui,
        index: &workspace::WorkspaceIndex,
        parent: &Path,
        active: &Option<PathBuf>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        for entry in index.children(parent).take(2000) {
            if entry.depth > 32 {
                continue;
            }
            ui.push_id(&entry.path, |ui| {
                let tooltip = crate::paths::display(&entry.path);
                if entry.is_dir {
                    let id = ui.make_persistent_id("folder");
                    let mut state =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            id,
                            active.as_ref().is_some_and(|p| p.starts_with(&entry.path)),
                        );
                    let arrow = if state.is_open() { "v" } else { ">" };
                    let response =
                        design::left_row(ui, &entry.name, Some((arrow, MUTED)), false, &tooltip);
                    self.file_context_menu(&response, &entry.path, true);
                    if response.clicked() {
                        state.toggle(ui);
                    }
                    state.show_body_indented(&response, ui, |ui| {
                        self.tree(ui, index, &entry.path, active, depth + 1)
                    });
                } else {
                    let selected = active.as_ref() == Some(&entry.path);
                    let response = design::left_row(
                        ui,
                        &entry.name,
                        Some((file_badge(&entry.path), file_color(Some(&entry.path)))),
                        selected,
                        &tooltip,
                    );
                    self.file_context_menu(&response, &entry.path, false);
                    if response.clicked() {
                        self.open_file(entry.path.clone());
                    }
                }
            });
        }
        if index.children(parent).count() > 2000 {
            ui.label(
                RichText::new("More files available in Ctrl+P")
                    .size(10.5)
                    .color(MUTED),
            );
        }
    }
    fn project_search_ui(&mut self, ui: &mut egui::Ui) {
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.project_query)
                .hint_text("Find text in project…")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.search_dirty = true;
            self.search_last_edit = Instant::now();
            self.search_cancel.store(true, Ordering::Relaxed);
            self.search_generation += 1;
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
            egui::ScrollArea::vertical()
                .id_salt("search_results")
                .show(ui, |ui| {
                    let mut previous = None;
                    for m in &results.matches {
                        if previous.as_ref() != Some(&m.path) {
                            ui.add_space(9.0);
                            ui.label(
                                RichText::new(file_title(&m.path))
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
                            clicked = Some((m.path.clone(), m.start_char..m.end_char));
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
            ui.label(RichText::new("Search source files in this project.\nIgnored and generated files are excluded.").size(12.0).color(MUTED));
        }
        if let Some((path, range)) = clicked {
            self.open_file(path);
            if let Some(t) = self.tab_mut() {
                t.set_cursor(range);
            }
        }
    }
    fn git_ui(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else { return };
        let root = p.git_root.clone().unwrap_or_else(|| p.root.clone());
        let repo = p.git_root.is_some();
        let loading = p.git_loading;
        let branch = p.git_branch.clone();
        let changes = p.git_changes.clone();
        let commits = p.recent_commits.clone();
        let error = p.git_error.clone();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(branch.unwrap_or_else(|| {
                    if loading {
                        "Loading…".into()
                    } else {
                        "No repository".into()
                    }
                }))
                .size(13.0)
                .strong()
                .color(ACCENT),
            );
            if loading {
                ui.spinner();
            }
        });
        if let Some(error) = error {
            ui.colored_label(ORANGE, error);
        }
        if !repo {
            if !loading {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Track this project's changes with Git.")
                        .color(MUTED)
                        .size(12.0),
                );
                if ui.button("Initialize repository").clicked() {
                    self.git_action(root, GitAction::Initialize);
                }
            }
            return;
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("CHANGES   {}", changes.len()))
                    .size(10.5)
                    .strong()
                    .color(MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if small_button(ui, "+", "Stage all changes").clicked() {
                    self.git_action(root.clone(), GitAction::StageAll);
                }
                if small_button(ui, "−", "Unstage all").clicked() {
                    self.git_action(root.clone(), GitAction::UnstageAll);
                }
            });
        });
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() * 0.45).max(100.0))
            .id_salt("git_changes")
            .show(ui, |ui| {
                if changes.is_empty() {
                    ui.label(
                        RichText::new("Working tree is clean")
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                for change in &changes {
                    ui.horizontal(|ui| {
                        let staged = change.is_staged();
                        if small_button(
                            ui,
                            if staged { "−" } else { "+" },
                            if staged { "Unstage" } else { "Stage" },
                        )
                        .clicked()
                        {
                            let mut paths = vec![PathBuf::from(&change.path)];
                            if let Some(old) = &change.original_path {
                                paths.push(PathBuf::from(old));
                            }
                            self.git_action(
                                root.clone(),
                                if staged {
                                    GitAction::Unstage(paths)
                                } else {
                                    GitAction::Stage(paths)
                                },
                            );
                        }
                        let mark = if change.is_conflicted() {
                            "!"
                        } else if change.is_untracked() {
                            "U"
                        } else if staged && change.is_unstaged() {
                            "SM"
                        } else if staged {
                            "S"
                        } else {
                            "M"
                        };
                        ui.label(RichText::new(mark).monospace().color(
                            if change.is_conflicted() {
                                RED
                            } else if staged {
                                ACCENT
                            } else {
                                ORANGE
                            },
                        ));
                        let response = ui.add(
                            egui::Label::new(RichText::new(&change.path).size(11.5).color(TEXT))
                                .truncate()
                                .sense(egui::Sense::click()),
                        );
                        if response.clicked() {
                            self.open_file(root.join(&change.path));
                        }
                        response.context_menu(|ui| {
                            if ui.button("View diff").clicked() {
                                let path = PathBuf::from(&change.path);
                                let worker_root = root.clone();
                                let title = format!(
                                    "{}{}",
                                    change.path,
                                    if staged { " · staged" } else { "" }
                                );
                                self.worker(move || WorkerResult::Diff {
                                    title,
                                    result: git::diff(&worker_root, &path, staged)
                                        .map_err(|e| e.to_string()),
                                });
                                ui.close();
                            }
                        });
                    });
                }
            });
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        if let Some(p) = self.projects.get_mut(self.active_project) {
            ui.add(
                egui::TextEdit::multiline(&mut p.commit_message)
                    .hint_text("Commit message")
                    .desired_width(f32::INFINITY)
                    .desired_rows(3),
            );
        }
        let message = self
            .project()
            .map(|p| p.commit_message.clone())
            .unwrap_or_default();
        let busy = self.git_busy.is_some();
        if ui
            .add_enabled_ui(
                !busy
                    && !message.trim().is_empty()
                    && changes.iter().any(|c| c.is_staged())
                    && !changes.iter().any(|c| c.is_conflicted()),
                |ui| {
                    design::primary_button(
                        ui,
                        "Commit staged changes",
                        Vec2::new(ui.available_width(), 32.0),
                    )
                },
            )
            .inner
            .on_hover_text("Commit the staged files with this message")
            .clicked()
        {
            self.git_action(root.clone(), GitAction::Commit(message));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (label, action) in [
                ("Fetch", GitAction::Fetch),
                ("Pull", GitAction::Pull),
                ("Push", GitAction::Push),
            ] {
                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                    self.git_action(root.clone(), action);
                }
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        ui.add_space(14.0);
        egui::CollapsingHeader::new(RichText::new("RECENT COMMITS").size(10.5).color(MUTED)).show(
            ui,
            |ui| {
                for commit in &commits {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&commit.hash[..commit.hash.len().min(7)])
                                .monospace()
                                .size(10.5)
                                .color(ACCENT),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&commit.subject).size(11.0).color(TEXT))
                                .truncate(),
                        );
                    });
                }
            },
        );
    }
}

impl MorrowApp {
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").exact_size(27.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(12, 2)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (mode, color) = if self.tab().is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview) { ("PREVIEW", BLUE) } else if self.settings.vim {
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
                    if let Some((message, error, time)) = &self.status
                        && (*error || time.elapsed() < Duration::from_secs(6)) {
                        let width = (ui.available_width() - 350.0).clamp(40.0, 440.0);
                        ui.add_sized([width, 20.0], egui::Label::new(RichText::new(message).size(10.5).color(if *error { RED } else { MUTED })).truncate()).on_hover_text(message);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(t) = self.tab() {
                            let (line, col) = t.lines.line_col(&t.text, t.revision, t.vim.cursor);
                            ui.label(RichText::new(format!("Ln {line}, Col {col}")).size(10.5).color(MUTED)).on_hover_text("Current cursor position - Ctrl+G to go to a line");
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
        let keyboard_active = self.render_pane_is_focused();
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
                    ui.add_space(5.0);
                    if ui
                        .push_id(id, |ui| {
                            design::left_row(
                                ui,
                                &path,
                                Some((
                                    file_badge(tab.path.as_deref().unwrap_or(Path::new(""))),
                                    file_color(tab.path.as_deref()),
                                )),
                                active,
                                "Open this review document",
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
        let compact_view = markdown && ui.available_width() < 320.0;
        let controls_width = if compact_view { 116.0 } else if markdown { 177.0 } else { 57.0 };
        let tab_width = (ui.available_width() - controls_width).max(20.0);
        ui.allocate_ui_with_layout(Vec2::new(tab_width, 37.0), Layout::left_to_right(Align::Center), |ui| {
        egui::ScrollArea::horizontal()
            .id_salt("document_tabs")
            .auto_shrink([false, true])
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
                            ui.allocate_exact_size(Vec2::new(width, 37.0), egui::Sense::click());
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
                            && !close_rect.contains(
                                ui.input(|input| input.pointer.interact_pos().unwrap_or_default()),
                            )
                        {
                            select = Some((*project_index, *tab_index));
                        }
                        response
                            .on_hover_text(
                                path.as_ref()
                                    .map(|p| crate::paths::display(p))
                                    .unwrap_or_else(|| "Unsaved draft".into()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                    ui.add_space(7.0);
                    if small_button(ui, "+", "New file - Ctrl+N").clicked() {
                        self.new_file();
                    }
                });
            });
        });
        ui.allocate_ui_with_layout(Vec2::new(controls_width, 37.0), Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(7.0);
            design::menu(ui, "Split", "Arrange this document in separate editor panes", |ui| self.split_menu_ui(ui));
            if markdown {
                ui.add_space(4.0);
                if compact_view {
                    let label = if view == crate::cli::DocumentView::Preview { "Preview" } else { "Source" };
                    let response = design::menu(ui, label, "Choose Markdown source or preview - Ctrl+Shift+V", |ui| {
                        if design::menu_item(ui, "Source", if view == crate::cli::DocumentView::Source { "active" } else { "" }, "Edit Markdown with Vim").clicked() {
                            self.set_document_view(crate::cli::DocumentView::Source);
                            ui.close();
                        }
                        if design::menu_item(ui, "Preview", if view == crate::cli::DocumentView::Preview { "active" } else { "" }, "Read formatted Markdown").clicked() {
                            self.set_document_view(crate::cli::DocumentView::Preview);
                            ui.close();
                        }
                    });
                    ui.painter().line_segment([response.rect.left_bottom(), response.rect.right_bottom()], Stroke::new(1.5, ACCENT));
                } else {
                let preview_response = design::ghost_button(ui, "Preview", "Read formatted Markdown - Ctrl+Shift+V. Double-click a block to reveal its source.");
                let source_response = design::ghost_button(ui, "Source", "Edit Markdown with Vim - Ctrl+Shift+V");
                let active = if view == crate::cli::DocumentView::Preview { &preview_response } else { &source_response };
                ui.painter().line_segment([active.rect.left_bottom(), active.rect.right_bottom()], Stroke::new(1.5, ACCENT));
                if source_response.clicked() { self.set_document_view(crate::cli::DocumentView::Source); }
                if preview_response.clicked() { self.set_document_view(crate::cli::DocumentView::Preview); }
                }
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
        let id = self.document_widget_id(document_id);
        let pane_focused = self.render_pane_is_focused();
        if !settings.vim
            && pane_focused
            && ui.is_enabled()
            && !egui::Popup::is_any_open(&ctx)
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
        if pane_focused && ctx.memory(|m| m.has_focus(id)) && !modal {
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
        let scroll_id = ui.make_persistent_id(("document_scroll", tab.id));
        let wheel_offset = (!settings.smooth_scroll)
            .then(|| crate::render::instant_scroll_offset(ui, scroll_id))
            .flatten();
        let mut scroll = egui::ScrollArea::new([!settings.word_wrap, true])
            .id_salt(("document_scroll", tab.id))
            .animated(settings.smooth_scroll)
            .scroll_source(egui::scroll_area::ScrollSource {
                mouse_wheel: settings.smooth_scroll,
                ..Default::default()
            })
            .auto_shrink([false, false]);
        if let Some(offset) = wheel_offset {
            scroll = scroll.scroll_offset(offset);
        }
        let scroll_output = scroll.show(ui, |ui| {
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
            let mut output = egui::TextEdit::multiline(&mut tab.text)
                .id(id)
                .interactive(pane_focused)
                .frame(egui::Frame::new().inner_margin(Margin {
                    left: 56,
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
            if tab.focus && pane_focused {
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
            let pointer_selection = output.response.is_pointer_button_down_on()
                && ui.input(|input| input.pointer.primary_down())
                || output.response.dragged_by(egui::PointerButton::Primary)
                || output.response.clicked()
                || output.response.double_clicked()
                || output.response.triple_clicked();
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
                } else if !modal && tab.vim.cursor != range.primary.index.0 {
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
            context_action = context_actions::editor_menu(
                ui,
                &output.response,
                !tab.loading && !tab.saving && tab.error.is_none(),
                !tab.selection.is_empty(),
            );
            let cursor = egui::text::CCursor::new(tab.vim.cursor);
            let local = output.galley.pos_from_cursor(cursor);
            let cursor_rect = local.translate(output.galley_pos.to_vec2());
            caret_rect = Some(cursor_rect);
            if tab.scroll_to_cursor {
                ui.scroll_to_rect(cursor_rect.expand(35.0), None);
                tab.scroll_to_cursor = false;
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
                    ui.painter().text(
                        egui::pos2(output.galley_pos.x - 17.0, y),
                        egui::Align2::RIGHT_TOP,
                        mapped.line.to_string(),
                        FontId::monospace((font - 2.0).max(10.0)),
                        if mapped.line == current_line {
                            ACCENT
                        } else {
                            MUTED.linear_multiply(0.7)
                        },
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
        });
        crate::render::remember_scroll_limits(&ctx, &scroll_output);
        self.editor_focused = focused;
        self.editor_rect = rect;
        self.editor_cursor_rect = caret_rect;
        if let Some(action) = context_action {
            self.apply_editor_context_action(document_id, action);
        }
    }
    fn find_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if small_button(
                        ui,
                        if self.replace_visible { "▾" } else { "▸" },
                        "Toggle replace",
                    )
                    .clicked()
                    {
                        self.replace_visible = !self.replace_visible;
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.find_text)
                            .hint_text("Find in this file")
                            .desired_width(230.0),
                    );
                    if self.find_focus {
                        response.request_focus();
                        self.find_focus = false;
                    }
                    if response.changed() {
                        self.find_match = None;
                    }
                    if (response.has_focus() || response.lost_focus())
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
                    {
                        self.find_next(false);
                        response.request_focus();
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = false;
                        }
                    }
                    ui.toggle_value(&mut self.find_case, "Aa")
                        .on_hover_text("Match case");
                    ui.toggle_value(&mut self.find_word, "ab")
                        .on_hover_text("Whole word");
                    if small_button(ui, "↑", "Previous · Shift+F3").clicked() {
                        self.find_next(true);
                    }
                    if small_button(ui, "↓", "Next · F3").clicked() {
                        self.find_next(false);
                    }
                    let count = self
                        .tab()
                        .map(|t| {
                            editor::find_matches(&t.text, &self.find_text, self.find_options())
                                .len()
                        })
                        .unwrap_or(0);
                    ui.label(
                        RichText::new(format!("{count} matches"))
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "×", "Close find").clicked() {
                            self.find_visible = false;
                            if let Some(t) = self.tab_mut() {
                                t.focus = true;
                            }
                        }
                    });
                });
                if self.replace_visible {
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.replace_text)
                                .hint_text("Replace with")
                                .desired_width(230.0),
                        );
                        if ui.button("Replace").clicked() {
                            let query = self.find_text.clone();
                            let replacement = self.replace_text.clone();
                            let options = self.find_options();
                            if let Some(t) = self.tab_mut()
                                && let Some(range) = editor::find_matches(&t.text, &query, options)
                                    .into_iter()
                                    .find(|r| r.start >= t.vim.cursor)
                            {
                                t.apply(editor::replace_selection(&t.text, range, &replacement));
                            }
                            self.find_next(false);
                        }
                        if ui.button("Replace all").clicked() {
                            let query = self.find_text.clone();
                            let replacement = self.replace_text.clone();
                            let options = self.find_options();
                            if let Some(t) = self.tab_mut() {
                                let (edit, count) = editor::replace_all(
                                    &t.text,
                                    t.selection.clone(),
                                    &query,
                                    &replacement,
                                    options,
                                );
                                if count > 0 {
                                    t.apply(edit);
                                }
                                self.status(format!("Replaced {count} matches"), false);
                            }
                        }
                    });
                }
            });
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
                "noh" | "nohlsearch" => self.find_visible = false,
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
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.15).max(35.0));
            brand(ui, 58.0);
            ui.add_space(22.0);
            ui.label(
                RichText::new("A quieter place to build.")
                    .size(34.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new("Your projects. Your motions. Room to focus.")
                    .size(15.0)
                    .color(MUTED),
            );
            ui.add_space(35.0);
            if accent_button(ui, "Open a project folder").clicked() {
                self.pick_project();
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new("Ctrl + Shift + O")
                    .monospace()
                    .size(11.0)
                    .color(MUTED),
            );
            ui.add_space(46.0);
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(10.0)
                .inner_margin(24)
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
                                ui.label(RichText::new(keys).monospace().size(11.0).color(MUTED));
                            });
                        });
                        ui.add_space(6.0);
                    }
                });
            ui.add_space(25.0);
            ui.label(
                RichText::new(format!("MORROW  /  {}", env!("CARGO_PKG_VERSION")))
                    .size(10.0)
                    .color(MUTED.linear_multiply(0.6)),
            );
        });
    }
}

const COMMANDS: &[(&str, &str)] = &[
    ("Add project folder", "Ctrl Shift O"),
    ("Open file", "Ctrl O"),
    ("New file", "Ctrl N"),
    ("Save current file", "Ctrl S"),
    ("Save all files", "Ctrl Shift S"),
    ("Find in file", "Ctrl F"),
    ("Replace in file", "Ctrl H"),
    ("Search project", "Ctrl Shift F"),
    ("Go to line", "Ctrl G"),
    ("Go to definition", "gd / F12"),
    ("Show completions", "Ctrl Space"),
    ("Show problems", "Ctrl Shift M"),
    ("Markdown source / preview", "Ctrl Shift V"),
    ("Split right", "Ctrl \\"),
    ("Split down", "Ctrl Shift \\"),
    ("Focus next pane", "Ctrl Alt Right"),
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
    ("Reload file from disk", ""),
    ("Settings", "Ctrl ,"),
];
impl MorrowApp {
    fn command_action(&mut self, label: &str) {
        match label {
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
                self.sidebar = SidebarView::Search;
                self.sidebar_visible = true;
            }
            "Go to line" => self.open_palette(Palette::Line),
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
            "Focus next pane" => self.split_command("next"),
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
            let mut open = true;
            let mut choose = None;
            let mut line = None;
            egui::Window::new(match palette {
                Palette::Files => "Open a file",
                Palette::Commands => "Command palette",
                Palette::Line => "Go to line",
            })
            .id(egui::Id::new("palette"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size([
                590.0,
                if palette == Palette::Line {
                    110.0
                } else {
                    440.0
                },
            ])
            .anchor(egui::Align2::CENTER_TOP, [0.0, 125.0])
            .open(&mut open)
            .show(&ctx, |ui| {
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
                ui.add_space(8.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .id_salt("palette_input")
                        .hint_text(match palette {
                            Palette::Files => "Search by file name or path…",
                            Palette::Commands => "What would you like to do?",
                            Palette::Line => "Line number",
                        })
                        .desired_width(f32::INFINITY)
                        .font(FontId::proportional(15.0)),
                );
                if self.palette_focus {
                    response.request_focus();
                    self.palette_focus = false;
                }
                if response.changed() {
                    self.palette_selected = 0;
                }
                let down = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                let up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
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
                    if down && !options.is_empty() {
                        self.palette_selected = (self.palette_selected + 1).min(options.len() - 1);
                    }
                    if up {
                        self.palette_selected = self.palette_selected.saturating_sub(1);
                    }
                    self.palette_selected =
                        self.palette_selected.min(options.len().saturating_sub(1));
                    egui::ScrollArea::vertical()
                        .max_height(340.0)
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
                                if (down || up) && i == self.palette_selected {
                                    response.scroll_to_me(None);
                                }
                            }
                            if options.is_empty() {
                                ui.add_space(10.0);
                                ui.label(RichText::new("No matches").color(MUTED));
                            }
                        });
                    ui.separator();
                    ui.label(
                        RichText::new("↑ ↓ to navigate    Enter to open    Esc to close")
                            .size(10.5)
                            .color(MUTED),
                    );
                }
            });
            let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
            if !open || escape || choose.is_some() || line.is_some() {
                self.palette = None;
                if let Some(t) = self.tab_mut() {
                    t.focus = true;
                }
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
            let mut open = true;
            let mut done = false;
            egui::Window::new("Settings")
                .collapsible(false)
                .resizable(false)
                .default_width(420.0)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    ui.add_space(7.0);
                    ui.label(
                        RichText::new("MAKE IT YOURS")
                            .size(10.5)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.add_space(12.0);
                    ui.checkbox(&mut self.settings.vim, "Vim motions (default)");
                    ui.label(
                        RichText::new(
                            "v selects characters · V selects lines · Esc returns to Normal",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                    ui.add_space(10.0);
                    ui.checkbox(
                        &mut self.settings.autosave,
                        "Save files automatically after a pause",
                    );
                    ui.label(
                        RichText::new("Recovery copies are kept even with auto save off.")
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(10.0);
                    ui.checkbox(&mut self.settings.word_wrap, "Wrap long lines");
                    ui.checkbox(&mut self.settings.smooth_scroll, "Smooth scrolling")
                        .on_hover_text("Animate source and Markdown scrolling; turn off for immediate movement.");
                    ui.add_space(10.0);
                    ui.add(
                        egui::Slider::new(&mut self.settings.font_size, 10.0..=24.0)
                            .text("Editor font size"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.settings.tab_width, 1..=8).text("Indent width"),
                    );
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
                        "Commands: :w :wa :q :wq :e path :42",
                    ] {
                        ui.label(RichText::new(text).monospace().size(11.5).color(TEXT));
                    }
                    ui.add_space(12.0);
                    if ui.button("Done").clicked() {
                        done = true;
                    }
                });
            self.settings_open = open && !done;
            self.session_dirty = true;
        }
        if let Some(action) = self.close_action
            && self.save_as.is_none()
        {
            let mut save = false;
            let mut discard = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("unsaved_changes")).show(&ctx, |ui| {
                ui.set_width(390.0);
                ui.label(
                    RichText::new("Keep your changes?")
                        .size(20.0)
                        .strong()
                        .color(TEXT),
                );
                ui.add_space(10.0);
                ui.label(RichText::new("Some files have unsaved changes.").color(MUTED));
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
            if cancel {
                self.close_action = None;
                self.close_after_save = false;
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
        if let Some((title, text)) = &self.diff {
            let mut open = true;
            egui::Window::new(title)
                .open(&mut open)
                .default_size([950.0, 620.0])
                .show(&ctx, |ui| {
                    egui::ScrollArea::both().show(ui, |ui| {
                        for line in text.lines() {
                            let color = if line.starts_with('+') {
                                ACCENT
                            } else if line.starts_with('-') {
                                RED
                            } else if line.starts_with("@@") {
                                BLUE
                            } else {
                                MUTED
                            };
                            ui.add(
                                egui::Label::new(
                                    RichText::new(line).monospace().size(12.0).color(color),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                        if text.is_empty() {
                            ui.label("No tracked diff. Open untracked files in the editor.");
                        }
                    });
                });
            if !open {
                self.diff = None;
            }
        }
    }
    fn draw_ui(&mut self, ui: &mut egui::Ui) {
        self.syntax.begin_frame(ui);
        let restore_after_menu = egui::Popup::is_any_open(&self.ctx)
            && self.ctx.input(|input| input.key_pressed(Key::Escape));
        self.poll();
        self.poll_control();
        self.apply_pending_agent_locations();
        self.poll_languages();
        self.focus_pane_from_pointer();
        self.context_paste_poll();
        self.shortcuts();
        self.handle_vim();
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
            && self.close_action.is_none()
            && self.save_as.is_none()
            && self.diff.is_none()
            && let Some(tab) = self.tab_mut()
        {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
        self.editor_ui(ui);
        self.overlays();
        self.completion_ui();
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
    let mut fonts = egui::FontDefinitions::default();
    for (name, file, family) in [
        ("morrow-ui", "segoeui.ttf", egui::FontFamily::Proportional),
        (
            "morrow-code",
            "CascadiaMono.ttf",
            egui::FontFamily::Monospace,
        ),
        (
            "morrow-symbols",
            "seguisym.ttf",
            egui::FontFamily::Proportional,
        ),
    ] {
        if let Ok(bytes) = fs::read(
            PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
                .join("Fonts")
                .join(file),
        ) {
            fonts
                .font_data
                .insert(name.into(), egui::FontData::from_owned(bytes).into());
            if name == "morrow-symbols" {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    fonts.families.entry(family).or_default().push(name.into());
                }
            } else {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, name.into());
            }
        }
    }
    if let Ok(bytes) = fs::read(
        PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
            .join("Fonts")
            .join("seguisb.ttf"),
    ) {
        fonts.font_data.insert(
            "morrow-semibold".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        let mut family = vec!["morrow-semibold".into()];
        family.extend(
            fonts
                .families
                .get(&egui::FontFamily::Proportional)
                .cloned()
                .unwrap_or_default(),
        );
        fonts
            .families
            .insert(egui::FontFamily::Name("morrow-semibold".into()), family);
    }
    ctx.set_fonts(fonts);
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
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.tab().is_none_or(|t| {
                !self
                    .ctx
                    .memory(|m| m.has_focus(self.document_widget_id(t.id)))
            })
        {
            self.completions = None;
            return false;
        }
        if self.completions.is_none() {
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
            if down {
                p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1));
            }
            if up {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, Key::Enter)
                || i.consume_key(egui::Modifiers::NONE, Key::Tab)
        }) {
            let selected = self.completions.as_ref().unwrap().selected;
            self.apply_completion(selected);
            return true;
        }
        false
    }
    fn completion_ui(&mut self) {
        let Some(p) = &self.completions else { return };
        let mut selected = None;
        let ctx = self.ctx.clone();
        let bounds = ctx.input(|i| i.content_rect());
        let caret = self.editor_cursor_rect.unwrap_or_else(|| {
            egui::Rect::from_min_size(
                egui::pos2(bounds.left() + 320.0, bounds.top() + 150.0),
                Vec2::ZERO,
            )
        });
        let popup_height = (p.items.len().min(8) as f32 * 29.0 + 45.0).min(320.0);
        let top = if caret.bottom() + popup_height > bounds.bottom() - 40.0 {
            (caret.top() - popup_height).max(bounds.top() + 70.0)
        } else {
            caret.bottom() + 4.0
        };
        let position = egui::pos2(
            caret.left().min(bounds.right() - 520.0).max(bounds.left()),
            top,
        );
        egui::Window::new("Suggestions")
            .id(egui::Id::new("completion"))
            .resizable(false)
            .collapsible(false)
            .title_bar(false)
            .default_width(490.0)
            .fixed_pos(position)
            .show(&ctx, |ui| {
                ui.label(
                    RichText::new("SUGGESTIONS  ·  Enter to accept")
                        .size(10.0)
                        .color(ACCENT),
                );
                egui::ScrollArea::vertical()
                    .max_height(270.0)
                    .show(ui, |ui| {
                        for (i, item) in p.items.iter().take(100).enumerate() {
                            if ui
                                .push_id(i, |ui| {
                                    design::detail_row(
                                        ui,
                                        &item.label,
                                        item.detail.as_deref().unwrap_or(""),
                                        i == p.selected,
                                        "Accept this suggestion",
                                    )
                                })
                                .inner
                                .clicked()
                            {
                                selected = Some(i);
                            }
                        }
                    });
            });
        if let Some(i) = selected {
            self.apply_completion(i);
        }
    }
    fn problems_ui(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else { return };
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
                        file_title(&path),
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
                    if response.clicked() {
                        jump = Some((path, d.range.start));
                    }
                    response.on_hover_text(d.message);
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
#[path = "intel_tests.rs"]
mod intel_tests;
