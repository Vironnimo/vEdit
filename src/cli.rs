//! Versioned, bounded controls for presenting work in a running Morrow window.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::PathBuf,
    time::{Duration, Instant},
};

#[path = "cli_spawn.rs"]
mod spawn;
#[path = "cli_transport.rs"]
mod transport;
#[allow(unused_imports)]
pub use transport::{Server, send};

pub const MAX_FILES: usize = 100;
pub const MAX_ANNOTATIONS: usize = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentView {
    #[default]
    Source,
    Preview,
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
fn default_limit() -> usize {
    100
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    pub vim: Option<bool>,
    pub autosave: Option<bool>,
    pub font_size: Option<f32>,
    pub tab_width: Option<usize>,
    pub word_wrap: Option<bool>,
    pub smooth_scroll: Option<bool>,
    pub leader_key: Option<String>,
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
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileAction {
    Rename { path: PathBuf, name: String },
    CreateFolder { parent: PathBuf, name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status {},
    Tabs {
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Open {
        files: Vec<OpenFile>,
    },
    Review {
        title: String,
        #[serde(default)]
        note: String,
        files: Vec<OpenFile>,
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
    },
    GitStatus {
        #[serde(default)]
        workspace: Option<PathBuf>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    Task {
        action: TaskAction,
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

pub const HELP: &str = r#"Morrow CLI — read, edit and arrange the running editor. Commands return JSON.

  morrow-cli open PATH... [--line N] [--column N] [--preview|--source]
  morrow-cli review MANIFEST.json
  morrow-cli status
  morrow-cli tabs [--limit N] [--offset N]
  morrow-cli focus TAB_ID [--line N] [--column N] [--preview|--source]
  morrow-cli close TAB_ID...
  morrow-cli clear-marks [TAB_ID...]
  morrow-cli workspace DIRECTORY
  morrow-cli panel explorer|search|source_control|review|output|problems|settings [--hide]
  morrow-cli read TAB_ID [--line N] [--count N]
  morrow-cli save TAB_ID...
  morrow-cli selection TAB_ID
  morrow-cli settings [PATCH.json]
  morrow-cli settings leader-key space|comma|backslash|CHAR
  morrow-cli bookmarks [list] [--limit N] [--offset N]
  morrow-cli bookmarks toggle [--tab ID] [--label TEXT]
  morrow-cli bookmarks remove|jump ID
  morrow-cli bookmarks rename ID --label TEXT
  morrow-cli command TAB_ID undo|redo|select_all|clear_selection|indent|outdent|toggle_comment|duplicate_line|delete_line|reload|find_next|find_previous|source|preview --revision N
  morrow-cli find TAB_ID QUERY [--case-sensitive] [--whole-word] [--limit N] [--offset N]
  morrow-cli replace TAB_ID QUERY REPLACEMENT --revision N [--case-sensitive] [--whole-word] [--first]
  morrow-cli diff TAB_ID [--count N]
  morrow-cli diagnostics TAB_ID [--limit N] [--offset N]
  morrow-cli search QUERY [WORKSPACE] [--case-sensitive] [--whole-word] [--limit N]
  morrow-cli output [--line N] [--count N]
  morrow-cli git-status [WORKSPACE] [--limit N] [--offset N]
  morrow-cli new [TITLE]
  morrow-cli task check|test|build|stop
  morrow-cli file ACTION.json
  morrow-cli file-job JOB_ID
  morrow-cli layout [CHANGE.json]
  morrow-cli raise
  morrow-cli request JSON_FILE       ('-' reads standard input)
  morrow-cli schema

Open/review/workspace start the sibling morrow.exe if needed. --no-start prevents
startup. Other commands require a running app. Relative paths resolve against
your working directory; review paths resolve against the manifest's directory.
Coordinates are 1-based lines and Unicode character columns, never byte offsets.
Opening is applied on the GUI thread; loading:true is pending, so inspect status
until the tab is ready. Existing unsaved text is preserved. Close refuses dirty
tabs. Batches report per-item results; retry only failed items.

Review example:
{"title":"Plan","files":[{"path":"plan.md","view":"preview"},
{"path":"src/main.rs","annotations":[{"start":{"line":12},
"end":{"line":16,"column":1},"note":"Check the changed flow"}]}]}
End coordinates are inclusive. Without end, the entire start line is marked.
Review replaces previous review marks. Marked files reveal their first mark
unless location is supplied. Open/review refresh clean buffers from disk;
unsaved buffers remain intact.

Use request JSON_FILE for atomic edits, save-as, selection changes and ordered
batches. Get the tab revision from read/tabs first; edit/replace/command/save_as
require expected_revision. A mismatch applies nothing. Edits are undoable and
pause autosave for that tab until explicitly saved, leaving global preferences
alone. Editing ranges have an EXCLUSIVE end; review marks have inclusive ends.
Send either set_text or nonoverlapping edits referring to the same revision:
{"op":"edit","tab_id":1,"expected_revision":3,"edits":[{"range":{
"start":{"line":2,"column":1},"end":{"line":2,"column":3}},"text":"96"}]}
layout changes use {"action":"split","orientation":"vertical","tab_id":1};
vertical means side by side, horizontal means top/bottom. To reorder or move a
tab, send {"action":"move_tab","source_pane_id":1,"pane_id":2,"tab_id":42,
"index":0}. Use actual IDs from layout; index is the final zero-based target
position after removing any existing target reference. The result returns the
updated groups and focus; an empty source split collapses. Use schema for complete
shapes and task/disk-search limitations. Batch stops at first failed/partial step
by default and reports completed steps; it never rolls back or replays them.
"#;

pub enum Parsed {
    Help,
    Schema,
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
        return Ok(Parsed::Help);
    }
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
            "--line" | "--column" | "--limit" | "--offset" | "--count" | "--revision" | "--tab"
            | "--label" => expects_value = true,
            _ => {}
        }
        true
    });
    if help {
        return Ok(Parsed::Help);
    }
    if args.is_empty() {
        return Err("A command is required after --no-start; use --help".into());
    }
    let command = args.remove(0);
    if command == "schema" {
        require_empty(&args)?;
        return Ok(Parsed::Schema);
    }
    let mut options = Options::default();
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
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
            "--label" => set_option(
                &mut options.label,
                iter.next().ok_or("--label needs text")?,
                "--label",
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
            Request::Open { files }
        }
        "review" => {
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
                [path] => Some(
                    transport::from_unique_json::<SettingsPatch>(json_bytes(&read_json_file(
                        &absolute(path.into()),
                    )?))
                    .map_err(|e| format!("Invalid settings patch: {e}"))?,
                ),
                _ => {
                    return Err(
                        "settings takes PATCH.json or leader-key space|comma|backslash|CHAR".into(),
                    );
                }
            },
        },
        "bookmarks" => Request::Bookmarks {
            change: parse_bookmarks(&positional, &options)?,
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
                workspace: positional.get(1).map(|p| absolute(p.into())),
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                limit: options.limit.unwrap_or(100),
            }
        }
        "output" => {
            require_empty(&positional)?;
            Request::Output {
                start_line: options.line.unwrap_or(1),
                line_count: options.count.unwrap_or(100),
            }
        }
        "git-status" => {
            if positional.len() > 1 {
                return Err("git-status takes an optional WORKSPACE".into());
            }
            Request::GitStatus {
                workspace: positional.first().map(|p| absolute(p.into())),
                limit: options.limit.unwrap_or(100),
                offset: options.offset.unwrap_or(0),
            }
        }
        "new" => {
            if positional.len() > 1 {
                return Err("new takes an optional TITLE; use request for path/text".into());
            }
            Request::New {
                title: positional.first().cloned(),
                text: String::new(),
                path: None,
            }
        }
        "task" => Request::Task {
            action: serde_json::from_value(json!(one_argument(
                &positional,
                "task needs check, test, build or stop"
            )?))
            .map_err(|_| "Unknown task; use --help")?,
        },
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
            let mut request: Request = transport::from_unique_json(json_bytes(&contents))
                .map_err(|e| format!("Invalid request: {e}"))?;
            resolve_paths(&mut request);
            request
        }
        _ => return Err(format!("Unknown command {command}; use --help")),
    };
    resolve_paths(&mut request);
    validate(&request)?;
    let start = !no_start
        && matches!(
            request,
            Request::Open { .. } | Request::Review { .. } | Request::Workspace { .. }
        );
    Ok(Parsed::Command { request, start })
}

