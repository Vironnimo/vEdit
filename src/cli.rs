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
use transport::send;
#[allow(unused_imports)]
pub use transport::{Envelope, Server, running_windows, saved_window_ids};

mod arguments;
mod client;
mod commands;
mod input;
mod validation;

// The GUI and CLI binaries consume different parts of this shared interface.
#[allow(unused_imports)]
pub use arguments::parse_args;
use arguments::{Options, ids, one_argument, require_empty, tab_id};
#[allow(unused_imports)]
pub use client::{execute, forward_paths, spawn_window, window_arguments, windows_response};
use commands::agent_mark_id;
use input::{absolute, decode_input, json_bytes, read_input_file, read_json_file, resolve_paths};
#[allow(unused_imports)]
pub use validation::normalize_search_excludes;
pub use validation::{normalize_leader_key, validate};

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

const MAX_FILES: usize = 100;
const MAX_ANNOTATIONS: usize = 500;

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

pub fn is_markdown(path: &std::path::Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
    })
}

fn schema() -> Value {
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
#[path = "cli/tests.rs"]
mod tests;
