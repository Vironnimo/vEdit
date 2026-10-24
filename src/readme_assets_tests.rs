//! Renders the README images from demo projects in real temporary Git
//! repositories. Agent-side state is created through the same control requests
//! `vedit-cli` sends; user-side state through ordinary keyboard input.
//! `python scripts/readme_assets.py` runs this generator and converts its PNGs.
use super::*;
use crate::cli::{DocumentView, LayoutAction, Parsed, SplitOrientation, parse_args};
use crate::git::ReviewSource;
use egui_kittest::{Harness, LazyRenderer};
use serde_json::{Value, json};
use std::ffi::OsString;

const DATE: &str = "2026-10-20T09:30:00+02:00";

const TIDES_COMMITTED: &str = r#"//! Tide predictions for one station, read from a local CSV table.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    High,
    Low,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tide {
    pub time: String,
    pub kind: Kind,
    pub height_cm: i32,
}

impl fmt::Display for Tide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            Kind::High => "high",
            Kind::Low => "low ",
        };
        write!(f, "{}  {kind}  {:>4} cm", self.time, self.height_cm)
    }
}

pub struct Table {
    tides: Vec<Tide>,
}

impl Table {
    /// Parses `time,kind,height` rows after the header line.
    pub fn parse(csv: &str) -> Result<Self, String> {
        let mut tides = Vec::new();
        for row in csv.lines().skip(1) {
            let fields: Vec<&str> = row.split(',').collect();
            let kind = if fields[1] == "H" { Kind::High } else { Kind::Low };
            let height_cm = fields[2].parse().map_err(|_| "bad height")?;
            let time = fields[0].to_owned();
            tides.push(Tide { time, kind, height_cm });
        }
        Ok(Self { tides })
    }

    pub fn next(&self, count: usize) -> impl Iterator<Item = &Tide> {
        self.tides.iter().take(count)
    }
}

pub fn load(station: &str) -> Result<Table, String> {
    let path = format!("data/{station}.csv");
    let csv = std::fs::read_to_string(&path)
        .map_err(|error| format!("{path}: {error}"))?;
    Table::parse(&csv)
}
"#;

const TIDES_CHANGED: &str = r#"//! Tide predictions for one station, read from a local CSV table.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    High,
    Low,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tide {
    pub time: String,
    pub kind: Kind,
    pub height_cm: i32,
}

impl fmt::Display for Tide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            Kind::High => "high",
            Kind::Low => "low ",
        };
        write!(f, "{}  {kind}  {:>4} cm", self.time, self.height_cm)
    }
}

pub struct Table {
    tides: Vec<Tide>,
}

impl Table {
    /// Parses `time,kind,height` rows after the header line.
    pub fn parse(csv: &str) -> Result<Self, String> {
        let mut tides = Vec::new();
        for (index, row) in csv.lines().enumerate().skip(1) {
            let number = index + 1;
            let tide = parse_row(row)
                .map_err(|problem| format!("line {number}: {problem}"))?;
            tides.push(tide);
        }
        Ok(Self { tides })
    }

    pub fn next(&self, count: usize) -> impl Iterator<Item = &Tide> {
        self.tides.iter().take(count)
    }
}

/// Checks one row, so an error can name the field that is wrong.
fn parse_row(row: &str) -> Result<Tide, String> {
    let fields: Vec<&str> = row.split(',').map(str::trim).collect();
    let [time, kind, height] = fields[..] else {
        return Err("expected 3 fields".into());
    };
    let kind = match kind {
        "H" => Kind::High,
        "L" => Kind::Low,
        other => return Err(format!("unknown kind {other:?}")),
    };
    let height_cm = height
        .parse()
        .map_err(|_| format!("invalid height {height:?}"))?;
    Ok(Tide { time: time.to_owned(), kind, height_cm })
}

pub fn load(station: &str) -> Result<Table, String> {
    let path = format!("data/{station}.csv");
    let csv = std::fs::read_to_string(&path)
        .map_err(|error| format!("{path}: {error}"))?;
    Table::parse(&csv)
}
"#;

const MAIN: &str = r#"mod tides;

use std::process::ExitCode;

