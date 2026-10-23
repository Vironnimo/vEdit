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

#[derive(Clone, Copy, Debug, PartialEq)]
enum ReviewKey {
    Lines(f32),
    Pages(f32),
    Top,
    Bottom,
    NextFile,
    PreviousFile,
    SeenNext,
    Open,
    Close,
    /// Shift+Arrow: extend the selected diff rows by one.
    Extend(isize),
    /// c: comment on the selected rows, or on the line on screen.
    Comment,
}

impl VEditApp {
    /// Whether a change review replaces the active project's editor panes.
    pub(super) fn change_review_visible(&self) -> bool {
        self.project()
            .is_some_and(|project| project.change_review.view.is_some())
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
        feedback::comment_dialog(ui.ctx(), body_id, view, actions);
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
    diff_body(ui, diff, view, seen, body_id, context.swap_jk, actions);
}

fn review_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &ChangeReview,
    seen: &SeenMarks,
    context: &feedback::ReviewContext,
    actions: &mut Vec<ReviewAction>,
) {
    ui.painter().rect_filled(rect, 0.0, PANEL);
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BORDER),
    );
    let mut header = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(10.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center))
            .id_salt("change-review-header"),
    );
    let ui = &mut header;
    ui.spacing_mut().item_spacing.x = 10.0;
    let close = ui
        .add(
            egui::Button::new(RichText::new("×").size(16.0))
                .frame_when_inactive(false)
                .min_size(Vec2::new(26.0, 26.0)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Close the review and return to the editor · Esc");
    close.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close change review")
    });
    if close.clicked() {
        actions.push(ReviewAction::Close);
    }
    let refresh = ui
        .add_enabled_ui(!view.loading, |ui| {
            design::ghost_button(
                ui,
                "Refresh",
                "Compare again; seen marks and the reading position stay",
            )
        })
        .inner;
    if refresh.clicked() {
        actions.push(ReviewAction::Refresh);
    }
    let marks = design::ghost_button(
        ui,
        &format!("For agent ({})", context.marked_blocks),
        "Open marked blocks and review comments across all projects",
    );
    marks.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "For agent")
    });
    if marks.clicked() {
        actions.push(ReviewAction::AgentMarks);
    }
    if context.stale && !view.loading {
        feedback::stale_indicator(ui);
    }
    if view.comparison.is_some() {
        let count = view.files.len();
        ui.label(
            RichText::new(format!("{} of {count} seen", view.seen_count(seen)))
                .size(11.5)
                .color(MUTED),
        )
        .on_hover_text("Space marks the current file seen and moves to the next unseen file. A file that changes again becomes unseen.");
        let (added, removed) = view.totals();
        let tip = "Lines added and removed in this comparison";
        ui.label(
            RichText::new(format!("−{}", git_panel::compact_number(removed)))
                .size(11.5)
                .color(RED),
        )
        .on_hover_text(format!("{removed} lines removed"));
        ui.label(
            RichText::new(format!("+{}", git_panel::compact_number(added)))
                .size(11.5)
                .color(GREEN),
        )
        .on_hover_text(format!("{added} lines added · {tip}"));
        let files = format!("{count} {}", if count == 1 { "file" } else { "files" });
        let label = ui.label(RichText::new(files).size(11.5).color(MUTED));
        if view.omitted_untracked > 0 {
            label.on_hover_text(format!(
                "{} more untracked files are not listed",
                view.omitted_untracked
            ));
        }
    }
    if view.loading {
        ui.add(egui::Spinner::new().size(13.0).color(MUTED))
            .on_hover_text("Comparing…");
    }
    if let Some(error) = &view.error
        && !view.files.is_empty()
    {
        ui.label(RichText::new("Refresh failed").size(11.5).color(ORANGE))
            .on_hover_details(error);
    }
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let detail = view
            .comparison
            .as_ref()
            .map_or("", |comparison| comparison.detail.as_str());
        let title = ui.add(
            egui::Label::new(RichText::new(view.title()).size(13.0).strong().color(TEXT))
                .truncate(),
        );
        if !detail.is_empty() {
            title.on_hover_details(detail);
        }
    });
}

