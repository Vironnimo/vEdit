//! Project application lifecycle and asynchronous, versioned launch configuration.
use super::*;
use crate::cli::{
    Response,
    launch::{self, Action, Configuration},
};
use serde_json::{Value, json};

pub(super) struct LaunchState {
    pub configuration: Configuration,
    pub selected: Option<String>,
    pub revision: u64,
    pub config_error: Option<String>,
    pub task: Option<tasks::RunningTask>,
    pub running_id: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub exit_code: Option<i32>,
    pub output: String,
    pub output_truncated: bool,
    pub show_output: bool,
    pending: Option<Pending>,
    refreshed: Option<Instant>,
    cache: task_output_ui::TaskOutputCache,
}

impl Default for LaunchState {
    fn default() -> Self {
        Self {
            configuration: Configuration::default(),
            selected: None,
            revision: 0,
            config_error: None,
            task: None,
            running_id: None,
            status: "Ready".into(),
            error: None,
            exit_code: None,
            output: String::new(),
            output_truncated: false,
            show_output: false,
            pending: None,
            refreshed: None,
            cache: Default::default(),
        }
    }
}

struct Pending {
    receiver: mpsc::Receiver<Result<Loaded, String>>,
    action: Action,
    reply: Option<mpsc::Sender<Response>>,
}

struct Loaded {
    configuration: Result<Configuration, String>,
    revision: u64,
}

/// JavaScript and jq round integers above 2^53, so revisions stay below it.
const REVISION_MASK: u64 = (1 << 53) - 1;

/// Zero is reserved for "no configuration file".
fn configuration_revision(hash: u64) -> u64 {
    (hash & REVISION_MASK).max(1)
}

fn read_or_configure(root: &Path, action: &Action) -> Result<Loaded, String> {
    let path = root.join(launch::CONFIG_PATH);
    let existing = match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Could not inspect {}: {error}", path.display())),
        Ok(_) => Some(workspace::read_document(&path, launch::MAX_CONFIG_BYTES)?),
    };
    let revision = existing
        .as_ref()
        .map_or(0, |doc| configuration_revision(doc.fingerprint.hash));
    if let Action::Configure {
        configuration,
        revision: expected,
    } = action
    {
        configuration.validate()?;
        // Earlier builds issued the unmasked 64-bit hash; keep accepting it.
        let expected = if *expected > REVISION_MASK {
            configuration_revision(*expected)
        } else {
            *expected
        };
        if expected != revision {
            return Err(format!(
                "Launch configuration changed (current revision {revision}). Nothing was written. Use launch list, merge the profiles, then configure with --revision {revision}."
            ));
        }
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(configuration).map_err(|e| e.to_string())? + "\n";
        let saved = workspace::save_document(
            &path,
            &text,
            workspace::LineEnding::Lf,
            false,
            existing.as_ref().map(|doc| &doc.fingerprint),
            None,
        )
        .map_err(|e| format!("Could not save launch configuration: {e}"))?;
        return Ok(Loaded {
            configuration: Ok(configuration.clone()),
            revision: configuration_revision(saved.fingerprint.hash),
        });
    }
    let configuration = existing.map_or_else(
        || Ok(Configuration::default()),
        |doc| {
            launch::decode(doc.text.as_bytes())
                .map_err(|e| format!("Invalid {}: {e}", launch::CONFIG_PATH))
                .and_then(|config| {
                    config.validate()?;
                    Ok(config)
                })
        },
    );
    Ok(Loaded {
        configuration,
        revision,
    })
}

impl LaunchState {
    fn begin(
        &mut self,
        root: PathBuf,
        action: Action,
        reply: Option<mpsc::Sender<Response>>,
        ctx: egui::Context,
    ) {
        let (sender, receiver) = mpsc::channel();
        let worker_action = action.clone();
        let fallback = sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("vedit-launch-config".into())
            .spawn(move || {
                let result = read_or_configure(&root, &worker_action);
                let _ = sender.send(result);
                ctx.request_repaint();
            })
        {
            let _ = fallback.send(Err(format!(
                "Could not start configuration worker: {error}"
            )));
        }
        self.pending = Some(Pending {
            receiver,
            action,
            reply,
        });
    }

