//! Project task profiles. Built-in commands never interpolate a file path into shell source.
use super::*;
use std::ffi::OsString;

// Compile source directly: compileall only accepts lowercase .py and may
// reuse a same-second pyc cache, hiding newly introduced syntax errors.
const PYTHON_SYNTAX_CHECK: &str = r#"import os, sys, tokenize, traceback
failed = False
checked = 0
excluded = {'.venv', 'venv', 'env', '.git', '.hg', '.svn', 'node_modules'}
def report(error):
    global failed
    failed = True
    print(str(error), file=sys.stderr)
def linked(path):
    return os.path.islink(path) or getattr(os.path, 'isjunction', lambda p: False)(path)
for directory, folders, files in os.walk('.', onerror=report, followlinks=False):
    folders[:] = sorted(name for name in folders if name.casefold() not in excluded and not linked(os.path.join(directory, name)))
    for name in sorted(files):
        if os.path.splitext(name)[1].casefold() not in {'.py', '.pyw', '.pyi'}:
            continue
        path = os.path.join(directory, name)
        if linked(path):
            continue
        checked += 1
        try:
            with tokenize.open(path) as source:
                compile(source.read(), path, 'exec', dont_inherit=True)
        except SyntaxError as error:
            failed = True
            sys.stderr.write(''.join(traceback.format_exception_only(type(error), error)))
        except (OSError, UnicodeError, ValueError) as error:
            report(str(path) + ': ' + str(error))
print('Python syntax check: ' + str(checked) + ' file(s), ' + ('failed' if failed else 'passed'))
sys.exit(1 if failed else 0)
"#;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TaskProfile {
    #[default]
    Auto,
    Python,
    Dart,
    Rust,
    Custom,
}

impl TaskProfile {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic (active file, then project)",
            Self::Python => "Python",
            Self::Dart => "Dart",
            Self::Rust => "Rust",
            Self::Custom => "Custom commands",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct TaskConfiguration {
    pub(super) profile: TaskProfile,
    pub(super) python_interpreter: String,
    pub(super) dart_executable: String,
    pub(super) check: String,
    pub(super) test: String,
    pub(super) build: String,
    pub(super) run_file: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProjectTaskAction {
    Check,
    Test,
    Build,
    RunFile,
}

pub(super) struct ResolvedTask {
    pub(super) root: PathBuf,
    pub(super) command: tasks::CommandSpec,
    pub(super) environment: Vec<(OsString, OsString)>,
}

impl ResolvedTask {
    fn display(&self) -> String {
        if let tasks::CommandSpec::Program { program, args } = &self.command
            && args.len() == 2
            && args[0] == "-c"
            && args[1] == PYTHON_SYNTAX_CHECK
        {
            format!(
                "{} · Python syntax check (.py, .pyw, .pyi)",
                program.to_string_lossy()
            )
        } else {
            self.command.display()
        }
    }
}

impl TaskConfiguration {
    /// An explicit relative interpreter path is relative to the project, not
    /// the editor process. A plain executable name uses PATH.
    pub(super) fn python_program(&self, root: &Path) -> String {
        let configured = self.python_interpreter.trim();
        if !configured.is_empty() {
            let path = Path::new(configured);
            return if !path.is_absolute() && (configured.contains('/') || configured.contains('\\'))
            {
                root.join(path).to_string_lossy().into_owned()
            } else {
                configured.into()
            };
        }
        for directory in [".venv", "venv", "env"] {
            let candidate = root.join(directory).join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            });
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
        if cfg!(windows) { "python" } else { "python3" }.into()
    }

    fn dart_program(&self, root: &Path) -> String {
        let configured = self.dart_executable.trim();
        if configured.is_empty() {
            return "dart".into();
        }
        let path = Path::new(configured);
        if !path.is_absolute() && (configured.contains('/') || configured.contains('\\')) {
            root.join(path).to_string_lossy().into_owned()
        } else {
            configured.into()
        }
    }

    fn effective_profile(&self, root: &Path, file: Option<&Path>) -> TaskProfile {
        if self.profile != TaskProfile::Auto {
            return self.profile;
        }
        if let Some(extension) = file
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str())
        {
            match extension.to_ascii_lowercase().as_str() {
                "py" | "pyw" | "pyi" => return TaskProfile::Python,
                "dart" => return TaskProfile::Dart,
                "rs" => return TaskProfile::Rust,
                _ => {}
            }
        }
        if root.join("pubspec.yaml").is_file() {
            TaskProfile::Dart
        } else if [
            "pyproject.toml",
            "setup.py",
            "requirements.txt",
            "Pipfile",
            ".python-version",
        ]
        .iter()
        .any(|name| root.join(name).is_file())
            || [".venv", "venv"]
                .iter()
                .any(|name| root.join(name).is_dir())
        {
            TaskProfile::Python
        } else if root.join("Cargo.toml").is_file() {
            TaskProfile::Rust
        } else {
            TaskProfile::Custom
        }
    }