fn empty_review(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &ChangeReview,
    actions: &mut Vec<ReviewAction>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(24.0))
            .layout(Layout::top_down(Align::Center))
            .id_salt("change-review-empty"),
    );
    let ui = &mut child;
    ui.add_space((rect.height() * 0.25).clamp(12.0, 160.0));
    if let Some(error) = &view.error {
        ui.label(
            RichText::new("The comparison could not be loaded")
                .strong()
                .color(TEXT),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(error).size(12.0).color(ORANGE)).wrap());
        ui.add_space(10.0);
        if !view.loading && design::ghost_button(ui, "Try again", "Compare again").clicked() {
            actions.push(ReviewAction::Refresh);
        }
    } else if view.loading || view.comparison.is_none() {
        ui.add(egui::Spinner::new().size(18.0).color(MUTED));
        ui.add_space(8.0);
        ui.label(RichText::new("Comparing…").size(12.0).color(MUTED));
    } else {
        ui.label(RichText::new("No changes").strong().color(TEXT));
        ui.add_space(6.0);
        let detail = match view.source {
            ReviewSource::WorkingTree => "The working tree matches the last commit.",
            ReviewSource::Commit(_) => "This commit changes no files.",
            ReviewSource::Branch { .. } => "This branch has no changes since it left its base.",
        };
        ui.label(RichText::new(detail).size(12.0).color(MUTED));
    }
}

enum ListRow {
    Folder(usize),
    File(usize, usize),
}

/// Files grouped by folder; the change set is already sorted that way.
fn list_rows(view: &ChangeReview) -> Vec<ListRow> {
    let mut rows = Vec::with_capacity(view.files.len() + 8);
    let mut folder = None;
    for (index, entry) in view.files.iter().enumerate() {
        let current = entry.folder();
        if !current.is_empty() && folder != Some(current) {
            rows.push(ListRow::Folder(index));
        }
        folder = Some(current);
        rows.push(ListRow::File(index, usize::from(!current.is_empty())));
    }
    rows
}

/// Returns true when a row or seen box was clicked.
fn file_list(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &mut ChangeReview,
    seen: &mut SeenMarks,
) -> bool {
    let rows = list_rows(view);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(4.0, 4.0)))
            .layout(Layout::top_down(Align::Min))
            .id_salt("change-review-list"),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("change-review-files")
        .auto_shrink([false, false])
        .animated(false);
    if view.follow_list
        && let Some(row) = rows
            .iter()
            .position(|row| matches!(row, ListRow::File(index, _) if *index == view.current))
    {
        let top = row as f32 * LIST_ROW_HEIGHT;
        if top < view.list_offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if top + LIST_ROW_HEIGHT > view.list_offset + view.list_height {
            scroll = scroll.vertical_scroll_offset(top + LIST_ROW_HEIGHT - view.list_height);
        }
    }
    view.follow_list = false;
    let mut jump = None;
    let mut toggle = None;
    let output = scroll.show_rows(ui, LIST_ROW_HEIGHT, rows.len(), |ui, range| {
        for row in &rows[range] {
            match *row {
                ListRow::Folder(index) => folder_row(ui, view.files[index].folder()),
                ListRow::File(index, depth) => {
                    let (clicked, toggled) = file_row(
                        ui,
                        &view.files[index],
                        depth,
                        index == view.current,
                        view.is_seen(seen, index),
                    );
                    if clicked {
                        jump = Some(index);
                    }
                    if toggled {
                        toggle = Some(index);
                    }
                }
            }
        }
    });
    view.list_offset = output.state.offset.y;
    view.list_height = output.inner_rect.height();
    if let Some(index) = toggle {
        let value = !view.is_seen(seen, index);
        view.set_seen(seen, index, value);
    } else if let Some(index) = jump {
        view.jump = Some(index);
        view.current = index;
    }
    toggle.is_some() || jump.is_some()
}

