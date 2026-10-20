//! Git change markers beside the line numbers: lines added, modified or
//! removed since HEAD, including unsaved edits.
//!
//! The committed version of a file is read in a worker and cached by
//! repository, HEAD and path. Markers belong to one document revision; small
//! documents are compared on the UI thread, larger ones in a worker. They are
//! kept per document and painted in every pane that shows it. Reverting a
//! change edits only the buffer, never the disk or the Git index.

use super::*;
use crate::line_diff::{self, Hunk, HunkKind};

/// Larger buffers get no markers, like committed versions above
/// [`git::BASE_VERSION_LIMIT`].
const MAX_BUFFER_BYTES: usize = git::BASE_VERSION_LIMIT as usize;
/// Committed version plus buffer up to this size are compared inline. In a
/// release build (`inline_comparison_budget`), 32 KiB took 0.08 ms with
/// scattered edits and 0.9 ms for a complete rewrite; 64 KiB already took
/// 3.5 ms and 256 KiB 59 ms when rewritten. Larger documents compare in a
/// worker.
const INLINE_DIFF_BYTES: usize = 32 * 1024;
const MAX_CACHED_VERSIONS: usize = 256;
const TOOLTIP_LINES: usize = 12;
const PEEK_LINES: usize = 40;
const LINE_CHARS: usize = 160;
/// Bar geometry, left of the gutter divider at `galley_pos.x - 8`.
const BAR_RIGHT: f32 = 10.0;
const BAR_WIDTH: f32 = 3.0;

/// The committed version a document is compared with.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct BaseKey {
    repository: PathBuf,
    /// `None` while the branch has no commit.
    head: Option<String>,
    /// Repository-relative path in HEAD; the previous path of a staged rename.
    path: String,
}

enum BaseEntry {
    Pending(u64),
    /// `None`: no markers, e.g. untracked, binary or too large. `committed`
    /// entries depend only on HEAD; the others also on the index.
    Ready {
        text: Option<Arc<str>>,
        committed: bool,
    },
    Failed,
}

struct DocumentMarkers {
    key: BaseKey,
    base: Arc<str>,
    revision: u64,
    hunks: Arc<[Hunk]>,
}

/// Markers to paint for one document. `current` is false while a newer
/// revision is being compared; such markers are only shown, never reverted.
#[derive(Clone)]
pub(super) struct GutterMarkers {
    pub(super) base: Arc<str>,
    pub(super) hunks: Arc<[Hunk]>,
    pub(super) revision: u64,
    pub(super) current: bool,
}

/// The open inline peek of one change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GutterPeek {
    pub(super) pane: u64,
    pub(super) document: u64,
    pub(super) revision: u64,
    pub(super) hunk: Hunk,
}

#[derive(Default)]
pub(super) struct GitGutterState {
    bases: HashMap<BaseKey, BaseEntry>,
    markers: HashMap<u64, DocumentMarkers>,
    /// Comparison in flight per document: request and revision.
    diffs: HashMap<u64, (u64, u64)>,
    /// The status fingerprint the cached index-dependent entries belong to.
    fingerprints: HashMap<PathBuf, Option<u64>>,
    next_request: u64,
    pub(super) peek: Option<GutterPeek>,
}

impl GitGutterState {
    fn request(&mut self) -> u64 {
        self.next_request = self.next_request.wrapping_add(1);
        self.next_request
    }

    /// A closed document's markers and pending work are forgotten.
    pub(super) fn forget(&mut self, document: u64) {
        self.markers.remove(&document);
        self.diffs.remove(&document);
        if self
            .peek
            .as_ref()
            .is_some_and(|peek| peek.document == document)
        {
            self.peek = None;
        }
    }
}

/// Changed regions of `text` compared with the committed `base`.
pub(super) fn changes(base: &str, text: &str) -> Vec<Hunk> {
    line_diff::hunks(base, text)
}

