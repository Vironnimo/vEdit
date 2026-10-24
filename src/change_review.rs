//! Change review: every change of the working tree, one commit or the current
//! branch in one continuous, read-only diff, with a file list to tick off.
//!
//! While open, the review replaces the active project's editor panes. It is
//! not a document tab: tabs, panes, recovery and saving stay untouched, and
//! closing it shows the same panes again. Git runs in workers, and every
//! result carries the project root and the generation it was requested for.
//! Generations are unique in the app, so a result for a closed project can
//! never match a review of a project reopened at the same root.

use super::*;
use crate::cli::{ChangeReviewSource, Location, Request, Response};
use crate::git_diff_view::{self as diff_view, ParsedPatch, RowMetrics, WrappedRows};
use crate::palette::BORDER_STRONG;
use crate::render;
use git::{FileChange, LineStats, ReviewComparison, ReviewFile, ReviewPatch, ReviewSource};
use serde_json::{Value, json};

#[path = "change_review/chrome.rs"]
mod chrome;
#[path = "change_review/input.rs"]
mod input;
#[path = "change_review/viewport.rs"]
mod viewport;

use chrome::{empty_review, file_list, review_header};
use input::{ReviewKey, review_action, review_hint};
use viewport::diff_body;

const HEADER_HEIGHT: f32 = 36.0;
const LIST_WIDTH: f32 = 248.0;
/// Below this width the file list gives its space to the diff.
const LIST_MIN_TOTAL_WIDTH: f32 = 560.0;
const LIST_ROW_HEIGHT: f32 = 24.0;
const FILE_HEADER_HEIGHT: f32 = 30.0;
const NOTE_HEIGHT: f32 = 40.0;
const FILE_GAP: f32 = 14.0;
/// Deferred patches requested together when they come into view.
const LAZY_FILES: usize = 24;
/// Line-number gutters stay aligned across files of ordinary length.
const MIN_NUMBER_DIGITS: usize = 4;

