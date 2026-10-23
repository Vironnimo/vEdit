//! Versioned, bounded controls for presenting work in a running vEdit window.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::PathBuf,
    time::{Duration, Instant},
};

#[path = "cli_help.rs"]
pub mod help;
#[path = "cli_launch.rs"]
pub mod launch;

#[path = "cli_spawn.rs"]
mod spawn;
#[path = "cli_transport.rs"]
mod transport;
#[allow(unused_imports)]
pub use transport::{Envelope, Server, running_windows, saved_window_ids, send};

/// Reject an obsolete instance selector instead of sending a request to the
/// default profile when an older script intended a separate test instance.
pub fn validate_environment() -> Result<(), String> {
    validate_control_environment(
        std::env::var_os("VEDIT_CONTROL_DIR").is_some(),
        std::env::var_os("MORROW_CONTROL_DIR").is_some(),
    )
}

fn validate_control_environment(current: bool, previous: bool) -> Result<(), String> {
    if previous && !current {
        Err("MORROW_CONTROL_DIR is no longer supported. Set VEDIT_CONTROL_DIR to the same directory, then repeat the command. No request was sent.".into())
    } else {
        Ok(())
    }
}

pub const MAX_FILES: usize = 100;
pub const MAX_ANNOTATIONS: usize = 500;

#[cfg(test)]
#[path = "cli_workflow_tests.rs"]
mod workflow_tests;