    fn custom_command(&self, action: ProjectTaskAction) -> &str {
        match action {
            ProjectTaskAction::Check => &self.check,
            ProjectTaskAction::Test => &self.test,
            ProjectTaskAction::Build => &self.build,
            ProjectTaskAction::RunFile => &self.run_file,
        }
    }

    pub(super) fn environment(
        &self,
        root: &Path,
        file: Option<&Path>,
    ) -> Vec<(OsString, OsString)> {
        let python_program = self.python_program(root);
        let mut environment: Vec<(OsString, OsString)> = vec![
            ("VEDIT_ROOT".into(), root.as_os_str().into()),
            (
                "VEDIT_FILE".into(),
                file.map_or_else(OsString::new, |file| file.as_os_str().into()),
            ),
            ("VEDIT_PYTHON".into(), python_program.clone().into()),
            // Python pipes otherwise buffer prints for long-running programs.
            ("PYTHONUNBUFFERED".into(), "1".into()),
            ("PYTHONIOENCODING".into(), "utf-8".into()),
        ];
        // Keep saved Morrow task commands working with the renamed variables.
        for (name, value) in environment.clone() {
            if let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix("VEDIT_")) {
                environment.push((format!("MORROW_{suffix}").into(), value));
            }
        }
        let mut executable_directories = Vec::new();
        if let Some(bin) = Path::new(&python_program).parent()
            && !bin.as_os_str().is_empty()
        {
            executable_directories.push(bin.to_owned());
            // System Python keeps pip/pytest entry points beside its Scripts
            // directory, whereas a venv already keeps python.exe in Scripts.
            #[cfg(windows)]
            if !bin
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("Scripts"))
                && bin.join("Scripts").is_dir()
            {
                executable_directories.push(bin.join("Scripts"));
            }
            if let Some(venv) = bin.parent()
                && venv.join("pyvenv.cfg").is_file()
            {
                environment.push(("VIRTUAL_ENV".into(), venv.as_os_str().into()));
            } else if !self.python_interpreter.trim().is_empty() {
                environment.push(("VIRTUAL_ENV".into(), OsString::new()));
            }
        }
        if !self.dart_executable.trim().is_empty() {
            let (program, _) =
                language_settings::executable_command(&self.dart_program(root), Vec::new(), root);
            if let Some(bin) = program.parent()
                && !bin.as_os_str().is_empty()
            {
                executable_directories.push(bin.to_owned());
            }
        }
        if !executable_directories.is_empty() {
            let current_path = std::env::var_os("PATH").unwrap_or_default();
            if let Ok(path) = std::env::join_paths(
                executable_directories
                    .into_iter()
                    .chain(std::env::split_paths(&current_path)),
            ) {
                environment.push(("PATH".into(), path));
            }
        }
        environment
    }

    pub(super) fn resolve(
        &self,
        root: &Path,
        file: Option<&Path>,
        action: ProjectTaskAction,
    ) -> Result<ResolvedTask, String> {
        let file_required = || {
            file.filter(|file| file.is_file())
                .ok_or_else(|| "Save and select a source file before running it.".to_owned())
        };
        if action == ProjectTaskAction::RunFile {
            file_required()?;
        }
        let environment = self.environment(root, file);
        let custom = self.custom_command(action).trim();
        let command = if !custom.is_empty() {
            tasks::CommandSpec::Shell(custom.into())
        } else {
            let profile = self.effective_profile(root, file);
            if action == ProjectTaskAction::RunFile
                || (profile == TaskProfile::Dart && action == ProjectTaskAction::Build)
            {
                let extension = file
                    .and_then(Path::extension)
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let supported = match profile {
                    TaskProfile::Python => matches!(extension.as_str(), "py" | "pyw"),
                    TaskProfile::Dart => extension == "dart",
                    TaskProfile::Rust => extension == "rs",
                    _ => false,
                };
                if !supported {
                    return Err("Select a Python, Dart or Rust source file, or configure a custom Run command for this language in Settings → Tasks.".into());
                }
            }
            let (program, args): (OsString, Vec<OsString>) = match (profile, action) {
                (TaskProfile::Python, ProjectTaskAction::RunFile) =>
                    (self.python_program(root).into(), vec!["-u".into(), file_required()?.as_os_str().into()]),
                (TaskProfile::Python, ProjectTaskAction::Check) =>
                    (self.python_program(root).into(), vec!["-c".into(), PYTHON_SYNTAX_CHECK.into()]),
                (TaskProfile::Python, ProjectTaskAction::Test) =>
                    (self.python_program(root).into(), vec!["-m".into(), "pytest".into()]),
                (TaskProfile::Python, ProjectTaskAction::Build) =>
                    (self.python_program(root).into(), vec!["-m".into(), "build".into()]),
                (TaskProfile::Dart, ProjectTaskAction::Check) => ("dart".into(), vec!["analyze".into()]),
                (TaskProfile::Dart, ProjectTaskAction::Test) => ("dart".into(), vec!["test".into()]),
                (TaskProfile::Dart, ProjectTaskAction::Build) =>
                    ("dart".into(), vec!["compile".into(), "exe".into(), file_required()?.as_os_str().into()]),
                (TaskProfile::Dart, ProjectTaskAction::RunFile) =>
                    ("dart".into(), vec!["run".into(), file_required()?.as_os_str().into()]),
                (TaskProfile::Rust, action) if root.join("Cargo.toml").is_file() =>
                    ("cargo".into(), vec![match action {
                        ProjectTaskAction::Check => "check",
                        ProjectTaskAction::Test => "test",
                        ProjectTaskAction::Build => "build",
                        ProjectTaskAction::RunFile => "run",
                    }.into()]),
                (TaskProfile::Rust, _) => return Err("Rust's built-in tasks need Cargo.toml at the project root. Open that folder or configure a custom command in Settings → Tasks.".into()),
                _ => return Err("No task command is configured for this language. Set the project's Run, Check, Test or Build command in Settings → Tasks.".into()),
            };
            let (program, args) = if program == "dart" {
                let (program, prefix) = language_settings::executable_command(
                    &self.dart_program(root),
                    Vec::new(),
                    root,
                );
                (
                    program.into_os_string(),
                    prefix.into_iter().map(OsString::from).chain(args).collect(),
                )
            } else {
                (program, args)
            };
            tasks::CommandSpec::Program { program, args }
        };
        Ok(ResolvedTask {
            root: root.to_owned(),
            command,
            environment,
        })
    }
}