fn main() -> ExitCode {
    let station = std::env::args().nth(1).unwrap_or_else(|| "kiel".into());
    match tides::load(&station) {
        Ok(table) => {
            for tide in table.next(4) {
                println!("{tide}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tidewatch: {error}");
            ExitCode::FAILURE
        }
    }
}
"#;

const PARSE_TESTS_COMMITTED: &str = r#"use tidewatch::tides::{Kind, Table};

#[test]
fn reads_high_and_low_tides() {
    let table = Table::parse("time,kind,height\n06:12,H,312\n12:40,L,88\n").unwrap();
    let kinds: Vec<Kind> = table.next(2).map(|tide| tide.kind).collect();
    assert_eq!(kinds, [Kind::High, Kind::Low]);
}
"#;

const PARSE_TESTS_CHANGED: &str = r#"use tidewatch::tides::{Kind, Table};

#[test]
fn reads_high_and_low_tides() {
    let table = Table::parse("time,kind,height\n06:12,H,312\n12:40,L,88\n").unwrap();
    let kinds: Vec<Kind> = table.next(2).map(|tide| tide.kind).collect();
    assert_eq!(kinds, [Kind::High, Kind::Low]);
}

#[test]
fn names_the_line_with_a_missing_field() {
    let error = Table::parse("time,kind,height\n06:12,H,312\n12:40,L\n").err();
    assert_eq!(error.as_deref(), Some("line 3: expected 3 fields"));
}

#[test]
fn rejects_an_unknown_tide_kind() {
    let error = Table::parse("time,kind,height\n06:12,X,312\n").err();
    assert_eq!(error.as_deref(), Some("line 2: unknown kind \"X\""));
}
"#;

const PLAN: &str = r#"# Plan: clearer errors for broken tide tables

`tidewatch` stops with a vague message when a station's CSV file has a bad row.
Report the line number and the field that is wrong.

## Steps

- [x] Number parse errors by their line in `src/tides.rs`
- [x] Reject unknown tide kinds instead of reading them as low tide
- [x] Add tests for a missing field and an unknown kind
- [ ] Mention the CSV format in the README

## Changed files

| File | Change |
| --- | --- |
| `src/tides.rs` | Line numbers, field checks, unknown kinds |
| `tests/parse.rs` | Two new tests for broken rows |

## Open question

Should one bad row skip that row or stop the program? The change keeps the
current behavior and stops.
"#;

const TIDEWATCH_README: &str = r#"# tidewatch

Prints the next high and low tides for a coastal station from a local CSV table.

```sh
cargo run -- kiel
```
"#;

const KIEL: &str = "time,kind,height\n06:12,H,312\n12:40,L,88\n18:31,H,305\n00:58,L,92\n";

const READER: &str = r#""""Read soil moisture sensors over I2C and keep a rolling average."""

from collections import deque
from dataclasses import dataclass


@dataclass
class Reading:
    sensor: str
    moisture: float


class Smoother:
    def __init__(self, size: int = 12) -> None:
        self.values: deque[float] = deque(maxlen=size)

    def add(self, value: float) -> float:
        self.values.append(value)
        return sum(self.values) / len(self.values)
"#;

/// One small Git repository with fixed authors, dates and no hooks.
struct DemoRepository {
    root: PathBuf,
}

impl DemoRepository {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join(name);
        fs::create_dir_all(&root).unwrap();
        let repository = Self { root };
        repository.git(&["init", "--quiet", "--initial-branch=main"]);
        let hooks = repository.root.join(".git").join("vedit-no-hooks");
        fs::create_dir_all(&hooks).unwrap();
        let config_path = repository.root.join(".git/config");
        let mut config = fs::read_to_string(&config_path).unwrap();
        config.push_str(
            "\n[user]\n\tname = Demo Author\n\temail = demo@example.invalid\n\
             [core]\n\tautocrlf = false\n\thooksPath = .git/vedit-no-hooks\n\
             [commit]\n\tgpgsign = false\n",
        );
        fs::write(config_path, config).unwrap();
        repository
    }

    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_AUTHOR_DATE", DATE)
            .env("GIT_COMMITTER_DATE", DATE)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write(&self, path: &str, text: &str) -> &Self {
        let path = PathBuf::from(self.path(path));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        self
    }

    fn commit(&self, message: &str) -> &Self {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
        self
    }

    fn path(&self, relative: &str) -> String {
        let path = relative
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(self.root.clone(), |path, part| path.join(part));
        crate::paths::display(&path)
    }
}

struct Demo {
    tidewatch: DemoRepository,
    sensors: DemoRepository,
    recipes: DemoRepository,
    lab: DemoRepository,
    scratch: PathBuf,
}

