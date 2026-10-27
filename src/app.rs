use crate::design::HoverDetails;
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

#[path = "agent_review_ui.rs"]
mod agent_review_ui;
#[path = "app_chrome.rs"]
mod app_chrome;
#[path = "app_overlays.rs"]
mod app_overlays;
#[path = "app_session.rs"]
mod app_session;
#[path = "app_workers.rs"]
mod app_workers;
#[path = "application_commands.rs"]
mod application_commands;
#[path = "command_palette.rs"]
mod command_palette;
#[path = "command_registry.rs"]
mod command_registry;
#[path = "document_loading.rs"]
mod document_loading;
#[path = "document_recovery.rs"]
mod document_recovery;
#[path = "document_saving.rs"]
mod document_saving;
#[path = "document_state.rs"]
mod document_state;
#[path = "editor_navigation.rs"]
mod editor_navigation;
#[path = "editor_view.rs"]
mod editor_view;
#[path = "file_dialogs.rs"]
mod file_dialogs;
#[path = "find_toolbar.rs"]
mod find_toolbar;
#[path = "git_actions.rs"]
mod git_actions;
#[path = "git_refresh.rs"]
mod git_refresh;
#[path = "language_integration.rs"]
mod language_integration;
#[path = "project_search_ui.rs"]
mod project_search_ui;
#[path = "project_workspace.rs"]
mod project_workspace;
#[path = "source_editor.rs"]
mod source_editor;
#[path = "task_controls.rs"]
mod task_controls;
#[path = "vim_input.rs"]
mod vim_input;
#[path = "workspace_shortcuts.rs"]
mod workspace_shortcuts;

use app_chrome::*;
use app_session::*;
use app_workers::*;
use command_palette::*;
use document_recovery::*;
use document_saving::*;
use document_state::*;
use editor_navigation::*;
use file_dialogs::*;
use git_actions::*;
use git_refresh::*;
use language_integration::*;
use project_workspace::*;

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
#[path = "language_intel.rs"]
mod language_intel;
#[path = "language_settings.rs"]
mod language_settings;
#[path = "language_support.rs"]
mod language_support;
#[cfg(test)]
#[path = "multicursor_tests.rs"]
mod multicursor_tests;
#[path = "multicursor_ui.rs"]
mod multicursor_ui;
#[path = "navigation.rs"]
mod navigation;
#[path = "notifications.rs"]
mod notifications;
#[path = "other_files.rs"]
mod other_files;
#[path = "problems.rs"]
mod problems;
#[path = "profile_migration.rs"]
pub(crate) mod profile_migration;
#[path = "project_launch.rs"]
mod project_launch;
#[path = "project_status.rs"]
mod project_status;
#[path = "project_tasks.rs"]
mod project_tasks;
#[path = "recent.rs"]
mod recent;
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
#[path = "status_messages.rs"]
mod status_messages;
#[path = "tab_drag.rs"]
mod tab_drag;
#[path = "task_output_ui.rs"]
mod task_output_ui;
#[path = "workspace_ui.rs"]
mod workspace_ui;

use crate::palette::{
    ACCENT, BG, BLUE, BORDER, GREEN, HOVER, MUTED, ORANGE, PANEL, RED, SURFACE, TEXT,
};
const MAX_DOCUMENT: u64 = workspace::MAX_DOCUMENT_BYTES;

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