/// `text` with `hunk`'s lines replaced by their committed version, and the
/// character offset where the restored lines begin. Inserted lines use the
/// line ending of the surrounding buffer, so a CRLF blob restores LF lines
/// in a normalized buffer and raw CRLF lines next to raw CRLF lines.
pub(super) fn revert_hunk(base: &str, text: &str, hunk: &Hunk) -> (String, usize) {
    let pieces: Vec<&str> = text.split_inclusive('\n').collect();
    let offset = |line: usize| -> usize {
        pieces[..line.min(pieces.len())]
            .iter()
            .map(|piece| piece.len())
            .sum()
    };
    let start = offset(hunk.new.start);
    let end = offset(hunk.new.end);
    let terminator = [hunk.new.start, hunk.new.start.wrapping_sub(1)]
        .iter()
        .filter_map(|&line| pieces.get(line))
        .chain(pieces.iter())
        .find(|piece| piece.ends_with('\n'))
        .map_or("\n", |piece| {
            if piece.ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            }
        });
    let committed = line_diff::lines(base);
    let restored = committed.get(hunk.old.clone()).unwrap_or_default();
    let open_end = !text.is_empty() && !text.ends_with('\n') && end == text.len();
    let mut replacement = String::new();
    if open_end && start == end {
        // Restoring lines after a last line without a line break.
        replacement.push_str(terminator);
        replacement.push_str(&restored.join(terminator));
    } else {
        for line in restored {
            replacement.push_str(line);
            replacement.push_str(terminator);
        }
        if open_end && !restored.is_empty() {
            replacement.truncate(replacement.len() - terminator.len());
        }
    }
    let mut result = String::with_capacity(text.len() + replacement.len());
    result.push_str(&text[..start]);
    result.push_str(&replacement);
    result.push_str(&text[end..]);
    let mut cursor = text[..start].chars().count();
    if open_end && start == end {
        cursor += terminator.chars().count();
    }
    (result, cursor)
}

fn kind_color(kind: HunkKind) -> Color32 {
    match kind {
        HunkKind::Added => GREEN,
        HunkKind::Modified => ORANGE,
        HunkKind::Removed => RED,
    }
}

fn count(value: usize, noun: &str) -> String {
    format!("{value} {noun}{}", if value == 1 { "" } else { "s" })
}

/// One-line summary used by tooltips, the peek and its accessibility label.
fn summary(hunk: &Hunk) -> String {
    match hunk.kind() {
        HunkKind::Added => format!("{} added", count(hunk.new.len(), "line")),
        HunkKind::Removed => format!("{} removed", count(hunk.old.len(), "line")),
        HunkKind::Modified => format!(
            "{} changed · {} in HEAD",
            count(hunk.new.len(), "line"),
            hunk.old.len()
        ),
    }
}

fn shortened(line: &str) -> String {
    let mut chars = line.chars();
    let mut text: String = chars.by_ref().take(LINE_CHARS).collect();
    if chars.next().is_some() {
        text.push('…');
    }
    text.replace('\t', "    ")
}

/// The committed lines of `hunk`, at most `limit`, and how many are left out.
fn committed_lines(base: &str, hunk: &Hunk, limit: usize) -> (Vec<String>, usize) {
    let lines = line_diff::lines(base);
    let old = lines.get(hunk.old.clone()).unwrap_or_default();
    (
        old.iter().take(limit).map(|line| shortened(line)).collect(),
        old.len().saturating_sub(limit),
    )
}

/// Where the markers of one hunk are on screen in this frame.
pub(super) struct HunkPlacement {
    pub(super) index: usize,
    pub(super) rect: egui::Rect,
}