/// What `change_review` compares; see `vedit-cli review --help`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReviewSource {
    /// Staged, unstaged and untracked changes compared with HEAD.
    Working,
    /// The commit `rev` compared with its first parent.
    Commit,
    /// HEAD since it left `base`, by default local main or master.
    Branch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentView {
    #[default]
    Source,
    Preview,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorFont {
    #[default]
    #[serde(rename = "jetbrains_mono", alias = "jetbrains-mono")]
    JetBrainsMono,
    Consolas,
    #[serde(alias = "cascadia-mono")]
    CascadiaMono,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub line: usize,
    #[serde(default = "one")]
    pub column: usize,
}
fn one() -> usize {
    1
}
fn is_one(value: &usize) -> bool {
    *value == 1
}
fn default_limit() -> usize {
    100
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn default_mark_text_count() -> usize {
    4096
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationSeverity {
    #[default]
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    #[serde(default)]
    pub id: String,
    pub start: Location,
    #[serde(default)]
    pub end: Option<Location>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub severity: AnnotationSeverity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenFile {
    pub path: PathBuf,
    #[serde(default)]
    pub location: Option<Location>,
    #[serde(default)]
    pub view: DocumentView,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
}
impl OpenFile {
    pub fn source(path: PathBuf) -> Self {
        Self {
            path,
            location: None,
            view: DocumentView::Source,
            annotations: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewManifest {
    pub title: String,
    #[serde(default)]
    pub note: String,
    pub files: Vec<OpenFile>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewSummary {
    pub title: String,
    pub note: String,
    pub files: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    LeftSidebar,
    RightSidebar,
    Explorer,
    Search,
    SourceControl,
    Review,
    Output,
    Problems,
    Settings,
}

/// Exact Unicode coordinate range; unlike annotations, the end is exclusive.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRange {
    pub start: Location,
    pub end: Location,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    pub range: TextRange,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineNumbers {
    #[default]
    Absolute,
    Relative,
    Off,
}

pub const MAX_SHORTCUT_ALIASES: usize = 8;
pub const MAX_SHORTCUT_BYTES: usize = 100;

/// A single shortcut retains its original JSON string representation. Multiple
/// shortcuts are an ordered list of alternatives for the same action.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ShortcutBinding {
    Single(String),
    Multiple(Vec<String>),
}

impl ShortcutBinding {
    pub fn values(&self) -> impl Iterator<Item = &str> {
        let values = match self {
            Self::Single(value) => std::slice::from_ref(value),
            Self::Multiple(values) => values.as_slice(),
        };
        values.iter().map(String::as_str)
    }
}

impl From<String> for ShortcutBinding {
    fn from(value: String) -> Self {
        Self::Single(value)
    }
}

impl From<&str> for ShortcutBinding {
    fn from(value: &str) -> Self {
        Self::Single(value.into())
    }
}

impl From<Vec<String>> for ShortcutBinding {
    fn from(mut values: Vec<String>) -> Self {
        if values.len() == 1 {
            Self::Single(values.remove(0))
        } else {
            Self::Multiple(values)
        }
    }
}

impl Serialize for ShortcutBinding {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Single(value) => value.serialize(serializer),
            Self::Multiple(values) if values.len() == 1 => values[0].serialize(serializer),
            Self::Multiple(values) => values.serialize(serializer),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    pub multicursor: Option<MulticursorPatch>,
    /// Complete override map: absent actions retain defaults, null disables, {} resets all.
    pub keybindings: Option<std::collections::BTreeMap<String, Option<ShortcutBinding>>>,
    pub vim: Option<bool>,
    pub swap_jk: Option<bool>,
    pub autosave: Option<bool>,
    pub external_open_new_window: Option<bool>,
    pub autosave_delay_ms: Option<u64>,
    pub font_size: Option<f32>,
    pub ui_scale: Option<f32>,
    pub line_height: Option<f32>,
    pub line_numbers: Option<LineNumbers>,
    pub highlight_current_line: Option<bool>,
    pub git_gutter: Option<bool>,
    pub show_ignored_files: Option<bool>,
    pub editor_font: Option<EditorFont>,
    pub tab_width: Option<usize>,
    pub detect_indentation: Option<bool>,
    pub indent_with_tabs: Option<bool>,
    pub language_indent_widths: Option<std::collections::HashMap<String, usize>>,
    pub language_indent_with_tabs: Option<std::collections::HashMap<String, bool>>,
    pub word_wrap: Option<bool>,
    pub smooth_scroll: Option<bool>,
    pub leader_key: Option<String>,
    pub search_excludes: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MulticursorModifier {
    #[default]
    Alt,
    Ctrl,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MulticursorPatch {
    pub modifier: Option<MulticursorModifier>,
    pub spread_paste: Option<bool>,
    pub match_case: Option<bool>,
    pub whole_word: Option<bool>,
    pub limit: Option<usize>,
}

fn validate_indent_languages<'a>(
    languages: impl Iterator<Item = &'a str>,
    count: usize,
) -> Result<(), String> {
    if count > 64 {
        return Err("Indentation language maps support at most 64 entries".into());
    }
    for language in languages {
        if language.is_empty()
            || language.len() > 32
            || !language
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("Indentation language keys must be lowercase language IDs (1..32 ASCII letters, digits or underscores), for example python, dart, javascript or shellscript".into());
        }
    }
    Ok(())
}

/// Session bookmarks use persistent IDs returned by the bookmark list.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BookmarkAction {
    List {
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Toggle {
        #[serde(default)]
        tab_id: Option<u64>,
        #[serde(default)]
        label: Option<String>,
    },
    Remove {
        id: u64,
    },
    Rename {
        id: u64,
        label: String,
    },
    Jump {
        id: u64,
    },
}

/// User-selected snippets are separate from agent-authored review annotations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMarksAction {
    List {
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Take {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Batch {
        #[serde(default)]
        batch_id: Option<u64>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Text {
        id: u64,
        #[serde(default)]
        batch_id: Option<u64>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_mark_text_count")]
        count: usize,
    },
    Ack {
        batch_id: u64,
    },
    Restore {
        #[serde(default)]
        batch_id: Option<u64>,
    },
    Clear {},
    Remove {
        id: u64,
    },
    Note {
        id: u64,
        note: String,
    },
    Add {
        #[serde(default)]
        tab_id: Option<u64>,
        #[serde(default)]
        note: Option<String>,
    },
}
impl Default for AgentMarksAction {
    fn default() -> Self {
        Self::List {
            limit: 100,
            offset: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorCommand {
    Undo,
    Redo,
    SelectAll,
    ClearSelection,
    Indent,
    Outdent,
    ToggleComment,
    DuplicateLine,
    DeleteLine,
    Reload,
    FindNext,
    FindPrevious,
    Source,
    Preview,
}

/// How prominently `notify` presents its message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// A change review a notification can open; see `vedit-cli notify --help`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationReview {
    Working,
    Branch,
}

/// The file a notification opens when the user clicks it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationOpen {
    pub path: PathBuf,
    #[serde(default)]
    pub location: Option<Location>,
}

pub const MAX_NOTIFICATION_TITLE: usize = 120;
pub const MAX_NOTIFICATION_MESSAGE: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitOrientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayoutAction {
    Status {},
    Split {
        orientation: SplitOrientation,
        #[serde(default)]
        pane_id: Option<u64>,
        #[serde(default)]
        tab_id: Option<u64>,
        #[serde(default)]
        view: Option<DocumentView>,
    },
    Focus {
        pane_id: u64,
    },
    Close {
        pane_id: u64,
    },
    CloseTab {
        pane_id: u64,
        tab_id: u64,
    },
    MoveTab {
        source_pane_id: u64,
        pane_id: u64,
        tab_id: u64,
        index: usize,
    },
    Assign {
        pane_id: u64,
        tab_id: u64,
        #[serde(default)]
        view: Option<DocumentView>,
    },
    Resize {
        pane_id: u64,
        ratio: f32,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Check,
    Test,
    Build,
    RunFile,
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileAction {
    Delete { path: PathBuf },
    Rename { path: PathBuf, name: String },
    CreateFolder { parent: PathBuf, name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Launch {
        #[serde(default)]
        workspace: Option<PathBuf>,
        action: launch::Action,
    },
    Status {},
    Tabs {
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Open {
        files: Vec<OpenFile>,
        /// Load without switching the visible project or raising the window.
        #[serde(default, skip_serializing_if = "is_false")]
        background: bool,
    },
    Review {
        title: String,
        #[serde(default)]
        note: String,
        files: Vec<OpenFile>,
        #[serde(default, skip_serializing_if = "is_false")]
        background: bool,
    },
    Focus {
        tab_id: u64,
        #[serde(default)]
        location: Option<Location>,
        #[serde(default)]
        view: Option<DocumentView>,
    },
    Close {
        tab_ids: Vec<u64>,
    },
    ClearMarks {
        #[serde(default)]
        tab_ids: Vec<u64>,
    },
    Workspace {
        path: PathBuf,
    },
    Panel {
        panel: Panel,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Read {
        tab_id: u64,
        #[serde(default = "one")]
        start_line: usize,
        #[serde(default = "default_limit")]
        line_count: usize,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        start_column: usize,
    },
    Save {
        tab_ids: Vec<u64>,
    },
    New {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        text: String,
        #[serde(default)]
        path: Option<PathBuf>,
    },
    SaveAs {
        tab_id: u64,
        expected_revision: u64,
        path: PathBuf,
    },
    Edit {
        tab_id: u64,
        expected_revision: u64,
        #[serde(default)]
        edits: Vec<TextEdit>,
        #[serde(default)]
        set_text: Option<String>,
    },
    Selection {
        tab_id: u64,
        #[serde(default)]
        range: Option<TextRange>,
        #[serde(default)]
        expected_revision: Option<u64>,
    },
    Settings {
        #[serde(default)]
        patch: Option<SettingsPatch>,
    },
    Bookmarks {
        change: BookmarkAction,
    },
    AgentMarks {
        #[serde(default)]
        action: AgentMarksAction,
    },
    Command {
        tab_id: u64,
        command: EditorCommand,
        expected_revision: u64,
    },
    Find {
        tab_id: u64,
        query: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Replace {
        tab_id: u64,
        expected_revision: u64,
        query: String,
        replacement: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
        #[serde(default = "default_true")]
        all: bool,
    },
    Diff {
        tab_id: u64,
        #[serde(default = "five_hundred")]
        max_lines: usize,
    },
    Diagnostics {
        tab_id: u64,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Search {
        query: String,
        #[serde(default)]
        workspace: Option<PathBuf>,
        #[serde(default)]
        excludes: Option<Vec<String>>,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Output {
        #[serde(default = "one")]
        start_line: usize,
        #[serde(default = "default_limit")]
        line_count: usize,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        start_column: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<PathBuf>,
    },
    GitStatus {
        #[serde(default)]
        workspace: Option<PathBuf>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    GitBranches {
        #[serde(default)]
        workspace: Option<PathBuf>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    GitHistory {
        #[serde(default)]
        workspace: Option<PathBuf>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    GitSwitchBranch {
        target: String,
        #[serde(default)]
        workspace: Option<PathBuf>,
    },
    ChangeReview {
        source: ChangeReviewSource,
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        workspace: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "is_false")]
        background: bool,
    },
    Task {
        action: TaskAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<PathBuf>,
    },
    Notify {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default)]
        level: NotificationLevel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        open: Option<NotificationOpen>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        review: Option<NotificationReview>,
    },
    File {
        action: FileAction,
    },
    FileJob {
        job_id: u64,
    },
    Layout {
        change: LayoutAction,
    },
    Batch {
        actions: Vec<Request>,
        #[serde(default = "default_true")]
        stop_on_error: bool,
    },
    Raise {},
}
fn default_true() -> bool {
    true
}
fn five_hundred() -> usize {
    500
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub result: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<Error>,
}
impl Response {
    pub fn success(result: Value) -> Self {
        Self {
            ok: true,
            result,
            error: None,
        }
    }
    pub fn failure(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: Value::Null,
            error: Some(Error {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

// A single startup parse result; keeping Request inline avoids a heap indirection
// on the normal command path. These values are never stored in a collection.
#[allow(clippy::large_enum_variant)]
pub enum Parsed {
    Help { command: Option<String> },
    Windows,
    Schema { topic: Option<String> },
    Command { request: Request, start: bool },
}

pub fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
    let mut args: Vec<String> = arguments
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| "Arguments must be valid Unicode".to_owned())
        })
        .collect::<Result<_, _>>()?;
    if args.is_empty() {
        return Ok(Parsed::Help { command: None });
    }
    args = normalize_equals(args);
    // A label or other option value may literally be `--help`, `--`, or
    // `--no-start`. Only interpret global flags in argument positions owned
    // by the CLI, never inside a supplied value.
    let mut literal = false;
    let mut expects_value = false;
    let mut help = false;
    let mut no_start = false;
    args.retain(|arg| {
        if literal {
            return true;
        }
        if expects_value {
            expects_value = false;
            return true;
        }
        match arg.as_str() {
            "--" => literal = true,
            "--help" | "-h" => help = true,
            "--no-start" => {
                no_start = true;
                return false;
            }
            value if help::takes_value(value) => expects_value = true,
            _ => {}
        }
        true
    });
    if help {
        let command = args
            .first()
            .filter(|arg| !matches!(arg.as_str(), "--help" | "-h"))
            .cloned();
        if let Some(name) = &command
            && help::command(name).is_none()
        {
            return Err(format!("Unknown command {name}; use vedit-cli --help"));
        }
        return Ok(Parsed::Help { command });
    }
    if args.is_empty() {
        return Err("A command is required after --no-start; use --help".into());
    }
    let command = args.remove(0);
    if command == "windows" {
        if !args.is_empty() {
            return Err("windows takes no arguments; use windows --help".into());
        }
        return Ok(Parsed::Windows);
    }
    if command == "schema" {
        if args.len() > 1 {
            return Err("schema takes at most one operation name; use schema --help".into());
        }
        let topic = args.first().cloned();
        schema_topic(topic.as_deref())?;
        return Ok(Parsed::Schema { topic });
    }
    if help::command(&command).is_none() {
        return Err(format!("Unknown command {command}; use vedit-cli --help"));
    }
    let mut options = Options::default();
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" => set_option(
                &mut options.workspace,
                iter.next().ok_or("--workspace needs a project path")?,
                "--workspace",
            )?,
            "--" => {
                positional.extend(iter);
                break;
            }
            "--line" => set_option(&mut options.line, number(iter.next(), "--line")?, "--line")?,
            "--column" => set_option(
                &mut options.column,
                number(iter.next(), "--column")?,
                "--column",
            )?,
            "--limit" => set_option(
                &mut options.limit,
                number(iter.next(), "--limit")?,
                "--limit",
            )?,
            "--offset" => set_option(
                &mut options.offset,
                number(iter.next(), "--offset")?,
                "--offset",
            )?,
            "--count" => set_option(
                &mut options.count,
                number(iter.next(), "--count")?,
                "--count",
            )?,
            "--revision" => set_option(
                &mut options.revision,
                iter.next()
                    .ok_or("--revision needs a value")?
                    .parse::<u64>()
                    .map_err(|_| "--revision needs a nonnegative integer")?,
                "--revision",
            )?,
            "--tab" => set_option(
                &mut options.tab,
                tab_id(&iter.next().ok_or("--tab needs a tab ID")?)?,
                "--tab",
            )?,
            "--file" => set_option(
                &mut options.file,
                iter.next().ok_or("--file needs a text file path")?,
                "--file",
            )?,
            "--path" => set_option(
                &mut options.path,
                iter.next().ok_or("--path needs a destination path")?,
                "--path",
            )?,
            "--label" => set_option(
                &mut options.label,
                iter.next().ok_or("--label needs text")?,
                "--label",
            )?,
            "--note" => set_option(
                &mut options.note,
                iter.next().ok_or("--note needs text")?,
                "--note",
            )?,
            "--batch" => set_option(
                &mut options.batch,
                agent_mark_id(&iter.next().ok_or("--batch needs a batch ID")?)?,
                "--batch",
            )?,
            "--request-id" => set_option(
                &mut options.request_id,
                iter.next().ok_or("--request-id needs a token")?,
                "--request-id",
            )?,
            "--exclude" => {
                if options.no_excludes {
                    return Err("--exclude and --no-excludes are contradictory".into());
                }
                options.excludes.get_or_insert_with(Vec::new).push(
                    iter.next()
                        .ok_or("--exclude needs a folder or glob pattern")?,
                );
            }
            "--no-excludes" => {
                if options.excludes.is_some() {
                    return Err("--exclude and --no-excludes are contradictory".into());
                }
                set_flag(&mut options.no_excludes, "--no-excludes")?;
            }
            "--working" => set_flag(&mut options.working, "--working")?,
            "--background" => set_flag(&mut options.background, "--background")?,
            "--title" => set_option(
                &mut options.title,
                iter.next().ok_or("--title needs text")?,
                "--title",
            )?,
            "--level" => set_option(
                &mut options.level,
                serde_json::from_value(json!(
                    iter.next()
                        .ok_or("--level needs info, success, warning or error")?
                        .to_ascii_lowercase()
                ))
                .map_err(|_| "--level needs info, success, warning or error")?,
                "--level",
            )?,
            "--open" => set_option(
                &mut options.open,
                iter.next().ok_or("--open needs PATH or PATH:LINE")?,
                "--open",
            )?,
            "--review" => set_option(
                &mut options.review,
                serde_json::from_value(json!(
                    iter.next()
                        .ok_or("--review needs working or branch")?
                        .to_ascii_lowercase()
                ))
                .map_err(|_| "--review needs working or branch")?,
                "--review",
            )?,
            "--branch" => set_flag(&mut options.branch, "--branch")?,
            "--commit" => set_option(
                &mut options.commit,
                iter.next().ok_or("--commit needs a revision")?,
                "--commit",
            )?,
            "--base" => set_option(
                &mut options.base,
                iter.next().ok_or("--base needs a branch or revision")?,
                "--base",
            )?,
            "--case-sensitive" => set_flag(&mut options.case_sensitive, "--case-sensitive")?,
            "--whole-word" => set_flag(&mut options.whole_word, "--whole-word")?,
            "--first" => set_flag(&mut options.first, "--first")?,
            "--preview" => set_option(&mut options.view, DocumentView::Preview, "view")?,
            "--source" => set_option(&mut options.view, DocumentView::Source, "view")?,
            "--hide" => {
                if options.hide {
                    return Err("Duplicate --hide".into());
                }
                options.hide = true;
            }
            "-" if command == "request" => positional.push(arg),
            value if value.starts_with('-') => {
                return Err(format!("Unknown option {value}; use --help"));
            }
            _ => positional.push(arg),
        }
    }
    options.allowed(&command)?;
    let mut request = match command.as_str() {
        "launch" => Request::Launch {
            workspace: options.workspace.as_ref().map(PathBuf::from),
            action: launch::parse(&positional, &options)?,
        },
        "status" => {
            require_empty(&positional)?;
            Request::Status {}
        }
        "raise" => {
            require_empty(&positional)?;
            Request::Raise {}
        }
        "tabs" => {
            require_empty(&positional)?;
            Request::Tabs {
                limit: options.limit.unwrap_or(100),
                offset: options.offset.unwrap_or(0),
            }
        }
        "open" => {
            if positional.is_empty() {
                return Err("open needs one or more paths".into());
            }
            let location = options.location()?;
            let files = positional
                .into_iter()
                .map(|path| OpenFile {
                    path: absolute(path.into()),
                    location: location.clone(),
                    view: options.view.unwrap_or_default(),
                    annotations: Vec::new(),
                })
                .collect();
            Request::Open {
                files,
                background: options.background,
            }
        }
        "review"
            if options.working
                || options.branch
                || options.commit.is_some()
                || options.base.is_some() =>
        {
            let source = match (options.working, options.commit.is_some(), options.branch) {
                (true, false, false) => ChangeReviewSource::Working,
                (false, true, false) => ChangeReviewSource::Commit,
                (false, false, true) => ChangeReviewSource::Branch,
                (false, false, false) => return Err("--base requires --branch".into()),
                _ => return Err("Choose one of --working, --commit REV or --branch".into()),
            };
            if positional.len() > 1 {
                return Err(
                    "review --working|--commit|--branch takes an optional WORKSPACE".into(),
                );
            }
            Request::ChangeReview {
                source,
                rev: options.commit.clone(),
                base: options.base.clone(),
                workspace: options.workspace_or(&positional)?,
                background: options.background,
            }
        }
        "review" => {
            if options.workspace.is_some() {
                return Err("--workspace applies to review --working|--commit|--branch; a manifest's files choose their projects".into());
            }
            let path = absolute(
                one_argument(&positional, "review needs exactly one manifest file")?.into(),
            );
            let contents = read_json_file(&path)?;
            let mut manifest: ReviewManifest =
                transport::from_unique_json(json_bytes(&contents))
                    .map_err(|e| format!("Invalid review manifest: {e}"))?;
            let base = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            for file in &mut manifest.files {
                if !file.path.is_absolute() {
                    file.path = base.join(&file.path);
                }
            }
            Request::Review {
                title: manifest.title,
                note: manifest.note,
                files: manifest.files,
                background: options.background,
            }
        }
        "focus" => Request::Focus {
            tab_id: tab_id(one_argument(&positional, "focus needs exactly one tab ID")?)?,
            location: options.location()?,
            view: options.view,
        },
        "close" => Request::Close {
            tab_ids: ids(&positional, true)?,
        },
        "clear-marks" => Request::ClearMarks {
            tab_ids: ids(&positional, false)?,
        },
        "edit" => Request::Edit {
            tab_id: tab_id(one_argument(
                &positional,
                "edit needs TAB_ID --file TEXT_FILE --revision N",
            )?)?,
            expected_revision: options.required_revision()?,
            edits: Vec::new(),
            set_text: Some(read_input_file(&absolute(
                options
                    .file
                    .as_ref()
                    .ok_or("edit requires --file TEXT_FILE")?
                    .into(),
            ))?),
        },
        "save-as" => {
            if positional.len() != 2 {
                return Err("save-as needs TAB_ID PATH --revision N".into());
            }
            Request::SaveAs {
                tab_id: tab_id(&positional[0])?,
                expected_revision: options.required_revision()?,
                path: absolute((&positional[1]).into()),
            }
        }
        "save" => Request::Save {
            tab_ids: ids(&positional, true)?,
        },
        "workspace" => Request::Workspace {
            path: absolute(
                one_argument(&positional, "workspace needs exactly one directory")?.into(),
            ),
        },
        "panel" => Request::Panel {
            panel: serde_json::from_value(json!(one_argument(
                &positional,
                "panel needs exactly one panel name"
            )?))
            .map_err(|_| "Unknown panel; use --help")?,
            visible: !options.hide,
        },
        "read" => Request::Read {
            tab_id: tab_id(one_argument(&positional, "read needs exactly one tab ID")?)?,
            start_line: options.line.unwrap_or(1),
            start_column: options.column.unwrap_or(1),
            line_count: options.count.unwrap_or(100),
        },
        "selection" => Request::Selection {
            tab_id: tab_id(one_argument(&positional, "selection needs one tab ID")?)?,
            range: None,
            expected_revision: None,
        },
        "settings" => Request::Settings {
            patch: match positional.as_slice() {
                [] => None,
                [setting, value] if setting == "leader-key" => Some(SettingsPatch {
                    leader_key: Some(normalize_leader_key(value)?),
                    ..Default::default()
                }),
                [setting, value] if setting == "swap-jk" => Some(SettingsPatch {
                    swap_jk: Some(match value.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return Err("settings swap-jk requires true or false".into()),
                    }),
                    ..Default::default()
                }),
                [setting, value] if setting == "font" => Some(SettingsPatch {
                    editor_font: Some(serde_json::from_value(json!(value)).map_err(
                        |_| "settings font requires jetbrains-mono, consolas, or cascadia-mono",
                    )?),
                    ..Default::default()
                }),
                [path] => Some(
                    transport::from_unique_json::<SettingsPatch>(json_bytes(&read_json_file(
                        &absolute(path.into()),
                    )?))
                    .map_err(|e| format!("Invalid settings patch: {e}"))?,
                ),
                _ => {
                    return Err(
                        "settings takes PATCH.json, font jetbrains-mono|consolas|cascadia-mono, swap-jk true|false, or leader-key space|comma|backslash|CHAR".into(),
                    );
                }
            },
        },
        "bookmarks" => Request::Bookmarks {
            change: parse_bookmarks(&positional, &options)?,
        },
        "agent-marks" => Request::AgentMarks {
            action: parse_agent_marks(&positional, &options)?,
        },
        "command" => {
            if positional.len() != 2 {
                return Err("command needs TAB_ID COMMAND --revision N".into());
            }
            Request::Command {
                tab_id: tab_id(&positional[0])?,
                command: serde_json::from_value(json!(positional[1]))
                    .map_err(|_| "Unknown editor command; use --help")?,
                expected_revision: options.required_revision()?,
            }
        }
        "find" => {
            if positional.len() != 2 {
                return Err("find needs TAB_ID QUERY".into());
            }
            Request::Find {
                tab_id: tab_id(&positional[0])?,
                query: positional[1].clone(),
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                limit: options.limit.unwrap_or(100),
                offset: options.offset.unwrap_or(0),
            }
        }
        "replace" => {
            if positional.len() != 3 {
                return Err("replace needs TAB_ID QUERY REPLACEMENT --revision N".into());
            }
            Request::Replace {
                tab_id: tab_id(&positional[0])?,
                expected_revision: options.required_revision()?,
                query: positional[1].clone(),
                replacement: positional[2].clone(),
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                all: !options.first,
            }
        }
        "diff" => Request::Diff {
            tab_id: tab_id(one_argument(&positional, "diff needs one tab ID")?)?,
            max_lines: options.count.unwrap_or(500),
        },
        "diagnostics" => Request::Diagnostics {
            tab_id: tab_id(one_argument(&positional, "diagnostics needs one tab ID")?)?,
            limit: options.limit.unwrap_or(100),
            offset: options.offset.unwrap_or(0),
        },
        "search" => {
            if positional.is_empty() || positional.len() > 2 {
                return Err("search needs QUERY and optional WORKSPACE".into());
            }
            Request::Search {
                query: positional[0].clone(),
                workspace: options.workspace_or(&positional[1..])?,
                excludes: if options.no_excludes {
                    Some(Vec::new())
                } else {
                    options.excludes.clone()
                },
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                limit: options.limit.unwrap_or(100),
            }
        }
        "output" => {
            require_empty(&positional)?;
            Request::Output {
                start_line: options.line.unwrap_or(1),
                start_column: options.column.unwrap_or(1),
                line_count: options.count.unwrap_or(100),
                workspace: options.workspace_or(&[])?,
            }
        }
        "git-status" | "git-branches" | "git-history" => {
            if positional.len() > 1 {
                return Err(format!("{command} takes an optional WORKSPACE"));
            }
            let workspace = options.workspace_or(&positional)?;
            let limit = options.limit.unwrap_or(100);
            let offset = options.offset.unwrap_or(0);
            match command.as_str() {
                "git-branches" => Request::GitBranches {
                    workspace,
                    limit,
                    offset,
                },
                "git-history" => Request::GitHistory {
                    workspace,
                    limit,
                    offset,
                },
                _ => Request::GitStatus {
                    workspace,
                    limit,
                    offset,
                },
            }
        }
        "git-switch-branch" => {
            if !(1..=2).contains(&positional.len()) {
                return Err("git-switch-branch needs REF and an optional WORKSPACE; use an exact ref from git-branches".into());
            }
            Request::GitSwitchBranch {
                target: positional[0].clone(),
                workspace: options.workspace_or(&positional[1..])?,
            }
        }

        "new" => {
            if positional.len() > 1 {
                return Err("new takes an optional TITLE, --file TEXT_FILE and --path PATH".into());
            }
            Request::New {
                title: positional.first().cloned(),
                text: options
                    .file
                    .as_ref()
                    .map(|path| read_input_file(&absolute(path.into())))
                    .transpose()?
                    .unwrap_or_default(),
                path: options.path.as_ref().map(|path| absolute(path.into())),
            }
        }
        "task" => Request::Task {
            action: serde_json::from_value(json!(one_argument(
                &positional,
                "task needs check, test, build, run_file or stop"
            )?))
            .map_err(|_| "Unknown task; use --help")?,
            workspace: options.workspace_or(&[])?,
        },
        "notify" => {
            let message = match positional.as_slice() {
                [message] => message.clone(),
                [] => {
                    return Err(
                        "notify needs MESSAGE, e.g. notify \"Tests pass; ready for review\"".into(),
                    );
                }
                _ => return Err("notify takes one MESSAGE; quote text that contains spaces".into()),
            };
            Request::Notify {
                message,
                title: options.title.clone(),
                level: options.level.unwrap_or_default(),
                workspace: options.workspace_or(&[])?,
                open: options.open.as_deref().map(notification_open).transpose()?,
                review: options.review,
            }
        }
        "file" => Request::File {
            action: transport::from_unique_json::<FileAction>(json_bytes(&read_json_file(
                &absolute(one_argument(&positional, "file needs an ACTION.json file")?.into()),
            )?))
            .map_err(|error| format!("Invalid file action: {error}"))?,
        },
        "file-job" => Request::FileJob {
            job_id: one_argument(&positional, "file-job needs one returned job ID")?
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or("File job ID must be a positive integer returned by file/status")?,
        },
        "layout" => Request::Layout {
            change: match positional.as_slice() {
                [] => LayoutAction::Status {},
                [path] => transport::from_unique_json::<LayoutAction>(json_bytes(&read_json_file(
                    &absolute(path.into()),
                )?))
                .map_err(|e| format!("Invalid layout change: {e}"))?,
                _ => return Err("layout takes an optional CHANGE.json".into()),
            },
        },
        "request" => {
            let input = one_argument(&positional, "request needs a JSON file or '-' for stdin")?;
            let contents = if input == "-" {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(1_048_577)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                bytes
            } else {
                read_json_file(&absolute(input.into()))?
            };
            if contents.len() > 1_048_576 {
                return Err("Request exceeds 1 MiB".into());
            }
            let contents = decode_input(&contents)?;
            let mut request: Request = transport::from_unique_json(contents.as_bytes())
                .map_err(|e| format!("Invalid request: {e}"))?;
            resolve_paths(&mut request);
            request
        }
        _ => return Err(format!("Unknown command {command}; use --help")),
    };
    resolve_paths(&mut request);
    validate(&request)?;
    // A background presentation never opens a new window in front of the user.
    let start = !no_start
        && matches!(
            request,
            Request::Open {
                background: false,
                ..
            } | Request::Review {
                background: false,
                ..
            } | Request::Workspace { .. }
        );
    Ok(Parsed::Command { request, start })
}

#[derive(Default)]
struct Options {
    workspace: Option<String>,
    file: Option<String>,
    path: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    view: Option<DocumentView>,
    limit: Option<usize>,
    offset: Option<usize>,
    count: Option<usize>,
    revision: Option<u64>,
    tab: Option<u64>,
    label: Option<String>,
    note: Option<String>,
    batch: Option<u64>,
    request_id: Option<String>,
    excludes: Option<Vec<String>>,
    no_excludes: bool,
    case_sensitive: bool,
    whole_word: bool,
    first: bool,
    hide: bool,
    working: bool,
    branch: bool,
    commit: Option<String>,
    base: Option<String>,
    background: bool,
    title: Option<String>,
    level: Option<NotificationLevel>,
    open: Option<String>,
    review: Option<NotificationReview>,
}
impl Options {
    /// The project from `--workspace` or a legacy positional WORKSPACE, never both.
    fn workspace_or(&self, positional: &[String]) -> Result<Option<PathBuf>, String> {
        match (&self.workspace, positional) {
            (Some(_), [_, ..]) => {
                Err("Give the project either as WORKSPACE or with --workspace, not both".into())
            }
            (Some(path), []) => Ok(Some(absolute(path.into()))),
            (None, [path, ..]) => Ok(Some(absolute(path.into()))),
            (None, []) => Ok(None),
        }
    }
    fn allowed(&self, command: &str) -> Result<(), String> {
        let definition = help::command(command)
            .ok_or_else(|| format!("Unknown command {command}; use vedit-cli --help"))?;
        for (present, flag) in [
            (self.workspace.is_some(), "--workspace"),
            (self.line.is_some(), "--line"),
            (self.column.is_some(), "--column"),
            (self.file.is_some(), "--file"),
            (self.path.is_some(), "--path"),
            (self.view.is_some(), "--preview"),
            (self.limit.is_some(), "--limit"),
            (self.offset.is_some(), "--offset"),
            (self.count.is_some(), "--count"),
            (self.revision.is_some(), "--revision"),
            (self.tab.is_some(), "--tab"),
            (self.label.is_some(), "--label"),
            (self.note.is_some(), "--note"),
            (self.batch.is_some(), "--batch"),
            (self.request_id.is_some(), "--request-id"),
            (self.excludes.is_some(), "--exclude"),
            (self.no_excludes, "--no-excludes"),
            (self.case_sensitive, "--case-sensitive"),
            (self.whole_word, "--whole-word"),
            (self.first, "--first"),
            (self.hide, "--hide"),
            (self.working, "--working"),
            (self.branch, "--branch"),
            (self.commit.is_some(), "--commit"),
            (self.base.is_some(), "--base"),
            (self.background, "--background"),
            (self.title.is_some(), "--title"),
            (self.level.is_some(), "--level"),
            (self.open.is_some(), "--open"),
            (self.review.is_some(), "--review"),
        ] {
            if present && !definition.flags.contains(&flag) {
                return Err(format!(
                    "{flag} is not supported by {command}; use vedit-cli {command} --help"
                ));
            }
        }
        if self.column.is_some() && self.line.is_none() {
            return Err("--column requires --line".into());
        }
        Ok(())
    }
    fn location(&self) -> Result<Option<Location>, String> {
        if self.column.is_some() && self.line.is_none() {
            return Err("--column requires --line".into());
        }
        Ok(self.line.map(|line| Location {
            line,
            column: self.column.unwrap_or(1),
        }))
    }
    fn required_revision(&self) -> Result<u64, String> {
        self.revision.ok_or_else(|| {
            "This command requires --revision N from read/tabs; stale edits are refused".into()
        })
    }
}
fn set_flag(field: &mut bool, name: &str) -> Result<(), String> {
    if *field {
        return Err(format!("Duplicate {name}"));
    }
    *field = true;
    Ok(())
}

fn parse_bookmarks(positional: &[String], options: &Options) -> Result<BookmarkAction, String> {
    let change = match positional {
        [] => BookmarkAction::List { limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0) },
        [action] if action == "list" => BookmarkAction::List { limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0) },
        [action] if action == "toggle" => BookmarkAction::Toggle {
            tab_id: options.tab,
            label: options.label.clone(),
        },
        [action, id] if action == "remove" => BookmarkAction::Remove { id: bookmark_id(id)? },
        [action, id] if action == "rename" => BookmarkAction::Rename {
            id: bookmark_id(id)?,
            label: options.label.clone().ok_or("bookmarks rename needs --label TEXT")?,
        },
        [action, id] if action == "jump" => BookmarkAction::Jump { id: bookmark_id(id)? },
        _ => return Err("bookmarks takes list, toggle [--tab ID] [--label TEXT], remove ID, rename ID --label TEXT, or jump ID".into()),
    };
    if options.tab.is_some() && !matches!(change, BookmarkAction::Toggle { .. }) {
        return Err("--tab is supported only by bookmarks toggle".into());
    }
    if options.label.is_some()
        && !matches!(
            change,
            BookmarkAction::Toggle { .. } | BookmarkAction::Rename { .. }
        )
    {
        return Err("--label is supported only by bookmarks toggle or rename".into());
    }
    if (options.limit.is_some() || options.offset.is_some())
        && !matches!(change, BookmarkAction::List { .. })
    {
        return Err("--limit and --offset are supported only by bookmarks list".into());
    }
    Ok(change)
}

/// `PATH` or `PATH:LINE`; a drive letter's colon is never read as a line.
fn notification_open(value: &str) -> Result<NotificationOpen, String> {
    let (path, location) = match value.rsplit_once(':') {
        Some((path, line))
            if !path.is_empty()
                && !line.is_empty()
                && line.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            let line = line
                .parse::<usize>()
                .ok()
                .filter(|line| *line > 0)
                .ok_or("--open LINE must be a positive integer, e.g. src/main.rs:42")?;
            (path, Some(Location { line, column: 1 }))
        }
        _ => (value, None),
    };
    if path.trim().is_empty() {
        return Err("--open needs PATH or PATH:LINE".into());
    }
    Ok(NotificationOpen {
        path: absolute(path.into()),
        location,
    })
}

fn bookmark_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "Bookmark ID must be a positive integer returned by bookmarks".into())
}

fn parse_agent_marks(positional: &[String], options: &Options) -> Result<AgentMarksAction, String> {
    let action = match positional {
        [] => AgentMarksAction::List {
            limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action] if action == "list" => AgentMarksAction::List {
            limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action] if action == "take" => AgentMarksAction::Take {
            request_id: options.request_id.clone(), limit: options.limit.unwrap_or(100),
        },
        [action] if action == "batch" => AgentMarksAction::Batch {
            batch_id: None, limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action, id] if action == "batch" => AgentMarksAction::Batch {
            batch_id: Some(agent_mark_id(id)?), limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action, id] if action == "text" => AgentMarksAction::Text {
            id: agent_mark_id(id)?, batch_id: options.batch,
            offset: options.offset.unwrap_or(0), count: options.count.unwrap_or(4096),
        },
        [action, id] if action == "ack" => AgentMarksAction::Ack { batch_id: agent_mark_id(id)? },
        [action] if action == "restore" => AgentMarksAction::Restore { batch_id: None },
        [action, id] if action == "restore" => AgentMarksAction::Restore { batch_id: Some(agent_mark_id(id)?) },
        [action] if action == "clear" => AgentMarksAction::Clear {},
        [action, id] if action == "remove" => AgentMarksAction::Remove { id: agent_mark_id(id)? },
        [action, id, note] if action == "note" => AgentMarksAction::Note { id: agent_mark_id(id)?, note: note.clone() },
        [action] if action == "add" => AgentMarksAction::Add { tab_id: options.tab, note: options.note.clone() },
        _ => return Err("agent-marks takes list, take, batch [BATCH_ID], text MARK_ID, ack BATCH_ID, restore [BATCH_ID], clear, remove MARK_ID, note MARK_ID TEXT, or add; use --help".into()),
    };
    for (present, allowed, flag) in [
        (
            options.limit.is_some(),
            matches!(
                action,
                AgentMarksAction::List { .. }
                    | AgentMarksAction::Take { .. }
                    | AgentMarksAction::Batch { .. }
            ),
            "--limit",
        ),
        (
            options.offset.is_some(),
            matches!(
                action,
                AgentMarksAction::List { .. }
                    | AgentMarksAction::Batch { .. }
                    | AgentMarksAction::Text { .. }
            ),
            "--offset",
        ),
        (
            options.count.is_some(),
            matches!(action, AgentMarksAction::Text { .. }),
            "--count",
        ),
        (
            options.batch.is_some(),
            matches!(action, AgentMarksAction::Text { .. }),
            "--batch",
        ),
        (
            options.request_id.is_some(),
            matches!(action, AgentMarksAction::Take { .. }),
            "--request-id",
        ),
        (
            options.tab.is_some(),
            matches!(action, AgentMarksAction::Add { .. }),
            "--tab",
        ),
        (
            options.note.is_some(),
            matches!(action, AgentMarksAction::Add { .. }),
            "--note",
        ),
    ] {
        if present && !allowed {
            return Err(format!(
                "{flag} is not supported by this agent-marks action"
            ));
        }
    }
    Ok(action)
}

fn agent_mark_id(value: &str) -> Result<u64, String> {
    value.parse().ok().filter(|id| *id > 0).ok_or_else(|| {
        "Agent mark/batch ID must be a positive integer returned by agent-marks".into()
    })
}

/// Accepted aliases denote exactly one key; never infer a nearby key spelling.
pub fn normalize_leader_key(value: &str) -> Result<String, String> {
    let normalized = match value {
        "space" => " ",
        "comma" => ",",
        "backslash" => "\\",
        _ => value,
    };
    let mut chars = normalized.chars();
    match (chars.next(), chars.next()) {
        (Some(key), None) if !key.is_control() => Ok(normalized.to_owned()),
        _ => Err("leader_key needs one printable character or space|comma|backslash".into()),
    }
}
/// Compile bounded gitignore-style rules before a request can change settings.
/// This pure helper is also used by the workspace search worker.
pub fn normalize_search_excludes(values: &[String]) -> Result<Vec<String>, String> {
    if values.len() > 128 {
        return Err("search_excludes/excludes accepts at most 128 folder or glob rules".into());
    }
    if values.iter().map(String::len).sum::<usize>() > 16 * 1024 {
        return Err("search_excludes/excludes rules exceed 16 KiB in total".into());
    }
    let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
    builder.allow_unclosed_class(false);
    builder
        .case_insensitive(cfg!(windows))
        .map_err(|error| error.to_string())?;
    let mut normalized = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if value.len() > 1024 {
            return Err(format!("Exclude rule {} exceeds 1024 bytes", index + 1));
        }
        let rule = value.trim().replace('\\', "/");
        if rule.is_empty() {
            continue;
        }
        if rule.starts_with('!') {
            return Err(format!(
                "Exclude rule {} uses unsupported ! negation; remove it or use excludes:[] for this search",
                index + 1
            ));
        }
        if rule.chars().any(char::is_control) {
            return Err(format!(
                "Exclude rule {} must be a single printable folder or glob pattern",
                index + 1
            ));
        }
        let literal_hash = rule.starts_with('#').then(|| format!("\\{rule}"));
        builder.add_line(None, literal_hash.as_deref().unwrap_or(&rule))
            .map_err(|error| format!("Invalid exclude rule {}: {error}; correct the folder/glob pattern before retrying", index + 1))?;
        if !normalized.contains(&rule) {
            normalized.push(rule);
        }
    }
    builder
        .build()
        .map_err(|error| format!("Invalid exclude rules: {error}"))?;
    Ok(normalized)
}
fn set_option<T>(field: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if field.is_some() {
        return Err(format!("Duplicate or contradictory {name}"));
    }
    *field = Some(value);
    Ok(())
}
fn number(value: Option<String>, flag: &str) -> Result<usize, String> {
    value
        .ok_or_else(|| format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag} needs a nonnegative integer"))
}
fn tab_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "Tab ID must be a positive integer returned by tabs/status".into())
}
fn ids(values: &[String], required: bool) -> Result<Vec<u64>, String> {
    if required && values.is_empty() {
        return Err("One or more tab IDs are required".into());
    }
    values.iter().map(|v| tab_id(v)).collect()
}
fn require_empty(values: &[String]) -> Result<(), String> {
    if values.is_empty() {
        Ok(())
    } else {
        Err("Unexpected positional arguments".into())
    }
}
fn one_argument<'a>(values: &'a [String], message: &str) -> Result<&'a str, String> {
    if values.len() == 1 {
        Ok(&values[0])
    } else {
        Err(message.into())
    }
}
fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
fn read_json_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1_048_576 {
        return Err("Input file exceeds 1 MiB; use smaller range edits or requests".into());
    }
    decode_input(&bytes).map(String::into_bytes)
}
fn json_bytes(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

fn read_input_file(path: &std::path::Path) -> Result<String, String> {
    String::from_utf8(read_json_file(path)?).map_err(|error| error.to_string())
}

fn decode_input(bytes: &[u8]) -> Result<String, String> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let big_endian = bytes[0] == 0xfe;
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err(
                "Incomplete UTF-16 input; save the complete file as UTF-8 or UTF-16 with BOM"
                    .into(),
            );
        }
        let units: Vec<_> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                let pair = [pair[0], pair[1]];
                if big_endian {
                    u16::from_be_bytes(pair)
                } else {
                    u16::from_le_bytes(pair)
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| {
            "Invalid UTF-16 input; save the file as valid UTF-8 or UTF-16 with BOM".to_owned()
        })?
    } else {
        std::str::from_utf8(json_bytes(bytes))
            .map_err(|_| "Input is not UTF-8; save it as UTF-8 or UTF-16 with BOM".to_owned())?
            .to_owned()
    };
    if text.len() > 1_048_576 {
        return Err("Decoded input exceeds 1 MiB; use smaller range edits or requests".into());
    }
    Ok(text)
}