static REVIEW_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_generation() -> u64 {
    REVIEW_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Comparison key, then path, then the content key of the version marked seen.
type SeenMarks = HashMap<String, HashMap<String, u64>>;

/// Per project. Seen marks outlive closing and reopening the review; they
/// are kept in memory only.
#[derive(Default)]
pub(super) struct ChangeReviewState {
    view: Option<ChangeReview>,
    /// The last closed review, reused with its position when reopened.
    parked: Option<ChangeReview>,
    seen: SeenMarks,
}

struct ChangeReview {
    source: ReviewSource,
    repository: PathBuf,
    /// The change-set load whose result this view accepts.
    generation: u64,
    loading: bool,
    error: Option<String>,
    comparison: Option<ReviewComparison>,
    files: Vec<ReviewEntry>,
    omitted_untracked: usize,
    /// Top of each file block in diff content coordinates, from the last frame.
    tops: Vec<f32>,
    current: usize,
    scroll: f32,
    /// Reading position to restore after a refresh: path and offset into its block.
    anchor: Option<(String, f32)>,
    /// A file to scroll to once the block geometry is known.
    jump: Option<usize>,
    /// Only one automatic batch of deferred patches is in flight.
    lazy_request: Option<u64>,
    focus_pending: bool,
    /// A list row or seen box was clicked this frame.
    list_clicked: bool,
    /// Palette commands wait for the current diff geometry.
    pending_commands: Vec<ReviewKey>,
    follow_list: bool,
    list_offset: f32,
    list_height: f32,
    /// CLI requests answered once the comparison has loaded.
    replies: Vec<Sender<Response>>,
    /// Diff rows chosen for a comment to the agent.
    selection: Option<feedback::RowSelection>,
    /// The open "Comment for agent" dialog.
    comment: Option<feedback::CommentDraft>,
    /// Repository state this comparison was read from, and the project's
    /// state when it arrived; together they tell whether it is still fresh.
    freshness: Option<feedback::Freshness>,
}

impl ChangeReview {
    fn new(source: ReviewSource, repository: PathBuf) -> Self {
        Self {
            selection: None,
            comment: None,
            freshness: None,
            source,
            repository,
            generation: 0,
            loading: false,
            error: None,
            comparison: None,
            files: Vec::new(),
            omitted_untracked: 0,
            tops: Vec::new(),
            current: 0,
            scroll: 0.0,
            anchor: None,
            jump: None,
            lazy_request: None,
            focus_pending: false,
            list_clicked: false,
            pending_commands: Vec::new(),
            follow_list: true,
            list_offset: 0.0,
            list_height: 0.0,
            replies: Vec::new(),
        }
    }

    fn position(&self) -> Option<(String, f32)> {
        let entry = self.files.get(self.current)?;
        let top = self.tops.get(self.current)?;
        Some((entry.path.clone(), (self.scroll - top).max(0.0)))
    }

    fn totals(&self) -> (u64, u64) {
        self.files
            .iter()
            .filter_map(|entry| entry.stats)
            .fold((0, 0), |(added, removed), stats| {
                (
                    added.saturating_add(stats.added),
                    removed.saturating_add(stats.removed),
                )
            })
    }

    fn title(&self) -> String {
        if let Some(comparison) = &self.comparison {
            return comparison.title.clone();
        }
        match &self.source {
            ReviewSource::WorkingTree => "Working tree".into(),
            ReviewSource::Commit(revision) => format!("Commit {revision}"),
            ReviewSource::Branch { base: Some(base) } => format!("Branch vs {base}"),
            ReviewSource::Branch { base: None } => "Branch vs main".into(),
        }
    }

    fn is_seen(&self, seen: &SeenMarks, index: usize) -> bool {
        let (Some(comparison), Some(entry)) = (&self.comparison, self.files.get(index)) else {
            return false;
        };
        seen.get(&comparison.key)
            .and_then(|files| files.get(&entry.path))
            == Some(&entry.content_key)
    }

    fn set_seen(&self, seen: &mut SeenMarks, index: usize, value: bool) {
        let (Some(comparison), Some(entry)) = (&self.comparison, self.files.get(index)) else {
            return;
        };
        let files = seen.entry(comparison.key.clone()).or_default();
        if value {
            files.insert(entry.path.clone(), entry.content_key);
        } else {
            files.remove(&entry.path);
        }
    }

    fn seen_count(&self, seen: &SeenMarks) -> usize {
        (0..self.files.len())
            .filter(|index| self.is_seen(seen, *index))
            .count()
    }

    /// The file whose block contains the top of the viewport at `offset`.
    fn file_at(&self, offset: f32) -> usize {
        self.tops
            .partition_point(|top| *top <= offset + 0.5)
            .saturating_sub(1)
            .min(self.files.len().saturating_sub(1))
    }
}

struct ReviewEntry {
    path: String,
    original_path: Option<String>,
    status: FileChange,
    stats: Option<LineStats>,
    content_key: u64,
    patch: EntryPatch,
    wrapped: WrappedRows,
}

impl ReviewEntry {
    fn new(file: ReviewFile, patch: EntryPatch) -> Self {
        Self {
            path: file.path,
            original_path: file.original_path,
            status: file.status,
            stats: file.stats,
            content_key: file.content_key,
            patch,
            wrapped: WrappedRows::default(),
        }
    }

    /// The listed file, for requesting its patch later.
    fn review_file(&self) -> ReviewFile {
        ReviewFile {
            path: self.path.clone(),
            original_path: self.original_path.clone(),
            status: self.status,
            stats: self.stats,
            content_key: self.content_key,
            patch: ReviewPatch::Deferred,
        }
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(folder, _)| folder)
    }

    fn body_height(&self) -> f32 {
        match &self.patch {
            EntryPatch::Parsed(patch) if patch.rows() > 0 => self.wrapped.height(),
            _ => NOTE_HEIGHT,
        }
    }
}

/// A file's patch as the view shows it. Parsing happens in the worker.
pub(super) enum EntryPatch {
    Parsed(ParsedPatch),
    Large,
    Deferred,
    Loading,
    Binary,
    Unavailable(String),
}

impl From<ReviewPatch> for EntryPatch {
    fn from(patch: ReviewPatch) -> Self {
        match patch {
            ReviewPatch::Loaded(text) => Self::Parsed(ParsedPatch::parse(&text)),
            ReviewPatch::Large => Self::Large,
            ReviewPatch::Deferred => Self::Deferred,
            ReviewPatch::Binary => Self::Binary,
            ReviewPatch::Unavailable(message) => Self::Unavailable(message),
        }
    }
}