/// Paint the markers of the visible rows and return their hit areas.
/// `rows` are the visible (row index, visual row) pairs of the galley.
pub(super) fn paint_markers(
    ui: &egui::Ui,
    markers: &GutterMarkers,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    rows: &[(usize, crate::render::VisualRow)],
    line_count: usize,
) -> Vec<HunkPlacement> {
    let hunks = &markers.hunks;
    if hunks.is_empty() || rows.is_empty() {
        return Vec::new();
    }
    let right = galley_pos.x - BAR_RIGHT;
    let mut placements: Vec<HunkPlacement> = Vec::new();
    let mut place = |index: usize, rect: egui::Rect| match placements.last_mut() {
        Some(last) if last.index == index => last.rect = last.rect.union(rect),
        _ => placements.push(HunkPlacement { index, rect }),
    };
    for &(row_index, mapped) in rows {
        let row = &galley.rows[row_index];
        let top = galley_pos.y + row.pos.y;
        let bottom = top + row.size.y;
        // Zero-based source line of this visual row.
        let line = mapped.line.saturating_sub(1);
        let found = hunks.partition_point(|hunk| hunk.new.end <= line);
        if let Some(hunk) = hunks.get(found)
            && hunk.new.contains(&line)
        {
            place(
                found,
                egui::Rect::from_min_max(
                    egui::pos2(right - BAR_WIDTH, top),
                    egui::pos2(right, bottom),
                ),
            );
        }
        if !mapped.first_in_line {
            continue;
        }
        // Removed lines sit between this line and the previous one; after the
        // last line, below it.
        let wedge = |y: f32| {
            egui::Rect::from_center_size(
                egui::pos2(right - BAR_WIDTH * 0.5, y),
                Vec2::new(8.0, 8.0),
            )
        };
        for (index, hunk) in hunks.iter().enumerate().skip(found.saturating_sub(1)) {
            if hunk.new.start > line + 1 {
                break;
            }
            if hunk.kind() != HunkKind::Removed {
                continue;
            }
            if hunk.new.start == line {
                place(index, wedge(top));
            } else if hunk.new.start == line + 1 && line + 1 >= line_count {
                let last = rows
                    .iter()
                    .filter(|(_, mapped)| mapped.line == line + 1)
                    .map(|(row_index, _)| galley_pos.y + galley.rows[*row_index].rect().bottom())
                    .fold(bottom, f32::max);
                place(index, wedge(last));
            }
        }
    }
    let painter = ui.painter();
    for placement in &placements {
        let hunk = &hunks[placement.index];
        let color = kind_color(hunk.kind());
        if hunk.kind() == HunkKind::Removed {
            let center = placement.rect.center();
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(-2.5, -4.0),
                    center + Vec2::new(2.5, 0.0),
                    center + Vec2::new(-2.5, 4.0),
                ],
                color,
                Stroke::NONE,
            ));
        } else {
            painter.rect_filled(placement.rect.shrink2(Vec2::new(0.0, 0.5)), 1.0, color);
        }
    }
    placements
}

/// The hover text of one marker: what changed and the committed lines.
pub(super) fn tooltip_ui(ui: &mut egui::Ui, markers: &GutterMarkers, hunk: &Hunk) {
    ui.label(RichText::new(summary(hunk)).size(12.0).color(TEXT));
    let (lines, more) = committed_lines(&markers.base, hunk, TOOLTIP_LINES);
    if !lines.is_empty() {
        ui.add_space(3.0);
        for line in lines {
            ui.label(
                RichText::new(format!("− {line}"))
                    .monospace()
                    .size(11.5)
                    .color(RED),
            );
        }
        if more > 0 {
            ui.label(
                RichText::new(format!("… {}", count(more, "more line")))
                    .size(11.0)
                    .color(MUTED),
            );
        }
    }
    ui.add_space(3.0);
    ui.label(
        RichText::new("Click to review or revert this change")
            .size(11.0)
            .color(MUTED),
    );
}

pub(super) enum PeekAction {
    Revert,
    Close,
}

/// The inline peek below a change: its committed lines and Revert change.
pub(super) fn peek_ui(
    ctx: &egui::Context,
    id: egui::Id,
    anchor: egui::Pos2,
    max_width: f32,
    markers: &GutterMarkers,
    hunk: &Hunk,
) -> (Option<PeekAction>, egui::Rect) {
    let mut action = None;
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, crate::palette::BORDER_STRONG))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_max_width(max_width.max(260.0));
                    ui.horizontal(|ui| {
                        let color = kind_color(hunk.kind());
                        ui.label(RichText::new("●").size(9.0).color(color));
                        ui.label(RichText::new(summary(hunk)).size(12.0).color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let close = ui
                                .add(
                                    egui::Button::new(RichText::new("×").size(14.0))
                                        .frame_when_inactive(false)
                                        .min_size(Vec2::new(22.0, 22.0)),
                                )
                                .on_hover_text("Close · Esc");
                            close.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    true,
                                    "Close change peek",
                                )
                            });
                            if close.clicked() {
                                action = Some(PeekAction::Close);
                            }
                            let revert = ui
                                .add_enabled_ui(markers.current, |ui| {
                                    design::ghost_button(
                                        ui,
                                        "Revert change",
                                        "Restore these lines from HEAD in the buffer; undo brings the change back. Disk and the Git index stay unchanged.",
                                    )
                                })
                                .inner;
                            if revert.clicked() {
                                action = Some(PeekAction::Revert);
                            }
                        });
                    });
                    let (lines, more) = committed_lines(&markers.base, hunk, PEEK_LINES);
                    ui.add_space(4.0);
                    if lines.is_empty() {
                        ui.label(
                            RichText::new(
                                "HEAD has no lines here. Reverting removes the added lines.",
                            )
                            .size(11.5)
                            .color(MUTED),
                        );
                    } else {
                        let tint = RED.linear_multiply(0.12);
                        egui::Frame::new()
                            .fill(tint)
                            .corner_radius(3.0)
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                for line in lines {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("− {line}"))
                                                .monospace()
                                                .size(12.0)
                                                .color(TEXT),
                                        )
                                        .truncate(),
                                    );
                                }
                            });
                        if more > 0 {
                            ui.label(
                                RichText::new(format!("… {}", count(more, "more line")))
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        }
                    }
                });
        });
    (action, area.response.rect)
}