fn folder_row(ui: &mut egui::Ui, folder: &str) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), LIST_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let galley = design::truncated_galley(
        ui,
        &format!("{folder}/"),
        FontId::proportional(11.0),
        MUTED,
        rect.width() - 12.0,
    );
    let truncated = galley.elided;
    ui.painter().galley(
        egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y * 0.5),
        galley,
        MUTED,
    );
    if truncated {
        response.on_hover_details(folder);
    }
}

/// Returns whether the row was clicked and whether its seen box was toggled.
fn file_row(
    ui: &mut egui::Ui,
    entry: &ReviewEntry,
    depth: usize,
    current: bool,
    seen: bool,
) -> (bool, bool) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), LIST_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let id = ui.id().with(("change-review-file", &entry.path));
    let row = ui.interact(rect, id, egui::Sense::click());
    row.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            current,
            &entry.path,
        )
    });
    let x = rect.left() + 6.0 + depth as f32 * 12.0;
    let check_rect =
        egui::Rect::from_center_size(egui::pos2(x + 6.0, rect.center().y), Vec2::splat(12.0));
    let check = ui.interact(
        check_rect.expand(4.0),
        id.with("seen"),
        egui::Sense::click(),
    );
    check.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            seen,
            format!("Seen {}", entry.path),
        )
    });
    if ui.is_rect_visible(rect) {
        if current || row.hovered() || check.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, if current { HOVER } else { SURFACE });
        }
        if design::keyboard_focus(&row) || design::keyboard_focus(&check) {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                4.0,
                Stroke::new(1.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        paint_seen_box(ui, check_rect, seen, check.hovered());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let (mark, color, _) = git_panel::change_marker(entry.status, false);
        painter.text(
            egui::pos2(x + 23.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            mark,
            FontId::monospace(10.0),
            color,
        );
        let stats_width =
            paint_list_stats(ui, &painter, rect.right() - 6.0, rect.center().y, entry);
        let color = if seen && !current {
            MUTED
        } else if current || row.hovered() {
            TEXT
        } else {
            TEXT.lerp_to_gamma(MUTED, 0.35)
        };
        let left = x + 33.0;
        let galley = design::truncated_galley(
            ui,
            entry.name(),
            FontId::proportional(12.0),
            color,
            (rect.right() - 12.0 - stats_width - left).max(1.0),
        );
        painter.galley(
            egui::pos2(left, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
    }
    let toggled = check.clicked();
    let clicked = row.clicked();
    row.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_details(match &entry.original_path {
            Some(original) => format!("{}\nRenamed from {original}", entry.path),
            None => entry.path.clone(),
        });
    check
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if seen {
            "Mark as not seen"
        } else {
            "Mark as seen"
        });
    (clicked, toggled)
}

fn paint_seen_box(ui: &egui::Ui, rect: egui::Rect, seen: bool, hovered: bool) {
    let painter = ui.painter();
    if seen {
        painter.rect_filled(rect, 3.0, BORDER_STRONG);
        painter.add(egui::Shape::line(
            vec![
                rect.left_center() + Vec2::new(2.5, 0.0),
                rect.center_bottom() + Vec2::new(-1.0, -3.0),
                rect.right_top() + Vec2::new(-2.5, 3.0),
            ],
            Stroke::new(1.5, TEXT),
        ));
    } else {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, if hovered { MUTED } else { BORDER_STRONG }),
            egui::StrokeKind::Inside,
        );
    }
}