pub(super) struct LoadedReview {
    comparison: ReviewComparison,
    files: Vec<(ReviewFile, EntryPatch)>,
    omitted_untracked: usize,
    /// HEAD and status fingerprint read just before the comparison.
    basis: Option<(Option<git::StatusHead>, u64)>,
}

pub(super) struct LoadedPatch {
    path: String,
    content_key: u64,
    patch: EntryPatch,
}

fn load_review(repository: &Path, source: &ReviewSource) -> Result<LoadedReview, String> {
    // Read before the comparison: a change in between shows the review as
    // possibly outdated rather than hiding a change from it.
    let basis = match source {
        ReviewSource::Commit(_) => None,
        _ => git::working_tree_fingerprint(repository).ok(),
    };
    let set = git::change_set(repository, source).map_err(|error| error.to_string())?;
    Ok(LoadedReview {
        basis,
        comparison: set.comparison,
        files: set
            .files
            .into_iter()
            .map(|mut file| {
                let patch = std::mem::replace(&mut file.patch, ReviewPatch::Deferred);
                (file, patch.into())
            })
            .collect(),
        omitted_untracked: set.omitted_untracked,
    })
}

fn load_patches(
    repository: &Path,
    comparison: &ReviewComparison,
    files: Vec<ReviewFile>,
) -> Vec<LoadedPatch> {
    let patches = git::review_patches(repository, comparison, &files);
    files
        .into_iter()
        .zip(patches)
        .map(|(file, patch)| LoadedPatch {
            path: file.path,
            content_key: file.content_key,
            patch: patch.into(),
        })
        .collect()
}

fn answer(replies: &mut Vec<Sender<Response>>, code: &str, message: &str) {
    for reply in replies.drain(..) {
        let _ = reply.send(Response::failure(code, message));
    }
}

fn review_body_id(root: &Path) -> egui::Id {
    egui::Id::new(("change-review-body", root))
}

fn review_summary(root: &Path, view: &ChangeReview, seen: &SeenMarks, visible: bool) -> Value {
    let (added, removed) = view.totals();
    json!({
        "applied": true,
        "visible": visible,
        "workspace": crate::paths::display(root),
        "repository": crate::paths::display(&view.repository),
        "source": view.source.kind(),
        "title": view.title(),
        "detail": view.comparison.as_ref().map(|comparison| comparison.detail.as_str()),
        "files": view.files.len(),
        "added": added,
        "removed": removed,
        "seen": view.seen_count(seen),
        "omitted_untracked": view.omitted_untracked,
    })
}

enum ReviewAction {
    Close,
    Refresh,
    AgentMarks,
    Open {
        file: usize,
        line: Option<usize>,
    },
    Load {
        files: Vec<usize>,
        automatic: bool,
    },
    Status(String),
    /// Add the comment of the dialog as an agent mark.
    Comment(feedback::CommentDraft),
    /// Show an unsaved document the review does not include.
    ShowDocument(PathBuf),
}

impl VEditApp {
    /// Whether a change review replaces the active project's editor panes.
    pub(super) fn change_review_visible(&self) -> bool {
        self.project()
            .is_some_and(|project| project.change_review.view.is_some())
    }

    /// Whether any project's change review still waits for Git or patch workers.
    #[cfg(test)]
    pub(super) fn change_review_loading(&self) -> bool {
        self.projects.iter().any(|project| {
            project.change_review.view.as_ref().is_some_and(|view| {
                view.loading
                    || view.lazy_request.is_some()
                    || view
                        .files
                        .iter()
                        .any(|entry| matches!(entry.patch, EntryPatch::Loading))
            })
        })
    }

    pub(super) fn review_command(&mut self, action: keybindings::Action) {
        let Some(command) = review_action(action) else {
            return;
        };
        let body_id = self.project().map(|project| review_body_id(&project.root));
        let Some(view) = self
            .projects
            .get_mut(self.active_project)
            .and_then(|project| project.change_review.view.as_mut())
        else {
            self.status("Open a change review to use this command", false);
            return;
        };
        if view.comment.is_some() || view.files.is_empty() {
            return;
        }
        view.pending_commands.push(command);
        view.focus_pending = true;
        if let Some(body_id) = body_id {
            self.ctx.memory_mut(|memory| memory.request_focus(body_id));
        }
        self.ctx.request_repaint();
    }