impl VEditApp {
    /// The committed version `document` is compared with, when it has one.
    fn git_gutter_key(&self, document: u64) -> Option<BaseKey> {
        let (project, index) = self.tab_location(document)?;
        let project = &self.projects[project];
        let tab = &project.tabs[index];
        if tab.loading || tab.error.is_some() || tab.text.len() > MAX_BUFFER_BYTES {
            return None;
        }
        let repository = project.git_root.as_ref()?;
        let head = match project.git_head.as_ref()? {
            git::StatusHead::Commit(hash) => Some(hash.clone()),
            git::StatusHead::Unborn => None,
        };
        let relative = tab.path.as_ref()?.strip_prefix(repository).ok()?;
        let parts: Option<Vec<&str>> = relative
            .components()
            .map(|part| match part {
                std::path::Component::Normal(part) => part.to_str(),
                _ => None,
            })
            .collect();
        let path = parts?.join("/");
        if path.is_empty() {
            return None;
        }
        // A staged rename compares with the committed previous path.
        let path = project
            .git_changes
            .iter()
            .find(|file| file.path == path && file.index == git::FileChange::Renamed)
            .and_then(|file| file.original_path.clone())
            .unwrap_or(path);
        Some(BaseKey {
            repository: repository.clone(),
            head,
            path,
        })
    }

    /// Drop cached versions that a changed HEAD or index made obsolete.
    fn invalidate_git_gutter(&mut self, key: &BaseKey, fingerprint: Option<u64>) {
        let state = &mut self.git_gutter;
        let previous = state
            .fingerprints
            .insert(key.repository.clone(), fingerprint);
        if previous.is_none_or(|previous| previous == fingerprint) {
            return;
        }
        state.bases.retain(|cached, entry| {
            cached.repository != key.repository
                || (cached.head == key.head
                    && matches!(
                        entry,
                        BaseEntry::Ready {
                            committed: true,
                            ..
                        }
                    ))
        });
    }

    /// Markers of `document` for this frame. Starts reading its committed
    /// version or comparing a new revision when needed.
    pub(super) fn git_gutter_markers(&mut self, document: u64) -> Option<GutterMarkers> {
        if !self.settings.git_gutter {
            self.git_gutter.forget(document);
            return None;
        }
        let Some(key) = self.git_gutter_key(document) else {
            self.git_gutter.forget(document);
            return None;
        };
        let fingerprint = self
            .tab_location(document)
            .and_then(|(project, _)| self.projects[project].git_fingerprint);
        self.invalidate_git_gutter(&key, fingerprint);
        let base = match self.git_gutter.bases.get(&key) {
            Some(BaseEntry::Ready {
                text: Some(text), ..
            }) => text.clone(),
            Some(BaseEntry::Ready { text: None, .. } | BaseEntry::Failed) => {
                self.git_gutter.forget(document);
                return None;
            }
            Some(BaseEntry::Pending(_)) => return self.previous_git_markers(document),
            None => {
                self.request_git_base(key);
                return self.previous_git_markers(document);
            }
        };
        let (project, index) = self.tab_location(document)?;
        let tab = &self.projects[project].tabs[index];
        let revision = tab.revision;
        if let Some(markers) = self.git_gutter.markers.get(&document)
            && markers.key == key
            && markers.revision == revision
            && Arc::ptr_eq(&markers.base, &base)
        {
            return self.previous_git_markers(document);
        }
        if base.len() + tab.text.len() <= INLINE_DIFF_BYTES {
            let hunks = changes(&base, &tab.text);
            self.git_gutter.diffs.remove(&document);
            self.git_gutter.markers.insert(
                document,
                DocumentMarkers {
                    key,
                    base,
                    revision,
                    hunks: hunks.into(),
                },
            );
            return self.previous_git_markers(document);
        }
        if !self.git_gutter.diffs.contains_key(&document) {
            let request = self.git_gutter.request();
            self.git_gutter.diffs.insert(document, (request, revision));
            let text = tab.text.clone();
            self.worker(move || {
                let hunks = changes(&base, &text);
                WorkerResult::GitGutterDiff {
                    document,
                    request,
                    revision,
                    key,
                    base,
                    hunks,
                }
            });
        }
        self.previous_git_markers(document)
    }