#[derive(Default)]
struct Options {
    line: Option<usize>,
    column: Option<usize>,
    view: Option<DocumentView>,
    limit: Option<usize>,
    offset: Option<usize>,
    count: Option<usize>,
    revision: Option<u64>,
    tab: Option<u64>,
    label: Option<String>,
    case_sensitive: bool,
    whole_word: bool,
    first: bool,
    hide: bool,
}
impl Options {
    fn allowed(&self, command: &str) -> Result<(), String> {
        for (present, allowed, flag) in [
            (
                self.line.is_some(),
                matches!(command, "open" | "focus" | "read" | "output"),
                "--line",
            ),
            (
                self.column.is_some(),
                matches!(command, "open" | "focus"),
                "--column",
            ),
            (
                self.view.is_some(),
                matches!(command, "open" | "focus"),
                "--preview/--source",
            ),
            (
                self.limit.is_some(),
                matches!(
                    command,
                    "tabs" | "find" | "search" | "diagnostics" | "git-status" | "bookmarks"
                ),
                "--limit",
            ),
            (
                self.offset.is_some(),
                matches!(
                    command,
                    "tabs" | "find" | "diagnostics" | "git-status" | "bookmarks"
                ),
                "--offset",
            ),
            (
                self.count.is_some(),
                matches!(command, "read" | "output" | "diff"),
                "--count",
            ),
            (
                self.revision.is_some(),
                matches!(command, "command" | "replace"),
                "--revision",
            ),
            (self.tab.is_some(), command == "bookmarks", "--tab"),
            (self.label.is_some(), command == "bookmarks", "--label"),
            (
                self.case_sensitive,
                matches!(command, "find" | "search" | "replace"),
                "--case-sensitive",
            ),
            (
                self.whole_word,
                matches!(command, "find" | "search" | "replace"),
                "--whole-word",
            ),
            (self.first, command == "replace", "--first"),
            (self.hide, command == "panel", "--hide"),
        ] {
            if present && !allowed {
                return Err(format!("{flag} is not supported by {command}"));
            }
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

fn bookmark_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "Bookmark ID must be a positive integer returned by bookmarks".into())
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
        return Err("JSON file exceeds 1 MiB".into());
    }
    Ok(bytes)
}
fn json_bytes(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}
fn resolve_paths(request: &mut Request) {
    match request {
        Request::Open { files } | Request::Review { files, .. } => {
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
            FileAction::Rename { path, .. } => *path = absolute(path.clone()),
            FileAction::CreateFolder { parent, .. } => *parent = absolute(parent.clone()),
        },
        Request::Search {
            workspace: Some(path),
            ..
        }
        | Request::GitStatus {
            workspace: Some(path),
            ..
        } => *path = absolute(path.clone()),
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
        Request::Open { files } | Request::Review { files, .. } => {
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
        } => {
            validate_id(*tab_id)?;
            if *start_line == 0 || *line_count == 0 || *line_count > 500 {
                return Err("read uses a positive start_line and 1..500 lines".into());
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
            if let Some(leader_key) = &patch.leader_key {
                normalize_leader_key(leader_key)?;
            }
            if patch
                .font_size
                .is_some_and(|n| !n.is_finite() || !(10.0..=24.0).contains(&n))
            {
                return Err("font_size must be 10..24".into());
            }
            if patch.tab_width.is_some_and(|n| !(1..=8).contains(&n)) {
                return Err("tab_width must be 1..8".into());
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
            limit,
            ..
        } => {
            validate_query(query)?;
            validate_page(*limit, 0)?;
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::Output {
            start_line,
            line_count,
        } => {
            if *start_line == 0 || !(1..=500).contains(line_count) {
                return Err("output uses a positive start_line and 1..500 lines".into());
            }
        }
        Request::GitStatus {
            workspace,
            limit,
            offset,
        } => {
            validate_page(*limit, *offset)?;
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
                FileAction::Rename { path, name } => (path, name),
                FileAction::CreateFolder { parent, name } => (parent, name),
            };
            validate_path(path)?;
            if name.trim().is_empty()
                || name.len() > 255
                || name.contains(['\\', '/'])
                || matches!(name.as_str(), "." | "..")
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
                    Request::Batch { .. } | Request::Search { .. } | Request::File { .. }
                ) {
                    return Err("Nested batches, background disk search and asynchronous file actions cannot be inside batch; run search/file separately".into());
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
    if transport::endpoint_alive() || !start {
        return send(request);
    }
    {
        let executable = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("CLI executable has no parent")?
            .join(if cfg!(windows) {
                "morrow.exe"
            } else {
                "morrow"
            });
        if !executable.is_file() {
            return Err(format!(
                "Morrow is not running and {} was not found. Start the app, or put both executables in the same directory.",
                executable.display()
            ));
        }
        let mut child =
            spawn::spawn(&executable).map_err(|e| format!("Cannot start Morrow: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if transport::endpoint_alive() {
                return send(request);
            }
            if child.has_exited().map_err(|e| e.to_string())? {
                return Err("Morrow exited before its controls became ready. An older open version may not support the CLI; close it normally and start the new version.".into());
            }
            if Instant::now() >= deadline {
                return Err("Morrow started but controls are not ready after 12 seconds. Inspect the app before retrying.".into());
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
        send(&Request::Open { files })?
    };
    Ok(Some(response))
}

pub fn schema() -> Value {
    serde_json::from_str(include_str!("cli_schema.json")).expect("bundled CLI schema is valid JSON")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args(args.iter().map(OsString::from))
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
            Parsed::Help
        ));
        assert!(
            matches!(parse(&["open", "--no-start", "--", "--no-start"]).unwrap(), Parsed::Command { request: Request::Open { files }, start: false } if files[0].path.file_name().unwrap() == "--no-start")
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
    fn open_preserves_preview_and_exact_location() {
        let Parsed::Command {
            request: Request::Open { files },
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
            "morrow-layout-close-tab-{}-{}.json",
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
            "morrow-layout-move-tab-{}-{}.json",
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
            request: Request::Open { files },
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
            request: Request::Open { files },
            ..
        } = parse(&["open", "--", "--no-start", "--help"]).unwrap()
        else {
            panic!()
        };
        assert!(files[0].path.ends_with("--no-start"));
        assert!(files[1].path.ends_with("--help"));
    }
}