fn demo(base: &Path) -> Demo {
    let projects = base.join("projects");
    let tidewatch = DemoRepository::new(&projects, "tidewatch");
    tidewatch
        .write(
            "Cargo.toml",
            "[package]\nname = \"tidewatch\"\nversion = \"0.3.0\"\nedition = \"2024\"\n",
        )
        .write(".gitignore", "/target\n")
        // The lock file rust-analyzer would otherwise create during the run.
        .write(
            "Cargo.lock",
            "# This file is automatically @generated by Cargo.\n\
             # It is not intended for manual editing.\n\
             version = 4\n\n\
             [[package]]\n\
             name = \"tidewatch\"\n\
             version = \"0.3.0\"\n",
        )
        .write("src/main.rs", MAIN)
        .write("src/lib.rs", "pub mod tides;\n")
        .write("src/tides.rs", TIDES_COMMITTED)
        .write("data/kiel.csv", KIEL)
        .commit("Read tide tables from CSV")
        .write("tests/parse.rs", PARSE_TESTS_COMMITTED)
        .write("README.md", TIDEWATCH_README)
        .write("data/husum.csv", KIEL)
        .commit("Add Husum station and a parser test");
    // The agent's saved work: tests and its plan. Its parser change stays an
    // unsaved Agent draft in the editor.
    tidewatch
        .write("tests/parse.rs", PARSE_TESTS_CHANGED)
        .write("PLAN.md", PLAN);

    let sensors = DemoRepository::new(&projects, "garden-sensors");
    sensors
        .write(
            "pyproject.toml",
            "[project]\nname = \"garden-sensors\"\nversion = \"0.1.0\"\n",
        )
        .write("sensors/__init__.py", "")
        .write("sensors/reader.py", READER)
        .commit("Smooth sensor readings");
    sensors.write("sensors/reader.py", &READER.replace("12", "24"));

    let recipes = DemoRepository::new(&projects, "recipe-site");
    recipes
        .write("index.html", "<!doctype html>\n<title>Recipes</title>\n")
        .write("recipes/flatbread.md", "# Flatbread\n")
        .commit("Start the recipe site");

    let lab = DemoRepository::new(&projects, "home-lab");
    lab.write("compose.yaml", "services: {}\n")
        .commit("Add compose file");
    lab.write("compose.yaml", "services:\n  dns: {}\n")
        .write("notes.md", "# Router\n");

    let scratch = base.join("agent");
    fs::create_dir_all(&scratch).unwrap();
    Demo {
        tidewatch,
        sensors,
        recipes,
        lab,
        scratch,
    }
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
    if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
        options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
    }
    let screen = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_pixels_per_point(2.0)
        .with_theme(egui::Theme::Dark)
        .with_os(egui::os::OperatingSystem::Windows)
        .renderer(LazyRenderer::new(move || {
            egui_kittest::wgpu::WgpuTestRenderer::from_setup(setup)
        }))
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_session_save = Instant::now();
                app.session_dirty = false;
                app.draw_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.visuals.text_cursor.blink = false;
        style.scroll_animation = egui::style::ScrollAnimation::none();
        style.animation_time = 0.0;
    });
    screen
}

/// Sends one `vedit-cli` command line to the app, as the CLI transport would.
fn cli(screen: &mut Harness<'_, VEditApp>, args: &[&str]) -> Value {
    let Ok(Parsed::Command { request, .. }) = parse_args(args.iter().map(OsString::from)) else {
        panic!("vedit-cli {args:?} does not parse");
    };
    let response = screen.state_mut().control_request(request);
    assert!(response.ok, "vedit-cli {args:?}: {response:?}");
    screen.run_steps(2);
    response.result
}

fn wait_until(screen: &mut Harness<'_, VEditApp>, what: &str, done: impl Fn(&VEditApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(screen.state()) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
        screen.run_steps(1);
    }
}

fn settled(app: &VEditApp) -> bool {
    app.projects
        .iter()
        .all(|project| !project.git_loading && project.tabs.iter().all(|tab| !tab.loading))
        && !app.change_review_loading()
}

fn tab_id(screen: &mut Harness<'_, VEditApp>, path: &str) -> (String, String) {
    let tabs = cli(screen, &["tabs"]);
    let tab = tabs["items"]
        .as_array()
        .unwrap_or_else(|| panic!("tabs response: {tabs}"))
        .iter()
        .find(|tab| tab["path"] == path)
        .unwrap_or_else(|| panic!("{path} is open: {tabs}"));
    (tab["id"].to_string(), tab["revision"].to_string())
}

fn line_of(text: &str, needle: &str) -> usize {
    1 + text
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} in demo text"))
}

/// Presses a key together with the text event a real keyboard produces.
fn type_key(screen: &mut Harness<'_, VEditApp>, key: Key, text: &str) {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    screen
        .input_mut()
        .events
        .extend([event(true), egui::Event::Text(text.into()), event(false)]);
    screen.run_steps(2);
}