    /// The latest markers of `document`, current or not.
    fn previous_git_markers(&self, document: u64) -> Option<GutterMarkers> {
        let markers = self.git_gutter.markers.get(&document)?;
        let revision = self
            .tab_location(document)
            .map(|(project, index)| self.projects[project].tabs[index].revision);
        Some(GutterMarkers {
            base: markers.base.clone(),
            hunks: markers.hunks.clone(),
            revision: markers.revision,
            current: revision == Some(markers.revision),
        })
    }

    fn request_git_base(&mut self, key: BaseKey) {
        if self.git_gutter.bases.len() >= MAX_CACHED_VERSIONS {
            // Visible documents read theirs again; pending reads stay valid.
            self.git_gutter
                .bases
                .retain(|_, entry| matches!(entry, BaseEntry::Pending(_)));
        }
        let request = self.git_gutter.request();
        self.git_gutter
            .bases
            .insert(key.clone(), BaseEntry::Pending(request));
        self.worker(move || {
            let result = git::base_version(&key.repository, key.head.as_deref(), &key.path)
                .map_err(|error| error.to_string());
            WorkerResult::GitGutterBase {
                key,
                request,
                result,
            }
        });
    }

    pub(super) fn apply_git_gutter_base(
        &mut self,
        key: BaseKey,
        request: u64,
        result: Result<git::BaseVersion, String>,
    ) {
        let Some(entry) = self.git_gutter.bases.get_mut(&key) else {
            return;
        };
        if !matches!(entry, BaseEntry::Pending(pending) if *pending == request) {
            return;
        }
        *entry = match result {
            Ok(git::BaseVersion::Text(text)) => BaseEntry::Ready {
                text: Some(text.into()),
                committed: true,
            },
            Ok(git::BaseVersion::Added) => BaseEntry::Ready {
                text: Some("".into()),
                committed: false,
            },
            Ok(
                git::BaseVersion::Untracked
                | git::BaseVersion::Binary
                | git::BaseVersion::TooLarge
                | git::BaseVersion::Unsupported,
            ) => BaseEntry::Ready {
                text: None,
                committed: false,
            },
            Err(_) => BaseEntry::Failed,
        };
        self.ctx.request_repaint();
    }

    /// A worker comparison applies only to the revision and committed
    /// version it was computed for; anything else is computed again.
    pub(super) fn apply_git_gutter_diff(
        &mut self,
        document: u64,
        request: u64,
        revision: u64,
        key: BaseKey,
        base: Arc<str>,
        hunks: Vec<Hunk>,
    ) {
        if self
            .git_gutter
            .diffs
            .get(&document)
            .map(|(pending, _)| *pending)
            != Some(request)
        {
            return;
        }
        self.git_gutter.diffs.remove(&document);
        self.ctx.request_repaint();
        let current_revision = self
            .tab_location(document)
            .map(|(project, index)| self.projects[project].tabs[index].revision);
        let current_base = matches!(
            self.git_gutter.bases.get(&key),
            Some(BaseEntry::Ready { text: Some(text), .. }) if Arc::ptr_eq(text, &base)
        );
        if current_revision != Some(revision)
            || !current_base
            || self.git_gutter_key(document).as_ref() != Some(&key)
        {
            return;
        }
        self.git_gutter.markers.insert(
            document,
            DocumentMarkers {
                key,
                base,
                revision,
                hunks: hunks.into(),
            },
        );
    }