    /// Open (or refresh) a review in `project_index` and give it the keyboard.
    pub(super) fn open_change_review(
        &mut self,
        project_index: usize,
        source: ReviewSource,
    ) -> Result<(), String> {
        self.load_change_review(project_index, source, false)
            .map(|_| ())
    }

    /// Load a review. In `background` the visible project, its panes and the
    /// keyboard stay as they are: another project shows the review when the
    /// user switches to it, and the visible project keeps it ready for the
    /// next review command unless it already shows this comparison.
    /// Returns whether the review is the one its project shows.
    pub(super) fn load_change_review(
        &mut self,
        project_index: usize,
        source: ReviewSource,
        background: bool,
    ) -> Result<bool, String> {
        let project = self
            .projects
            .get(project_index)
            .ok_or("The project is no longer open")?;
        let repository = project.git_root.clone().ok_or(
            "This project has no Git repository. Initialize or select one in Source control.",
        )?;
        if let ReviewSource::Commit(revision)
        | ReviewSource::Branch {
            base: Some(revision),
        } = &source
        {
            git::validate_review_revision(revision).map_err(|error| error.to_string())?;
        }
        if !background {
            if project_index != self.active_project {
                self.switch_project(project_index);
            }
            // The review takes the keyboard: finish pending Vim input as a tab switch would.
            self.cancel_vim_search(false);
            if let Some(tab) = self.tab_mut() {
                let action = tab.vim.finish_insert(&mut tab.text);
                if action.text_changed {
                    tab.changed();
                }
            }
            self.completions = None;
            self.completion_request = None;
        }
        let visible_project = project_index == self.active_project;
        let project = &mut self.projects[project_index];
        if !background {
            project.git_view.detail_open = false;
        }
        let root = project.root.clone();
        let state = &mut project.change_review;
        let fits = |view: &ChangeReview| view.source == source && view.repository == repository;
        let shown = !background || !visible_project || state.view.as_ref().is_some_and(fits);
        let mut view = if !shown {
            // Prepared out of sight; the user's panes stay on screen.
            match state.parked.take() {
                Some(previous) if fits(&previous) => previous,
                Some(mut previous) => {
                    answer(
                        &mut previous.replies,
                        "review_replaced",
                        "Another change review replaced this one before it finished loading",
                    );
                    ChangeReview::new(source, repository)
                }
                None => ChangeReview::new(source, repository),
            }
        } else if state.view.as_ref().is_some_and(fits) {
            state.view.take().expect("checked review")
        } else {
            let reused = state.parked.take_if(|view| fits(view));
            if let Some(mut previous) = state.view.take() {
                answer(
                    &mut previous.replies,
                    "review_replaced",
                    "Another change review replaced this one before it finished loading",
                );
                state.parked = Some(previous);
            }
            reused.unwrap_or_else(|| ChangeReview::new(source, repository))
        };
        if view.anchor.is_none() {
            view.anchor = view.position();
        }
        for entry in &mut view.files {
            if matches!(entry.patch, EntryPatch::Loading) {
                entry.patch = EntryPatch::Deferred;
            }
        }
        view.generation = next_generation();
        view.loading = true;
        view.lazy_request = None;
        view.focus_pending = !background;
        let generation = view.generation;
        let source = view.source.clone();
        let repository = view.repository.clone();
        if shown {
            state.view = Some(view);
        } else {
            state.parked = Some(view);
        }
        self.worker(move || WorkerResult::ChangeReview {
            root,
            generation,
            result: load_review(&repository, &source),
        });
        if !background {
            self.focus_editor();
        }
        self.ctx.request_repaint();
        Ok(shown)
    }

    /// Open a review from a menu or command; problems go to the status bar.
    pub(super) fn show_change_review(&mut self, project_index: usize, source: ReviewSource) {
        if let Err(error) = self.open_change_review(project_index, source) {
            self.status(error, true);
        }
    }

