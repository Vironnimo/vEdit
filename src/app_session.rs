//! Application preferences and compatible session restoration and persistence.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    pub(super) keybindings: keybindings::Bindings,
    pub(super) vim_keys: keybindings::vim_keys::Preferences,
    pub(super) multicursor: multicursor_ui::Preferences,
    pub(super) vim: bool,
    pub(super) swap_jk: bool,
    pub(super) autosave: bool,
    pub(super) external_open_new_window: bool,
    pub(super) autosave_delay_ms: u64,
    pub(super) font_size: f32,
    pub(super) ui_scale: f32,
    pub(super) line_height: f32,
    pub(super) line_numbers: crate::cli::LineNumbers,
    pub(super) highlight_current_line: bool,
    /// Lines added, modified or removed since HEAD beside the line numbers.
    pub(super) git_gutter: bool,
    pub(super) show_ignored_files: bool,
    pub(super) explorer_preview_tabs: bool,
    pub(super) editor_font: crate::cli::EditorFont,
    pub(super) tab_width: usize,
    pub(super) detect_indentation: bool,
    pub(super) indent_with_tabs: bool,
    pub(super) language_indent_widths: HashMap<String, usize>,
    pub(super) language_indent_with_tabs: HashMap<String, bool>,
    pub(super) word_wrap: bool,
    pub(super) smooth_scroll: bool,
    pub(super) leader_key: String,
    pub(super) search_excludes: Vec<String>,
    pub(super) language_servers: Vec<language_settings::LanguageServerSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keybindings: keybindings::Bindings::new(),
            vim_keys: keybindings::vim_keys::Preferences::default(),
            multicursor: multicursor_ui::Preferences::default(),
            vim: true,
            swap_jk: false,
            autosave: true,
            external_open_new_window: false,
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
pub(super) struct Session {
    pub(super) recent: Vec<recent::Entry>,
    pub(super) projects: Vec<SessionProject>,
    pub(super) active_project: usize,
    pub(super) sidebar: Option<SidebarView>,
    pub(super) sidebars: Option<sidebar_layout::SidebarSession>,
    pub(super) autosave: Option<bool>,
    pub(super) settings: Option<Settings>,
    pub(super) layout: Option<split::SessionLayout>,
    pub(super) bookmarks: Vec<crate::bookmarks::Bookmark>,
    pub(super) agent_marks: crate::agent_marks::AgentMarks,
    pub(super) notifications: notifications::Inbox,
    /// The Find toolbar searches with regular expressions.
    pub(super) find_regex: bool,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(super) struct SessionProject {
    pub(super) kind: WorkspaceKind,
    pub(super) launch_selected: Option<String>,
    pub(super) root: PathBuf,
    pub(super) tasks: project_tasks::TaskConfiguration,
    pub(super) open_files: Vec<PathBuf>,
    pub(super) active_file: Option<PathBuf>,
    pub(super) tabs: Vec<SessionTab>,
    pub(super) active_tab_index: Option<usize>,
    pub(super) layout: Option<split::SessionLayout>,
    pub(super) git_split_ratio: Option<f32>,
    pub(super) git_approved_parent: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(super) struct SessionTab {
    pub(super) path: Option<PathBuf>,
    pub(super) title: String,
    pub(super) recovery_path: Option<PathBuf>,
    pub(super) cursor: usize,
    pub(super) view: crate::cli::DocumentView,
    pub(super) agent_unsaved: bool,
}

impl VEditApp {
    pub(super) fn restore_session_workspace(
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
    pub(super) fn restore_session_projects(
        &mut self,
        entries: Vec<SessionProject>,
    ) -> Vec<Option<usize>> {
        let mut mapping = vec![None; entries.len()];
        for (original, entry) in entries.into_iter().enumerate() {
            let root = if entry.kind == WorkspaceKind::OtherFiles {
                other_files::layout_key()
            } else {
                entry.root
            };
            if entry.kind == WorkspaceKind::Project
                && !root.is_dir()
                && !entry.tabs.iter().any(|tab| {
                    tab.recovery_path
                        .as_ref()
                        .is_some_and(|path| path.is_file())
                })
            {
                continue;
            }
            let mut project = if entry.kind == WorkspaceKind::OtherFiles {
                Project::other_files()
            } else {
                Project::new(root.clone())
            };
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
    pub(super) fn session(&self) -> Session {
        Session {
            recent: self.recent.clone(),
            projects: self
                .projects
                .iter()
                .map(|p| SessionProject {
                    kind: p.kind,
                    launch_selected: p.launch.selected.clone(),
                    root: p
                        .folder_root()
                        .unwrap_or_else(|| Path::new(""))
                        .to_path_buf(),
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
                            // A document whose recovery file is known to be
                            // removed no longer refers to it.
                            recovery_path: (t.dirty
                                || t.loading
                                || t.saving
                                || t.error.is_some()
                                || t.recovery_stale
                                || t.recovery_pending
                                || t.recovery_cleanup_pending.is_some()
                                || t.recovery_revision != t.revision)
                                .then(|| t.recovery_path.clone()),
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
            notifications: self.notifications.clone(),
            find_regex: self.find_regex,
        }
    }
    pub(super) fn persist_session(&self) {
        if let Ok(bytes) = serde_json::to_vec(&self.session()) {
            let stamp = self.recovery_writer.stamp();
            self.recovery_writer.write_in_background(
                self.recovery_session_path.clone(),
                stamp,
                bytes,
            );
        }
    }
}
