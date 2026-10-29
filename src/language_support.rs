//! Independent per-language sessions with visible state and explicit recovery.
use super::*;
use language_settings::LanguageServerSettings;

pub(super) struct LanguageSession {
    pub config: LanguageServerSettings,
    pub effective_config: Option<crate::lsp::ServerConfig>,
    pub server: Option<crate::lsp::LanguageServer>,
    pub ready: bool,
    pub error: Option<String>,
    pub opened: HashMap<PathBuf, (u64, i32)>,
    pub diagnostics: HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>,
    /// Which transmitted document each entry in `diagnostics` describes.
    pub diagnostic_origins: HashMap<PathBuf, DiagnosticsOrigin>,
    pub retired_paths: std::collections::HashSet<PathBuf>,
}
/// The buffer text one server's diagnostics were computed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DiagnosticsOrigin {
    /// LSP document version named by publishDiagnostics; None when the server
    /// sent no version.
    pub version: Option<i32>,
    /// Tab revision transmitted as `version`. Without a published version, the
    /// last revision transmitted before the diagnostics arrived.
    pub revision: Option<u64>,
}
impl DiagnosticsOrigin {
    /// Map a publication onto the tab revision recorded for its LSP version.
    pub fn new(version: Option<i32>, transmitted: Option<&(u64, i32)>) -> Self {
        let revision = match (version, transmitted) {
            (Some(version), Some((revision, current))) => {
                (version == *current).then_some(*revision)
            }
            (None, Some((revision, _))) => Some(*revision),
            (_, None) => None,
        };
        Self { version, revision }
    }
    /// (fresh, computed revision) relative to the tab's current revision.
    fn freshness(&self, current: u64) -> (Option<bool>, Option<u64>) {
        match (self.version, self.revision) {
            (Some(_), Some(revision)) => (Some(revision == current), Some(revision)),
            // Unversioned results describe at most the last transmitted text.
            (None, Some(revision)) if revision != current => (Some(false), None),
            _ => (None, None),
        }
    }
}
impl LanguageSession {
    pub fn new(config: LanguageServerSettings) -> Self {
        Self {
            config,
            effective_config: None,
            server: None,
            ready: false,
            error: None,
            opened: HashMap::new(),
            diagnostics: HashMap::new(),
            diagnostic_origins: HashMap::new(),
            retired_paths: std::collections::HashSet::new(),
        }
    }
    pub fn active(&self) -> bool {
        self.ready
            && self
                .server
                .as_ref()
                .is_some_and(|server| !server.is_stopped())
    }
}
pub(super) struct LanguageAnalysisStatus {
    pub label: String,
    pub hint: String,
    pub color: Color32,
}
impl Project {
    pub(super) fn clear_language_diagnostics(&mut self, language: &str) {
        let Some(session) = self.languages.get_mut(language) else {
            return;
        };
        self.diagnostics.retain(|path, _| {
            !session.config.matches(path) && !session.diagnostics.contains_key(path)
        });
        session.diagnostics.clear();
        session.diagnostic_origins.clear();
        let mut remaining: HashMap<PathBuf, Vec<crate::lsp::Diagnostic>> = HashMap::new();
        for session in self.languages.values() {
            for (path, diagnostics) in &session.diagnostics {
                remaining
                    .entry(path.clone())
                    .or_default()
                    .extend(diagnostics.iter().cloned());
            }
        }
        self.diagnostics.extend(remaining);
        self.diagnostics_revision = self.diagnostics_revision.wrapping_add(1);
    }
    pub(super) fn language_for(&self, path: &Path) -> Option<&LanguageSession> {
        self.languages
            .values()
            .find(|session| session.config.enabled && session.config.matches(path))
    }
    pub(super) fn language_document_version(&self, path: &Path) -> Option<&(u64, i32)> {
        self.language_for(path)?.opened.get(path)
    }
    /// Whether the combined diagnostics for `path` describe tab revision
    /// `current`: (fresh, revision they were computed for). Unknown is None;
    /// with several servers, any stale contribution makes the result stale.
    pub(super) fn diagnostics_freshness(
        &self,
        path: &Path,
        current: u64,
    ) -> (Option<bool>, Option<u64>) {
        let mut fresh = Some(true);
        let mut revision: Option<Option<u64>> = None;
        let mut any = false;
        for session in self.languages.values() {
            if !session.diagnostics.contains_key(path) {
                continue;
            }
            any = true;
            let (session_fresh, session_revision) = session
                .diagnostic_origins
                .get(path)
                .map_or((None, None), |origin| origin.freshness(current));
            fresh = match (fresh, session_fresh) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
            revision = Some(match (revision, session_revision) {
                (None, value) => value,
                (Some(Some(a)), Some(b)) => Some(a.min(b)),
                _ => None,
            });
        }
        if any {
            (fresh, revision.flatten())
        } else {
            (None, None)
        }
    }
    pub(super) fn language_status_json(&self) -> serde_json::Value {
        let mut sessions: Vec<_> = self
            .languages
            .values()
            .filter(|session| session.server.is_some() || session.error.is_some())
            .collect();
        sessions.sort_by_key(|session| &session.config.language_id);
        serde_json::json!(sessions.into_iter().map(|session| serde_json::json!({"language_id":session.config.language_id,"name":session.config.name,"ready":session.active(),"error":session.error,"program":session.effective_config.as_ref().map(|config|&config.program)})).collect::<Vec<_>>())
    }
    #[cfg(test)]
    pub(super) fn rust_language_mut(&mut self) -> &mut LanguageSession {
        self.languages
            .entry("rust".into())
            .or_insert_with(|| LanguageSession::new(language_settings::defaults().remove(0)))
    }
    #[cfg(test)]
    pub(super) fn rust_language(&self) -> &LanguageSession {
        self.languages.get("rust").expect("test Rust session")
    }
    pub(super) fn close_language_paths(&mut self, prefix: &Path) {
        for session in self.languages.values_mut() {
            let closed: std::collections::HashSet<_> = session
                .opened
                .keys()
                .chain(session.diagnostics.keys())
                .filter(|path| path.starts_with(prefix))
                .cloned()
                .collect();
            for path in closed {
                if let Some(server) = &session.server {
                    server.close(&path);
                }
                session.opened.remove(&path);
                session.diagnostics.remove(&path);
                session.diagnostic_origins.remove(&path);
                self.diagnostics.remove(&path);
                self.diagnostics_revision = self.diagnostics_revision.wrapping_add(1);
                session.retired_paths.insert(path);
            }
        }
    }
}
impl VEditApp {
    fn configured_language(&self, path: &Path) -> Option<&LanguageServerSettings> {
        self.settings
            .language_servers
            .iter()
            .find(|config| config.enabled && config.matches(path))
            .or_else(|| {
                self.settings
                    .language_servers
                    .iter()
                    .find(|config| config.matches(path))
            })
    }
    pub(super) fn restart_project_language(&mut self, project_index: usize, language: &str) {
        let Some(project) = self.projects.get_mut(project_index) else {
            return;
        };
        project.clear_language_diagnostics(language);
        project.languages.remove(language);
        if self.active_project == project_index {
            self.completions = None;
            self.completion_request = None;
            self.definition_request = None;
        }
        self.ctx.request_repaint();
    }
    pub(super) fn restart_project_languages(&mut self, project_index: usize) {
        let Some(project) = self.projects.get_mut(project_index) else {
            return;
        };
        for session in project.languages.values_mut() {
            session.server = None;
            session.ready = false;
            session.error = None;
            session.opened.clear();
            session.effective_config = None;
            session.diagnostics.clear();
            session.diagnostic_origins.clear();
        }
        project.diagnostics.clear();
        project.diagnostics_revision = project.diagnostics_revision.wrapping_add(1);
        if project_index == self.active_project {
            self.completions = None;
            self.completion_request = None;
            self.definition_request = None;
        }
        self.ctx.request_repaint();
    }
    pub(super) fn language_response_succeeded(&mut self, project_index: usize, language: &str) {
        let Some(session) = self.projects[project_index].languages.get_mut(language) else {
            return;
        };
        if !session.active() {
            return;
        }
        if let Some(error) = session.error.take()
            && self
                .status
                .as_ref()
                .is_some_and(|(message, is_error, _)| *is_error && message == &error)
        {
            self.status = None;
        }
    }
    pub(super) fn restart_active_language(&mut self) {
        if self.active_folder().is_none() {
            self.status("Open a project folder to use language analysis.", false);
            return;
        }
        let language = self
            .tab()
            .and_then(|tab| tab.path.as_deref())
            .and_then(|path| self.configured_language(path))
            .cloned();
        let Some(config) = language else {
            self.status(
                "Open a source file and configure its server in Settings → Languages",
                false,
            );
            return;
        };
        if !config.enabled {
            self.status(
                format!(
                    "{} analysis is disabled. Enable it in Settings → Languages.",
                    config.name
                ),
                false,
            );
            return;
        }
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return;
        };
        if config.language_id == "rust" && !project.rust_project {
            self.status(
                "Rust analysis requires Cargo.toml. Open its project folder first.",
                false,
            );
            return;
        }
        project.clear_language_diagnostics(&config.language_id);
        project.languages.insert(
            config.language_id.clone(),
            LanguageSession::new(config.clone()),
        );
        self.completions = None;
        self.completion_request = None;
        self.definition_request = None;
        self.status(format!("Restarting {} analysis…", config.name), false);
        self.ctx.request_repaint();
    }
    fn status_for_language(&self, config: &LanguageServerSettings) -> LanguageAnalysisStatus {
        let project = self.project();
        let session = project.and_then(|project| project.languages.get(&config.language_id));
        let (state, hint, color) = if !config.enabled {
            (
                "disabled",
                "Enable this language in Settings → Languages.".into(),
                MUTED,
            )
        } else if let Some(error) = session.and_then(|session| session.error.as_ref()) {
            (
                if session.is_some_and(LanguageSession::active) {
                    "warning"
                } else {
                    "unavailable"
                },
                format!("{error}\nUse Restart language analysis to reconnect."),
                ORANGE,
            )
        } else if session.is_some_and(|session| {
            session
                .server
                .as_ref()
                .is_some_and(|server| server.is_stopped())
        }) {
            (
                "stopped",
                "Use Restart language analysis to reconnect.".into(),
                ORANGE,
            )
        } else if session.is_some_and(LanguageSession::active) {
            ("ready", "Analysis connected. Request suggestions or go to definition from the editor. Click for Problems and language controls.".into(),GREEN)
        } else if config.language_id == "rust"
            && project.is_some_and(|project| !project.rust_project && !project.indexing)
        {
            (
                "unavailable",
                "Rust analysis requires Cargo.toml. Open its project folder first.".into(),
                ORANGE,
            )
        } else {
            ("loading","Language analysis is starting in the background. Click for Problems and language controls.".into(),MUTED)
        };
        LanguageAnalysisStatus {
            label: format!("{}: {state}", config.name),
            hint,
            color,
        }
    }
    // Retained name for the footer's existing integration point; all languages use it.
    pub(super) fn rust_analysis_status(&self) -> Option<LanguageAnalysisStatus> {
        let path = self.tab()?.path.as_deref()?;
        if let Some(config) = self.configured_language(path) {
            return Some(self.status_for_language(config));
        }
        Some(LanguageAnalysisStatus { label:"Analysis: not configured".into(), hint:"Syntax highlighting and editing work without a server. Add this language's server in Settings → Languages for diagnostics, completion and definitions.".into(),color:MUTED })
    }
    pub(super) fn rust_analysis_status_ui(&mut self, ui: &mut egui::Ui) {
        if self.project().is_some_and(Project::is_other_files) {
            return;
        }
        let Some(status) = self.rust_analysis_status() else {
            return;
        };
        if design::status_item(ui, &status.label, status.color, &status.hint).clicked() {
            self.output_visible = true;
            self.problems_visible = true;
        }
    }
    pub(super) fn language_request_unavailable(&mut self) {
        if self.project().is_some_and(Project::is_other_files) {
            self.status(
                "Open the file's project folder to use language analysis.",
                false,
            );
            return;
        }
        if let Some(status) = self.rust_analysis_status() {
            self.status(status.hint, status.color == ORANGE);
        } else {
            self.status(
                "Save the file with an extension and configure its server in Settings → Languages.",
                false,
            );
        }
    }
    pub(super) fn language_controls_ui(&mut self, ui: &mut egui::Ui) {
        if self.project().is_some_and(Project::is_other_files) {
            return;
        }
        let relevant: Vec<_> = self
            .settings
            .language_servers
            .iter()
            .filter(|config| {
                self.project().is_some_and(|project| {
                    project
                        .tabs
                        .iter()
                        .any(|tab| tab.path.as_deref().is_some_and(|path| config.matches(path)))
                        || project
                            .languages
                            .get(&config.language_id)
                            .is_some_and(|session| {
                                session.server.is_some() || session.error.is_some()
                            })
                })
            })
            .cloned()
            .collect();
        for config in relevant {
            let status = self.status_for_language(&config);
            let error = self
                .project()
                .and_then(|project| project.languages.get(&config.language_id))
                .and_then(|session| session.error.clone());
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(status.color, status.label);
                if let Some(error) = &error
                    && ui.button("Error details…").clicked()
                {
                    self.open_message_details(error.clone(), true);
                }
                if config.enabled
                    && ui
                        .button(format!("Restart {} analysis", config.name))
                        .clicked()
                {
                    if let Some(project) = self.projects.get_mut(self.active_project) {
                        project.clear_language_diagnostics(&config.language_id);
                        project.languages.insert(
                            config.language_id.clone(),
                            LanguageSession::new(config.clone()),
                        );
                    }
                    self.completions = None;
                    self.completion_request = None;
                    self.definition_request = None;
                    self.ctx.request_repaint();
                }
            });
            if let Some(error) = error {
                ui.add(
                    egui::Label::new(
                        RichText::new(error.lines().next().unwrap_or("Language analysis failed"))
                            .size(11.0)
                            .color(MUTED),
                    )
                    .truncate(),
                );
            } else {
                ui.label(RichText::new(status.hint).size(11.0).color(MUTED));
            }
        }
        if ui.button("Language settings…").clicked() {
            self.open_settings_section("Languages");
        }
        if self.project().is_some_and(|project| {
            project
                .languages
                .values()
                .any(|session| session.server.is_some())
        }) && ui.button("Restart all languages").clicked()
        {
            self.restart_project_languages(self.active_project);
        }
        ui.add_space(6.0);
    }
    pub(super) fn sync_languages(&mut self, force: bool) {
        for project in &mut self.projects {
            if project.is_other_files() {
                continue;
            }
            let removed: Vec<_> = project
                .languages
                .iter()
                .filter(|(id, session)| {
                    self.settings
                        .language_servers
                        .iter()
                        .find(|config| &config.language_id == *id)
                        .is_none_or(|config| !config.enabled || *config != session.config)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in removed {
                project.clear_language_diagnostics(&id);
                project.languages.remove(&id);
            }
            for config in &self.settings.language_servers {
                if !config.enabled {
                    continue;
                }
                let wanted = (config.language_id == "rust" && project.rust_project)
                    || (config.language_id != "rust"
                        && project.tabs.iter().any(|tab| {
                            tab.path.as_deref().is_some_and(|path| config.matches(path))
                        }));
                if !wanted {
                    continue;
                }
                let session = project
                    .languages
                    .entry(config.language_id.clone())
                    .or_insert_with(|| LanguageSession::new(config.clone()));
                if let Err(error) = config.validate(&self.settings.language_servers) {
                    session.error = Some(format!("Invalid language settings: {error}"));
                    continue;
                }
                if session.server.is_none() {
                    let effective =
                        config.resolve(&project.root, &project.tasks.python_program(&project.root));
                    session.server = Some(crate::lsp::LanguageServer::start_configured(
                        project.root.clone(),
                        effective.clone(),
                        Some(self.ctx.clone()),
                    ));
                    session.effective_config = Some(effective);
                }
            }
            let stopped: Vec<_> = project
                .languages
                .iter()
                .filter(|(_, session)| {
                    // A stopped server stays in the registry for its visible
                    // error. Clean up once, not on every subsequent UI frame:
                    // clearing diagnostics rebuilds the other sessions' lists.
                    (session.ready
                        || !session.opened.is_empty()
                        || !session.diagnostics.is_empty()
                        || session.error.is_none())
                        && session
                            .server
                            .as_ref()
                            .is_some_and(crate::lsp::LanguageServer::is_stopped)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for language in stopped {
                project.clear_language_diagnostics(&language);
                let session = project.languages.get_mut(&language).unwrap();
                session.ready = false;
                session.opened.clear();
                session.error.get_or_insert_with(|| {
                    format!(
                        "{} analysis stopped. Use Restart language analysis to reconnect.",
                        session.config.name
                    )
                });
            }
            for session in project.languages.values_mut() {
                if !session.ready {
                    continue;
                }
                let Some(server) = &session.server else {
                    continue;
                };
                let closed: Vec<_> = session
                    .opened
                    .keys()
                    .filter(|path| {
                        !project.tabs.iter().any(|tab| {
                            tab.path.as_ref() == Some(path) && session.config.matches(path)
                        })
                    })
                    .cloned()
                    .collect();
                for path in closed {
                    if server.close(&path) {
                        session.opened.remove(&path);
                    }
                }
                for tab in &project.tabs {
                    if tab.loading {
                        continue;
                    }
                    let Some(path) = tab
                        .path
                        .as_deref()
                        .filter(|path| session.config.matches(path))
                    else {
                        continue;
                    };
                    if let Some((revision, version)) = session.opened.get_mut(path) {
                        if *revision != tab.revision
                            && (force || tab.last_edit.elapsed() > Duration::from_millis(160))
                            && server.change(path, &tab.text, version.saturating_add(1))
                        {
                            *revision = tab.revision;
                            *version = version.saturating_add(1);
                        }
                    } else if server.open(path, &tab.text) {
                        session.retired_paths.remove(path);
                        session.opened.insert(path.to_owned(), (tab.revision, 1));
                    }
                }
            }
        }
    }
}
#[cfg(test)]
#[path = "language_support_tests.rs"]
mod tests;
