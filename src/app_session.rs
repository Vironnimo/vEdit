//! Application preferences and compatible session restoration and persistence.
use super::*;

/// Prepare disk-backed session state while the native graphics context starts.
pub struct Startup(Option<std::thread::JoinHandle<StartupResult>>);

type StartupResult = (StartupData, Result<crate::cli::Server, String>);

pub(super) struct StartupData {
    pub(super) manifest: Option<Session>,
    pub(super) inherited_settings: Option<Settings>,
    pub(super) external_open: bool,
    pub(super) fonts: (crate::cli::EditorFont, egui::FontDefinitions),
}

impl Startup {
    pub fn prepare() -> Self {
        let directory = recovery_directory();
        let parent =
            (crate::profile::window_id() != "main").then(crate::profile::base_recovery_directory);
        Self(Some(std::thread::spawn(move || {
            let control = crate::cli::Server::prepare();
            (StartupData::read(&directory, parent.as_deref()), control)
        })))
    }

    pub(super) fn finish(mut self) -> StartupResult {
        self.0
            .take()
            .expect("startup consumed once")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        // A graphics initialization failure must finish endpoint publication and
        // drop its server before main releases the per-window instance lock.
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}

impl StartupData {
    fn read(directory: &Path, parent: Option<&Path>) -> Self {
        let mut manifest = fs::read(directory.join("session.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok());
        let inherited_settings = if manifest
            .as_ref()
            .is_none_or(|session| session.settings.is_none())
        {
            parent.and_then(|parent| {
                fs::read(parent.join("session.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok())
                    .and_then(|session| session.settings)
            })
        } else {
            None
        };
        if let Some(session) = &mut manifest {
            for project in &mut session.projects {
                project.restore_available = Some(project.available());
            }
        }
        let font = manifest
            .as_ref()
            .and_then(|session| session.settings.as_ref())
            .or(inherited_settings.as_ref())
            .map(|settings| settings.editor_font)
            .unwrap_or_default();
        // Prepare definitions and warm font files without locking the live context.
        let fonts = crate::typography::fonts(font);
        Self {
            manifest,
            inherited_settings,
            external_open: crate::profile::read_open_mode(parent.unwrap_or(directory)),
            fonts: (font, fonts),
        }
    }
}

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
    /// Startup's filesystem observation; never part of a persisted session.
    #[serde(skip)]
    pub(super) restore_available: Option<bool>,
    pub(super) kind: WorkspaceKind,
    pub(super) launch_selected: Option<String>,
    pub(super) terminals: project_terminal::SessionTerminals,
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

impl SessionProject {
    fn available(&self) -> bool {
        self.kind == WorkspaceKind::OtherFiles
            || self.root.is_dir()
            || self.tabs.iter().any(|tab| {
                tab.recovery_path
                    .as_ref()
                    .is_some_and(|path| path.is_file())
            })
    }
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
            if !entry.restore_available.unwrap_or_else(|| entry.available()) {
                continue;
            }
            let root = if entry.kind == WorkspaceKind::OtherFiles {
                other_files::layout_key()
            } else {
                entry.root
            };
            let mut project = if entry.kind == WorkspaceKind::OtherFiles {
                Project::other_files()
            } else {
                Project::new(root.clone())
            };
            project.tasks = entry.tasks;
            project.launch.selected = entry.launch_selected;
            if project.folder_root().is_some() {
                project.terminals = self.restore_terminals(entry.terminals);
            }
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
                    restore_available: None,
                    kind: p.kind,
                    launch_selected: p.launch.selected.clone(),
                    terminals: p.terminals.session(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_snapshot_preserves_project_eligibility_and_window_preferences() {
        let root = std::env::temp_dir().join(format!(
            "vedit-startup-snapshot-{}",
            crate::recovery::session_prefix()
        ));
        let window = root.join("windows/w1");
        let folder = root.join("project");
        fs::create_dir_all(&window).unwrap();
        fs::create_dir_all(&folder).unwrap();
        let recovery = root.join("draft.txt");
        fs::write(&recovery, "retained draft").unwrap();
        let parent = Session {
            settings: Some(Settings {
                font_size: 19.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        fs::write(
            root.join("session.json"),
            serde_json::to_vec(&parent).unwrap(),
        )
        .unwrap();
        fs::write(root.join("external-open.json"), "true").unwrap();
        for own_settings in [
            None,
            Some(Settings {
                font_size: 16.0,
                ..Default::default()
            }),
        ] {
            let session = Session {
                settings: own_settings.clone(),
                projects: vec![
                    SessionProject {
                        root: folder.clone(),
                        ..Default::default()
                    },
                    SessionProject {
                        root: root.join("missing"),
                        ..Default::default()
                    },
                    SessionProject {
                        root: root.join("offline"),
                        tabs: vec![SessionTab {
                            recovery_path: Some(recovery.clone()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    SessionProject {
                        kind: WorkspaceKind::OtherFiles,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            fs::write(
                window.join("session.json"),
                serde_json::to_vec(&session).unwrap(),
            )
            .unwrap();
            let snapshot = StartupData::read(&window, Some(&root));
            assert!(snapshot.external_open);
            assert_eq!(
                snapshot
                    .inherited_settings
                    .as_ref()
                    .map(|settings| settings.font_size),
                own_settings.is_none().then_some(19.0)
            );
            let manifest = snapshot.manifest.unwrap();
            assert_eq!(
                manifest
                    .settings
                    .as_ref()
                    .map(|settings| settings.font_size),
                own_settings.as_ref().map(|settings| settings.font_size)
            );
            assert_eq!(
                manifest
                    .projects
                    .iter()
                    .map(|project| project.restore_available)
                    .collect::<Vec<_>>(),
                [Some(true), Some(false), Some(true), Some(true)]
            );
            let persisted = serde_json::to_value(&manifest).unwrap();
            assert!(
                persisted["projects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|project| project.get("restore_available").is_none())
            );
        }
        fs::remove_file(window.join("session.json")).unwrap();
        let fresh = StartupData::read(&window, Some(&root));
        assert!(fresh.manifest.is_none());
        assert_eq!(fresh.inherited_settings.unwrap().font_size, 19.0);
        assert!(
            recovery.is_file(),
            "startup must not remove a retained draft"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