/// Right-aligned compact line counts; returns their width.
fn paint_list_stats(
    ui: &egui::Ui,
    painter: &egui::Painter,
    right: f32,
    center: f32,
    entry: &ReviewEntry,
) -> f32 {
    let font = FontId::monospace(10.0);
    let parts: Vec<(String, Color32)> = match entry.stats {
        Some(stats) if stats.binary => vec![("binary".into(), MUTED)],
        Some(stats) => vec![
            (
                format!("+{}", git_panel::compact_number(stats.added)),
                GREEN,
            ),
            (
                format!("−{}", git_panel::compact_number(stats.removed)),
                RED,
            ),
        ],
        None => return 0.0,
    };
    let mut x = right;
    for (text, color) in parts.into_iter().rev() {
        let galley = design::truncated_galley(ui, &text, font.clone(), color, 60.0);
        x -= galley.size().x;
        painter.galley(egui::pos2(x, center - galley.size().y * 0.5), galley, color);
        x -= 5.0;
    }
    right - x - 5.0
}

/// Take this frame's review keys. Letters normally arrive as a key press and
/// its text; the text decides, and the key alone counts only without text.
fn take_review_keys(ctx: &egui::Context, swap_jk: bool) -> Vec<ReviewKey> {
    let (down, up) = if swap_jk { ('k', 'j') } else { ('j', 'k') };
    let character = |ch: char| match ch {
        ch if ch == down => Some(ReviewKey::Lines(1.0)),
        ch if ch == up => Some(ReviewKey::Lines(-1.0)),
        'n' | ']' => Some(ReviewKey::NextFile),
        'p' | '[' => Some(ReviewKey::PreviousFile),
        ' ' => Some(ReviewKey::SeenNext),
        'o' => Some(ReviewKey::Open),
        'c' => Some(ReviewKey::Comment),
        _ => None,
    };
    let mut keys = Vec::new();
    ctx.input_mut(|input| {
        let text: String = input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        input.events.retain(|event| match event {
            egui::Event::Text(value) => {
                if !value.is_empty() && value.chars().all(|ch| character(ch).is_some()) {
                    keys.extend(value.chars().filter_map(character));
                    false
                } else {
                    true
                }
            }
            egui::Event::Key {
                key: key @ (Key::ArrowDown | Key::ArrowUp),
                pressed: true,
                modifiers,
                ..
            } if *modifiers == egui::Modifiers::SHIFT => {
                keys.push(ReviewKey::Extend(if *key == Key::ArrowDown {
                    1
                } else {
                    -1
                }));
                false
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if modifiers.is_none() => {
                let letter = match key {
                    Key::J => Some('j'),
                    Key::K => Some('k'),
                    Key::N => Some('n'),
                    Key::P => Some('p'),
                    Key::O => Some('o'),
                    Key::C => Some('c'),
                    Key::Space => Some(' '),
                    Key::OpenBracket => Some('['),
                    Key::CloseBracket => Some(']'),
                    _ => None,
                };
                let command = match key {
                    Key::ArrowDown => Some(ReviewKey::Lines(1.0)),
                    Key::ArrowUp => Some(ReviewKey::Lines(-1.0)),
                    Key::PageDown => Some(ReviewKey::Pages(1.0)),
                    Key::PageUp => Some(ReviewKey::Pages(-1.0)),
                    Key::Home => Some(ReviewKey::Top),
                    Key::End => Some(ReviewKey::Bottom),
                    Key::Enter => Some(ReviewKey::Open),
                    Key::Escape => Some(ReviewKey::Close),
                    _ => match letter {
                        Some(ch) if text.contains(ch) => return false,
                        Some(ch) => character(ch),
                        None => None,
                    },
                };
                match command {
                    Some(command) => {
                        keys.push(command);
                        false
                    }
                    None => true,
                }
            }
            _ => true,
        });
    });
    keys
}

/// The first new-side line below the sticky header of `file`.
fn line_at(view: &ChangeReview, file: usize, offset: f32) -> Option<usize> {
    let entry = view.files.get(file)?;
    let EntryPatch::Parsed(patch) = &entry.patch else {
        return None;
    };
    if patch.rows() == 0 {
        return None;
    }
    let local = (offset - view.tops.get(file).copied().unwrap_or(0.0)).max(0.0);
    patch.new_line_at(entry.wrapped.visible(local..local + 1.0).start)
}

fn diff_body(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &mut ChangeReview,
    seen: &mut SeenMarks,
    body_id: egui::Id,
    swap_jk: bool,
    actions: &mut Vec<ReviewAction>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min))
            .id_salt("change-review-diff"),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 0.0;
    let content_width = (rect.width() - ui.spacing().scroll.allocated_width())
        .floor()
        .max(1.0);
    let digits = view
        .files
        .iter()
        .filter_map(|entry| match &entry.patch {
            EntryPatch::Parsed(patch) => Some(patch.number_digits()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .max(MIN_NUMBER_DIGITS);
    let metrics = RowMetrics::new(ui, digits, content_width);
    let mut tops = Vec::with_capacity(view.files.len());
    let mut bottom = 0.0;
    for entry in &mut view.files {
        if let EntryPatch::Parsed(patch) = &entry.patch {
            entry.wrapped.update(ui, patch, &metrics);
        }
        tops.push(bottom);
        bottom += FILE_HEADER_HEIGHT + entry.body_height() + FILE_GAP;
    }
    view.tops = tops;
    let viewport_height = rect.height().max(1.0);
    // The last file's header can always scroll to the top.
    let last_top = view.tops.last().copied().unwrap_or(0.0);
    let content_height = (bottom - FILE_GAP).max(last_top + viewport_height);
    let maximum = (content_height - viewport_height).max(0.0);

    let body = ui.interact(rect, body_id, egui::Sense::click());
    body.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Change review diff")
    });
    // The list is for pointing; the keys keep working on the diff. Requested
    // after the body's interaction, so the click elsewhere does not undo it.
    if body.clicked() || body.secondary_clicked() || std::mem::take(&mut view.list_clicked) {
        body.request_focus();
    }
    feedback::validate_selection(view);
    // A click chooses a diff row, Shift+click extends the choice, and a
    // right-click outside it chooses the row under the pointer.
    let pointer = (body.clicked() || body.secondary_clicked())
        .then(|| body.interact_pointer_pos())
        .flatten()
        .map(|position| feedback::RowClick {
            position,
            extend: body.clicked() && ui.input(|input| input.modifiers.shift),
            secondary: body.secondary_clicked(),
        });
    if view.focus_pending {
        let focused = ui.ctx().memory(|memory| memory.focused());
        if focused.is_none_or(|id| id == body_id) {
            body.request_focus();
        }
        // A closing dialog can still hold the frame's focus; ask again then.
        view.focus_pending = focused.is_none();
    }
    let focused = ui.ctx().memory(|memory| memory.has_focus(body_id));
    if focused {
        ui.ctx().memory_mut(|memory| {
            memory.set_focus_lock_filter(
                body_id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                    tab: false,
                },
            )
        });
    }

    let salt = "change-review-diff-scroll";
    // ScrollArea builds its ID from the salt wrapped in IdSalt.
    let scroll_id = ui.make_persistent_id(egui::IdSalt::new(salt));
    let mut offset = view.scroll;
    if let Some(wheel) =
        render::instant_scroll_offset_axes(ui, scroll_id, egui::Vec2b::new(false, true))
    {
        offset = wheel.y;
    }
    if !view.loading
        && let Some((path, within)) = view.anchor.take()
        && let Some(index) = view.files.iter().position(|entry| entry.path == path)
    {
        let height = FILE_HEADER_HEIGHT + view.files[index].body_height();
        offset = view.tops[index] + within.min(height);
    }
    let top_layer = ui
        .ctx()
        .memory(|memory| memory.areas().top_layer_id(ui.layer_id().order));
    let keys = if focused
        && ui.is_enabled()
        && !egui::Popup::is_any_open(ui.ctx())
        && top_layer.is_none_or(|layer| layer == ui.layer_id())
    {
        take_review_keys(ui.ctx(), swap_jk)
    } else {
        Vec::new()
    };
    for key in keys {
        let current = view.file_at(offset);
        match key {
            ReviewKey::Lines(lines) => offset += lines * metrics.row_height,
            ReviewKey::Pages(pages) => offset += pages * viewport_height * 0.9,
            ReviewKey::Top => offset = 0.0,
            ReviewKey::Bottom => offset = maximum,
            ReviewKey::NextFile => {
                if let Some(top) = view.tops.get(current + 1) {
                    offset = *top;
                }
            }
            ReviewKey::PreviousFile => {
                offset = if offset > view.tops[current] + 0.5 {
                    view.tops[current]
                } else {
                    view.tops[current.saturating_sub(1)]
                };
            }
            ReviewKey::SeenNext => {
                view.set_seen(seen, current, true);
                let count = view.files.len();
                match (1..count)
                    .map(|step| (current + step) % count)
                    .find(|index| !view.is_seen(seen, *index))
                {
                    Some(next) => offset = view.tops[next],
                    None => actions.push(ReviewAction::Status("Every file is marked seen".into())),
                }
            }
            ReviewKey::Open => actions.push(match &view.selection {
                Some(selection) => ReviewAction::Open {
                    file: selection.file,
                    line: feedback::selection_line(view, selection),
                },
                None => ReviewAction::Open {
                    file: current,
                    line: line_at(view, current, offset),
                },
            }),
            ReviewKey::Close if view.selection.is_some() => view.selection = None,
            ReviewKey::Close => actions.push(ReviewAction::Close),
            ReviewKey::Extend(step) => {
                if let Some(target) = feedback::extend_selection(view, current, offset, step) {
                    // Keep the moving end below the sticky header and on screen.
                    let top = target - FILE_HEADER_HEIGHT;
                    let bottom = target + metrics.row_height - viewport_height;
                    offset = offset.clamp(bottom, top.max(bottom));
                }
            }
            ReviewKey::Comment => {
                if view.selection.is_none() {
                    view.selection = feedback::row_on_screen(view, current, offset);
                }
                match feedback::CommentDraft::new(view) {
                    Some(draft) => view.comment = Some(draft),
                    None => actions.push(ReviewAction::Status(
                        "Select changed or unchanged diff lines to comment on".into(),
                    )),
                }
            }
        }
        offset = offset.clamp(0.0, maximum);
    }
    if let Some(index) = view.jump.take()
        && let Some(top) = view.tops.get(index)
    {
        offset = *top;
    }
    offset = offset.clamp(0.0, maximum);

    let scroll = egui::ScrollArea::vertical()
        .id_salt(salt)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .content_margin(egui::Margin::ZERO)
        .auto_shrink([false, false])
        .animated(false)
        .scroll_offset(Vec2::new(0.0, offset))
        .scroll_source(egui::scroll_area::ScrollSource {
            mouse_wheel: false,
            ..Default::default()
        });
    let keyboard = design::keyboard_focus(&body);
    let palette = diff_view::Palette::new(ui);
    let output = scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(Vec2::new(content_width, content_height));
        let origin = ui.max_rect().min;
        let first = view
            .tops
            .partition_point(|top| *top <= viewport.top())
            .saturating_sub(1);
        let mut visible = Vec::new();
        let mut deferred = Vec::new();
        let mut hit = None;
        for index in first..view.files.len() {
            let top = view.tops[index];
            // Request deferred patches a screen ahead of the reader.
            if top >= viewport.bottom() + viewport.height() {
                break;
            }
            let entry = &view.files[index];
            if matches!(entry.patch, EntryPatch::Deferred) {
                deferred.push(index);
            }
            if top >= viewport.bottom() {
                continue;
            }
            let body_top = top + FILE_HEADER_HEIGHT;
            let height = entry.body_height();
            if body_top + height < viewport.top() {
                continue;
            }
            visible.push(index);
            let body_rect = egui::Rect::from_min_size(
                origin + Vec2::new(0.0, body_top),
                Vec2::new(content_width, height),
            );
            match &entry.patch {
                EntryPatch::Parsed(patch) if patch.rows() > 0 => {
                    if let Some(click) = &pointer
                        && body_rect.contains(click.position)
                    {
                        let local = click.position.y - body_rect.top();
                        hit = Some((index, entry.wrapped.visible(local..local + 0.01).start));
                    }
                    for row in entry
                        .wrapped
                        .visible(viewport.top() - body_top..viewport.bottom() - body_top)
                    {
                        let row_rect = egui::Rect::from_min_max(
                            origin + Vec2::new(0.0, body_top + entry.wrapped.offset(row)),
                            origin
                                + Vec2::new(
                                    content_width,
                                    body_top + entry.wrapped.offset(row + 1),
                                ),
                        );
                        diff_view::paint_row(
                            ui,
                            &palette,
                            patch,
                            row,
                            row_rect,
                            &metrics,
                            body_id.with((index, row)),
                        );
                        if view
                            .selection
                            .as_ref()
                            .is_some_and(|selection| selection.contains(index, row))
                        {
                            feedback::paint_selected_row(ui, row_rect);
                        }
                    }
                }
                patch => {
                    if file_note(ui, body_rect, index, entry, patch) {
                        actions.push(ReviewAction::Load {
                            files: vec![index],
                            automatic: false,
                        });
                    }
                }
            }
        }
        // Headers paint above the rows. The current one sticks to the top
        // until the end of its file pushes it away.
        let current = view.file_at(viewport.top());
        let mut toggles = Vec::new();
        for index in visible {
            let top = view.tops[index];
            let y = if index == current {
                let end = top + FILE_HEADER_HEIGHT + view.files[index].body_height();
                viewport
                    .top()
                    .clamp(top, (end - FILE_HEADER_HEIGHT).max(top))
            } else {
                top
            };
            let rect = egui::Rect::from_min_size(
                origin + Vec2::new(0.0, y),
                Vec2::new(content_width, FILE_HEADER_HEIGHT),
            );
            if pointer
                .as_ref()
                .is_some_and(|click| rect.contains(click.position))
            {
                // The header covers rows scrolled beneath it.
                hit = None;
            }
            let response = file_header(
                ui,
                rect,
                &view.files[index],
                view.is_seen(seen, index),
                index == current && keyboard,
            );
            if response.open {
                let line = match &view.files[index].patch {
                    EntryPatch::Parsed(patch) if patch.rows() > 0 => patch.new_line_at(0),
                    _ => None,
                };
                actions.push(ReviewAction::Open { file: index, line });
            }
            if let Some(value) = response.seen {
                toggles.push((index, value));
            }
        }
        (deferred, toggles, hit)
    });
    render::remember_scroll_limits(ui.ctx(), &output);
    let (deferred, toggles, hit) = output.inner;
    if let (Some(click), Some((file, row))) = (pointer, hit) {
        feedback::click_row(view, file, row, click);
    }
    if let Some(comment) = feedback::diff_menu(&body, view) {
        match comment {
            Some(draft) => view.comment = Some(draft),
            None => actions.push(ReviewAction::Status(
                "Select changed or unchanged diff lines to comment on".into(),
            )),
        }
    }
    if !toggles.is_empty() {
        ui.ctx().memory_mut(|memory| memory.request_focus(body_id));
    }
    for (index, value) in toggles {
        view.set_seen(seen, index, value);
    }
    if !deferred.is_empty() && view.lazy_request.is_none() {
        actions.push(ReviewAction::Load {
            files: deferred.into_iter().take(LAZY_FILES).collect(),
            automatic: true,
        });
    }
    view.scroll = output.state.offset.y;
    let current = view.file_at(view.scroll);
    if current != view.current {
        view.current = current;
        view.follow_list = true;
    }
}