fn capture(screen: &mut Harness<'_, VEditApp>, directory: &Path, name: &str) {
    wait_until(screen, "background work", settled);
    visual_tests::settle_render(screen);
    let image = screen.render().expect("README asset render");
    let path = directory.join(format!("{name}.png"));
    image.save(&path).expect("write README asset");
    eprintln!("wrote {}", path.display());
}

#[test]
#[ignore = "manual asset generator: run python scripts/readme_assets.py"]
fn readme_assets() {
    let output = PathBuf::from(
        std::env::var_os("VEDIT_README_ASSETS_DIR").expect("VEDIT_README_ASSETS_DIR"),
    );
    fs::create_dir_all(&output).unwrap();
    let base = std::env::temp_dir().join(format!(
        "vedit-readme-assets-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let demo = demo(&base);

    let mut app = VEditApp::empty(egui::Context::default());
    app.recovery_session_path = base.join("session.json");
    app.settings.autosave = false;
    let mut screen = harness(app);
    screen.run_steps(2);

    // The user's projects; tidewatch ends up in front.
    for project in [&demo.sensors, &demo.recipes, &demo.lab, &demo.tidewatch] {
        cli(&mut screen, &["workspace", &project.path("")]);
    }
    for index in 0..screen.state().projects.len() {
        screen.state_mut().refresh_git(index, true);
    }
    cli(&mut screen, &["open", &demo.tidewatch.path("src/tides.rs")]);
    wait_until(&mut screen, "tidewatch files", settled);

    // The agent leaves its parser change as an unsaved draft, shows its plan
    // beside the code and tells the user it is done.
    let (tides, revision) = tab_id(&mut screen, &demo.tidewatch.path("src/tides.rs"));
    let draft = demo.scratch.join("tides.rs");
    fs::write(&draft, TIDES_CHANGED).unwrap();
    cli(
        &mut screen,
        &[
            "edit",
            &tides,
            "--file",
            &crate::paths::display(&draft),
            "--revision",
            &revision,
        ],
    );
    cli(
        &mut screen,
        &["open", &demo.tidewatch.path("PLAN.md"), "--preview"],
    );
    wait_until(&mut screen, "plan", settled);
    let (plan, _) = tab_id(&mut screen, &demo.tidewatch.path("PLAN.md"));
    cli(&mut screen, &["focus", &tides, "--source"]);
    screen
        .state_mut()
        .control_layout(LayoutAction::Split {
            orientation: SplitOrientation::Vertical,
            pane_id: None,
            tab_id: Some(plan.parse().unwrap()),
            view: Some(DocumentView::Preview),
        })
        .expect("plan beside the source");
    let layout = screen.state().layout_status();
    let source_pane = layout["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["view"] == "source")
        .and_then(|pane| pane["id"].as_u64())
        .expect("source pane");
    let app = screen.state_mut();
    app.control_layout(LayoutAction::Resize {
        pane_id: source_pane,
        ratio: 0.64,
    })
    .expect("wider source pane");
    app.control_layout(LayoutAction::Focus {
        pane_id: source_pane,
    })
    .expect("focus the source");
    cli(&mut screen, &["focus", &tides, "--line", "34", "--source"]);
    cli(
        &mut screen,
        &[
            "notify",
            "The parser names the broken line and field. All 3 tests pass; one open question is in PLAN.md.",
            "--title",
            "Clearer parse errors",
            "--level",
            "success",
            "--review",
            "working",
            "--workspace",
            &demo.tidewatch.path(""),
        ],
    );
    cli(
        &mut screen,
        &[
            "notify",
            "The rolling average now uses 24 readings.",
            "--title",
            "Smoother window doubled",
            "--workspace",
            &demo.sensors.path(""),
        ],
    );
    capture(&mut screen, &output, "workspace");

    // The agent points the user to the lines that matter.
    let close = screen.state().layout_status()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["view"] == "preview")
        .and_then(|pane| pane["id"].as_u64())
        .expect("plan pane");
    screen
        .state_mut()
        .control_layout(LayoutAction::Close { pane_id: close })
        .expect("one editor pane");
    let manifest = json!({
        "title": "Clearer parse errors",
        "note": "Start with parse_row. Both new errors have a test.",
        "files": [
            {
                "path": demo.tidewatch.path("src/tides.rs"),
                "annotations": [
                    {
                        "start": {"line": line_of(TIDES_CHANGED, "for (index, row)")},
                        "end": {"line": line_of(TIDES_CHANGED, "tides.push(tide)")},
                        "note": "Errors from a row now carry its line number.",
                        "severity": "info"
                    },
                    {
                        "start": {"line": line_of(TIDES_CHANGED, "other => return")},
                        "note": "Unknown kinds stop parsing instead of counting as low tide.",
                        "severity": "warning"
                    }
                ]
            },
            {
                "path": demo.tidewatch.path("tests/parse.rs"),
                "annotations": [
                    {
                        "start": {"line": line_of(PARSE_TESTS_CHANGED, "fn names_the_line")},
                        "end": {"line": line_of(PARSE_TESTS_CHANGED, "fn rejects_an_unknown")},
                        "note": "New tests for a missing field and an unknown kind.",
                        "severity": "info"
                    }
                ]
            }
        ]
    });
    let manifest_path = demo.scratch.join("review.json");
    fs::write(&manifest_path, manifest.to_string()).unwrap();
    cli(
        &mut screen,
        &["review", &crate::paths::display(&manifest_path)],
    );
    let (tides, _) = tab_id(&mut screen, &demo.tidewatch.path("src/tides.rs"));
    let first = line_of(TIDES_CHANGED, "pub fn parse(").to_string();
    cli(
        &mut screen,
        &["focus", &tides, "--line", &first, "--source"],
    );
    capture(&mut screen, &output, "agent-review");

    // The user keeps the draft, then reads all changes as one diff and
    // answers the agent on a line.
    let (tides, _) = tab_id(&mut screen, &demo.tidewatch.path("src/tides.rs"));
    cli(&mut screen, &["save", &tides]);
    wait_until(&mut screen, "saved draft", |app| {
        app.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .all(|tab| !tab.saving && tab.text == tab.saved_text)
    });
    cli(&mut screen, &["clear-marks"]);
    let project = screen
        .state()
        .projects
        .iter()
        .position(|project| project.root.ends_with("tidewatch"))
        .unwrap();
    screen.state_mut().refresh_git(project, true);
    wait_until(&mut screen, "saved status", settled);
    screen
        .state_mut()
        .show_change_review(project, ReviewSource::WorkingTree);
    wait_until(&mut screen, "change review", settled);
    type_key(&mut screen, Key::N, "n");
    // From the top of src/tides.rs to its `parse_row(row)...?` rows.
    for _ in 0..12 {
        type_key(&mut screen, Key::J, "j");
    }
    for _ in 0..2 {
        screen.key_press_modifiers(egui::Modifiers::SHIFT, Key::ArrowDown);
    }
    screen.run_steps(1);
    type_key(&mut screen, Key::C, "c");
    assert!(screen.state().agent_marks_modal_open(), "comment dialog");
    screen.event(egui::Event::Text(
        "Skip a broken row with a warning instead of stopping. husum.csv has one.".into(),
    ));
    screen.run_steps(2);
    capture(&mut screen, &output, "change-review");

    // Marks the user collects for the agent across files.
    screen.key_press_modifiers(egui::Modifiers::COMMAND, Key::Enter);
    screen.run_steps(2);
    screen.state_mut().close_change_review(project);
    cli(&mut screen, &["open", &demo.tidewatch.path("src/main.rs")]);
    wait_until(&mut screen, "main.rs", settled);
    for (path, from, to, note) in [
        (
            "src/main.rs",
            "let station =",
            "let station =",
            "Read the station from TIDEWATCH_STATION when no argument is given.",
        ),
        (
            "src/tides.rs",
            "let height_cm = height",
            "map_err(|_| format!(\"invalid height",
            "Also accept heights with a decimal point, such as 312.5.",
        ),
    ] {
        let (id, _) = tab_id(&mut screen, &demo.tidewatch.path(path));
        cli(&mut screen, &["focus", &id, "--source"]);
        let app = screen.state_mut();
        let tab = app.tab_mut().unwrap();
        let text = tab.text.clone();
        tab.vim.cursor = editor::go_to_line(&text, line_of(&text, from));
        tab.vim.handle_key(&mut tab.text, VimKey::Char('V'));
        for _ in line_of(&text, from)..line_of(&text, to) {
            tab.vim.handle_key(&mut tab.text, VimKey::Char('j'));
        }
        tab.focus = true;
        tab.pending_selection = tab.vim.selection(&tab.text);
        screen.run_steps(2);
        screen.key_press_modifiers(
            egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
            Key::A,
        );
        screen.run_steps(2);
        let app = screen.state_mut();
        let tab = app.tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Escape);
        if !note.is_empty() {
            let id = screen.state().agent_marks.items.last().unwrap().id;
            screen.state_mut().agent_marks_set_note(id, note);
        }
    }
    screen.state_mut().status = None;
    screen.state_mut().open_agent_marks();
    screen.run_steps(4);
    capture(&mut screen, &output, "agent-marks");
}