/// Expand option=value only where the parser owns the token, never inside data.
fn normalize_equals(args: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    let mut literal = false;
    let mut expects_value = false;
    for arg in args {
        if literal || expects_value {
            expects_value = false;
            result.push(arg);
            continue;
        }
        if arg == "--" {
            literal = true;
        }
        if let Some((flag, value)) = arg.split_once('=')
            && help::takes_value(flag)
        {
            result.push(flag.to_owned());
            result.push(value.to_owned());
            continue;
        }
        expects_value = help::takes_value(&arg);
        result.push(arg);
    }
    result
}
fn resolve_paths(request: &mut Request) {
    match request {
        Request::Open { files, .. } | Request::Review { files, .. } => {
            for file in files {
                file.path = absolute(file.path.clone());
            }
        }
        Request::Workspace { path } => *path = absolute(path.clone()),
        Request::New {
            path: Some(path), ..
        }
        | Request::SaveAs { path, .. } => *path = absolute(path.clone()),
        Request::File { action } => match action {
            FileAction::Rename { path, .. } | FileAction::Delete { path } => {
                *path = absolute(path.clone())
            }
            FileAction::CreateFolder { parent, .. } => *parent = absolute(parent.clone()),
        },
        Request::Search {
            workspace: Some(path),
            ..
        }
        | Request::GitBranches {
            workspace: Some(path),
            ..
        }
        | Request::GitHistory {
            workspace: Some(path),
            ..
        }
        | Request::GitSwitchBranch {
            workspace: Some(path),
            ..
        }
        | Request::GitStatus {
            workspace: Some(path),
            ..
        }
        | Request::ChangeReview {
            workspace: Some(path),
            ..
        }
        | Request::Launch {
            workspace: Some(path),
            ..
        }
        | Request::Task {
            workspace: Some(path),
            ..
        }
        | Request::Output {
            workspace: Some(path),
            ..
        } => *path = absolute(path.clone()),
        Request::Notify {
            workspace, open, ..
        } => {
            if let Some(path) = workspace {
                *path = absolute(path.clone());
            }
            if let Some(open) = open {
                open.path = absolute(open.path.clone());
            }
        }
        Request::Batch { actions, .. } => {
            for action in actions {
                resolve_paths(action);
            }
        }
        _ => {}
    }
}