/// A placeholder in place of a patch. Returns true when "Load" was clicked.
fn file_note(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    index: usize,
    entry: &ReviewEntry,
    patch: &EntryPatch,
) -> bool {
    let changed = entry
        .stats
        .map(|stats| stats.added + stats.removed)
        .unwrap_or(0);
    let text = match patch {
        EntryPatch::Large => format!("Large diff · {changed} changed lines"),
        EntryPatch::Deferred | EntryPatch::Loading => "Loading diff…".into(),
        EntryPatch::Binary => "Binary file · no text diff".into(),
        EntryPatch::Unavailable(message) => message.clone(),
        EntryPatch::Parsed(_) => "No text changes in this comparison".into(),
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(14.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center))
            .id_salt(("change-review-note", index)),
    );
    child.add(egui::Label::new(RichText::new(text).size(12.0).color(MUTED)).truncate());
    matches!(patch, EntryPatch::Large)
        && design::ghost_button(&mut child, "Load", "Load this file's complete diff").clicked()
}

struct HeaderResponse {
    open: bool,
    seen: Option<bool>,
}

fn file_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    entry: &ReviewEntry,
    seen: bool,
    keyboard_current: bool,
) -> HeaderResponse {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, PANEL);
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, BORDER),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BORDER),
    );
    if keyboard_current {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            0.0,
            ACCENT,
        );
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center))
            .id_salt(("change-review-file-header", &entry.path)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 10.0;
    let mut checked = seen;
    let toggle = ui
        .add(egui::Checkbox::new(
            &mut checked,
            RichText::new("Seen").size(12.0).color(MUTED),
        ))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Space marks the current file seen and moves to the next unseen file");
    toggle.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            checked,
            format!("Seen {}", entry.path),
        )
    });
    let deleted = entry.status == FileChange::Deleted;
    let open = ui
        .add_enabled_ui(!deleted, |ui| {
            design::ghost_button(
                ui,
                "Open",
                "Open the current file in the editor · Enter or o opens it at the line on screen",
            )
        })
        .inner;
    open.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            !deleted,
            format!("Open {}", entry.path),
        )
    });
    if let Some(stats) = entry.stats.filter(|stats| !stats.binary) {
        ui.label(
            RichText::new(format!("−{}", stats.removed))
                .monospace()
                .size(11.0)
                .color(RED),
        );
        ui.label(
            RichText::new(format!("+{}", stats.added))
                .monospace()
                .size(11.0)
                .color(GREEN),
        );
    }
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (mark, color, status) = git_panel::change_marker(entry.status, false);
        ui.label(RichText::new(mark).monospace().size(11.0).color(color))
            .on_hover_text(status);
        let folder = entry.folder();
        let mut job = egui::text::LayoutJob::default();
        if !folder.is_empty() {
            job.append(
                &format!("{folder}/"),
                0.0,
                egui::TextFormat::simple(FontId::proportional(12.5), MUTED),
            );
        }
        job.append(
            entry.name(),
            0.0,
            egui::TextFormat::simple(FontId::proportional(12.5), TEXT),
        );
        if let Some(original) = &entry.original_path {
            job.append(
                &format!("  ← {original}"),
                0.0,
                egui::TextFormat::simple(FontId::proportional(11.5), MUTED),
            );
        }
        ui.add(egui::Label::new(job).truncate())
            .on_hover_details(match &entry.original_path {
                Some(original) => format!("{}\nRenamed from {original}", entry.path),
                None => entry.path.clone(),
            });
    });
    HeaderResponse {
        open: open.clicked(),
        seen: toggle.changed().then_some(checked),
    }
}

#[path = "change_review_feedback.rs"]
mod feedback;

#[cfg(test)]
#[path = "change_review_tests.rs"]
mod tests;
