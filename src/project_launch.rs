//! Project application lifecycle and asynchronous, versioned launch configuration.
use super::*;
use crate::cli::{
    Response,
    launch::{self, Action, Configuration},
};
use serde_json::{Value, json};
use std::collections::VecDeque;

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
    pub output: OutputLog,
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
            output: OutputLog::default(),
            show_output: false,
            pending: None,
            refreshed: None,
            cache: Default::default(),
        }
    }
}

/// Rolling application log. Line numbers are absolute for the run: trimming
/// pops whole lines from the front and never renumbers the lines that remain.
pub(super) struct OutputLog {
    /// Complete lines, each ending with '\n'.
    lines: VecDeque<String>,
    /// The unterminated last line.
    partial: String,
    /// Bytes in `lines` and `partial`.
    bytes: usize,
    /// Absolute number of the first retained line, 1 for the run's first line.
    first: usize,
    /// Unicode scalars cut from the front of the first retained line after a
    /// single line exceeded the byte limit. Its columns stay absolute.
    first_column_offset: usize,
    truncated: bool,
    /// Joined text for the output view and copying, rebuilt only after changes.
    display: String,
    display_stale: bool,
}

impl Default for OutputLog {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            partial: String::new(),
            bytes: 0,
            first: 1,
            first_column_offset: 0,
            truncated: false,
            display: String::new(),
            display_stale: false,
        }
    }
}