pub struct VEditApp {
    recent: Vec<recent::Entry>,
    recent_open_request: u64,
    recent_opening: std::collections::HashSet<u64>,
    projects: Vec<Project>,
    closed_other_files: Vec<reopen::ClosedTab>,
    active_project: usize,
    workspace_ui: workspace_ui::WorkspaceUiState,
    explorer_navigation: explorer_navigation::ExplorerNavigationState,
    sidebar: SidebarView,
    sidebar_visible: bool,
    sidebar_layout: sidebar_layout::SidebarLayout,
    settings: Settings,
    external_open_revision: u64,
    external_open_save_pending: bool,
    bookmarks: crate::bookmarks::BookmarkStore,
    bookmark_lines: BookmarkLineCache,
    pending_bookmark_jump: Option<(u64, crate::bookmarks::Bookmark)>,
    navigation: navigation::NavigationState,
    screen_jump: Option<ActiveScreenJump>,
    sender: Sender<WorkerResult>,
    receiver: Receiver<WorkerResult>,
    ctx: egui::Context,
    status: Option<(String, bool, Instant)>,
    status_details: Option<status_messages::Details>,
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
    search_owner: Option<project_search_ui::SearchOwner>,
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
    git_action: Option<ActiveGitAction>,
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
    intel: language_intel::IntelState,
    /// Projects follow external file changes; tests opt in explicitly.
    file_watching: bool,
    /// A back/forward destination waiting for its document to load.
    pending_jump: Option<(u64, jump_list::Jump)>,
    /// The latest clicked output location; older resolutions are dropped.
    output_link_request: u64,
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
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let stored: Session = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        let manifest: Option<Session> = fs::read(recovery_directory().join("session.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        // Only a readable manifest proves which recovery files are still in use.
        if let Some(session) = &manifest {
            let referenced: Vec<PathBuf> = session
                .projects
                .iter()
                .flat_map(|project| &project.tabs)
                .filter_map(|tab| tab.recovery_path.clone())
                .collect();
            std::thread::spawn(move || {
                crate::recovery::remove_orphans(
                    &recovery_directory(),
                    &referenced,
                    ORPHANED_RECOVERY_AGE,
                );
            });
        }
        let mut session = manifest.unwrap_or(stored);
        if session.settings.is_none() && crate::profile::window_id() != "main" {
            // Inherit preferences, never another window's documents or layout.
            session.settings =
                fs::read(crate::profile::base_recovery_directory().join("session.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok())
                    .and_then(|primary| primary.settings);
        }
        let mut app = Self::empty(cc.egui_ctx.clone());
        app.native_dialog_builder = Some(rfd::FileDialog::new().set_parent(cc));
        app.settings = session.settings.unwrap_or_default();
        app.settings.external_open_new_window = crate::profile::external_open_new_window();
        app.settings.show_ignored_files = true;
        app.settings.normalize_appearance();
        let shortcut_warnings = keybindings::restore_bindings(&mut app.settings.keybindings);
        app.ctx.set_zoom_factor(app.settings.ui_scale);
        app.apply_editor_font();
        app.settings.leader_key = crate::cli::normalize_leader_key(&app.settings.leader_key)
            .unwrap_or_else(|_| " ".into());
        app.bookmarks = crate::bookmarks::BookmarkStore::from_items(session.bookmarks);
        app.agent_marks = session.agent_marks;
        app.notifications = session.notifications;
        app.find_regex = session.find_regex;
        app.restore_recent(session.recent);
        if let Some(autosave) = session.autosave {
            app.settings.autosave = autosave;
        }
        app.restore_session_workspace(session.projects, session.active_project, session.layout);
        app.restore_sidebars(session.sidebars, session.sidebar);
        for index in 0..app.projects.len() {
            app.index_project(index);
        }
        app.session_dirty = !shortcut_warnings.is_empty();
        // Folders first, so mixed shell arguments use the same ownership rule as drops.
        let (folders, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|path| path.is_dir());
        for path in folders.into_iter().chain(files) {
            app.open_argument(path);
        }
        if !shortcut_warnings.is_empty() {
            app.status(
                format!("Some keyboard shortcuts could not be restored. Open Settings > Keyboard to review them.\n{}", shortcut_warnings.join("\n")),
                true,
            );
        }
        app.start_control();
        app
    }
    fn empty(ctx: egui::Context) -> Self {
        set_style(&ctx);
        let (sender, receiver) = mpsc::channel();
        Self {
            projects: Vec::new(),
            closed_other_files: Vec::new(),
            recent: Vec::new(),
            recent_open_request: 0,
            recent_opening: Default::default(),
            active_project: 0,
            workspace_ui: workspace_ui::WorkspaceUiState::default(),
            explorer_navigation: explorer_navigation::ExplorerNavigationState::default(),
            sidebar: SidebarView::Explorer,
            sidebar_visible: true,
            sidebar_layout: sidebar_layout::SidebarLayout::default(),
            settings: Settings::default(),
            external_open_revision: 0,
            external_open_save_pending: false,
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
            status_details: None,
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
            search_owner: None,
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
            git_action: None,
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
            intel: Default::default(),
            file_watching: !cfg!(test),
            pending_jump: None,
            output_link_request: 0,
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
}

impl VEditApp {
    fn draw_ui(&mut self, ui: &mut egui::Ui) {
        design::begin_hover_frame(&self.ctx);
        self.prune_empty_other_files();
        self.receive_file_drop();
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
        self.hover_escape();
        let dragging_tab =
            self.project_tab_drag_keys() || self.tab_drag_keys() || self.sidebar_drag_keys();
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
        self.track_project_search_documents();
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
            && self.status_details.is_none()
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
        self.intel.begin_frame();
        self.editor_ui(ui);
        self.finish_workspace_action();
        self.finish_region_render_owner();
        self.finish_region_focus();
        self.overlays();
        self.agent_marks_dialog();
        self.notifications_dialog();
        self.status_details_ui();
        self.sync_agent_marks();
        self.navigation_ui();
        self.completion_ui();
        self.hover_ui();
        design::end_hover_frame(&self.ctx);
        self.finish_agent_mark_input_owner();
        self.finish_shortcut_input();
        self.autosave();
        self.sync_languages(false);
        self.watch_disk();
        self.paint_file_drop();
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

#[cfg(test)]
#[path = "visual_tests.rs"]
mod visual_tests;

#[cfg(test)]
#[path = "readme_assets_tests.rs"]
mod readme_assets_tests;

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
#[path = "language_intel_tests.rs"]
mod language_intel_tests;

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