impl VEditApp {
    fn inherited_dart_executable(&self) -> String {
        self.settings
            .language_servers
            .iter()
            .find(|language| language.language_id == "dart")
            .filter(|language| {
                Path::new(language.program.trim())
                    .file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case("dart"))
            })
            .map_or_else(String::new, |language| language.program.clone())
    }

    pub(super) fn project_task_configuration(&self) -> Option<TaskConfiguration> {
        self.project().map(|project| {
            let mut config = project.tasks.clone();
            if config.dart_executable.trim().is_empty() {
                config.dart_executable = self.inherited_dart_executable();
            }
            config
        })
    }

    pub(super) fn project_task_unsaved_count(&self) -> usize {
        let Some(project) = self.project() else {
            return 0;
        };
        self.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .filter(|tab| {
                (tab.dirty || tab.saving)
                    && tab
                        .path
                        .as_ref()
                        .is_some_and(|path| context_actions::path_contains(&project.root, path))
            })
            .count()
    }

    pub(super) fn resolve_project_task(
        &self,
        action: ProjectTaskAction,
    ) -> Result<ResolvedTask, String> {
        if self.task.is_some() {
            return Err("Stop the current task first.".into());
        }
        let project = self
            .project()
            .ok_or_else(|| "Open a project before running a task.".to_owned())?;
        let unsaved = self.project_task_unsaved_count();
        if unsaved > 0
            || (action == ProjectTaskAction::RunFile
                && self.tab().is_some_and(|tab| tab.dirty || tab.saving))
        {
            return Err("Save your changes before running a task. Use Save all files in Output, wait for saving to finish, then run again.".into());
        }
        self.project_task_configuration().unwrap().resolve(
            &project.root,
            self.tab().and_then(|tab| tab.path.as_deref()),
            action,
        )
    }

    pub(super) fn run_project_task(&mut self, action: ProjectTaskAction) {
        match self.resolve_project_task(action) {
            Ok(task) => self.start_resolved_task(task),
            Err(error) => {
                self.output_visible = true;
                self.problems_visible = false;
                self.status(error, true);
            }
        }
    }

    pub(super) fn start_resolved_task(&mut self, task: ResolvedTask) {
        if let Some(project) = self.projects.get_mut(self.active_project) {
            project.launch.show_output = false;
        }
        self.output_visible = true;
        self.problems_visible = false;
        self.output = format!(
            "> {}\n{}\nRuns saved files on disk.\n\n",
            task.display(),
            crate::paths::display(&task.root)
        );
        self.task_status = "Running".into();
        self.task = Some(tasks::spawn_configured(
            task.root,
            task.command,
            task.environment,
        ));
    }

    pub(super) fn open_project_task_settings(&mut self) {
        self.open_settings_section("Tasks");
        self.project_tasks_settings_requested = true;
    }

    pub(super) fn apply_pending_project_interpreter(&mut self) {
        if let Some(project) = self.project_tasks_interpreter_pending.take() {
            self.restart_project_language(project, "python");
        }
    }

    pub(super) fn project_tasks_settings_ui(&mut self, ui: &mut egui::Ui) {
        let heading = ui.label(RichText::new("PROJECT TASKS").strong().color(MUTED));
        if self.project_tasks_settings_requested {
            heading.scroll_to_me(Some(Align::Min));
            self.project_tasks_settings_requested = false;
        }
        let active_path = self.tab().and_then(|tab| tab.path.clone());
        let inherited_dart = self.inherited_dart_executable();
        let active_project = self.active_project;
        let Some(project) = self.projects.get_mut(active_project) else {
            ui.label("Open a project to configure its commands and Python interpreter.");
            return;
        };
        ui.label(
            RichText::new(crate::paths::display(&project.root))
                .size(11.0)
                .color(MUTED),
        );
        let previous = project.tasks.clone();
        egui::ComboBox::from_label("Task profile")
            .selected_text(project.tasks.profile.label())
            .show_ui(ui, |ui| {
                for profile in [
                    TaskProfile::Auto,
                    TaskProfile::Python,
                    TaskProfile::Dart,
                    TaskProfile::Rust,
                    TaskProfile::Custom,
                ] {
                    ui.selectable_value(&mut project.tasks.profile, profile, profile.label());
                }
            });
        let profile = project
            .tasks
            .effective_profile(&project.root, active_path.as_deref());
        ui.label(
            RichText::new(format!("Current task language: {}", profile.label()))
                .size(11.0)
                .color(MUTED),
        );
        ui.label("Python interpreter (running and analysis)");
        ui.label(
            RichText::new("Executable path or command name, without quotes or extra arguments.")
                .size(10.5)
                .color(MUTED),
        );
        let interpreter = ui.add(
            egui::TextEdit::singleline(&mut project.tasks.python_interpreter)
                .desired_width(f32::INFINITY)
                .hint_text("Automatic: .venv, venv, env, then PATH"),
        );
        let mut restart = interpreter.lost_focus();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Choose interpreter…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_directory(&project.root)
                    .pick_file()
            {
                project.tasks.python_interpreter = path.to_string_lossy().into_owned();
                restart = true;
            }
            if ui.button("Use automatic interpreter").clicked() {
                project.tasks.python_interpreter.clear();
                restart = true;
            }
        });
        ui.label(
            RichText::new(format!(
                "Using: {}",
                project.tasks.python_program(&project.root)
            ))
            .size(10.5)
            .color(MUTED),
        );
        ui.label("Dart executable for tasks");
        ui.add(
            egui::TextEdit::singleline(&mut project.tasks.dart_executable)
                .desired_width(f32::INFINITY)
                .hint_text(if inherited_dart.is_empty() {
                    "Automatic: dart on PATH"
                } else {
                    &inherited_dart
                }),
        );
        ui.label(RichText::new("Leave empty to use the Dart SDK selected in Languages, then PATH. Enter an executable path without quotes or arguments.").size(10.5).color(MUTED));
        ui.label(RichText::new("Empty commands use the current language's defaults. Overrides apply to this project, including other languages in it.").size(11.0).color(MUTED));
        for (action, label) in [
            (ProjectTaskAction::RunFile, "Run active file"),
            (ProjectTaskAction::Check, "Check project"),
            (ProjectTaskAction::Test, "Run tests"),
            (ProjectTaskAction::Build, "Build project"),
        ] {
            let default = TaskConfiguration {
                profile: project.tasks.profile,
                python_interpreter: project.tasks.python_interpreter.clone(),
                dart_executable: if project.tasks.dart_executable.trim().is_empty() {
                    inherited_dart.clone()
                } else {
                    project.tasks.dart_executable.clone()
                },
                ..Default::default()
            }
            .resolve(&project.root, active_path.as_deref(), action)
            .map(|task| task.display())
            .unwrap_or_else(|_| "Enter a command".into());
            let value = match action {
                ProjectTaskAction::RunFile => &mut project.tasks.run_file,
                ProjectTaskAction::Check => &mut project.tasks.check,
                ProjectTaskAction::Test => &mut project.tasks.test,
                ProjectTaskAction::Build => &mut project.tasks.build,
            };
            ui.label(label);
            ui.add(
                egui::TextEdit::singleline(value)
                    .desired_width(f32::INFINITY)
                    .hint_text(default),
            );
        }
        ui.label(RichText::new(if cfg!(windows) {
            "Custom commands use PowerShell. Example: & $env:VEDIT_PYTHON -u $env:VEDIT_FILE --verbose"
        } else {
            "Custom commands use /bin/sh. Example: \"$VEDIT_PYTHON\" -u \"$VEDIT_FILE\" --verbose"
        }).size(10.5).color(MUTED));
        ui.label(RichText::new("VEDIT_ROOT, VEDIT_FILE and VEDIT_PYTHON are environment variables. Working directory: this project. Python Test uses pytest; Build uses the build module (install in your environment if needed). Rust Run uses cargo run.").size(10.5).color(MUTED));
        if ui.button("Reset project task commands").clicked() {
            let python_interpreter = project.tasks.python_interpreter.clone();
            let dart_executable = project.tasks.dart_executable.clone();
            project.tasks = TaskConfiguration {
                python_interpreter,
                dart_executable,
                ..Default::default()
            };
        }
        self.session_dirty |= previous != project.tasks;
        if previous.python_interpreter != project.tasks.python_interpreter {
            self.project_tasks_interpreter_pending = Some(active_project);
        }
        if restart || !interpreter.has_focus() {
            self.apply_pending_project_interpreter();
        }
        ui.add_space(12.0);
    }
}

#[cfg(test)]
#[path = "project_tasks_tests.rs"]
mod tests;