impl OutputLog {
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Costs O(text + removed lines), independent of the retained size.
    pub fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut rest = text;
        while let Some(index) = rest.find('\n') {
            let (line, tail) = rest.split_at(index + 1);
            let mut complete = std::mem::take(&mut self.partial);
            complete.push_str(line);
            self.bytes += line.len();
            self.lines.push_back(complete);
            if self.lines.len() > tasks::MAX_OUTPUT_LINES {
                self.pop_front();
            }
            rest = tail;
        }
        self.partial.push_str(rest);
        self.bytes += rest.len();
        while self.bytes > tasks::MAX_OUTPUT_BYTES && self.line_count() > 1 {
            self.pop_front();
        }
        if self.bytes > tasks::MAX_OUTPUT_BYTES {
            // One line exceeds the limit on its own: keep its latest tail.
            let line = match self.lines.front_mut() {
                Some(line) => line,
                None => &mut self.partial,
            };
            let mut cut = self.bytes - tasks::MAX_OUTPUT_BYTES;
            while !line.is_char_boundary(cut) {
                cut += 1;
            }
            self.first_column_offset += line[..cut].chars().count();
            line.drain(..cut);
            self.bytes -= cut;
            self.truncated = true;
        }
        self.display_stale = true;
    }

    fn pop_front(&mut self) {
        if let Some(line) = self.lines.pop_front() {
            self.bytes -= line.len();
            self.first += 1;
            self.first_column_offset = 0;
            self.truncated = true;
        }
    }

    /// Retained lines, counting a nonempty unterminated last line.
    fn line_count(&self) -> usize {
        self.lines.len() + usize::from(!self.partial.is_empty())
    }

    /// Absolute number of the last line; an empty log has the empty line `first`.
    fn last_line(&self) -> usize {
        self.first + self.line_count().max(1) - 1
    }

    /// The retained text as one string, joined at most once per change.
    pub fn text(&mut self) -> &str {
        if self.display_stale {
            self.display.clear();
            self.display.reserve(self.bytes);
            for line in &self.lines {
                self.display.push_str(line);
            }
            self.display.push_str(&self.partial);
            self.display_stale = false;
        }
        &self.display
    }

    /// Page with absolute line numbers that stay valid while old lines are
    /// trimmed. A start before the first retained line begins there and
    /// reports the dropped lines; one past the last line is an empty page.
    pub fn page(
        &self,
        start_line: usize,
        start_column: usize,
        line_count: usize,
        budget: usize,
    ) -> Result<Value, String> {
        let last = self.last_line();
        if start_line == 0 || start_column == 0 || start_line > last + 1 {
            return Err(format!(
                "Output has lines {}..{last}; requested line {start_line}, column {start_column}. Read from line {} for new output. Nothing changed.",
                self.first,
                last + 1
            ));
        }
        if start_line == last + 1 {
            if start_column != 1 {
                return Err(format!(
                    "Line {start_line} has no output yet; use column 1. Nothing changed."
                ));
            }
            return Ok(
                json!({"text":"","start_line":start_line,"start_column":1,"end_line":last,
                "total_lines":last,"first_line":self.first,"skipped_lines":0,"truncated":false,
                "byte_limit_reached":false,"next_line":null,"next_column":null}),
            );
        }
        let skipped = self.first.saturating_sub(start_line);
        let line = start_line.max(self.first);
        let index = line - self.first;
        // Columns on a line whose head was trimmed remain absolute.
        let offset = if index == 0 {
            self.first_column_offset
        } else {
            0
        };
        let column = if skipped > 0 || start_column <= offset {
            offset + 1
        } else {
            start_column
        };
        let mut retained = self
            .lines
            .range(index.min(self.lines.len())..)
            .map(String::as_str)
            .chain((!self.partial.is_empty()).then_some(self.partial.as_str()));
        let first = retained.next().unwrap_or("");
        let content = first
            .strip_suffix('\n')
            .map_or(first, |line| line.strip_suffix('\r').unwrap_or(line));
        let length = offset + content.chars().count();
        if column > length + 1 {
            return Err(format!(
                "Line {line} has {length} characters; column {column} is outside it. Nothing changed."
            ));
        }
        // One extra line lets text_page report whether more lines follow.
        let count = line_count.max(1);
        let mut text = first.to_owned();
        for next in retained.take(count) {
            text.push_str(next);
        }
        let mut value = control::text_page(&text, 1, column - offset, count, budget, true)?;
        if offset > 0
            && value["next_line"].as_u64() == Some(1)
            && let Some(next) = value["next_column"].as_u64()
        {
            value["next_column"] = json!(next as usize + offset);
        }
        for key in ["start_line", "end_line", "next_line"] {
            if let Some(number) = value[key].as_u64() {
                value[key] = json!(number as usize + line - 1);
            }
        }
        value["start_column"] = json!(column);
        value["total_lines"] = json!(last);
        value["first_line"] = json!(self.first);
        value["skipped_lines"] = json!(skipped);
        Ok(value)
    }

    #[cfg(test)]
    fn joined(&self) -> String {
        self.lines
            .iter()
            .map(String::as_str)
            .chain([self.partial.as_str()])
            .collect()
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
        // The periodic re-read usually finds the same file. Applying that
        // result can wait for the next scheduled frame instead of waking an
        // idle window every few seconds.
        let known =
            (reply.is_none() && matches!(action, Action::List) && self.config_error.is_none())
                .then_some(self.revision);
        if let Err(error) = std::thread::Builder::new()
            .name("vedit-launch-config".into())
            .spawn(move || {
                let result = read_or_configure(&root, &worker_action);
                let unchanged = known.is_some_and(|revision| {
                    matches!(&result, Ok(Loaded { revision: loaded, configuration: Ok(_) }) if *loaded == revision)
                });
                let _ = sender.send(result);
                if !unchanged {
                    ctx.request_repaint();
                }
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
                Ok(tasks::TaskEvent::Output(text)) => self.output.append(&text),
                Ok(tasks::TaskEvent::Error(error)) => {
                    self.output.append(&format!("\n{error}\n"));
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
        let (index, source) = match self.control_project_index(workspace.as_deref()) {
            Ok(found) => found,
            Err(response) => {
                let _ = reply.send(response);
                return true;
            }
        };
        let reply = control::reply_naming_workspace(
            reply,
            self.projects[index].root.clone(),
            source,
            false,
        );
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
            let mut value = match state
                .output
                .page(start_line, start_column, line_count, 48 * 1024)
            {
                Ok(value) => value,
                Err(error) => return Some(Response::failure("invalid_location", error)),
            };
            value["run"] = state.snapshot(&project.root);
            value["output_truncated"] = json!(state.output.truncated());
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
            // A periodic re-read wakes the window itself when it found a
            // change, so only requests that someone waits for are polled.
            let awaited = project
                .launch
                .pending
                .as_ref()
                .is_some_and(|p| p.reply.is_some() || !matches!(p.action, Action::List));
            if project.launch.task.is_some() || awaited {
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
            // A new run starts again at line 1.
            state.output = OutputLog::default();
            state.output.append(&format!(
                "{} · {}\n> {}\nRuns saved files on disk.\n\n",
                profile.name,
                crate::paths::display(&cwd),
                command.display()
            ));
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
                ui.ctx().copy_text(state.output.text().to_owned());
            }
        });
        if let Some(error) = &state.error {
            ui.colored_label(ORANGE, error);
        }
        if state.output.truncated() {
            ui.label("Showing the latest 1 MiB of output.");
        }
        egui::ScrollArea::both()
            .id_salt(("application_output", &project.root))
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                state.cache.show(ui, state.output.text(), TEXT);
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