    fn snapshot(&self, root: &Path) -> Value {
        json!({"workspace":crate::paths::display(root), "configuration_path":crate::paths::display(&root.join(launch::CONFIG_PATH)),
            "configuration":self.configuration,"revision":self.revision,"configuration_error":self.config_error,
            "selected":self.configuration.selected(self.selected.as_deref()).map(|p| &p.id),
            "running":self.task.is_some(),"running_id":self.running_id,"status":self.status,
            "exit_code":self.exit_code,"error":self.error,"pending":self.pending.is_some()})
    }

    fn append(&mut self, text: &str) {
        self.output.push_str(text);
        if self.output.len() > tasks::MAX_OUTPUT_BYTES {
            let mut cut = self.output.len() - tasks::MAX_OUTPUT_BYTES;
            while !self.output.is_char_boundary(cut) {
                cut += 1;
            }
            self.output.drain(..cut);
            self.output_truncated = true;
        }
        // Keep line layout bounded for streams of very short lines as well.
        if let Some((cut, _)) = self
            .output
            .match_indices('\n')
            .rev()
            .nth(tasks::MAX_OUTPUT_LINES)
        {
            self.output.drain(..cut + 1);
            self.output_truncated = true;
        }
    }

    fn poll_task(&mut self) {
        // Bound work per frame even for a continuously writing process.
        for _ in 0..32 {
            let Some(task) = &self.task else {
                break;
            };
            match task.receiver.try_recv() {
                Ok(tasks::TaskEvent::Started) => {
                    if self.status != "Stopping" {
                        self.status = "Running".into();
                    }
                }
                Ok(tasks::TaskEvent::Output(text)) => self.append(&text),
                Ok(tasks::TaskEvent::Error(error)) => {
                    self.append(&format!("\n{error}\n"));
                    self.error = Some(error);
                }
                Ok(tasks::TaskEvent::Finished { code, cancelled }) => {
                    self.exit_code = code;
                    self.status = if cancelled {
                        "Stopped".into()
                    } else if self.error.is_some() || code != Some(0) {
                        "Failed".into()
                    } else {
                        "Completed".into()
                    };
                    self.task = None;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Application worker stopped unexpectedly.".into());
                    self.status = "Failed".into();
                    self.task = None;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "project_launch_tests.rs"]
mod tests;

impl VEditApp {
    pub(super) fn control_launch_async(
        &mut self,
        request: &crate::cli::Request,
        reply: mpsc::Sender<Response>,
    ) -> bool {
        let crate::cli::Request::Launch { workspace, action } = request else {
            return false;
        };
        let index = match self.control_query_project(workspace.as_deref()) {
            Ok(project) => self
                .projects
                .iter()
                .position(|p| p.root == project.root)
                .unwrap(),
            Err(response) => {
                let _ = reply.send(response);
                return true;
            }
        };
        self.begin_launch_action(index, action.clone(), Some(reply));
        true
    }

    fn begin_launch_action(
        &mut self,
        index: usize,
        action: Action,
        reply: Option<mpsc::Sender<Response>>,
    ) {
        let result = self.launch_action(index, action, reply.clone());
        if let Some(result) = result {
            if let Some(reply) = reply {
                let _ = reply.send(result);
            } else if !result.ok {
                self.status(result.error.map(|e| e.message).unwrap_or_default(), true);
            }
        }
    }

    fn launch_action(
        &mut self,
        index: usize,
        action: Action,
        reply: Option<mpsc::Sender<Response>>,
    ) -> Option<Response> {
        if !matches!(action, Action::List | Action::Output { .. })
            && let Some(dialog) = self.control_dialog()
        {
            return Some(Response::failure(
                "user_dialog_open",
                format!("Finish the {dialog} interaction before changing application state."),
            ));
        }
        let project = &mut self.projects[index];
        let state = &mut project.launch;
        if let Action::Output {
            start_line,
            start_column,
            line_count,
        } = action
        {
            let mut value = match control::text_page(
                &state.output,
                start_line,
                start_column,
                line_count,
                48 * 1024,
                true,
            ) {
                Ok(value) => value,
                Err(error) => return Some(Response::failure("invalid_location", error)),
            };
            value["run"] = state.snapshot(&project.root);
            value["output_truncated"] = json!(state.output_truncated);
            value["snapshot"] = json!(true);
            return Some(Response::success(value));
        }
        if matches!(action, Action::Stop) {
            if state
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.action, Action::Start { .. }))
            {
                if let Some(pending) = state.pending.take()
                    && let Some(reply) = pending.reply
                {
                    let _ = reply.send(Response::failure(
                        "launch_cancelled",
                        "Application start was cancelled before spawning.",
                    ));
                }
                state.status = "Stopped".into();
            }
            if let Some(task) = &state.task {
                task.cancel();
                state.status = "Stopping".into();
            }
            return Some(Response::success(state.snapshot(&project.root)));
        }
        if matches!(action, Action::Start { .. }) && state.task.is_some() {
            return Some(Response::failure(
                "launch_running",
                format!(
                    "{} is already running for this project. Use launch stop and wait for output.run.running:false before starting again.",
                    state.running_id.as_deref().unwrap_or("An application")
                ),
            ));
        }
        if matches!(action, Action::Configure { .. })
            && project.tabs.iter().any(|t| {
                t.path.as_ref().is_some_and(|p| {
                    control::same_workspace(p, &project.root.join(launch::CONFIG_PATH))
                }) && (t.dirty || t.saving || t.agent_unsaved)
            })
        {
            return Some(Response::failure(
                "unsaved_configuration",
                "Save or discard the open launch configuration draft before replacing it.",
            ));
        }
        if state
            .pending
            .as_ref()
            .is_some_and(|p| p.reply.is_some() || !matches!(p.action, Action::List))
        {
            return Some(Response::failure(
                "launch_busy",
                "This project's launch configuration is being processed. Retry after it finishes.",
            ));
        }
        state.begin(project.root.clone(), action, reply, self.ctx.clone());
        None
    }

    pub(super) fn poll_launches(&mut self) {
        for index in 0..self.projects.len() {
            self.projects[index].launch.poll_task();
            let ready = self.projects[index].launch.pending.as_ref().and_then(|p| {
                match p.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Configuration worker stopped unexpectedly.".into()))
                    }
                }
            });
            if let Some(loaded) = ready {
                let pending = self.projects[index].launch.pending.take().unwrap();
                self.projects[index].launch.refreshed = Some(Instant::now());
                let response = self.apply_launch_loaded(index, &pending.action, loaded);
                if let Some(reply) = pending.reply {
                    let _ = reply.send(response);
                } else if !response.ok && !matches!(pending.action, Action::List) {
                    self.status(response.error.map(|e| e.message).unwrap_or_default(), true);
                }
            }
            let project = &mut self.projects[index];
            if project.launch.pending.is_none()
                && project
                    .launch
                    .refreshed
                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(3))
            {
                project
                    .launch
                    .begin(project.root.clone(), Action::List, None, self.ctx.clone());
            }
            if project.launch.task.is_some() || project.launch.pending.is_some() {
                self.ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        if !self.projects.is_empty() {
            self.ctx.request_repaint_after(Duration::from_secs(3));
        }
    }

    fn apply_launch_loaded(
        &mut self,
        index: usize,
        action: &Action,
        loaded: Result<Loaded, String>,
    ) -> Response {
        let project = &mut self.projects[index];
        let state = &mut project.launch;
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                if !matches!(action, Action::Configure { .. }) {
                    state.config_error = Some(error.clone());
                }
                return Response::failure("launch_configuration", error);
            }
        };
        state.revision = loaded.revision;
        match loaded.configuration {
            Ok(config) => {
                state.configuration = config;
                state.config_error = None;
            }
            Err(error) => {
                state.config_error = Some(error.clone());
                let mut response = Response::failure("launch_configuration", error);
                response.result = state.snapshot(&project.root);
                return response;
            }
        }
        let id = match action {
            Action::Select { id } | Action::Start { id: Some(id) } => Some(id.as_str()),
            _ => None,
        };
        if let Some(id) = id
            && !state.configuration.profiles.iter().any(|p| p.id == id)
        {
            return Response::failure(
                "unknown_launch",
                format!(
                    "No launch profile {id}. Available IDs: {}. Nothing started.",
                    state
                        .configuration
                        .profiles
                        .iter()
                        .map(|p| p.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        if let Action::Select { id } = action {
            state.selected = Some(id.clone());
            self.session_dirty = true;
        }
        if matches!(action, Action::Start { .. }) {
            // Recheck after asynchronous disk I/O; edits may have arrived meanwhile.
            if project
                .tabs
                .iter()
                .any(|t| t.dirty || t.saving || t.agent_unsaved || t.loading)
            {
                return Response::failure(
                    "unsaved_project",
                    "Save project buffers and wait for loading/saving to finish before starting. Agent drafts remain unsaved.",
                );
            }
            if self.close_action.is_some() {
                return Response::failure(
                    "closing",
                    "The editor is closing; no application was started.",
                );
            }
            let Some(profile) = state
                .configuration
                .selected(id.or(state.selected.as_deref()))
                .cloned()
            else {
                return Response::failure(
                    "no_launch_profiles",
                    "No launch options configured. Use launch configure --file CONFIG.json; see launch --help.",
                );
            };
            let root = &project.root;
            let program_path = Path::new(&profile.program);
            let program = if program_path.is_relative()
                && (profile.program.contains(['/', '\\']) || profile.program.starts_with('.'))
            {
                root.join(program_path).into_os_string()
            } else {
                profile.program.into()
            };
            let cwd = root.join(&profile.cwd);
            let command = tasks::CommandSpec::Program {
                program,
                args: profile.args.into_iter().map(Into::into).collect(),
            };
            let mut environment: Vec<_> = profile
                .env
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect();
            environment.push(("VEDIT_ROOT".into(), root.as_os_str().into()));
            state.output = format!(
                "{} · {}\n> {}\nRuns saved files on disk.\n\n",
                profile.name,
                crate::paths::display(&cwd),
                command.display()
            );
            state.output_truncated = false;
            state.status = "Starting".into();
            state.error = None;
            state.exit_code = None;
            state.running_id = Some(profile.id.clone());
            state.selected = Some(profile.id);
            state.task = Some(tasks::spawn_application(cwd, command, environment));
            state.show_output = true;
            self.session_dirty = true;
            if index == self.active_project {
                self.output_visible = true;
                self.problems_visible = false;
            }
        }
        Response::success(state.snapshot(&project.root))
    }

    pub(super) fn launch_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.project() else {
            return;
        };
        if project.launch.configuration.profiles.is_empty()
            && project.launch.config_error.is_none()
            && project.launch.task.is_none()
        {
            return;
        }
        egui::Panel::top("project_launch_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(12, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.launch_controls(ui);
                });
            });
    }

    fn launch_controls(&mut self, ui: &mut egui::Ui) {
        let index = self.active_project;
        let project = &self.projects[index];
        let state = &project.launch;
        let running = state.task.is_some();
        let pending = state
            .pending
            .as_ref()
            .is_some_and(|p| !matches!(p.action, Action::List));
        let available = !state.configuration.profiles.is_empty() && state.config_error.is_none();
        let mut action = None;
        if ui
            .add_enabled(
                available && !running && !pending,
                egui::Button::new("▶ Run").frame_when_inactive(false),
            )
            .on_hover_text("Start the selected application using saved project files")
            .clicked()
        {
            action = Some(Action::Start { id: None });
        }
        let selected = state.configuration.selected(state.selected.as_deref());
        let mut selected_id = selected.map(|p| p.id.clone());
        let choice = egui::ComboBox::from_id_salt(("launch_profile", &project.root))
            .width(ui.available_width().clamp(60.0, 180.0))
            .selected_text(selected.map_or("Launch options", |p| &p.name))
            .show_ui(ui, |ui| {
                for profile in &state.configuration.profiles {
                    if ui
                        .selectable_value(&mut selected_id, Some(profile.id.clone()), &profile.name)
                        .clicked()
                    {
                        action = Some(Action::Select {
                            id: profile.id.clone(),
                        });
                    }
                }
            });
        choice.response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(
                egui::WidgetType::ComboBox,
                ui.is_enabled(),
                "Launch option",
            );
            info.current_text_value = selected.map(|p| p.name.clone());
            info
        });
        if (running || pending)
            && ui
                .button("■ Stop")
                .on_hover_text("Stop this project's application and its child processes")
                .clicked()
        {
            action = Some(Action::Stop);
        }
        if ui.available_width() > 90.0
            && design::ghost_button(
                ui,
                "Application output",
                "Show this project's application log",
            )
            .clicked()
        {
            self.projects[index].launch.show_output = true;
            self.output_visible = true;
            self.problems_visible = false;
        }
        let state = &self.projects[index].launch;
        if ui.available_width() > 80.0 && (state.task.is_some() || state.error.is_some()) {
            ui.label(
                RichText::new(&state.status)
                    .size(11.0)
                    .color(if state.error.is_some() { ORANGE } else { MUTED }),
            );
        }
        if let Some(error) = &self.projects[index].launch.config_error {
            ui.colored_label(ORANGE, "Configuration error")
                .on_hover_text(error);
        }
        if let Some(action) = action {
            self.begin_launch_action(index, action, None);
        }
    }

    pub(super) fn launch_output_tab(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return;
        };
        if project.launch.configuration.profiles.is_empty() && project.launch.output.is_empty() {
            return;
        }
        if ui
            .selectable_label(
                project.launch.show_output && !self.problems_visible,
                "Application",
            )
            .clicked()
        {
            project.launch.show_output = true;
            self.problems_visible = false;
        }
    }

    pub(super) fn launch_output_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return false;
        };
        if self.problems_visible || !project.launch.show_output {
            return false;
        }
        let state = &mut project.launch;
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} · {}",
                state.running_id.as_deref().unwrap_or("Application"),
                state.status
            ));
            if let Some(code) = state.exit_code {
                ui.label(format!("Exit {code}"));
            }
            if let Some(task) = &state.task
                && ui.button("Stop application").clicked()
            {
                task.cancel();
                state.status = "Stopping".into();
            }
            if ui.button("Copy application output").clicked() {
                ui.ctx().copy_text(state.output.clone());
            }
        });
        if let Some(error) = &state.error {
            ui.colored_label(ORANGE, error);
        }
        if state.output_truncated {
            ui.label("Showing the latest 1 MiB of output.");
        }
        egui::ScrollArea::both()
            .id_salt(("application_output", &project.root))
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                state.cache.show(ui, &state.output, TEXT);
            });
        true
    }

    pub(super) fn run_selected_application(&mut self) {
        if !self.projects.is_empty() {
            self.begin_launch_action(self.active_project, Action::Start { id: None }, None);
        }
    }

    pub(super) fn stop_application(&mut self) {
        if !self.projects.is_empty() {
            self.begin_launch_action(self.active_project, Action::Stop, None);
        }
    }
}