    /// Close the review and return the keyboard to the editor panes.
    pub(super) fn close_change_review(&mut self, project_index: usize) {
        if self.park_change_review(project_index)
            && project_index == self.active_project
            && let Some(tab) = self.tab_mut()
        {
            // The panes are drawn again from the next frame on; the document
            // takes the focus there, as after switching tabs.
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
    }

    /// Hide the review without moving focus, e.g. because a document opens.
    pub(super) fn park_change_review(&mut self, project_index: usize) -> bool {
        let Some(project) = self.projects.get_mut(project_index) else {
            return false;
        };
        let Some(mut view) = project.change_review.view.take() else {
            return false;
        };
        answer(
            &mut view.replies,
            "review_closed",
            "The change review was closed before it finished loading",
        );
        view.focus_pending = false;
        project.change_review.parked = Some(view);
        let id = review_body_id(&project.root);
        self.ctx.memory_mut(|memory| {
            if memory.has_focus(id) {
                memory.surrender_focus(id);
            }
        });
        self.ctx.request_repaint();
        true
    }

    /// Focus the visible review instead of a document. Returns false when
    /// no review is visible.
    pub(super) fn focus_change_review(&mut self) -> bool {
        let Some(project) = self.projects.get_mut(self.active_project) else {
            return false;
        };
        let Some(view) = project.change_review.view.as_mut() else {
            return false;
        };
        // The body takes focus when it is drawn. Requesting it now could name
        // a widget this frame does not contain, which accessibility rejects.
        view.focus_pending = true;
        self.ctx.request_repaint();
        true
    }

    pub(super) fn apply_change_review(
        &mut self,
        root: PathBuf,
        generation: u64,
        result: Result<LoadedReview, String>,
    ) {
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.root == root)
        else {
            return;
        };
        let visible_project = index == self.active_project;
        let project = &mut self.projects[index];
        let current_status = (project.git_head.clone(), project.git_fingerprint);
        let ChangeReviewState { view, parked, seen } = &mut project.change_review;
        // A background review loads while parked; it answers as not visible.
        let (view, shown) = match view.as_mut().filter(|view| view.generation == generation) {
            Some(view) => (view, true),
            None => match parked.as_mut().filter(|view| view.generation == generation) {
                Some(view) => (view, false),
                None => return,
            },
        };
        view.loading = false;
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(error) => {
                answer(&mut view.replies, "review_failed", &error);
                view.error = Some(error);
                return;
            }
        };
        view.freshness = loaded.basis.map(|(head, fingerprint)| feedback::Freshness {
            head,
            fingerprint,
            arrived_head: current_status.0,
            arrived_fingerprint: current_status.1,
        });
        // Unchanged files keep their parsed patch and wrapped geometry.
        let mut previous: HashMap<(String, u64), ReviewEntry> = view
            .files
            .drain(..)
            .filter(|entry| matches!(entry.patch, EntryPatch::Parsed(_)))
            .map(|entry| ((entry.path.clone(), entry.content_key), entry))
            .collect();
        view.files = loaded
            .files
            .into_iter()
            .map(
                |(file, patch)| match previous.remove(&(file.path.clone(), file.content_key)) {
                    Some(mut kept) => {
                        kept.status = file.status;
                        kept.stats = file.stats;
                        kept.original_path = file.original_path;
                        kept
                    }
                    None => ReviewEntry::new(file, patch),
                },
            )
            .collect();
        view.comparison = Some(loaded.comparison);
        view.omitted_untracked = loaded.omitted_untracked;
        view.error = None;
        view.tops.clear();
        view.current = view.current.min(view.files.len().saturating_sub(1));
        view.follow_list = true;
        let summary = review_summary(&project.root, view, seen, shown && visible_project);
        for reply in view.replies.drain(..) {
            let _ = reply.send(Response::success(summary.clone()));
        }
        self.ctx.request_repaint();
    }

    pub(super) fn apply_change_review_patches(
        &mut self,
        root: PathBuf,
        generation: u64,
        request: u64,
        patches: Vec<LoadedPatch>,
    ) {
        let Some(view) = self
            .projects
            .iter_mut()
            .find(|project| project.root == root)
            .and_then(|project| project.change_review.view.as_mut())
            .filter(|view| view.generation == generation)
        else {
            return;
        };
        if view.lazy_request == Some(request) {
            view.lazy_request = None;
        }
        for patch in patches {
            if let Some(entry) = view.files.iter_mut().find(|entry| {
                entry.path == patch.path
                    && entry.content_key == patch.content_key
                    && matches!(
                        entry.patch,
                        EntryPatch::Loading | EntryPatch::Deferred | EntryPatch::Large
                    )
            }) {
                entry.patch = patch.patch;
                entry.wrapped = WrappedRows::default();
            }
        }
        self.ctx.request_repaint();
    }

    fn request_review_patches(&mut self, project_index: usize, files: Vec<usize>, automatic: bool) {
        let Some(project) = self.projects.get_mut(project_index) else {
            return;
        };
        let root = project.root.clone();
        let state = &mut project.change_review;
        let Some(view) = state.view.as_mut() else {
            return;
        };
        let Some(comparison) = view.comparison.clone() else {
            return;
        };
        if automatic && view.lazy_request.is_some() {
            return;
        }
        let mut requested = Vec::new();
        for index in files {
            if let Some(entry) = view.files.get_mut(index)
                && matches!(entry.patch, EntryPatch::Deferred | EntryPatch::Large)
            {
                entry.patch = EntryPatch::Loading;
                requested.push(entry.review_file());
            }
        }
        if requested.is_empty() {
            return;
        }
        let request = next_generation();
        if automatic {
            view.lazy_request = Some(request);
        }
        let generation = view.generation;
        let repository = view.repository.clone();
        self.worker(move || WorkerResult::ChangeReviewPatches {
            root,
            generation,
            request,
            patches: load_patches(&repository, &comparison, requested),
        });
    }

    /// Open the reviewed file in the editor at `line`. Showing the document
    /// closes the review; reopening it restores the reading position.
    fn open_review_file(&mut self, project_index: usize, file: usize, line: Option<usize>) {
        let Some((repository, path, status)) = self
            .projects
            .get(project_index)
            .and_then(|project| project.change_review.view.as_ref())
            .and_then(|view| {
                let entry = view.files.get(file)?;
                Some((view.repository.clone(), entry.path.clone(), entry.status))
            })
        else {
            return;
        };
        if status == FileChange::Deleted {
            self.status(
                format!("{path} is deleted in this comparison; there is no file to open"),
                true,
            );
            return;
        }
        let path = path
            .split('/')
            .fold(repository, |path, part| path.join(part));
        self.open_file(path);
        if let Some(line) = line
            && let Some(tab) = self.tab_mut()
        {
            tab.pending_agent_location = Some(Location { line, column: 1 });
            tab.agent_location_error = None;
            self.apply_pending_agent_locations();
        }
    }

    /// The review in place of the editor panes.
    pub(super) fn change_review_ui(&mut self, ui: &mut egui::Ui) {
        let index = self.active_project;
        let rect = ui.available_rect_before_wrap();
        // Spatial focus treats the review as the focused pane's region.
        self.cover_editor_panes(rect);
        let root = self.projects[index].root.clone();
        let body_id = review_body_id(&root);
        self.register_region_focus_widget(body_id);
        let context = self.review_context(index);
        let mut state = std::mem::take(&mut self.projects[index].change_review);
        let mut actions = Vec::new();
        let source = if let ChangeReviewState {
            view: Some(view),
            seen,
            ..
        } = &mut state
        {
            review_ui(ui, rect, view, seen, body_id, &context, &mut actions);
            Some(view.source.clone())
        } else {
            None
        };
        self.projects[index].change_review = state;
        ui.allocate_rect(rect, egui::Sense::hover());
        for action in actions {
            match action {
                ReviewAction::Close => self.close_change_review(index),
                ReviewAction::AgentMarks => self.open_agent_marks(),
                ReviewAction::Refresh => {
                    if let Some(source) = source.clone()
                        && let Err(error) = self.open_change_review(index, source)
                    {
                        self.status(error, true);
                    }
                }
                ReviewAction::Open { file, line } => self.open_review_file(index, file, line),
                ReviewAction::Load { files, automatic } => {
                    self.request_review_patches(index, files, automatic)
                }
                ReviewAction::Status(text) => self.status(text, false),
                ReviewAction::Comment(draft) => self.add_review_comment(index, draft),
                ReviewAction::ShowDocument(path) => self.open_file(path),
            }
        }
    }

    /// `vedit-cli review --working|--commit REV|--branch`: show the review
    /// and answer once its comparison has loaded.
    pub(super) fn control_change_review(
        &mut self,
        request: &Request,
        reply: Sender<Response>,
    ) -> bool {
        let Request::ChangeReview {
            source,
            rev,
            base,
            workspace,
            background,
        } = request
        else {
            return false;
        };
        let background = *background;
        if let Err(error) = crate::cli::validate(request) {
            let _ = reply.send(Response::failure("invalid_request", error));
            return true;
        }
        if !background && let Some(dialog) = self.control_dialog() {
            let _ = reply.send(Response::failure(
                "user_dialog_open",
                format!(
                    "The user's {dialog} interaction is active. No review was opened; inspect status after the user finishes it."
                ),
            ));
            return true;
        }
        let (project_index, workspace_source) =
            match self.control_project_index(workspace.as_deref()) {
                Ok(found) => found,
                Err(response) => {
                    let _ = reply.send(response);
                    return true;
                }
            };
        let reply = control::reply_naming_workspace(
            reply,
            self.projects[project_index].root.clone(),
            workspace_source,
            background,
        );
        if self.projects[project_index].git_root.is_none() {
            let _ = reply.send(Response::failure(
                "no_repository",
                "This workspace has no loaded repository. Inspect git-status for loading or errors.",
            ));
            return true;
        }
        let source = match source {
            ChangeReviewSource::Working => ReviewSource::WorkingTree,
            ChangeReviewSource::Commit => ReviewSource::Commit(rev.clone().unwrap_or_default()),
            ChangeReviewSource::Branch => ReviewSource::Branch { base: base.clone() },
        };
        let shown = match self.load_change_review(project_index, source, background) {
            Ok(shown) => shown,
            Err(error) => {
                let _ = reply.send(Response::failure("review_unavailable", error));
                return true;
            }
        };
        let state = &mut self.projects[project_index].change_review;
        if let Some(view) = if shown {
            state.view.as_mut()
        } else {
            state.parked.as_mut()
        } {
            view.replies.push(reply);
        }
        if !background {
            self.raise_control_window();
        }
        true
    }
}

