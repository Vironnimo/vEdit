//! User-owned language-server commands; arguments are passed directly, never through a shell.
use super::*;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct LanguageServerSettings {
    pub language_id: String,
    pub name: String,
    pub extensions: Vec<String>,
    pub enabled: bool,
    /// Empty selects the built-in server discovery for Rust, Python or Dart.
    pub program: String,
    pub args: Vec<String>,
    pub initialization_options: Value,
    pub settings: Value,
}
impl Default for LanguageServerSettings {
    fn default() -> Self {
        Self {
            language_id: String::new(),
            name: String::new(),
            extensions: Vec::new(),
            enabled: true,
            program: String::new(),
            args: Vec::new(),
            initialization_options: json!({}),
            settings: json!({}),
        }
    }
}
impl LanguageServerSettings {
    pub fn matches(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| {
                self.extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(ext))
            })
    }
    pub fn validate(&self, others: &[Self]) -> Result<(), String> {
        if self.language_id.trim().is_empty() || self.language_id.chars().any(char::is_whitespace) {
            return Err("Language ID must be a nonempty LSP identifier without spaces (for example python).".into());
        }
        if self.name.trim().is_empty() || self.extensions.is_empty() {
            return Err("Give the language a name and at least one file extension.".into());
        }
        if self.extensions.iter().any(|ext| {
            ext.is_empty()
                || ext
                    .chars()
                    .any(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        }) {
            return Err(
                "Extensions must be letters/digits without a dot (for example py, pyi).".into(),
            );
        }
        if self.program.trim().is_empty()
            && !matches!(self.language_id.as_str(), "rust" | "python" | "dart")
        {
            return Err("Choose an executable for this custom language server.".into());
        }
        if self.program.contains('\0') || self.args.iter().any(|s| s.contains('\0')) {
            return Err("Executable and arguments must not contain NUL characters.".into());
        }
        if !self.settings.is_object()
            || !(self.initialization_options.is_object() || self.initialization_options.is_null())
        {
            return Err("Server settings must be a JSON object; initialization options must be an object or null.".into());
        }
        if self.enabled
            && let Some(other) = others.iter().find(|other| {
                other.enabled
                    && other.language_id != self.language_id
                    && other.extensions.iter().any(|ext| {
                        self.extensions
                            .iter()
                            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
                    })
            })
        {
            return Err(format!(
                "An extension is already assigned to {}. Disable it or remove the duplicate extension first.",
                other.name
            ));
        }
        Ok(())
    }
    pub fn install_hint(&self) -> String {
        match self.language_id.as_str() {
            "python" => "The portable app includes basedpyright. Automatic selection also finds basedpyright, pyright or pylsp in the project environment or PATH. Choose the Python interpreter in Settings → Tasks. For a separate installation: python -m pip install basedpyright (or python-lsp-server).".into(),
            "dart" => "Install Dart or Flutter and make dart available on PATH, or select the Dart SDK's bin/dart.exe in Settings → Languages.".into(),
            "rust" => "Install rust-analyzer with rustup component add rust-analyzer, or select its executable in Settings → Languages.".into(),
            _ => format!("Install the {} language server and choose its executable and arguments in Settings → Languages.", self.name),
        }
    }
    pub fn resolve(&self, root: &Path, python_interpreter: &str) -> crate::lsp::ServerConfig {
        let mut args = self.args.clone();
        let mut settings = self.settings.clone();
        let program = if self.program.trim().is_empty() {
            match self.language_id.as_str() {
                "python" => {
                    let found = ["basedpyright-langserver", "pyright-langserver", "pylsp"]
                        .into_iter()
                        .find_map(|name| find_program(name, root).map(|path| (name, path)));
                    if found.is_none()
                        && let Some((program, bundled_args)) = bundled_python_server(root)
                    {
                        let mut config = self.clone();
                        config.program = program.to_string_lossy().into_owned();
                        config.args = bundled_args;
                        config.args.extend(args);
                        return config.resolve(root, python_interpreter);
                    }
                    let (name, path) = found.unwrap_or_else(|| {
                        (
                            "basedpyright-langserver",
                            PathBuf::from("basedpyright-langserver"),
                        )
                    });
                    if args.is_empty() && name != "pylsp" {
                        args.push("--stdio".into());
                    }
                    path
                }
                "dart" => {
                    if args.is_empty() {
                        args.push("language-server".into());
                    }
                    find_program("dart", root).unwrap_or_else(|| PathBuf::from("dart"))
                }
                "rust" => find_program("rust-analyzer", root)
                    .unwrap_or_else(|| PathBuf::from("rust-analyzer")),
                _ => PathBuf::new(),
            }
        } else {
            find_program(self.program.trim(), root)
                .unwrap_or_else(|| PathBuf::from(self.program.trim()))
        };
        if args.is_empty() {
            let executable = program
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if self.language_id == "dart" && executable == "dart" {
                args.push("language-server".into());
            }
            if self.language_id == "python"
                && matches!(
                    executable.as_str(),
                    "basedpyright-langserver" | "pyright-langserver"
                )
            {
                args.push("--stdio".into());
            }
        }
        if self.language_id == "python"
            && let Some(map) = settings.as_object_mut()
        {
            let interpreter_path = find_program_with_local(python_interpreter, root, false)
                .or_else(|| {
                    let path = Path::new(python_interpreter);
                    (path.is_absolute() || path.components().count() > 1).then(|| {
                        if path.is_absolute() {
                            path.to_owned()
                        } else {
                            root.join(path)
                        }
                    })
                });
            let interpreter_value = interpreter_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| python_interpreter.into());
            let python = map.entry("python").or_insert_with(|| json!({}));
            if let Some(python) = python.as_object_mut() {
                python
                    .entry("pythonPath")
                    .or_insert_with(|| json!(interpreter_value));
            }
            // pylsp's Jedi interpreter setting uses a different namespace.
            let pylsp = map.entry("pylsp").or_insert_with(|| json!({}));
            if let Some(pylsp) = pylsp.as_object_mut() {
                let plugins = pylsp.entry("plugins").or_insert_with(|| json!({}));
                if let Some(plugins) = plugins.as_object_mut() {
                    let jedi = plugins.entry("jedi").or_insert_with(|| json!({}));
                    if let Some(jedi) = jedi.as_object_mut()
                        && let Some(interpreter) = &interpreter_path
                    {
                        jedi.entry("environment")
                            .or_insert_with(|| json!(interpreter.to_string_lossy()));
                    }
                }
            }
        }
        let (program, args) = unwrap_windows_launcher(program, args, root);
        crate::lsp::ServerConfig {
            name: self.name.clone(),
            program: program.to_string_lossy().into_owned(),
            args,
            language_id: self.language_id.clone(),
            initialization_options: self.initialization_options.clone(),
            settings,
            install_hint: self.install_hint(),
        }
    }
}