pub fn validate(request: &Request) -> Result<(), String> {
    match request {
        Request::Launch { workspace, action } => {
            if let Some(path) = workspace {
                validate_path(path)?;
            }
            match action {
                launch::Action::Configure { configuration, .. } => configuration.validate()?,
                launch::Action::Select { id } | launch::Action::Start { id: Some(id) } => {
                    launch::validate_id(id)?
                }
                launch::Action::Output {
                    start_line,
                    start_column,
                    line_count,
                } if *start_line == 0 || *start_column == 0 || !(1..=500).contains(line_count) => {
                    return Err("Launch output uses 1-based positions and 1..500 lines.".into());
                }
                _ => {}
            }
        }
        Request::Open { files, .. } | Request::Review { files, .. } => {
            if files.is_empty() || files.len() > MAX_FILES {
                return Err(format!("Supply 1..{MAX_FILES} files"));
            }
            let mut total = 0;
            for file in files {
                if file.path.as_os_str().is_empty() || !file.path.is_absolute() {
                    return Err("File paths must be absolute; CLI resolves relative paths".into());
                }
                if file.view == DocumentView::Preview && !is_markdown(&file.path) {
                    return Err(format!(
                        "Preview requires Markdown: {}",
                        file.path.display()
                    ));
                }
                if let Some(location) = &file.location {
                    validate_location(location)?;
                }
                total += file.annotations.len();
                let mut ids = std::collections::HashSet::new();
                for annotation in &file.annotations {
                    validate_location(&annotation.start)?;
                    if let Some(end) = &annotation.end {
                        validate_location(end)?;
                        if (end.line, end.column) < (annotation.start.line, annotation.start.column)
                        {
                            return Err("Annotation end must not precede its start".into());
                        }
                    }
                    if annotation.note.len() > 16_384 || annotation.id.len() > 256 {
                        return Err("Annotation note/ID exceeds its size limit".into());
                    }
                    if !annotation.id.is_empty() && !ids.insert(&annotation.id) {
                        return Err("Duplicate annotation ID within one file".into());
                    }
                }
            }
            if total > MAX_ANNOTATIONS {
                return Err(format!(
                    "A batch supports at most {MAX_ANNOTATIONS} annotations"
                ));
            }
            if let Request::Review { title, note, .. } = request
                && (title.trim().is_empty() || title.len() > 256 || note.len() > 16_384)
            {
                return Err(
                    "Review needs a nonempty title (max 256 bytes); note max 16 KiB".into(),
                );
            }
        }
        Request::Focus {
            tab_id, location, ..
        } => {
            validate_id(*tab_id)?;
            if let Some(location) = location {
                validate_location(location)?;
            }
        }
        Request::Close { tab_ids } | Request::Save { tab_ids } => {
            if tab_ids.is_empty() {
                return Err("Supply one or more tab IDs".into());
            }
            validate_ids(tab_ids)?;
        }
        Request::ClearMarks { tab_ids } => validate_ids(tab_ids)?,
        Request::Tabs { limit, .. } => {
            if *limit == 0 || *limit > 100 {
                return Err("tabs limit must be 1..100".into());
            }
        }
        Request::Read {
            tab_id,
            start_line,
            line_count,
            start_column,
        } => {
            validate_id(*tab_id)?;
            if *start_line == 0 || *start_column == 0 || *line_count == 0 || *line_count > 500 {
                return Err("read uses positive start_line/start_column and 1..500 lines".into());
            }
        }
        Request::Workspace { path } if !path.is_absolute() => {
            return Err("Workspace path must be absolute".into());
        }
        Request::New { title, text, path } => {
            if title
                .as_ref()
                .is_some_and(|t| t.trim().is_empty() || t.len() > 256)
            {
                return Err("Title must contain 1..256 bytes".into());
            }
            if text.len() > 8 * 1024 * 1024 {
                return Err("Document exceeds 8 MiB".into());
            }
            if let Some(path) = path {
                validate_path(path)?;
            }
        }
        Request::SaveAs { tab_id, path, .. } => {
            validate_id(*tab_id)?;
            validate_path(path)?;
        }
        Request::Edit {
            tab_id,
            edits,
            set_text,
            ..
        } => {
            validate_id(*tab_id)?;
            if set_text.is_some() == !edits.is_empty() {
                return Err(
                    "Supply either set_text (including empty string) or 1..256 edits, never both"
                        .into(),
                );
            }
            if edits.len() > 256 {
                return Err("At most 256 edits in one atomic document change".into());
            }
            let mut bytes = set_text.as_ref().map_or(0, String::len);
            for edit in edits {
                validate_range(&edit.range)?;
                bytes += edit.text.len();
            }
            if bytes > 8 * 1024 * 1024 {
                return Err("Replacement text exceeds 8 MiB".into());
            }
        }
        Request::Selection {
            tab_id,
            range,
            expected_revision,
        } => {
            validate_id(*tab_id)?;
            if let Some(range) = range {
                validate_range(range)?;
                if expected_revision.is_none() {
                    return Err(
                        "Setting selection requires expected_revision from read/tabs".into(),
                    );
                }
            } else if expected_revision.is_some() {
                return Err("expected_revision is only valid with a selection range".into());
            }
        }
        Request::Settings { patch: Some(patch) } => {
            if patch
                .multicursor
                .as_ref()
                .and_then(|p| p.limit)
                .is_some_and(|limit| !(100..=10_000).contains(&limit))
            {
                return Err(
                    "multicursor.limit must be 100..10000; no settings were changed".into(),
                );
            }
            if let Some(bindings) = &patch.keybindings
                && (bindings.len() > 128
                    || bindings.iter().any(|(id, binding)| {
                        id.is_empty()
                            || id.len() > 80
                            || binding.as_ref().is_some_and(|binding| {
                                !(1..=MAX_SHORTCUT_ALIASES).contains(&binding.values().count())
                                    || binding.values().any(|value| {
                                        value.is_empty() || value.len() > MAX_SHORTCUT_BYTES
                                    })
                            })
                    }))
            {
                return Err("keybindings supports at most 128 action IDs (1..80 bytes), each set to null or 1..8 shortcut strings (1..100 bytes each); no settings were changed".into());
            }
            if let Some(excludes) = &patch.search_excludes {
                normalize_search_excludes(excludes)?;
            }
            if let Some(leader_key) = &patch.leader_key {
                normalize_leader_key(leader_key)?;
            }
            if patch
                .font_size
                .is_some_and(|n| !n.is_finite() || !(10.0..=32.0).contains(&n))
            {
                return Err("font_size must be 10..32".into());
            }
            if patch
                .ui_scale
                .is_some_and(|n| !n.is_finite() || !(0.75..=2.0).contains(&n))
            {
                return Err("ui_scale must be 0.75..2.0".into());
            }
            if patch
                .line_height
                .is_some_and(|n| !n.is_finite() || !(1.0..=2.0).contains(&n))
            {
                return Err("line_height must be 1.0..2.0".into());
            }
            if patch
                .autosave_delay_ms
                .is_some_and(|n| !(300..=5000).contains(&n))
            {
                return Err("autosave_delay_ms must be 300..5000".into());
            }
            if patch.tab_width.is_some_and(|n| !(1..=8).contains(&n)) {
                return Err("tab_width must be 1..8".into());
            }
            if let Some(widths) = &patch.language_indent_widths {
                validate_indent_languages(widths.keys().map(String::as_str), widths.len())?;
                if widths.values().any(|width| !(1..=8).contains(width)) {
                    return Err("language_indent_widths values must be 1..8".into());
                }
            }
            if let Some(styles) = &patch.language_indent_with_tabs {
                validate_indent_languages(styles.keys().map(String::as_str), styles.len())?;
            }
        }
        Request::Command { tab_id, .. } => validate_id(*tab_id)?,
        Request::Find {
            tab_id,
            query,
            limit,
            offset,
            ..
        } => {
            validate_id(*tab_id)?;
            validate_query(query)?;
            validate_page(*limit, *offset)?;
        }
        Request::Replace {
            tab_id,
            query,
            replacement,
            ..
        } => {
            validate_id(*tab_id)?;
            validate_query(query)?;
            if replacement.len() > 1024 * 1024 {
                return Err("Replacement exceeds 1 MiB".into());
            }
        }
        Request::Diff { tab_id, max_lines } => {
            validate_id(*tab_id)?;
            if !(1..=1000).contains(max_lines) {
                return Err("max_lines must be 1..1000".into());
            }
        }
        Request::Diagnostics {
            tab_id,
            limit,
            offset,
        } => {
            validate_id(*tab_id)?;
            validate_page(*limit, *offset)?;
        }
        Request::Search {
            query,
            workspace,
            excludes,
            limit,
            ..
        } => {
            validate_query(query)?;
            validate_page(*limit, 0)?;
            if let Some(path) = workspace {
                validate_path(path)?;
            }
            if let Some(excludes) = excludes {
                normalize_search_excludes(excludes)?;
            }
        }
        Request::Output {
            start_line,
            line_count,
            start_column,
            workspace,
        } => {
            if *start_line == 0 || *start_column == 0 || !(1..=500).contains(line_count) {
                return Err("output uses positive start_line/start_column and 1..500 lines".into());
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::Task {
            workspace: Some(path),
            ..
        } => validate_path(path)?,
        Request::Notify {
            message,
            title,
            workspace,
            open,
            review,
            ..
        } => {
            if open.is_some() && review.is_some() {
                return Err("A notification opens one thing when clicked: give --open or --review, not both. Nothing was sent".into());
            }
            if message.trim().is_empty() || message.len() > MAX_NOTIFICATION_MESSAGE {
                return Err(format!(
                    "A notification message needs 1..{MAX_NOTIFICATION_MESSAGE} bytes of text"
                ));
            }
            if message
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
            {
                return Err("A notification message can contain line breaks and tabs, but no other control characters".into());
            }
            if let Some(title) = title
                && (title.trim().is_empty()
                    || title.len() > MAX_NOTIFICATION_TITLE
                    || title.chars().any(char::is_control))
            {
                return Err(format!(
                    "A notification title is one line of 1..{MAX_NOTIFICATION_TITLE} bytes"
                ));
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
            if let Some(open) = open {
                validate_path(&open.path)?;
                if let Some(location) = &open.location {
                    validate_location(location)?;
                }
            }
        }
        Request::GitStatus {
            workspace,
            limit,
            offset,
        }
        | Request::GitBranches {
            workspace,
            limit,
            offset,
        }
        | Request::GitHistory {
            workspace,
            limit,
            offset,
        } => {
            validate_page(*limit, *offset)?;
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::ChangeReview {
            source,
            rev,
            base,
            workspace,
            ..
        } => {
            match source {
                ChangeReviewSource::Commit if rev.is_none() => {
                    return Err("A commit review needs rev, e.g. HEAD or a commit hash".into());
                }
                ChangeReviewSource::Working | ChangeReviewSource::Branch if rev.is_some() => {
                    return Err("rev applies only to source commit".into());
                }
                ChangeReviewSource::Working | ChangeReviewSource::Commit if base.is_some() => {
                    return Err("base applies only to source branch".into());
                }
                _ => {}
            }
            for revision in rev.iter().chain(base) {
                // Mirrors git::validate_review_revision; vedit-cli has no Git layer.
                if revision.is_empty()
                    || revision.len() > 256
                    || revision.starts_with('-')
                    || revision.chars().any(char::is_control)
                {
                    return Err(
                        "A revision has 1..256 bytes, no leading '-' and no control characters"
                            .into(),
                    );
                }
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::GitSwitchBranch { target, workspace } => {
            if !target.starts_with("refs/heads/") && !target.starts_with("refs/remotes/")
                || target.contains(['\n', '\r', '\0'])
                || target.len() > 4096
            {
                return Err(
                    "Use an exact refs/heads/... or refs/remotes/... ref from git-branches".into(),
                );
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::Bookmarks { change } => match change {
            BookmarkAction::List { limit, offset } => validate_page(*limit, *offset)?,
            BookmarkAction::Toggle { tab_id, label } => {
                if let Some(tab_id) = tab_id {
                    validate_id(*tab_id)?;
                }
                if let Some(label) = label {
                    validate_bookmark_label(label)?;
                }
            }
            BookmarkAction::Remove { id } | BookmarkAction::Jump { id } => {
                validate_bookmark_id(*id)?
            }
            BookmarkAction::Rename { id, label } => {
                validate_bookmark_id(*id)?;
                validate_bookmark_label(label)?;
            }
        },
        Request::AgentMarks { action } => match action {
            AgentMarksAction::List { limit, .. } | AgentMarksAction::Batch { limit, .. } => {
                validate_agent_mark_page(*limit)?;
                if let AgentMarksAction::Batch {
                    batch_id: Some(id), ..
                } = action
                {
                    validate_agent_mark_id(*id)?;
                }
            }
            AgentMarksAction::Take { request_id, limit } => {
                validate_agent_mark_page(*limit)?;
                if let Some(request_id) = request_id
                    && (request_id.is_empty()
                        || request_id.len() > 128
                        || request_id.chars().any(char::is_control))
                {
                    return Err(
                        "request_id must contain 1..128 UTF-8 bytes without control characters"
                            .into(),
                    );
                }
            }
            AgentMarksAction::Text {
                id,
                batch_id,
                count,
                ..
            } => {
                validate_agent_mark_id(*id)?;
                if let Some(batch_id) = batch_id {
                    validate_agent_mark_id(*batch_id)?;
                }
                if !(1..=65536).contains(count) {
                    return Err("Agent mark text count must be 1..65536 Unicode characters; offset must be inside the captured snippet".into());
                }
            }
            AgentMarksAction::Ack { batch_id } => validate_agent_mark_id(*batch_id)?,
            AgentMarksAction::Restore { batch_id } => {
                if let Some(batch_id) = batch_id {
                    validate_agent_mark_id(*batch_id)?;
                }
            }
            AgentMarksAction::Remove { id } => validate_agent_mark_id(*id)?,
            AgentMarksAction::Note { id, note } => {
                validate_agent_mark_id(*id)?;
                validate_agent_mark_note(note)?;
            }
            AgentMarksAction::Add { tab_id, note } => {
                if let Some(tab_id) = tab_id {
                    validate_id(*tab_id)?;
                }
                if let Some(note) = note {
                    validate_agent_mark_note(note)?;
                }
            }
            AgentMarksAction::Clear {} => {}
        },
        Request::Layout { change } => match change {
            LayoutAction::Status {} => {}
            LayoutAction::Split {
                pane_id, tab_id, ..
            } => {
                if let Some(id) = pane_id {
                    validate_id(*id)?;
                }
                if let Some(id) = tab_id {
                    validate_id(*id)?;
                }
            }
            LayoutAction::Focus { pane_id } | LayoutAction::Close { pane_id } => {
                validate_id(*pane_id)?
            }
            LayoutAction::Assign {
                pane_id, tab_id, ..
            }
            | LayoutAction::CloseTab { pane_id, tab_id } => {
                validate_id(*pane_id)?;
                validate_id(*tab_id)?;
            }
            LayoutAction::MoveTab {
                source_pane_id,
                pane_id,
                tab_id,
                index,
            } => {
                validate_id(*source_pane_id)?;
                validate_id(*pane_id)?;
                validate_id(*tab_id)?;
                if *index > 100_000 {
                    return Err(
                        "Tab index must be 0..100000 and within the target tab group".into(),
                    );
                }
            }
            LayoutAction::Resize { pane_id, ratio } => {
                validate_id(*pane_id)?;
                if !ratio.is_finite() || !(0.15..=0.85).contains(ratio) {
                    return Err("Split ratio must be 0.15..0.85".into());
                }
            }
        },
        Request::File { action } => {
            let (path, name) = match action {
                FileAction::Rename { path, name } => (path, Some(name)),
                FileAction::CreateFolder { parent, name } => (parent, Some(name)),
                FileAction::Delete { path } => (path, None),
            };
            validate_path(path)?;
            if let Some(name) = name
                && (name.trim().is_empty()
                    || name.len() > 255
                    || name.contains(['\\', '/'])
                    || matches!(name.as_str(), "." | ".."))
            {
                return Err(
                    "File action name must be a single new name (1..255 bytes), never a path"
                        .into(),
                );
            }
        }
        Request::FileJob { job_id } => validate_id(*job_id)?,
        Request::Batch { actions, .. } => {
            if actions.is_empty() || actions.len() > 100 {
                return Err("Batch requires 1..100 ordered actions".into());
            }
            for action in actions {
                if matches!(
                    action,
                    Request::Batch { .. }
                        | Request::Search { .. }
                        | Request::File { .. }
                        | Request::GitBranches { .. }
                        | Request::GitHistory { .. }
                        | Request::GitSwitchBranch { .. }
                        | Request::ChangeReview { .. }
                        | Request::Launch { .. }
                ) {
                    return Err("Nested batches, background disk search and asynchronous file actions cannot be inside batch; run search/file/git-branches/git-history/git-switch-branch/change review separately".into());
                }
                validate(action)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate_query(query: &str) -> Result<(), String> {
    if query.is_empty() || query.len() > 16_384 {
        Err("Literal search query must contain 1..16384 bytes".into())
    } else {
        Ok(())
    }
}

fn validate_bookmark_label(label: &str) -> Result<(), String> {
    if label.len() > 256 || label.chars().any(char::is_control) {
        Err("Bookmark label must be single-line text of at most 256 UTF-8 bytes".into())
    } else {
        Ok(())
    }
}

fn validate_bookmark_id(id: u64) -> Result<(), String> {
    if id == 0 {
        Err("Bookmark ID must be a positive integer returned by bookmarks".into())
    } else {
        Ok(())
    }
}
fn validate_agent_mark_id(id: u64) -> Result<(), String> {
    if id == 0 {
        Err("Agent mark/batch ID must be a positive integer returned by agent-marks".into())
    } else {
        Ok(())
    }
}
fn validate_agent_mark_note(note: &str) -> Result<(), String> {
    if note.chars().count() > 4096 {
        Err("Agent mark notes accept at most 4096 Unicode characters".into())
    } else {
        Ok(())
    }
}
fn validate_agent_mark_page(limit: usize) -> Result<(), String> {
    if !(1..=500).contains(&limit) {
        Err("Agent marks page limit must be 1..500".into())
    } else {
        Ok(())
    }
}
fn validate_page(limit: usize, offset: usize) -> Result<(), String> {
    if !(1..=500).contains(&limit) || offset > 100_000 {
        Err("limit must be 1..500; offset must be <=100000".into())
    } else {
        Ok(())
    }
}
fn validate_path(path: &std::path::Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        Err("Paths must be absolute; CLI resolves relative paths".into())
    } else {
        Ok(())
    }
}
fn validate_range(range: &TextRange) -> Result<(), String> {
    validate_location(&range.start)?;
    validate_location(&range.end)?;
    if (range.end.line, range.end.column) < (range.start.line, range.start.column) {
        return Err("Edit/selection exclusive end must not precede start".into());
    }
    Ok(())
}
fn validate_location(location: &Location) -> Result<(), String> {
    if location.line == 0 || location.column == 0 {
        Err("Lines and columns are 1-based positive integers".into())
    } else {
        Ok(())
    }
}
fn validate_id(id: u64) -> Result<(), String> {
    if id == 0 {
        Err("Tab ID must be positive".into())
    } else {
        Ok(())
    }
}
fn validate_ids(ids: &[u64]) -> Result<(), String> {
    if ids.len() > MAX_FILES {
        return Err("At most 100 tab IDs per request".into());
    }
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        validate_id(*id)?;
        if !seen.insert(id) {
            return Err("Duplicate tab ID".into());
        }
    }
    Ok(())
}
pub fn is_markdown(path: &std::path::Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
    })
}

pub fn execute(request: &Request, start: bool) -> Result<Response, String> {
    // Refuse an ambiguous destination before any startup or mutation.
    if crate::profile::selected_window().is_none() && running_windows().len() > 1 {
        return send(request);
    }
    if transport::endpoint_alive() || !start {
        return send(request);
    }
    if crate::profile::selected_window().is_none() {
        // Startup can also recover additional windows. Pin this request to the
        // main window we start, rather than becoming ambiguous partway through.
        crate::profile::select_window("main".into())?;
    }
    if crate::profile::window_id() != "main" && !crate::profile::recovery_directory().is_dir() {
        return Err("The selected window has no saved session. Use vedit-cli windows to choose a window, or vedit --new-window to create one. No request was sent.".into());
    }
    {
        let executable = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("CLI executable has no parent")?
            .join(if cfg!(windows) { "vedit.exe" } else { "vedit" });
        if !executable.is_file() {
            return Err(format!(
                "vEdit is not running and {} was not found. Start the app, or put both executables in the same directory.",
                executable.display()
            ));
        }
        let mut child = spawn::spawn_window(&executable, crate::profile::window_id())
            .map_err(|e| format!("Cannot start vEdit: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if transport::endpoint_alive() {
                return send(request);
            }
            if child.has_exited().map_err(|e| e.to_string())? {
                return Err("vEdit exited before its controls became ready. An older open version may not support the CLI; close it normally and start the new version.".into());
            }
            if Instant::now() >= deadline {
                return Err("vEdit started but controls are not ready after 12 seconds. Inspect the app before retrying.".into());
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    }
}

pub fn forward_paths(paths: Vec<PathBuf>) -> Result<Option<Response>, String> {
    if !transport::endpoint_alive() {
        return Ok(None);
    }
    let mut files = Vec::new();
    for path in paths {
        let path = absolute(path);
        if path.is_dir() {
            let response = send(&Request::Workspace { path })?;
            if !response.ok {
                return Ok(Some(response));
            }
        } else {
            files.push(OpenFile::source(path));
        }
    }
    let response = if files.is_empty() {
        send(&Request::Raise {})?
    } else {
        send(&Request::Open {
            files,
            background: false,
        })?
    };
    Ok(Some(response))
}

pub fn spawn_window(executable: &std::path::Path, id: &str) -> std::io::Result<()> {
    spawn::spawn_window(executable, id).map(|_| ())
}

/// A prefix keeps --window-looking labels and filenames literal in commands.
pub fn window_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(Option<String>, Vec<OsString>), String> {
    let mut args: Vec<_> = arguments.into_iter().collect();
    if args.first().is_some_and(|arg| arg == "--window") {
        if args.len() < 3 {
            return Err(
                "Use vedit-cli --window ID COMMAND; list IDs with vedit-cli windows".into(),
            );
        }
        let id = args[1]
            .to_str()
            .filter(|id| crate::profile::valid_window(id))
            .ok_or("Invalid window ID; use vedit-cli windows")?
            .to_owned();
        args.drain(..2);
        Ok((Some(id), args))
    } else {
        Ok((None, args))
    }
}

pub fn windows_response() -> Response {
    let running = running_windows();
    let saved = saved_window_ids()
        .into_iter()
        .filter(|id| !running.iter().any(|window| &window.id == id))
        .filter(|id| {
            crate::profile::window_directory(&crate::profile::base_recovery_directory(), id)
                .join("session.json")
                .is_file()
        })
        .collect::<Vec<_>>();
    let windows: Vec<_> = running
        .iter()
        .map(|window| {
            let mut value = serde_json::to_value(window).expect("window metadata");
            let endpoint =
                crate::profile::window_directory(&transport::base_endpoint_directory(), &window.id)
                    .join("control.json");
            match transport::send_to(&Request::Status {}, &endpoint) {
                Ok(response) if response.ok => {
                    value["active_workspace"] = response.result["active_workspace"].clone();
                    value["active_workspace_kind"] =
                        response.result["active_workspace_kind"].clone();
                    value["workspaces"] = response.result["workspaces"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .map(|item| item["path"].clone())
                                .collect::<Vec<_>>()
                        })
                        .into();
                    value["active_tab"] = response.result["active_tab"].clone();
                }
                Ok(response) => value["error"] = serde_json::to_value(response.error).unwrap(),
                Err(error) => value["error"] = error.into(),
            }
            value
        })
        .collect();
    Response::success(
        json!({"windows": windows, "saved_windows": saved, "select": "vedit-cli --window ID COMMAND", "restore": "vedit --window ID"}),
    )
}

pub fn schema() -> Value {
    serde_json::from_str(include_str!("cli_schema.json")).expect("bundled CLI schema is valid JSON")
}

pub fn schema_topic(topic: Option<&str>) -> Result<Value, String> {
    let full = schema();
    let Some(topic) = topic else {
        return Ok(full);
    };
    let operation = topic.replace('-', "_");
    if operation == "windows" {
        return Ok(
            json!({"protocol": full["protocol"], "commands": [], "windows": full["windows"]}),
        );
    }
    let examples: Vec<_> = full["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|example| example["op"] == operation)
        .cloned()
        .collect();
    if examples.is_empty() {
        return Err(format!(
            "Unknown schema operation {topic}; use vedit-cli schema for available operations"
        ));
    }
    let mut selected = json!({"protocol":full["protocol"], "commands":examples, "coordinates":full["coordinates"], "ids":full["ids"], "strictness":full["strictness"], "workspace_resolution":full["workspace_resolution"]});
    let section = match operation.as_str() {
        "edit" | "new" | "save" | "save_as" | "selection" | "find" | "replace" | "command" => {
            "editing"
        }
        "file" | "file_job" => "file_actions",
        "read" | "diff" | "diagnostics" | "search" | "output" | "git_status" | "task" => "queries",
        "open" | "review" | "focus" | "close" | "clear_marks" => "defaults",
        "panel" => "panels",
        other => other,
    };
    if let Some(value) = full.get(section) {
        selected[section] = if section == "queries" {
            json!({operation.clone(): value[&operation]})
        } else {
            value.clone()
        };
    }
    if let Some(value) = full.get("responses") {
        selected["responses"] = value.clone();
    }
    // Keep limits discoverable without requiring the complete catalog.
    selected["limits"] = full["limits"].clone();
    Ok(selected)
}
#[cfg(test)]
mod tests {
    #[test]
    fn old_instance_variable_cannot_silently_target_the_default_editor() {
        assert!(
            super::validate_control_environment(false, true)
                .unwrap_err()
                .contains("VEDIT_CONTROL_DIR")
        );
        assert!(super::validate_control_environment(true, true).is_ok());
        assert!(super::validate_control_environment(false, false).is_ok());
    }
    use super::*;
    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args(args.iter().map(OsString::from))
    }
    fn mark_request(args: &[&str]) -> Request {
        let Parsed::Command { request, start } = parse(args).unwrap() else {
            panic!("expected an application request")
        };
        assert!(!start, "agent marks must use the existing window");
        request
    }
    #[test]
    fn agent_marks_cli_preserves_collection_targets_and_action_defaults() {
        let cases = [
            (
                vec!["agent-marks"],
                json!({"action":"list","limit":100,"offset":0}),
            ),
            (
                vec![
                    "agent-marks",
                    "list",
                    "--limit",
                    "500",
                    "--offset",
                    "100001",
                ],
                json!({"action":"list","limit":500,"offset":100001}),
            ),
            (
                vec!["agent-marks", "take"],
                json!({"action":"take","request_id":null,"limit":100}),
            ),
            (
                vec![
                    "agent-marks",
                    "take",
                    "--request-id",
                    "collection-42",
                    "--limit",
                    "3",
                ],
                json!({"action":"take","request_id":"collection-42","limit":3}),
            ),
            (
                vec!["agent-marks", "batch"],
                json!({"action":"batch","batch_id":null,"limit":100,"offset":0}),
            ),
            (
                vec!["agent-marks", "batch", "42", "--offset", "101"],
                json!({"action":"batch","batch_id":42,"limit":100,"offset":101}),
            ),
            (
                vec!["agent-marks", "text", "9"],
                json!({"action":"text","id":9,"batch_id":null,"offset":0,"count":4096}),
            ),
            (
                vec![
                    "agent-marks",
                    "text",
                    "9",
                    "--batch",
                    "42",
                    "--offset",
                    "4096",
                    "--count",
                    "65536",
                ],
                json!({"action":"text","id":9,"batch_id":42,"offset":4096,"count":65536}),
            ),
            (
                vec!["agent-marks", "ack", "42"],
                json!({"action":"ack","batch_id":42}),
            ),
            (
                vec!["agent-marks", "restore"],
                json!({"action":"restore","batch_id":null}),
            ),
            (
                vec!["agent-marks", "restore", "42"],
                json!({"action":"restore","batch_id":42}),
            ),
            (vec!["agent-marks", "clear"], json!({"action":"clear"})),
            (
                vec!["agent-marks", "remove", "9"],
                json!({"action":"remove","id":9}),
            ),
            (
                vec!["agent-marks", "note", "9", ""],
                json!({"action":"note","id":9,"note":""}),
            ),
            (
                vec!["agent-marks", "add"],
                json!({"action":"add","tab_id":null,"note":null}),
            ),
            (
                vec![
                    "agent-marks",
                    "add",
                    "--tab",
                    "7",
                    "--note",
                    " Prüfe Überlauf 🎯\nZeile zwei ",
                ],
                json!({"action":"add","tab_id":7,"note":" Prüfe Überlauf 🎯\nZeile zwei "}),
            ),
        ];
        for (args, action) in cases {
            let request = mark_request(&args);
            assert_eq!(
                serde_json::to_value(request).unwrap(),
                json!({"op":"agent_marks","action":action}),
                "{args:?}"
            );
        }
        assert!(matches!(
            mark_request(&["clear-marks"]),
            Request::ClearMarks { .. }
        ));
    }

    #[test]
    fn agent_marks_parser_keeps_literal_notes_and_refuses_unrelated_constraints() {
        for payload in ["--help", "-h", "--no-start", "--", "{\"action\":\"clear\"}"] {
            let request = mark_request(&["agent-marks", "add", "--note", payload]);
            assert!(
                matches!(request, Request::AgentMarks { action: AgentMarksAction::Add { note: Some(note), .. } } if note == payload)
            );
            let request = mark_request(&["agent-marks", "note", "9", "--", payload]);
            assert!(
                matches!(request, Request::AgentMarks { action: AgentMarksAction::Note { id: 9, note } } if note == payload)
            );
        }
        let request = mark_request(&["agent-marks", "take", "--request-id", "--no-start"]);
        assert!(
            matches!(request, Request::AgentMarks { action: AgentMarksAction::Take { request_id: Some(token), .. } } if token == "--no-start")
        );
        for args in [
            &["agent-marks", "list", "--tab", "7"][..],
            &["agent-marks", "take", "--offset", "1"],
            &["agent-marks", "take", "--count", "1"],
            &["agent-marks", "take", "--batch", "42"],
            &["agent-marks", "text", "9", "--limit", "1"],
            &["agent-marks", "text", "9", "--request-id", "token"],
            &["agent-marks", "batch", "42", "--batch", "43"],
            &["agent-marks", "batch", "--note", "x"],
            &["agent-marks", "ack", "42", "--limit", "2"],
            &["agent-marks", "restore", "42", "--offset", "0"],
            &["agent-marks", "clear", "--tab", "7"],
            &["agent-marks", "remove", "9", "--note", "x"],
            &["agent-marks", "note", "9", "x", "--note", "y"],
            &["agent-marks", "add", "--offset", "0"],
            &["agent-marks", "add", "--line", "7"],
            &["agent-marks", "add", "--label", "x"],
            &[
                "agent-marks",
                "take",
                "--request-id",
                "a",
                "--request-id",
                "b",
            ],
            &["agent-marks", "text", "9", "--batch", "42", "--batch", "43"],
            &["agent-marks", "add", "--note", "a", "--note", "b"],
            &["agent-marks", "add", "--tab", "7", "--tab", "8"],
            &["agent-marks", "ack"],
            &["agent-marks", "remove", "0"],
            &["agent-marks", "batch", "-1"],
            &["agent-marks", "note", "9"],
            &["agent-marks", "add", "arbitrary-range"],
            &["status", "--request-id", "x"],
            &["bookmarks", "--batch", "42"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn agent_marks_json_has_readonly_omission_and_strict_explicit_intent() {
        let request: Request = serde_json::from_value(json!({"op":"agent_marks"})).unwrap();
        validate(&request).unwrap();
        assert!(matches!(
            request,
            Request::AgentMarks {
                action: AgentMarksAction::List {
                    limit: 100,
                    offset: 0
                }
            }
        ));
        let action = |value: Value| {
            serde_json::from_value::<Request>(json!({"op":"agent_marks","action":value}))
        };
        for invalid in [
            Value::Null,
            json!("take"),
            json!({}),
            json!({"action":"Take"}),
            json!({"action":"take","offset":1}),
            json!({"action":"take","request_id":42}),
            json!({"action":"remove","id":9,"force":true}),
            json!({"action":"note","id":9,"note":null}),
            json!({"action":"add","range":{"start":{"line":1},"end":{"line":2}}}),
            json!({"action":"text","id":9,"tab_id":7}),
            json!({"action":"batch","batch_id":-1}),
            json!({"action":"clear","workspace":"C:/other"}),
        ] {
            assert!(action(invalid.clone()).is_err(), "{invalid}");
        }
        for valid in [
            json!({"action":"batch"}),
            json!({"action":"batch","batch_id":null}),
            json!({"action":"text","id":9,"batch_id":null}),
            json!({"action":"restore","batch_id":null}),
            json!({"action":"take","request_id":null}),
            json!({"action":"add","tab_id":null,"note":null}),
        ] {
            validate(&action(valid.clone()).unwrap()).unwrap();
        }
        for bytes in [
            br#"{"op":"agent_marks","action":{"action":"take","request_id":"a","request_id":"b"}}"#
                .as_slice(),
            br#"{"op":"agent_marks","action":{"action":"remove","id":9,"id":10}}"#,
            br#"{"op":"agent_marks","action":{"action":"list"},"action":{"action":"clear"}}"#,
        ] {
            assert!(transport::from_unique_json::<Request>(bytes).is_err());
        }
    }

    #[test]
    fn agent_marks_bounds_use_unicode_notes_positive_ids_and_bounded_payloads() {
        let request = |action: Value| {
            serde_json::from_value::<Request>(json!({"op":"agent_marks","action":action})).unwrap()
        };
        for note in [
            "".to_owned(),
            "🎯".repeat(4096),
            " literal {\"op\":\"clear\"}\n\t ".to_owned(),
        ] {
            let input = request(json!({"action":"note","id":9,"note":note}));
            validate(&input).unwrap();
            assert!(
                matches!(input, Request::AgentMarks { action: AgentMarksAction::Note { note: actual, .. } } if actual == note)
            );
        }
        for invalid in [
            json!({"action":"list","limit":0}),
            json!({"action":"take","limit":501}),
            json!({"action":"batch","batch_id":0}),
            json!({"action":"text","id":0}),
            json!({"action":"text","id":9,"batch_id":0}),
            json!({"action":"text","id":9,"count":0}),
            json!({"action":"text","id":9,"count":65537}),
            json!({"action":"ack","batch_id":0}),
            json!({"action":"restore","batch_id":0}),
            json!({"action":"add","tab_id":0}),
            json!({"action":"add","note":"x".repeat(4097)}),
            json!({"action":"note","id":9,"note":"🎯".repeat(4097)}),
            json!({"action":"take","request_id":""}),
            json!({"action":"take","request_id":"é".repeat(65)}),
            json!({"action":"take","request_id":"line\nbreak"}),
        ] {
            assert!(validate(&request(invalid.clone())).is_err(), "{invalid}");
        }
        for valid in [
            json!({"action":"list","limit":500,"offset":usize::MAX}),
            json!({"action":"take","request_id":"é".repeat(64)}),
            json!({"action":"text","id":u64::MAX,"offset":usize::MAX,"count":65536}),
        ] {
            validate(&request(valid)).unwrap();
        }
    }
    #[test]
    fn bookmark_commands_preserve_targets_labels_and_existing_instance_scope() {
        for args in [&["bookmarks"][..], &["bookmarks", "list"]] {
            assert!(matches!(
                parse(args).unwrap(),
                Parsed::Command {
                    request: Request::Bookmarks {
                        change: BookmarkAction::List {
                            limit: 100,
                            offset: 0
                        }
                    },
                    start: false
                }
            ));
        }
        let Parsed::Command {
            request:
                Request::Bookmarks {
                    change: BookmarkAction::Toggle { tab_id, label },
                },
            start,
        } = parse(&[
            "bookmarks",
            "toggle",
            "--tab",
            "42",
            "--label",
            " Prüfe Überlauf 🎯 ",
        ])
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(tab_id, Some(42));
        assert_eq!(label.as_deref(), Some(" Prüfe Überlauf 🎯 "));
        assert!(!start);
        assert!(matches!(
            parse(&["bookmarks", "toggle"]).unwrap(),
            Parsed::Command {
                request: Request::Bookmarks {
                    change: BookmarkAction::Toggle {
                        tab_id: None,
                        label: None
                    }
                },
                ..
            }
        ));
        assert!(matches!(
            parse(&["bookmarks", "remove", "7"]).unwrap(),
            Parsed::Command {
                request: Request::Bookmarks {
                    change: BookmarkAction::Remove { id: 7 }
                },
                ..
            }
        ));
        assert!(matches!(
            parse(&["bookmarks", "jump", "9"]).unwrap(),
            Parsed::Command {
                request: Request::Bookmarks {
                    change: BookmarkAction::Jump { id: 9 }
                },
                ..
            }
        ));
        assert!(
            matches!(parse(&["bookmarks", "rename", "7", "--label", ""]).unwrap(), Parsed::Command { request: Request::Bookmarks { change: BookmarkAction::Rename { id: 7, label } }, .. } if label.is_empty())
        );
    }

    #[test]
    fn bookmark_parser_refuses_unrelated_or_contradictory_constraints() {
        for args in [
            &["bookmarks", "--tab", "42"][..],
            &["bookmarks", "list", "--label", "x"],
            &["bookmarks", "remove", "7", "--label", "x"],
            &["bookmarks", "rename", "7"],
            &["bookmarks", "rename", "7", "--label", "x", "--tab", "42"],
            &["bookmarks", "toggle", "--tab", "0"],
            &["bookmarks", "toggle", "--tab", "1", "--tab", "2"],
            &["bookmarks", "toggle", "--label", "a", "--label", "b"],
            &["bookmarks", "toggle", "--line", "7"],
            &["bookmarks", "toggle", "--revision", "3"],
            &["bookmarks", "toggle", "--limit", "3"],
            &["bookmarks", "rename", "7", "--label", "x", "--offset", "2"],
            &["bookmarks", "toggle", "extra"],
            &["bookmarks", "jump", "0"],
            &["bookmarks", "remove", "18446744073709551616"],
            &["status", "--label", "x"],
            &["focus", "1", "--tab", "2"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn bookmark_pages_are_bounded_and_legacy_list_json_keeps_defaults() {
        for args in [
            &["bookmarks", "--limit", "500", "--offset", "13"][..],
            &["bookmarks", "list", "--limit", "500", "--offset", "13"],
        ] {
            assert!(matches!(
                parse(args).unwrap(),
                Parsed::Command {
                    request: Request::Bookmarks {
                        change: BookmarkAction::List {
                            limit: 500,
                            offset: 13
                        }
                    },
                    ..
                }
            ));
        }
        let request: Request =
            serde_json::from_value(json!({"op":"bookmarks","change":{"action":"list"}})).unwrap();
        validate(&request).unwrap();
        assert!(matches!(
            request,
            Request::Bookmarks {
                change: BookmarkAction::List {
                    limit: 100,
                    offset: 0
                }
            }
        ));
        for limit in [0, 501] {
            let request: Request = serde_json::from_value(
                json!({"op":"bookmarks","change":{"action":"list","limit":limit}}),
            )
            .unwrap();
            assert!(validate(&request).is_err());
            assert!(parse(&["bookmarks", "--limit", &limit.to_string()]).is_err());
        }
    }

    #[test]
    fn labels_that_look_like_global_options_remain_literal_payload() {
        for label in ["--help", "-h", "--no-start", "--"] {
            let Parsed::Command {
                request:
                    Request::Bookmarks {
                        change:
                            BookmarkAction::Toggle {
                                label: Some(actual),
                                ..
                            },
                    },
                ..
            } = parse(&["bookmarks", "toggle", "--label", label]).unwrap()
            else {
                panic!()
            };
            assert_eq!(actual, label);
        }
        assert!(matches!(
            parse(&["bookmarks", "toggle", "--label", "x", "--help"]).unwrap(),
            Parsed::Help { .. }
        ));
        assert!(
            matches!(parse(&["open", "--no-start", "--", "--no-start"]).unwrap(), Parsed::Command { request: Request::Open { files, .. }, start: false } if files[0].path.file_name().unwrap() == "--no-start")
        );
    }

    #[test]
    fn bookmark_json_keeps_explicit_targets_and_refuses_unknown_features() {
        let request: Request = serde_json::from_value(json!({"op":"bookmarks","change":{"action":"toggle","tab_id":42,"label":"source/plan.md"}})).unwrap();
        validate(&request).unwrap();
        assert!(
            matches!(request, Request::Bookmarks { change: BookmarkAction::Toggle { tab_id: Some(42), label: Some(label) } } if label == "source/plan.md")
        );
        for change in [
            json!({"action":"toggle","tab_id":42,"line":7}),
            json!({"action":"list","workspace":"C:/project"}),
            json!({"action":"jump","id":9,"force":true}),
            json!({"action":"remove","tab_id":42}),
            json!({"action":"rename","id":9}),
            json!({"action":"jump","id":-1}),
        ] {
            assert!(
                serde_json::from_value::<Request>(json!({"op":"bookmarks","change":change}))
                    .is_err()
            );
        }
        for change in [
            json!({"action":"jump","id":0}),
            json!({"action":"toggle","tab_id":0}),
        ] {
            let request: Request =
                serde_json::from_value(json!({"op":"bookmarks","change":change})).unwrap();
            assert!(validate(&request).is_err());
        }
    }

    #[test]
    fn bookmark_labels_have_unicode_byte_boundaries_and_clear_semantics() {
        let allowed = "é".repeat(128);
        let refused = "é".repeat(129);
        for label in ["", allowed.as_str(), " leading and trailing "] {
            let request = Request::Bookmarks {
                change: BookmarkAction::Rename {
                    id: 1,
                    label: label.to_owned(),
                },
            };
            validate(&request).unwrap();
        }
        for label in [refused.as_str(), "line\nbreak", "tab\tlabel", "null\0label"] {
            let request = Request::Bookmarks {
                change: BookmarkAction::Toggle {
                    tab_id: Some(42),
                    label: Some(label.to_owned()),
                },
            };
            assert!(validate(&request).is_err());
        }
    }

    #[test]
    fn leader_key_cli_normalizes_only_exact_aliases_and_preserves_unicode() {
        for (input, expected) in [
            ("space", " "),
            ("comma", ","),
            ("backslash", "\\"),
            (" ", " "),
            ("ß", "ß"),
            ("🎯", "🎯"),
            ("x", "x"),
        ] {
            let Parsed::Command {
                request: Request::Settings { patch: Some(patch) },
                start,
            } = parse(&["settings", "leader-key", input]).unwrap()
            else {
                panic!()
            };
            assert_eq!(patch.leader_key.as_deref(), Some(expected));
            assert!(!start);
            assert!(
                patch.vim.is_none() && patch.autosave.is_none() && patch.smooth_scroll.is_none()
            );
        }
        assert!(
            matches!(parse(&["settings", "leader-key", "--", "-"]).unwrap(), Parsed::Command { request: Request::Settings { patch: Some(patch) }, .. } if patch.leader_key.as_deref() == Some("-"))
        );
        for value in ["", "SPACE", "comma ", "Space", "ab", "\n", "\t", "\0"] {
            assert!(
                parse(&["settings", "leader-key", value]).is_err(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn leader_key_json_validation_matches_cli_without_changing_payload_fields() {
        for value in [" ", "space", "comma", "backslash", "ö"] {
            let request: Request =
                serde_json::from_value(json!({"op":"settings","patch":{"leader_key":value}}))
                    .unwrap();
            validate(&request).unwrap();
        }
        for value in ["", "spacebar", "a b", "\u{7f}", "\r"] {
            let request: Request =
                serde_json::from_value(json!({"op":"settings","patch":{"leader_key":value}}))
                    .unwrap();
            assert!(validate(&request).is_err());
        }
        assert!(
            serde_json::from_value::<Request>(
                json!({"op":"settings","patch":{"leader_key":" ","leader_timeout":500}})
            )
            .is_err()
        );
    }
    #[test]
    fn search_exclusion_options_preserve_inheritance_and_explicit_empty_override() {
        let search = |args: &[&str]| {
            let Parsed::Command {
                request: Request::Search { excludes, .. },
                ..
            } = parse(args).unwrap()
            else {
                panic!("expected Search")
            };
            excludes
        };
        assert_eq!(search(&["search", "needle"]), None);
        for pattern in ["--help", "-h", "--no-start", "--"] {
            assert_eq!(
                search(&["search", "needle", "--exclude", pattern]),
                Some(vec![pattern.into()])
            );
        }
        assert_eq!(
            search(&["search", "needle", "--no-excludes"]),
            Some(Vec::new())
        );
        let rules = search(&[
            "search",
            "needle",
            "--exclude",
            "src\\generated",
            "--exclude",
            "**/*.min.js",
        ])
        .unwrap();
        assert_eq!(
            normalize_search_excludes(&rules).unwrap(),
            ["src/generated", "**/*.min.js"]
        );
        for args in [
            vec![
                "search",
                "needle",
                "--exclude",
                "generated",
                "--no-excludes",
            ],
            vec![
                "search",
                "needle",
                "--no-excludes",
                "--exclude",
                "generated",
            ],
            vec!["search", "needle", "--no-excludes", "--no-excludes"],
            vec!["search", "needle", "--exclude"],
            vec!["read", "1", "--exclude", "generated"],
            vec!["settings", "--no-excludes"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        for json in [
            json!({"op":"search","query":"needle"}),
            json!({"op":"search","query":"needle","excludes":null}),
        ] {
            let request: Request = serde_json::from_value(json).unwrap();
            validate(&request).unwrap();
            assert!(matches!(request, Request::Search { excludes: None, .. }));
        }
    }
    #[test]
    fn exclusion_rules_validate_globs_and_actual_payload_bounds_before_effects() {
        let normalized = normalize_search_excludes(&[
            "  src\\generated  ".into(),
            "".into(),
            " ".into(),
            "src/generated".into(),
            "#cache".into(),
            "**/*.min.js".into(),
        ])
        .unwrap();
        assert_eq!(normalized, ["src/generated", "#cache", "**/*.min.js"]);
        for rules in [
            vec!["!keep.rs".into()],
            vec!["[abc".into()],
            vec!["folder\nfile".into()],
            vec!["folder\tfile".into()],
            vec!["x".repeat(1025)],
            vec!["x".into(); 129],
            vec!["x".repeat(1024); 17],
        ] {
            assert!(normalize_search_excludes(&rules).is_err(), "{rules:?}");
            for json in [
                json!({"op":"search","query":"needle","excludes":rules}),
                json!({"op":"settings","patch":{"swap_jk":true,"search_excludes":rules}}),
            ] {
                let request: Request = serde_json::from_value(json).unwrap();
                assert!(validate(&request).is_err());
            }
        }
        assert!(normalize_search_excludes(&vec!["x".repeat(1024); 16]).is_ok());
        for json in [
            json!({"op":"settings","patch":{"swap_jk":"true"}}),
            json!({"op":"search","query":"needle","excludes":[42]}),
            json!({"op":"search","query":"needle","exclude":["target"]}),
        ] {
            assert!(serde_json::from_value::<Request>(json).is_err());
        }
    }
    #[test]
    fn keybinding_settings_preserve_strings_disable_and_aliases_with_bounded_payloads() {
        for (bindings, expected) in [
            (json!({}), json!({})),
            (
                json!({"save":"F8","toggle_left_sidebar":null}),
                json!({"save":"F8","toggle_left_sidebar":null}),
            ),
            (json!({"save":["F8"]}), json!({"save":"F8"})),
            (
                json!({"save":["Ctrl+S","F8"]}),
                json!({"save":["Ctrl+S","F8"]}),
            ),
            (
                json!({"save":["F1","F2","F3","F4","F5","F6","F7","F8"]}),
                json!({"save":["F1","F2","F3","F4","F5","F6","F7","F8"]}),
            ),
        ] {
            let request: Request =
                serde_json::from_value(json!({"op":"settings","patch":{"keybindings":bindings}}))
                    .unwrap();
            validate(&request).unwrap();
            assert_eq!(
                serde_json::to_value(&request).unwrap()["patch"]["keybindings"],
                expected
            );
        }
        for bindings in [
            json!({"save":[]}),
            json!({"save":vec!["F8"; MAX_SHORTCUT_ALIASES + 1]}),
            json!({"save":""}),
            json!({"save":["F8", ""]}),
            json!({"save":"x".repeat(MAX_SHORTCUT_BYTES + 1)}),
            json!({"save":["F8", "x".repeat(MAX_SHORTCUT_BYTES + 1)]}),
        ] {
            let request: Request = serde_json::from_value(
                json!({"op":"settings","patch":{"vim":false,"keybindings":bindings}}),
            )
            .unwrap();
            assert!(validate(&request).is_err(), "{bindings}");
        }
        for binding in [json!(42), json!(["F8", null]), json!({"keys":"F8"})] {
            assert!(
                serde_json::from_value::<Request>(
                    json!({"op":"settings","patch":{"keybindings":{"save":binding}}})
                )
                .is_err()
            );
        }
        assert!(
            transport::from_unique_json::<Request>(
                br#"{"op":"settings","patch":{"keybindings":{"save":"F8","save":["F9"]}}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn swap_jk_cli_is_explicit_boolean_and_leaves_unrelated_settings_omitted() {
        for (input, expected) in [("true", true), ("false", false)] {
            let Parsed::Command {
                request: Request::Settings { patch: Some(patch) },
                start,
            } = parse(&["settings", "swap-jk", input]).unwrap()
            else {
                panic!()
            };
            assert_eq!(patch.swap_jk, Some(expected));
            assert!(
                patch.vim.is_none()
                    && patch.leader_key.is_none()
                    && patch.search_excludes.is_none()
            );
            assert!(!start);
        }
        for input in ["", "on", "1", "TRUE", "false "] {
            assert!(parse(&["settings", "swap-jk", input]).is_err());
        }
        let request: Request =
            serde_json::from_value(json!({"op":"settings","patch":{"word_wrap":true}})).unwrap();
        assert!(matches!(
            request,
            Request::Settings {
                patch: Some(SettingsPatch {
                    swap_jk: None,
                    search_excludes: None,
                    ..
                })
            }
        ));
    }
    #[test]
    fn editor_font_cli_accepts_equivalent_spellings_and_changes_only_the_font() {
        for (input, expected, serialized) in [
            (
                "jetbrains-mono",
                EditorFont::JetBrainsMono,
                "jetbrains_mono",
            ),
            (
                "jetbrains_mono",
                EditorFont::JetBrainsMono,
                "jetbrains_mono",
            ),
            ("consolas", EditorFont::Consolas, "consolas"),
            ("cascadia-mono", EditorFont::CascadiaMono, "cascadia_mono"),
            ("cascadia_mono", EditorFont::CascadiaMono, "cascadia_mono"),
        ] {
            let Parsed::Command {
                request: Request::Settings { patch: Some(patch) },
                start,
            } = parse(&["settings", "font", input]).unwrap()
            else {
                panic!("expected a font settings patch")
            };
            assert_eq!(patch.editor_font, Some(expected));
            let mut fields = serde_json::to_value(patch).unwrap();
            assert_eq!(
                fields.as_object_mut().unwrap().remove("editor_font"),
                Some(json!(serialized))
            );
            assert!(fields.as_object().unwrap().values().all(Value::is_null));
            assert!(!start, "font selection must not start another app");
        }
        for args in [
            vec!["settings", "font", "JetBrainsMono"],
            vec!["settings", "font", "jetbrains"],
            vec!["settings", "font", "consolas-bold"],
            vec!["settings", "font", "cascadia-code"],
            vec!["settings", "font", ""],
            vec!["settings", "font", "consolas", "cascadia-mono"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn editor_font_json_preserves_null_and_omission_and_refuses_unknown_choices() {
        assert_eq!(EditorFont::default(), EditorFont::JetBrainsMono);
        for patch in [
            json!({}),
            json!({"editor_font":null}),
            json!({"word_wrap":true}),
        ] {
            let request: Request =
                serde_json::from_value(json!({"op":"settings","patch":patch})).unwrap();
            validate(&request).unwrap();
            assert!(matches!(
                request,
                Request::Settings {
                    patch: Some(SettingsPatch {
                        editor_font: None,
                        ..
                    })
                }
            ));
        }
        for (input, expected) in [
            ("jetbrains_mono", EditorFont::JetBrainsMono),
            ("jetbrains-mono", EditorFont::JetBrainsMono),
            ("consolas", EditorFont::Consolas),
            ("cascadia_mono", EditorFont::CascadiaMono),
            ("cascadia-mono", EditorFont::CascadiaMono),
        ] {
            let request: Request =
                serde_json::from_value(json!({"op":"settings","patch":{"editor_font":input}}))
                    .unwrap();
            validate(&request).unwrap();
            assert!(
                matches!(request, Request::Settings { patch: Some(SettingsPatch { editor_font: Some(font), .. }) } if font == expected)
            );
        }
        for patch in [
            json!({"editor_font":"consola","autosave":false}),
            json!({"editor_font":"Consolas"}),
            json!({"editor_font":42}),
            json!({"editor_font":"consolas","font_weight":"bold"}),
        ] {
            assert!(
                serde_json::from_value::<Request>(json!({"op":"settings","patch":patch})).is_err()
            );
        }
        assert!(transport::from_unique_json::<Request>(br#"{"op":"settings","patch":{"editor_font":"consolas","editor_font":"cascadia_mono"}}"#).is_err());
        let schema = schema();
        assert_eq!(
            schema["settings"]["editor_font"]["enum"],
            json!(["jetbrains_mono", "consolas", "cascadia_mono"])
        );
        assert_eq!(
            schema["settings"]["editor_font"]["default"],
            "jetbrains_mono"
        );
    }
    #[test]
    fn open_preserves_preview_and_exact_location() {
        let Parsed::Command {
            request: Request::Open { files, .. },
            start,
        } = parse(&[
            "open",
            "plan.md",
            "--preview",
            "--line",
            "7",
            "--column",
            "3",
        ])
        .unwrap()
        else {
            panic!()
        };
        assert!(start);
        assert_eq!(files[0].view, DocumentView::Preview);
        assert_eq!(files[0].location, Some(Location { line: 7, column: 3 }));
        assert!(files[0].path.is_absolute());
    }
    #[test]
    fn rejects_contradictory_and_unsupported_options() {
        for args in [
            &["focus", "1", "--preview", "--source"][..],
            &["status", "--line", "2"],
            &["open", "a.rs", "--preview"],
            &["open", "a.rs", "--column", "2"],
            &["tabs", "--limit", "0"],
            &["close", "1", "1"],
            &["--no-start"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn unknown_constraints_are_not_dropped() {
        assert!(
            serde_json::from_value::<Request>(
                json!({"op":"open","files":[{"path":"C:/x.md","split":true}]})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<Request>(json!({"op":"status","force":true})).is_err());
    }
    #[test]
    fn every_advertised_schema_example_is_a_complete_valid_typed_request() {
        for example in schema()["commands"].as_array().unwrap() {
            let request: Request = serde_json::from_value(example.clone())
                .unwrap_or_else(|error| panic!("{example}: {error}"));
            validate(&request).unwrap_or_else(|error| panic!("{example}: {error}"));
        }
    }
    #[test]
    fn local_tab_close_is_typed_distinct_from_global_close_and_validates_ids() {
        let request: Request = serde_json::from_value(json!({
            "op": "layout",
            "change": { "action": "close_tab", "pane_id": 2, "tab_id": 42 }
        }))
        .unwrap();
        validate(&request).unwrap();
        assert!(matches!(
            request,
            Request::Layout {
                change: LayoutAction::CloseTab {
                    pane_id: 2,
                    tab_id: 42
                }
            }
        ));
        for (pane_id, tab_id) in [(0, 42), (2, 0)] {
            let request: Request = serde_json::from_value(json!({
                "op": "layout",
                "change": { "action": "close_tab", "pane_id": pane_id, "tab_id": tab_id }
            }))
            .unwrap();
            assert!(validate(&request).is_err());
        }
        for change in [
            json!({"action":"close_tab","pane_id":2}),
            json!({"action":"close_tab","tab_id":42}),
            json!({"action":"close_tab","pane_id":2,"tab_id":42,"force":true}),
        ] {
            assert!(serde_json::from_value::<LayoutAction>(change).is_err());
        }
        let Parsed::Command {
            request: Request::Close { tab_ids },
            ..
        } = parse(&["close", "42"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(tab_ids, [42]);
    }
    #[test]
    fn layout_change_file_parser_preserves_local_tab_close() {
        let path = std::env::temp_dir().join(format!(
            "vedit-layout-close-tab-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            b"\xef\xbb\xbf{\"action\":\"close_tab\",\"pane_id\":2,\"tab_id\":42}",
        )
        .unwrap();
        let result = parse_args([
            OsString::from("--no-start"),
            OsString::from("layout"),
            path.as_os_str().to_owned(),
        ]);
        std::fs::remove_file(&path).unwrap();
        let Parsed::Command {
            request:
                Request::Layout {
                    change: LayoutAction::CloseTab { pane_id, tab_id },
                },
            start,
        } = result.unwrap()
        else {
            panic!()
        };
        assert_eq!((pane_id, tab_id), (2, 42));
        assert!(!start);
    }
    #[test]
    fn tab_move_preserves_explicit_panes_position_and_requires_every_constraint() {
        for (source_pane_id, pane_id, index) in [(1, 2, 0), (2, 2, 3), (3, 1, 100_000)] {
            let request: Request = serde_json::from_value(json!({
                "op": "layout",
                "change": { "action": "move_tab", "source_pane_id": source_pane_id,
                    "pane_id": pane_id, "tab_id": 42, "index": index }
            }))
            .unwrap();
            validate(&request).unwrap();
            let Request::Layout {
                change:
                    LayoutAction::MoveTab {
                        source_pane_id: actual_source,
                        pane_id: actual_target,
                        tab_id,
                        index: actual_index,
                    },
            } = request
            else {
                panic!()
            };
            assert_eq!(
                (actual_source, actual_target, tab_id, actual_index),
                (source_pane_id, pane_id, 42, index)
            );
        }
        for (source_pane_id, pane_id, tab_id, index) in [
            (0, 2, 42, 0),
            (1, 0, 42, 0),
            (1, 2, 0, 0),
            (1, 2, 42, 100_001),
        ] {
            let request: Request = serde_json::from_value(json!({
                "op": "layout",
                "change": { "action": "move_tab", "source_pane_id": source_pane_id,
                    "pane_id": pane_id, "tab_id": tab_id, "index": index }
            }))
            .unwrap();
            assert!(validate(&request).is_err());
        }
        let valid = json!({"action":"move_tab","source_pane_id":1,
            "pane_id":2,"tab_id":42,"index":0});
        for missing in ["source_pane_id", "pane_id", "tab_id", "index"] {
            let mut change = valid.clone();
            change.as_object_mut().unwrap().remove(missing);
            assert!(serde_json::from_value::<LayoutAction>(change).is_err());
        }
        for invalid in [
            json!({"action":"move_tab","source_pane_id":1,"target_pane_id":2,"tab_id":42,"index":0}),
            json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":-1}),
            json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":1.5}),
            json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":0,"view":"preview"}),
            json!({"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,"index":0,"copy":true}),
        ] {
            assert!(serde_json::from_value::<LayoutAction>(invalid).is_err());
        }
    }
    #[test]
    fn layout_change_file_parser_preserves_zero_based_tab_move() {
        let path = std::env::temp_dir().join(format!(
            "vedit-layout-move-tab-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path,
            b"\xef\xbb\xbf{\"action\":\"move_tab\",\"source_pane_id\":3,\"pane_id\":2,\"tab_id\":42,\"index\":0}")
            .unwrap();
        let result = parse_args([
            OsString::from("--no-start"),
            OsString::from("layout"),
            path.as_os_str().to_owned(),
        ]);
        std::fs::remove_file(&path).unwrap();
        let Parsed::Command {
            request:
                Request::Layout {
                    change:
                        LayoutAction::MoveTab {
                            source_pane_id,
                            pane_id,
                            tab_id,
                            index,
                        },
                },
            start,
        } = result.unwrap()
        else {
            panic!()
        };
        assert_eq!((source_pane_id, pane_id, tab_id, index), (3, 2, 42, 0));
        assert!(!start);
    }
    #[test]
    fn edits_require_revision_preserve_payload_and_reject_conflicting_shapes() {
        assert!(
            serde_json::from_value::<Request>(json!({"op":"edit","tab_id":1,"set_text":"erase"}))
                .is_err()
        );
        let payload = "literal {\"path\":\"C:/a\",\"op\":\"delete\"} 😀\n";
        let request: Request = serde_json::from_value(
            json!({"op":"edit","tab_id":1,"expected_revision":0,"set_text":payload}),
        )
        .unwrap();
        validate(&request).unwrap();
        let Request::Edit { set_text, .. } = request else {
            panic!()
        };
        assert_eq!(set_text.unwrap(), payload);
        let request:Request=serde_json::from_value(json!({"op":"edit","tab_id":1,"expected_revision":0,"set_text":"","edits":[{"range":{"start":{"line":1},"end":{"line":1}},"text":"x"}]})).unwrap();
        assert!(validate(&request).is_err());
        for value in [
            json!({"op":"edit","tab_id":1,"expected_revision":0,"set_text":"x","force":true}),
            json!({"op":"settings","patch":{"word_wrap":true,"word_wrap_width":80}}),
            json!({"op":"layout","change":{"action":"split","orientation":"vertical","tab_id":1,"clone_text":true}}),
        ] {
            assert!(serde_json::from_value::<Request>(value).is_err());
        }
    }
    #[test]
    fn broad_cli_wrappers_require_revision_and_preserve_explicit_options() {
        assert!(parse(&["replace", "1", "64", "96"]).is_err());
        let Parsed::Command {
            request:
                Request::Replace {
                    expected_revision,
                    case_sensitive,
                    whole_word,
                    all,
                    replacement,
                    ..
                },
            start,
        } = parse(&[
            "replace",
            "1",
            "64",
            "",
            "--revision",
            "3",
            "--case-sensitive",
            "--whole-word",
            "--first",
        ])
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(expected_revision, 3);
        assert!(case_sensitive);
        assert!(whole_word);
        assert!(!all);
        assert!(replacement.is_empty());
        assert!(!start);
        for args in [
            &["command", "1", "undo", "--revision", "3", "--revision", "4"][..],
            &["find", "1", "q", "--revision", "3"],
            &["settings", "--whole-word"],
            &["search", "q", "--offset", "1"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        let Parsed::Command {
            request:
                Request::Layout {
                    change: LayoutAction::Status {},
                },
            ..
        } = parse(&["layout"]).unwrap()
        else {
            panic!()
        };
    }
    #[test]
    fn delete_file_command_resolves_unicode_target_and_does_not_autostart() {
        let manifest = std::env::temp_dir().join(format!(
            "vedit-delete-action-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &manifest,
            serde_json::to_vec(&json!({"action":"delete", "path":"obsolete/Pläne 東京.txt"}))
                .unwrap(),
        )
        .unwrap();
        let parsed = parse_args([OsString::from("file"), manifest.as_os_str().to_owned()]);
        std::fs::remove_file(&manifest).unwrap();
        let Parsed::Command {
            request:
                Request::File {
                    action: FileAction::Delete { path },
                },
            start,
        } = parsed.unwrap()
        else {
            panic!()
        };
        assert!(!start);
        assert_eq!(path, absolute("obsolete/Pläne 東京.txt".into()));
    }

    #[test]
    fn delete_action_requires_an_explicit_path_and_rejects_extra_payload() {
        let request: Request = serde_json::from_value(json!({"op":"file", "action":{"action":"delete", "path":absolute("obsolete.txt".into())}})).unwrap();
        validate(&request).unwrap();
        assert!(
            serde_json::from_value::<Request>(json!({"op":"file", "action":{"action":"delete"}}))
                .is_err()
        );
        assert!(serde_json::from_value::<Request>(json!({"op":"file", "action":{"action":"delete", "path":"obsolete.txt", "name":"different.txt"}})).is_err());
    }

    #[test]
    fn selections_and_lifecycle_constraints_are_never_silently_ignored() {
        let request:Request=serde_json::from_value(json!({"op":"selection","tab_id":1,"range":{"start":{"line":1},"end":{"line":1,"column":2}}})).unwrap();
        assert!(validate(&request).is_err());
        let request = Request::File {
            action: FileAction::CreateFolder {
                parent: absolute(".".into()),
                name: "../escape".into(),
            },
        };
        assert!(validate(&request).is_err());
        for action in [
            Request::Search {
                query: "x".into(),
                workspace: None,
                excludes: None,
                case_sensitive: false,
                whole_word: false,
                limit: 100,
            },
            Request::File {
                action: FileAction::Rename {
                    path: absolute("old.md".into()),
                    name: "new.md".into(),
                },
            },
        ] {
            let request = Request::Batch {
                actions: vec![Request::Raise {}, action],
                stop_on_error: false,
            };
            assert!(validate(&request).is_err());
        }
    }
    #[test]
    fn annotation_end_cannot_reverse_range() {
        let request = Request::Open {
            background: false,
            files: vec![OpenFile {
                path: absolute("x.rs".into()),
                location: None,
                view: DocumentView::Source,
                annotations: vec![Annotation {
                    id: String::new(),
                    start: Location { line: 4, column: 3 },
                    end: Some(Location { line: 4, column: 2 }),
                    note: "look".into(),
                    severity: AnnotationSeverity::Info,
                }],
            }],
        };
        assert!(validate(&request).is_err());
    }
    #[test]
    fn no_start_preserves_command_effect() {
        let Parsed::Command {
            request: Request::Open { files, .. },
            start,
        } = parse(&["--no-start", "open", "x.md", "--source"]).unwrap()
        else {
            panic!()
        };
        assert!(!start);
        assert_eq!(files[0].view, DocumentView::Source);
    }
    #[test]
    fn source_default_and_one_based_column() {
        let item: OpenFile =
            serde_json::from_value(json!({"path":"C:/x.md","location":{"line":9}})).unwrap();
        assert_eq!(item.view, DocumentView::Source);
        assert_eq!(item.location.unwrap().column, 1);
    }
    #[test]
    fn json_bom_and_literal_flag_filenames_are_preserved() {
        let request: Request =
            serde_json::from_slice(json_bytes(b"\xef\xbb\xbf{\"op\":\"status\"}")).unwrap();
        assert!(matches!(request, Request::Status {}));
        let Parsed::Command {
            request: Request::Open { files, .. },
            ..
        } = parse(&["open", "--", "--no-start", "--help"]).unwrap()
        else {
            panic!()
        };
        assert!(files[0].path.ends_with("--no-start"));
        assert!(files[1].path.ends_with("--help"));
    }
    #[test]
    fn git_commands_parse_exact_refs_and_bounded_pages() {
        for command in ["git-branches", "git-history"] {
            let Parsed::Command { request, .. } =
                parse(&[command, "--limit", "1", "--offset", "2"]).unwrap()
            else {
                panic!()
            };
            let value = serde_json::to_value(request).unwrap();
            assert_eq!(value["limit"], 1);
            assert_eq!(value["offset"], 2);
        }
        let Parsed::Command { request, .. } =
            parse(&["git-switch-branch", "refs/heads/feature/Überarbeitung"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            serde_json::to_value(request).unwrap()["target"],
            "refs/heads/feature/Überarbeitung"
        );
        for args in [
            vec!["git-switch-branch"],
            vec!["git-switch-branch", "main"],
            vec!["git-branches", "--limit", "0"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
}