fn review_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &mut ChangeReview,
    seen: &mut SeenMarks,
    body_id: egui::Id,
    context: &feedback::ReviewContext,
    actions: &mut Vec<ReviewAction>,
) {
    ui.painter().rect_filled(rect, 0.0, BG);
    let header = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT));
    review_header(ui, header, view, seen, context, actions);
    let mut body = egui::Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);
    if !context.drafts.is_empty() {
        let banner =
            egui::Rect::from_min_size(body.min, Vec2::new(body.width(), feedback::BANNER_HEIGHT));
        feedback::drafts_banner(ui, banner, &context.drafts, actions);
        body.min.y = banner.bottom();
    }
    if view.comment.is_some() {
        feedback::comment_dialog(ui.ctx(), body_id, view, &context.bindings, actions);
    }
    if view.files.is_empty() {
        empty_review(ui, body, view, actions);
        return;
    }
    let (list, diff) = if body.width() >= LIST_MIN_TOTAL_WIDTH {
        let width = LIST_WIDTH.min((body.width() * 0.32).floor());
        let list =
            egui::Rect::from_min_max(body.min, egui::pos2(body.left() + width, body.bottom()));
        ui.painter().line_segment(
            [list.right_top(), list.right_bottom()],
            Stroke::new(1.0, BORDER),
        );
        (
            Some(list),
            egui::Rect::from_min_max(egui::pos2(list.right() + 1.0, body.top()), body.max),
        )
    } else {
        (None, body)
    };
    if let Some(list) = list {
        view.list_clicked = file_list(ui, list, view, seen);
    }
    diff_body(ui, diff, view, seen, body_id, context, actions);
}

#[path = "change_review_feedback.rs"]
mod feedback;

#[cfg(test)]
#[path = "change_review_tests.rs"]
mod tests;