    /// Restore one change from HEAD as a single undoable buffer edit.
    pub(super) fn revert_git_change(
        &mut self,
        document: u64,
        revision: u64,
        hunk: &Hunk,
    ) -> Result<(), String> {
        let stale = "The document changed; review the change again before reverting it";
        let markers = self.git_gutter.markers.get(&document).ok_or(stale)?;
        if markers.revision != revision || !markers.hunks.contains(hunk) {
            return Err(stale.into());
        }
        let base = markers.base.clone();
        let (project, index) = self.tab_location(document).ok_or(stale)?;
        let tab = &self.projects[project].tabs[index];
        if tab.revision != revision || tab.loading {
            return Err(stale.into());
        }
        let (text, cursor) = revert_hunk(&base, &tab.text, hunk);
        if text == tab.text {
            return Ok(());
        }
        let tab = &mut self.projects[project].tabs[index];
        if tab.vim.mode == VimMode::Insert {
            // Close the user's typing as its own undo step first.
            let before = tab.text.clone();
            tab.vim.finish_insert(&mut tab.text);
            if tab.text != before {
                tab.changed();
                return Err(stale.into());
            }
        }
        self.context_native_undo_checkpoint(document);
        let tab = &mut self.projects[project].tabs[index];
        tab.vim.begin_external_edit(&tab.text);
        tab.text = text;
        tab.changed();
        tab.set_cursor(cursor..cursor);
        tab.vim.set_selection(&tab.text, cursor..cursor);
        self.sync_external_document_navigation(document);
        self.session_dirty = true;
        self.ctx.request_repaint();
        Ok(())
    }

    /// Show the open peek of the pane being drawn and apply its action.
    pub(super) fn git_peek_ui(
        &mut self,
        document: u64,
        markers: Option<&GutterMarkers>,
        placements: &[HunkPlacement],
        text_left: f32,
        max_width: f32,
    ) {
        let pane = self.current_pane_id();
        let Some(peek) = self
            .git_gutter
            .peek
            .clone()
            .filter(|peek| peek.pane == pane && peek.document == document)
        else {
            return;
        };
        let placement = markers.and_then(|markers| {
            placements
                .iter()
                .find(|placement| markers.hunks.get(placement.index) == Some(&peek.hunk))
        });
        let (Some(markers), Some(placement)) = (markers, placement) else {
            // The change was reverted, edited or scrolled out of view.
            self.git_gutter.peek = None;
            return;
        };
        if markers.revision != peek.revision {
            self.git_gutter.peek = None;
            return;
        }
        let anchor = egui::pos2(text_left - 4.0, placement.rect.bottom() + 3.0);
        let id = egui::Id::new(("git_change_peek", pane, document));
        let (action, rect) = peek_ui(&self.ctx, id, anchor, max_width, markers, &peek.hunk);
        let outside = self.ctx.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.interact_pos().is_some_and(|position| {
                    !rect.contains(position) && !placement.rect.expand(4.0).contains(position)
                })
        });
        match action {
            Some(PeekAction::Revert) => {
                self.git_gutter.peek = None;
                match self.revert_git_change(document, peek.revision, &peek.hunk) {
                    Ok(()) => {
                        if let Some(tab) = self.tab_mut() {
                            tab.focus = true;
                        }
                        self.status("Change reverted in the buffer · Undo restores it", false);
                    }
                    Err(error) => self.status(error, true),
                }
            }
            Some(PeekAction::Close) => {
                self.git_gutter.peek = None;
                if let Some(tab) = self.tab_mut() {
                    tab.focus = true;
                }
            }
            None if outside => self.git_gutter.peek = None,
            None => {}
        }
    }

    /// Escape closes an open peek before the editor sees the key.
    pub(super) fn git_peek_escape(&mut self) {
        if self.git_gutter.peek.is_some()
            && self
                .ctx
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.git_gutter.peek = None;
            if let Some(tab) = self.tab_mut() {
                tab.focus = true;
            }
        }
    }
}

#[cfg(test)]
#[path = "git_gutter_tests.rs"]
mod tests;