pub(super) fn defaults() -> Vec<LanguageServerSettings> {
    vec![
        LanguageServerSettings {
            language_id: "rust".into(),
            name: "Rust".into(),
            extensions: vec!["rs".into()],
            initialization_options: json!({"checkOnSave":true,"cargo":{"targetDir":true},"procMacro":{"enable":true},"cachePriming":{"numThreads":2}}),
            settings: json!({"rust-analyzer":{"checkOnSave":true,"cargo":{"targetDir":true},"procMacro":{"enable":true},"cachePriming":{"numThreads":2}}}),
            ..Default::default()
        },
        LanguageServerSettings {
            language_id: "python".into(),
            name: "Python".into(),
            extensions: vec!["py".into(), "pyi".into(), "pyw".into()],
            ..Default::default()
        },
        LanguageServerSettings {
            language_id: "dart".into(),
            name: "Dart".into(),
            extensions: vec!["dart".into()],
            initialization_options: json!({"onlyAnalyzeProjectsWithOpenFiles":true}),
            ..Default::default()
        },
    ]
}

fn find_program(program: &str, root: &Path) -> Option<PathBuf> {
    find_program_with_local(program, root, true)
}

fn find_program_with_local(program: &str, root: &Path, prefer_local: bool) -> Option<PathBuf> {
    let value = Path::new(program);
    let extensions = if cfg!(windows) {
        vec!["", ".exe", ".cmd", ".bat"]
    } else {
        vec![""]
    };
    let candidates = if value.components().count() > 1 || value.is_absolute() {
        vec![if value.is_absolute() {
            value.to_owned()
        } else {
            root.join(value)
        }]
    } else {
        let mut dirs = if prefer_local {
            vec![
                root.join(".venv/Scripts"),
                root.join("venv/Scripts"),
                root.join(".venv/bin"),
                root.join("venv/bin"),
            ]
        } else {
            Vec::new()
        };
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.into_iter().map(|dir| dir.join(program)).collect()
    };
    for candidate in candidates {
        for ext in &extensions {
            let path = if ext.is_empty() {
                candidate.clone()
            } else {
                PathBuf::from(format!("{}{ext}", candidate.to_string_lossy()))
            };
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub(super) fn executable_command(
    program: &str,
    args: Vec<String>,
    root: &Path,
) -> (PathBuf, Vec<String>) {
    unwrap_windows_launcher(
        find_program(program, root).unwrap_or_else(|| PathBuf::from(program)),
        args,
        root,
    )
}

fn bundled_python_server(root: &Path) -> Option<(PathBuf, Vec<String>)> {
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    for tools in [
        directory.join("tools"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools"),
    ] {
        let script = tools.join("basedpyright/node_modules/basedpyright/langserver.index.js");
        if !script.is_file() {
            continue;
        }
        let bundled_node = tools.join(if cfg!(windows) { "node.exe" } else { "node" });
        let node = if bundled_node.is_file() {
            Some(bundled_node)
        } else {
            find_program("node", root)
        }?;
        return Some((
            node,
            vec![script.to_string_lossy().into_owned(), "--stdio".into()],
        ));
    }
    None
}

fn unwrap_windows_launcher(
    program: PathBuf,
    mut args: Vec<String>,
    root: &Path,
) -> (PathBuf, Vec<String>) {
    if !cfg!(windows)
        || !program
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    {
        return (program, args);
    }
    if let Some(dir) = program.parent() {
        let name = program
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if name == "dart" {
            for path in [
                dir.join("dart.exe"),
                dir.join("cache/dart-sdk/bin/dart.exe"),
            ] {
                if path.is_file() {
                    return (path, args);
                }
            }
        }
        if matches!(
            name.as_str(),
            "pyright-langserver" | "basedpyright-langserver"
        ) {
            let package = name.trim_end_matches("-langserver");
            for script in [
                dir.join("node_modules")
                    .join(package)
                    .join("langserver.index.js"),
                dir.parent()
                    .unwrap_or(dir)
                    .join(package)
                    .join("langserver.index.js"),
            ] {
                if script.is_file()
                    && let Some(node) = find_program("node", root)
                {
                    args.insert(0, script.to_string_lossy().into_owned());
                    return (node, args);
                }
            }
        }
    }
    (program, args)
}

pub(super) struct LanguageSettingsDraft {
    config: LanguageServerSettings,
    extensions: String,
    args: String,
    init: String,
    settings: String,
    error: Option<String>,
}
impl LanguageSettingsDraft {
    fn new(config: LanguageServerSettings) -> Self {
        Self {
            extensions: config.extensions.join(", "),
            args: config.args.join("\n"),
            init: serde_json::to_string_pretty(&config.initialization_options).unwrap_or_default(),
            settings: serde_json::to_string_pretty(&config.settings).unwrap_or_default(),
            config,
            error: None,
        }
    }
    fn parse(&self) -> Result<LanguageServerSettings, String> {
        let mut config = self.config.clone();
        config.name = config.name.trim().into();
        config.program = config.program.trim().into();
        config.extensions = self
            .extensions
            .split(',')
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect();
        config.args = self
            .args
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        config.initialization_options = serde_json::from_str(&self.init)
            .map_err(|error| format!("Initialization options: {error}"))?;
        config.settings = serde_json::from_str(&self.settings)
            .map_err(|error| format!("Server settings: {error}"))?;
        Ok(config)
    }
}
impl MorrowApp {
    pub(super) fn language_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(RichText::new("LANGUAGES").size(11.0).color(ACCENT));
        ui.label("Diagnostics, completion and go to definition use an installed language server. Multiple languages can run in the same project.");
        let configurations = self.settings.language_servers.clone();
        let project_root = self
            .project()
            .map(|project| project.root.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        let interpreter = self
            .project()
            .map(|project| project.tasks.python_program(&project.root))
            .unwrap_or_else(|| "python".into());
        let mut changed = Vec::new();
        let mut remove = None;
        for (index, config) in configurations.iter().enumerate() {
            let draft = self
                .language_settings_drafts
                .entry(config.language_id.clone())
                .or_insert_with(|| LanguageSettingsDraft::new(config.clone()));
            egui::CollapsingHeader::new(format!("{} · {}", config.name, if config.enabled { "enabled" } else { "disabled" })).id_salt(("language_settings", &config.language_id)).show(ui, |ui| {
                ui.label(config.install_hint());
                let effective = config.resolve(&project_root, &interpreter);
                ui.label(RichText::new(format!("Current command: {} {}", effective.program, effective.args.iter().map(|arg|format!("{arg:?}")).collect::<Vec<_>>().join(" "))).size(10.5).color(MUTED));
                ui.checkbox(&mut draft.config.enabled, "Enable language analysis");
                ui.label("Display name"); ui.text_edit_singleline(&mut draft.config.name);
                if !matches!(config.language_id.as_str(), "rust" | "python" | "dart") { ui.label("LSP language ID"); ui.text_edit_singleline(&mut draft.config.language_id); }
                ui.label("File extensions (comma separated)"); ui.text_edit_singleline(&mut draft.extensions);
                ui.label("Executable (empty = automatic for built-in languages)"); ui.text_edit_singleline(&mut draft.config.program);
                ui.label("Arguments (one argument per line; no shell quoting)"); ui.add(egui::TextEdit::multiline(&mut draft.args).desired_rows(2).desired_width(f32::INFINITY));
                ui.collapsing("Advanced initialization and server settings", |ui| {
                    ui.label("Initialization options (JSON)"); ui.add(egui::TextEdit::multiline(&mut draft.init).code_editor().desired_rows(4).desired_width(f32::INFINITY));
                    ui.label("Server settings (JSON; basedpyright example: {\"basedpyright\":{\"analysis\":{\"typeCheckingMode\":\"basic\"}}})"); ui.add(egui::TextEdit::multiline(&mut draft.settings).code_editor().desired_rows(4).desired_width(f32::INFINITY));
                });
                if let Some(error) = &draft.error { ui.colored_label(RED, error); }
                ui.horizontal(|ui| {
                    if ui.button("Apply and restart language").clicked() {
                        let parsed = draft.parse().and_then(|parsed| {
                            if parsed.language_id != config.language_id && configurations.iter().any(|other| other.language_id == parsed.language_id) { return Err("That language ID already exists.".into()); }
                            parsed.validate(&configurations.iter().filter(|other| other.language_id != config.language_id).cloned().collect::<Vec<_>>())?; Ok(parsed)
                        });
                        match parsed { Ok(parsed) => { self.settings.language_servers[index] = parsed; draft.error = None; changed.push(config.language_id.clone()); }, Err(error) => draft.error = Some(error) }
                    }
                    if ui.button("Discard edits").clicked() { *draft = LanguageSettingsDraft::new(config.clone()); }
                    if !matches!(config.language_id.as_str(), "rust" | "python" | "dart") && ui.button("Remove language").clicked() { remove = Some(index); }
                });
            });
        }
        if let Some(index) = remove {
            let config = self.settings.language_servers.remove(index);
            self.language_settings_drafts.remove(&config.language_id);
            changed.push(config.language_id);
        }
        if ui.button("Add language server…").clicked() {
            let mut number = 1;
            while self
                .settings
                .language_servers
                .iter()
                .any(|config| config.language_id == format!("custom{number}"))
            {
                number += 1;
            }
            self.settings.language_servers.push(LanguageServerSettings {
                language_id: format!("custom{number}"),
                name: "New language".into(),
                enabled: false,
                ..Default::default()
            });
            self.session_dirty = true;
        }
        if !changed.is_empty() {
            self.language_settings_drafts.clear();
            self.session_dirty = true;
            for project in &mut self.projects {
                for language in &changed {
                    project.clear_language_diagnostics(language);
                    project.languages.remove(language);
                }
            }
            self.completions = None;
            self.completion_request = None;
            self.definition_request = None;
            self.status(
                "Language settings saved; changed servers reconnect using current buffers",
                false,
            );
            self.ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configs_round_trip_and_extensions_have_one_owner() {
        let settings = defaults();
        for config in &settings {
            config.validate(&settings).unwrap();
        }
        assert!(settings[1].matches(Path::new("source.PYI")));
        let mut duplicate = settings[1].clone();
        duplicate.language_id = "custom".into();
        duplicate.program = "custom-server".into();
        assert!(
            duplicate
                .validate(&settings)
                .unwrap_err()
                .contains("already assigned")
        );
        duplicate.enabled = false;
        duplicate.validate(&settings).unwrap();
        let restored: Vec<LanguageServerSettings> =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored, settings);
    }
    #[test]
    fn malformed_json_draft_does_not_change_applied_settings() {
        let config = defaults().remove(1);
        let mut draft = LanguageSettingsDraft::new(config.clone());
        draft.settings = "{".into();
        assert!(draft.parse().is_err());
        assert_eq!(config.name, "Python");
    }
    #[test]
    fn direct_arguments_preserve_spaces_and_shell_characters() {
        let mut config = defaults().remove(1);
        config.program = "server-executable".into();
        config.args = vec!["a b & c.py".into()];
        let resolved = config.resolve(Path::new("C:/workspace"), "C:/Python/python.exe");
        assert_eq!(resolved.args, vec!["a b & c.py"]);
        assert_eq!(
            resolved.settings["python"]["pythonPath"],
            "C:/Python/python.exe"
        );
        assert_eq!(
            resolved.settings["pylsp"]["plugins"]["jedi"]["environment"],
            "C:/Python/python.exe"
        );
    }
    #[test]
    fn explicit_builtin_executables_keep_their_required_default_arguments() {
        let mut dart = defaults().remove(2);
        dart.program = "C:/installed SDK/bin/dart.exe".into();
        assert_eq!(
            dart.resolve(Path::new("C:/workspace"), "python").args,
            vec!["language-server"]
        );
        let mut python = defaults().remove(1);
        python.program = "C:/tools/pyright-langserver.exe".into();
        assert_eq!(
            python.resolve(Path::new("C:/workspace"), "python").args,
            vec!["--stdio"]
        );
        python.args = vec!["--custom".into()];
        assert_eq!(
            python.resolve(Path::new("C:/workspace"), "python").args,
            vec!["--custom"]
        );
    }
    #[test]
    fn jedi_never_receives_an_unresolved_command_as_a_virtualenv_path() {
        let mut python = defaults().remove(1);
        python.program = "pylsp".into();
        let config = python.resolve(
            Path::new("C:/workspace"),
            "morrow-test-nonexistent-python-command",
        );
        assert!(config.settings["pylsp"]["plugins"]["jedi"]["environment"].is_null());
        python.settings = json!({"pylsp":{"plugins":{"jedi":{"environment":"C:/my-environment"}}}});
        let config = python.resolve(
            Path::new("C:/workspace"),
            "morrow-test-nonexistent-python-command",
        );
        assert_eq!(
            config.settings["pylsp"]["plugins"]["jedi"]["environment"],
            "C:/my-environment"
        );
    }
    #[test]
    fn an_explicit_interpreter_command_uses_path_instead_of_a_project_virtualenv() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "morrow-python-path-{}-{unique}",
            std::process::id()
        ));
        let fake = root.join(if cfg!(windows) {
            ".venv/Scripts/python.exe"
        } else {
            ".venv/bin/python"
        });
        fs::create_dir_all(fake.parent().unwrap()).unwrap();
        fs::write(&fake, "fixture").unwrap();
        let mut python = defaults().remove(1);
        python.program = "pylsp".into();
        let explicit = python.resolve(&root, "python");
        assert_ne!(
            explicit.settings["python"]["pythonPath"],
            json!(fake.to_string_lossy())
        );
        let automatic = python.resolve(&root, &fake.to_string_lossy());
        assert_eq!(
            automatic.settings["python"]["pythonPath"],
            json!(fake.to_string_lossy())
        );
        assert!(
            root.starts_with(std::env::temp_dir())
                && root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("morrow-python-path-")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
