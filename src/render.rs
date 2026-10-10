//! Persistent, asynchronous syntax coloring. Never parse source on the UI thread.
//!
//! egui_extras' default highlighting cache retains only the previous frame. An
//! editor needs results to survive inactive tabs, and its parser must not hold
//! the live egui Context's memory lock while it works.

use eframe::egui::{self, Color32, FontId, text::LayoutJob};
use egui_extras::syntax_highlighting::{self, CodeTheme, SyntectSettings};
use std::{
    cell::RefCell,
    collections::HashMap,
    mem::size_of,
    sync::{Arc, Mutex, OnceLock, Weak, mpsc},
};

const MAX_DOCUMENT_BYTES: usize = 192 * 1024;
const MAX_DOCUMENTS: usize = 16;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_GALLEY_BYTES: usize = 32 * 1024 * 1024;
const MAX_GALLEYS_PER_DOCUMENT: usize = 4;
const MAX_SYNTAX_JOB_BYTES: usize = 16 * 1024 * 1024;

/// Character-based line starts shared by the gutter and status bar. The text
/// owner must invalidate this on disk reloads which do not increment revision.
#[derive(Default)]
pub struct LineIndex {
    cached: RefCell<Option<IndexedLines>>,
}

struct IndexedLines {
    revision: u64,
    byte_len: usize,
    char_len: usize,
    starts: Vec<usize>,
    crlf: Vec<usize>,
}

impl LineIndex {
    pub fn line_col(&self, text: &str, revision: u64, cursor: usize) -> (usize, usize) {
        let mut cached = self.cached.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|index| index.revision != revision || index.byte_len != text.len())
        {
            let mut starts = vec![0];
            let mut crlf = Vec::new();
            let mut char_len = 0;
            let mut previous = None;
            for (position, ch) in text.chars().enumerate() {
                char_len = position + 1;
                if ch == '\n' {
                    starts.push(position + 1);
                    if previous == Some('\r') {
                        crlf.push(position - 1);
                    }
                }
                previous = Some(ch);
            }
            *cached = Some(IndexedLines {
                revision,
                byte_len: text.len(),
                char_len,
                starts,
                crlf,
            });
        }
        let index = cached.as_ref().unwrap();
        let cursor = cursor.min(index.char_len);
        let row = index.starts.partition_point(|start| *start <= cursor) - 1;
        let start = index.starts[row];
        let skipped = index.crlf.partition_point(|position| *position < cursor)
            - index.crlf.partition_point(|position| *position < start);
        (row + 1, cursor - start + 1 - skipped)
    }

    pub fn invalidate(&self) {
        *self.cached.borrow_mut() = None;
    }

    /// Convert one-based Unicode scalar coordinates to an editor character
    /// offset. The position immediately before the line break is valid; columns
    /// beyond it and nonexistent lines are rejected instead of being clamped.
    pub fn position(&self, text: &str, revision: u64, line: usize, column: usize) -> Option<usize> {
        let row = line.checked_sub(1)?;
        let offset = column.checked_sub(1)?;
        self.line_col(text, revision, 0);
        let cached = self.cached.borrow();
        let index = cached.as_ref()?;
        let start = *index.starts.get(row)?;
        let end = indexed_line_end(index, row)?;
        let position = start.checked_add(offset)?;
        (position <= end).then_some(position)
    }

    /// Source lines, including the final empty line after a trailing newline.
    pub fn line_count(&self, text: &str, revision: u64) -> usize {
        self.line_col(text, revision, 0);
        self.cached
            .borrow()
            .as_ref()
            .map_or(1, |index| index.starts.len())
    }

    /// End character offset of a one-based line, excluding LF or CRLF.
    pub fn line_end(&self, text: &str, revision: u64, line: usize) -> Option<usize> {
        let row = line.checked_sub(1)?;
        self.line_col(text, revision, 0);
        indexed_line_end(self.cached.borrow().as_ref()?, row)
    }
}

fn indexed_line_end(index: &IndexedLines, row: usize) -> Option<usize> {
    let start = *index.starts.get(row)?;
    let Some(next_start) = index.starts.get(row + 1) else {
        return Some(index.char_len);
    };
    let mut end = next_start - 1;
    if end > start && index.crlf.binary_search(&(end - 1)).is_ok() {
        end -= 1;
    }
    Some(end)
}

/// A visual row is either the beginning or a continuation of a source line.
/// Character boundaries include no newline, even for the last row of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualRow {
    pub line: usize,
    pub first_in_line: bool,
    pub start: usize,
    pub end: usize,
}

pub fn visual_rows(galley: &egui::Galley) -> impl Iterator<Item = VisualRow> + '_ {
    galley.rows.iter().scan((0, 1, true), |state, row| {
        let mapped = VisualRow {
            line: state.1,
            first_in_line: state.2,
            start: state.0,
            end: state.0 + row.char_count_excluding_newline().0,
        };
        state.0 = mapped.end + usize::from(row.ends_with_newline);
        state.1 += usize::from(row.ends_with_newline);
        state.2 = row.ends_with_newline;
        Some(mapped)
    })
}

/// Restore a menu-owned selection in the existing text shape, behind its glyphs.
/// Keep the cached syntax galley and the widget's original clip rectangle intact.
pub fn paint_retained_text_selection(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    selection: std::ops::Range<usize>,
) -> bool {
    if selection.is_empty() {
        return false;
    }
    let position = output.galley_pos - egui::vec2(output.galley.rect.left(), 0.0);
    let mut selected = output.galley.clone();
    egui::text_selection::visuals::paint_text_selection(
        &mut selected,
        ui.visuals(),
        &egui::text::CCursorRange::two(
            egui::text::CCursor::new(selection.start),
            egui::text::CCursor::new(selection.end),
        ),
        None,
    );
    ui.ctx().graphics_mut(|graphics| {
        let Some(shapes) = graphics.get_mut(ui.layer_id()) else {
            return false;
        };
        let index = shapes.all_entries().enumerate().find_map(|(index, shape)| {
            matches!(&shape.shape, egui::Shape::Text(text)
                if text.pos == position && Arc::ptr_eq(&text.galley, &output.galley))
            .then_some(egui::layers::ShapeIdx(index))
        });
        let Some(index) = index else {
            return false;
        };
        shapes.mutate_shape(index, |shape| {
            if let egui::Shape::Text(text) = &mut shape.shape {
                text.galley = selected;
            }
        });
        true
    })
}

/// Clip a half-open character range to one shaped visual row. Keeping this in
/// character space avoids marking the next wrapped row at an exclusive end.
pub fn range_on_visual_row(
    galley: &egui::Galley,
    row_index: usize,
    mapped: VisualRow,
    start: usize,
    end: usize,
) -> Option<egui::Rect> {
    if end < mapped.start
        || start > mapped.end
        || (start < end
            && mapped.start < mapped.end
            && (end <= mapped.start || start >= mapped.end))
    {
        return None;
    }
    let row = &galley.rows[row_index];
    if start == end
        && start == mapped.end
        && !row.ends_with_newline
        && row_index + 1 < galley.rows.len()
    {
        return None;
    }
    let left = if start <= mapped.start {
        row.pos.x
    } else {
        galley
            .pos_from_cursor(egui::text::CCursor {
                index: start.into(),
                prefer_next_row: true,
            })
            .left()
    };
    let right = if end >= mapped.end {
        row.pos.x + row.size.x
    } else {
        galley.pos_from_cursor(egui::text::CCursor::new(end)).left()
    };
    Some(egui::Rect::from_min_max(
        egui::pos2(left, row.pos.y),
        egui::pos2(right.max(left + 6.0), row.pos.y + row.size.y),
    ))
}

/// Consume an editor-owned wheel event directly instead of applying egui's
/// multi-frame wheel filter. Other panels keep their normal scrolling policy.
pub fn instant_scroll_offset(ui: &egui::Ui, scroll_id: egui::Id) -> Option<egui::Vec2> {
    instant_scroll_offset_axes(ui, scroll_id, egui::Vec2b::TRUE)
}

pub fn instant_scroll_offset_axes(
    ui: &egui::Ui,
    scroll_id: egui::Id,
    axes: egui::Vec2b,
) -> Option<egui::Vec2> {
    let viewport = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    if !ui.is_enabled() || egui::Popup::is_any_open(ui.ctx()) || !ui.rect_contains_pointer(viewport)
    {
        return None;
    }
    let options = ui.ctx().options(|options| options.input_options);
    let delta = ui.input_mut(|input| {
        let mut total = egui::Vec2::ZERO;
        let mut consumed = false;
        let mut consumed_axes = egui::Vec2b::FALSE;
        input.events.retain_mut(|event| {
            let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                phase,
            } = event
            else {
                return true;
            };
            if modifiers.ctrl || modifiers.command {
                return true;
            }
            let mut delta = match unit {
                egui::MouseWheelUnit::Point => *delta,
                egui::MouseWheelUnit::Line => *delta * options.line_scroll_speed,
                egui::MouseWheelUnit::Page => *delta * viewport.height(),
            };
            let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
            let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
            if horizontal && !vertical {
                delta = egui::vec2(delta.x + delta.y, 0.0);
            } else if vertical && !horizontal {
                delta = egui::vec2(0.0, delta.x + delta.y);
            }
            let handled = egui::vec2(
                if axes.x { delta.x } else { 0.0 },
                if axes.y { delta.y } else { 0.0 },
            );
            if handled == egui::Vec2::ZERO {
                return true;
            }
            total += handled;
            consumed = true;
            consumed_axes.x |= handled.x != 0.0;
            consumed_axes.y |= handled.y != 0.0;
            let remainder = delta - handled;
            if remainder != egui::Vec2::ZERO {
                *event = egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: remainder,
                    modifiers: egui::Modifiers::NONE,
                    phase: *phase,
                };
                true
            } else {
                false
            }
        });
        if consumed_axes.x {
            input.smooth_scroll_delta.x = 0.0;
        }
        if consumed_axes.y {
            input.smooth_scroll_delta.y = 0.0;
        }
        consumed.then_some(total)
    })?;
    let offset = egui::scroll_area::State::load(ui.ctx(), scroll_id)
        .unwrap_or_default()
        .offset;
    let maximum = ui.ctx().data(|data| {
        data.get_temp::<egui::Vec2>(scroll_id.with("document_scroll_limit"))
            .unwrap_or(egui::Vec2::splat(f32::INFINITY))
    });
    Some((offset - delta).max(egui::Vec2::ZERO).min(maximum))
}

pub fn remember_scroll_limits<R>(
    ctx: &egui::Context,
    scroll: &egui::scroll_area::ScrollAreaOutput<R>,
) {
    ctx.data_mut(|data| {
        data.insert_temp(
            scroll.id.with("document_scroll_limit"),
            (scroll.content_size - scroll.inner_rect.size()).max(egui::Vec2::ZERO),
        )
    });
}

#[derive(Clone, Default)]
pub struct SyntaxCache {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Mutex<Option<mpsc::SyncSender<()>>>,
}

#[derive(Default)]
struct State {
    documents: HashMap<u64, Document>,
    serial: u64,
    atlas: Option<AtlasStamp>,
}

struct Document {
    ready: Option<Ready>,
    pending: Option<Request>,
    galleys: Vec<CachedGalley>,
    last_used: u64,
}

struct CachedGalley {
    value: Arc<egui::Galley>,
    bytes: usize,
    revision: u64,
    last_used: u64,
}

struct AtlasStamp {
    ctx: egui::Context,
    pixels_per_point: f32,
    probe: Arc<egui::Galley>,
}

struct Ready {
    language: String,
    font: f32,
    job: Arc<LayoutJob>,
}

#[derive(Clone)]
struct Request {
    document: u64,
    serial: u64,
    revision: u64,
    text: Arc<str>,
    language: String,
    font: f32,
    ctx: egui::Context,
    queued: bool,
}

impl SyntaxCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep the atlas identity probe alive on every UI pass, including Markdown
    /// preview, palettes and other views which do not lay out a source editor.
    /// Otherwise egui's frame GC could evict the probe and make a return to the
    /// editor look like an atlas reset even though its cached UVs remain valid.
    pub fn begin_frame(&self, ui: &egui::Ui) {
        let probe = ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "vedit atlas generation".into(),
                FontId::monospace(1.0),
                Color32::WHITE,
            )
        });
        let pixels_per_point = ui.ctx().pixels_per_point();
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let same_atlas = state.atlas.as_ref().is_some_and(|stamp| {
            stamp.ctx == *ui.ctx()
                && stamp.pixels_per_point == pixels_per_point
                && Arc::ptr_eq(&stamp.probe, &probe)
        });
        if !same_atlas {
            for document in state.documents.values_mut() {
                document.galleys.clear();
            }
            state.atlas = Some(AtlasStamp {
                ctx: ui.ctx().clone(),
                pixels_per_point,
                probe,
            });
        }
    }

    /// Returns cached colors or plain text immediately and requests coloring.
    ///
    /// Text identity is checked as well as the revision: TextEdit may call its
    /// layouter after changing the buffer, before the owner increments revision.
    /// Font size and wrap width belong to the current view, never a stale tab.
    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        &self,
        ctx: &egui::Context,
        document: u64,
        revision: u64,
        text: &str,
        language: &str,
        font: f32,
        fallback: Color32,
        wrap_width: f32,
    ) -> LayoutJob {
        if text.is_empty() || text.len() > MAX_DOCUMENT_BYTES || language.is_empty() {
            return plain(text, font, fallback, wrap_width);
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.serial = state.serial.wrapping_add(1);
        let serial = state.serial;
        let entry = state.documents.entry(document).or_insert(Document {
            ready: None,
            pending: None,
            galleys: Vec::new(),
            last_used: serial,
        });
        entry.last_used = serial;
        if let Some(ready) = &entry.ready
            && ready.language == language
            && ready.font == font
            && ready.job.text == text
        {
            let mut job = (*ready.job).clone();
            job.wrap.max_width = wrap_width;
            return job;
        }
        let preview = entry
            .ready
            .as_ref()
            .filter(|ready| ready.language == language && ready.font == font)
            .map(|ready| carry_colors(&ready.job, text, font, fallback, wrap_width));
        let already_pending = entry.pending.as_ref().is_some_and(|pending| {
            pending.language == language && pending.font == font && &*pending.text == text
        });
        if !already_pending {
            entry.pending = Some(Request {
                document,
                serial,
                revision,
                text: Arc::from(text),
                language: language.into(),
                font,
                ctx: ctx.clone(),
                queued: true,
            });
        } else if let Some(pending) = &mut entry.pending {
            // Contexts can change between frames (including in the UI harness).
            pending.ctx = ctx.clone();
            pending.revision = revision;
            if pending.queued {
                pending.serial = serial;
            }
        }
        trim(&mut state, document);
        drop(state);
        if !already_pending {
            self.wake_worker();
        }
        preview.unwrap_or_else(|| plain(text, font, fallback, wrap_width))
    }

    /// Retains shaped glyphs across inactive tabs as well as syntax sections.
    /// A constant probe detects recreation of egui's font atlas: cached galleys
    /// contain atlas UVs and must be discarded when that atlas is recreated.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn galley(
        &self,
        ui: &egui::Ui,
        document: u64,
        revision: u64,
        text: &str,
        language: &str,
        font: f32,
        fallback: Color32,
        wrap_width: f32,
    ) -> Arc<egui::Galley> {
        self.galley_with_line_height(
            ui, document, revision, text, language, font, fallback, wrap_width, 1.35,
        )
    }

    /// Line spacing changes geometry without requiring syntax to be parsed again.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn galley_with_line_height(
        &self,
        ui: &egui::Ui,
        document: u64,
        revision: u64,
        text: &str,
        language: &str,
        font: f32,
        fallback: Color32,
        wrap_width: f32,
        line_height: f32,
    ) -> Arc<egui::Galley> {
        self.galley_with_preferences(
            ui,
            document,
            revision,
            text,
            language,
            font,
            fallback,
            wrap_width,
            line_height,
            4,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn galley_with_preferences(
        &self,
        ui: &egui::Ui,
        document: u64,
        revision: u64,
        text: &str,
        language: &str,
        font: f32,
        fallback: Color32,
        wrap_width: f32,
        line_height: f32,
        tab_width: usize,
    ) -> Arc<egui::Galley> {
        self.begin_frame(ui);
        let mut job = self.layout(
            ui.ctx(),
            document,
            revision,
            text,
            language,
            font,
            fallback,
            wrap_width,
        );
        for section in &mut job.sections {
            section.format.line_height = Some(font * line_height);
        }
        if tab_width != 4 && job.text.contains('\t') {
            let mut sections = Vec::with_capacity(job.sections.len());
            for section in std::mem::take(&mut job.sections) {
                let mut start = section.byte_range.start;
                for (offset, _) in job.text[section.byte_range.start.0..section.byte_range.end.0]
                    .match_indices('\t')
                {
                    let at = section.byte_range.start + offset;
                    if start < at {
                        let mut before = section.clone();
                        before.byte_range = start..at;
                        sections.push(before);
                    }
                    let mut tab = section.clone();
                    tab.byte_range = at..at + 1;
                    tab.format.font_id.family = crate::typography::tab_family(tab_width);
                    sections.push(tab);
                    start = at + 1;
                }
                if start < section.byte_range.end {
                    let mut after = section;
                    after.byte_range.start = start;
                    sections.push(after);
                }
            }
            job.sections = sections;
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.serial = state.serial.wrapping_add(1);
        let serial = state.serial;
        if let Some(entry) = state.documents.get_mut(&document) {
            entry.last_used = serial;
            // Keep view layouts only for the current buffer. TextEdit can mutate
            // text before the revision changes, so compare both identities.
            entry
                .galleys
                .retain(|cached| cached.revision == revision && cached.value.job.text == job.text);
            if let Some(cached) = entry
                .galleys
                .iter_mut()
                .find(|cached| *cached.value.job == job)
            {
                cached.last_used = serial;
                return Arc::clone(&cached.value);
            }
        }
        // Font shaping can be expensive. Do it outside our worker/cache lock.
        drop(state);
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let bytes = galley_bytes(&galley);
        if bytes <= MAX_GALLEY_BYTES {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.serial = state.serial.wrapping_add(1);
            let serial = state.serial;
            let entry = state.documents.entry(document).or_insert(Document {
                ready: None,
                pending: None,
                galleys: Vec::new(),
                last_used: serial,
            });
            entry.last_used = serial;
            if entry.galleys.len() == MAX_GALLEYS_PER_DOCUMENT {
                let oldest = entry
                    .galleys
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, cached)| cached.last_used)
                    .map(|(index, _)| index)
                    .unwrap();
                entry.galleys.remove(oldest);
            }
            entry.galleys.push(CachedGalley {
                value: Arc::clone(&galley),
                bytes,
                revision,
                last_used: serial,
            });
            trim(&mut state, document);
        }
        galley
    }

    /// Allows deterministic screenshot tests to settle asynchronous colors.
    #[cfg(test)]
    pub fn is_ready(&self, document: u64, text: &str, language: &str, font: f32) -> bool {
        if text.is_empty() || text.len() > MAX_DOCUMENT_BYTES || language.is_empty() {
            return true;
        }
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .documents
            .get(&document)
            .and_then(|entry| entry.ready.as_ref())
            .is_some_and(|ready| {
                ready.language == language && ready.font == font && ready.job.text == text
            })
    }

    fn wake_worker(&self) {
        let mut wake = self.shared.wake.lock().unwrap_or_else(|e| e.into_inner());
        if wake.is_none() {
            let (sender, receiver) = mpsc::sync_channel(1);
            let shared = Arc::downgrade(&self.shared);
            if std::thread::Builder::new()
                .name("vedit-syntax".into())
                .spawn(move || worker(shared, receiver))
                .is_ok()
            {
                *wake = Some(sender);
            }
        }
        if let Some(sender) = wake.as_ref() {
            let _ = sender.try_send(());
        }
    }
}

fn plain(text: &str, font: f32, color: Color32, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple(text.into(), FontId::monospace(font), color, wrap_width);
    source_format(&mut job, font);
    job
}

/// Source uses a stable row grid, including before syntax colors arrive. Token
/// emphasis must not change glyph geometry or make comments visually compete
/// with code; egui_extras also maps syntect italics to an underline.
fn source_format(job: &mut LayoutJob, font: f32) {
    for section in &mut job.sections {
        section.format.line_height = Some(font * 1.35);
        section.format.italics = false;
        section.format.underline = egui::Stroke::NONE;
    }
}

/// Preserve stable token colors while a new parse is pending. Only unchanged
/// prefix/suffix bytes inherit their old styles; the edited span is neutral.
fn carry_colors(
    old: &LayoutJob,
    text: &str,
    font: f32,
    fallback: Color32,
    width: f32,
) -> LayoutJob {
    let mut prefix = old
        .text
        .bytes()
        .zip(text.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.text.is_char_boundary(prefix) || !text.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = old
        .text
        .bytes()
        .rev()
        .zip(text.bytes().rev())
        .take(old.text.len().min(text.len()) - prefix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.text.is_char_boundary(old.text.len() - suffix)
        || !text.is_char_boundary(text.len() - suffix)
    {
        suffix -= 1;
    }
    let old_tail = old.text.len() - suffix;
    let new_tail = text.len() - suffix;
    let mut job = LayoutJob {
        text: text.into(),
        ..Default::default()
    };
    job.wrap.max_width = width;
    for section in &old.sections {
        let start = section.byte_range.start.0;
        let end = section.byte_range.end.0.min(prefix);
        if start < end {
            append_section(&mut job, start, end, section.format.clone());
        }
    }
    if prefix < new_tail {
        append_section(
            &mut job,
            prefix,
            new_tail,
            egui::TextFormat {
                font_id: FontId::monospace(font),
                color: fallback,
                ..Default::default()
            },
        );
    }
    for section in &old.sections {
        let start = section.byte_range.start.0.max(old_tail);
        let end = section.byte_range.end.0;
        if start < end {
            append_section(
                &mut job,
                new_tail + start - old_tail,
                new_tail + end - old_tail,
                section.format.clone(),
            );
        }
    }
    source_format(&mut job, font);
    job
}

fn append_section(job: &mut LayoutJob, start: usize, end: usize, format: egui::TextFormat) {
    use egui::text::ByteIndex;
    if let Some(last) = job.sections.last_mut()
        && last.byte_range.end.0 == start
        && last.format == format
    {
        last.byte_range.end = ByteIndex(end);
    } else {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(start)..ByteIndex(end),
            format,
        });
    }
}

fn trim(state: &mut State, active: u64) {
    loop {
        let bytes: usize = state.documents.values().map(document_bytes).sum();
        if state.documents.len() <= MAX_DOCUMENTS && bytes <= MAX_CACHE_BYTES {
            break;
        }
        let oldest = state
            .documents
            .iter()
            .filter(|(id, _)| **id != active)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(id, _)| *id);
        if let Some(oldest) = oldest {
            state.documents.remove(&oldest);
        } else if let Some(entry) = state.documents.get_mut(&active) {
            // Multiple wrapped panes may each retain a large layout. The active
            // document must obey the global budget even when it is the only tab.
            let oldest = entry
                .galleys
                .iter()
                .enumerate()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(index, _)| index);
            let Some(oldest) = oldest else { break };
            entry.galleys.remove(oldest);
        } else {
            break;
        }
    }
}

fn document_bytes(document: &Document) -> usize {
    document.ready.as_ref().map_or(0, |ready| {
        ready.job.text.len() + ready.job.sections.len() * size_of::<egui::text::LayoutSection>()
    }) + document
        .pending
        .as_ref()
        .map_or(0, |pending| pending.text.len())
        + document
            .galleys
            .iter()
            .map(|galley| galley.bytes)
            .sum::<usize>()
}

fn galley_bytes(galley: &egui::Galley) -> usize {
    galley.job.text.len()
        + galley.job.sections.len() * size_of::<egui::text::LayoutSection>()
        + galley.rows.len() * size_of::<egui::epaint::text::PlacedRow>()
        + galley
            .rows
            .iter()
            .map(|row| row.glyphs.len() * size_of::<egui::epaint::text::Glyph>())
            .sum::<usize>()
        + galley.num_vertices * size_of::<egui::epaint::Vertex>()
        + galley.num_indices * size_of::<u32>()
}

fn next_request(shared: &Weak<Shared>) -> Option<Request> {
    let shared = shared.upgrade()?;
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    // The most recently displayed file wins over older queued tab requests.
    let document = state
        .documents
        .iter()
        .filter_map(|(id, entry)| {
            entry
                .pending
                .as_ref()
                .filter(|pending| pending.queued)
                .map(|pending| (*id, pending.serial))
        })
        .max_by_key(|(_, serial)| *serial)
        .map(|(id, _)| id)?;
    let request = state.documents.get_mut(&document)?.pending.as_mut()?;
    request.queued = false;
    Some(request.clone())
}

fn worker(shared: Weak<Shared>, receiver: mpsc::Receiver<()>) {
    // Grammar initialization stays off the UI thread and is shared by workers.
    // Each parser keeps its own egui Context and never locks live UI caches.
    let settings = syntax_settings();
    let parser_ctx = egui::Context::default();
    while receiver.recv().is_ok() {
        while let Some(request) = next_request(&shared) {
            let style = egui::Style {
                override_font_id: Some(FontId::monospace(request.font)),
                ..Default::default()
            };
            let mut job = syntax_highlighting::highlight_with(
                &parser_ctx,
                &style,
                &CodeTheme::dark(request.font),
                &request.text,
                &request.language,
                settings,
            );
            if job.text.len() + job.sections.len() * size_of::<egui::text::LayoutSection>()
                > MAX_SYNTAX_JOB_BYTES
            {
                job = plain(
                    &request.text,
                    request.font,
                    Color32::LIGHT_GRAY,
                    f32::INFINITY,
                );
            }
            source_format(&mut job, request.font);
            // Own the persistent cache here; the library's one-frame cache must
            // not accumulate every historical document revision on this thread.
            parser_ctx.memory_mut(|memory| memory.caches = Default::default());
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state.documents.get_mut(&request.document) else {
                continue;
            };
            if entry
                .pending
                .as_ref()
                .is_some_and(|latest| latest.serial == request.serial)
            {
                let ctx = entry.pending.take().unwrap().ctx;
                entry.ready = Some(Ready {
                    language: request.language,
                    font: request.font,
                    job: Arc::new(job),
                });
                trim(&mut state, request.document);
                drop(state);
                ctx.request_repaint();
            }
        }
    }
}

fn syntax_settings() -> &'static SyntectSettings {
    static SETTINGS: OnceLock<SyntectSettings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let mut settings = SyntectSettings::default();
        let mut builder = settings.ps.into_builder();
        builder.add(
            syntect::parsing::SyntaxDefinition::load_from_str(
                include_str!("../assets/syntaxes/Dart.sublime-syntax"),
                true,
                Some("Dart"),
            )
            .expect("the bundled Dart syntax must parse"),
        );
        settings.ps = builder.build();
        settings
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn dart_scopes_at(source: &str, needle: &str) -> Vec<String> {
        use syntect::easy::ScopeRegionIterator;
        use syntect::parsing::{ParseState, ScopeStack};
        let settings = syntax_settings();
        let syntax = settings.ps.find_syntax_by_extension("dart").unwrap();
        let mut parser = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let wanted = source.find(needle).expect("sample token exists");
        let mut offset = 0;
        for line in syntect::util::LinesWithEndings::from(source) {
            let operations = parser.parse_line(line, &settings.ps).unwrap();
            for (region, operation) in ScopeRegionIterator::new(&operations, line) {
                stack.apply(operation).unwrap();
                if (offset..offset + region.len()).contains(&wanted) {
                    return stack.as_slice().iter().map(ToString::to_string).collect();
                }
                offset += region.len();
            }
        }
        panic!("token {needle:?} has no parsed region");
    }

    #[test]
    fn dart_grammar_handles_keywords_strings_interpolation_and_nested_comments() {
        let source = concat!(
            "sealed class Animal {}\n",
            "String greet(String name) {\n",
            "  // return 'not code';\n",
            "  final value = 'hello $name ${1 + 2}';\n",
            "  final raw = r'raw $name';\n",
            "  /* outer /* nested */ still comment */\n",
            "  return value;\n",
            "}\n",
        );
        for (needle, scope) in [
            ("sealed", "keyword.declaration.dart"),
            ("class", "keyword.declaration.dart"),
            ("return 'not code'", "comment.line.double-slash.dart"),
            ("hello", "string.interpolated.single.dart"),
            ("$name", "meta.embedded.expression.dart"),
            ("1 + 2", "constant.numeric.dart"),
            ("raw $name", "string.quoted.single.dart"),
            ("nested", "comment.block.dart"),
            ("still comment", "comment.block.dart"),
            ("return value", "keyword.control.return.dart"),
        ] {
            let scopes = dart_scopes_at(source, needle);
            assert!(
                scopes.iter().any(|value| value == scope),
                "{needle:?}: {scopes:?}"
            );
        }
        assert!(
            !dart_scopes_at(source, "raw $name")
                .iter()
                .any(|value| value.starts_with("meta.embedded"))
        );
    }

    #[test]
    fn dart_multiline_strings_and_documentation_fences_end_before_following_code() {
        let source = concat!(
            "final text = \"\"\"first\n",
            "  $name\n",
            "last\"\"\";\n",
            "/// ```dart\n",
            "/// final inside = 1;\n",
            "/// ```\n",
            "final outside = true;\n",
        );
        assert!(
            dart_scopes_at(source, "last")
                .contains(&"string.interpolated.triple.double.dart".into())
        );
        assert!(
            dart_scopes_at(source, "inside").contains(&"comment.block.documentation.dart".into())
        );
        let outside = dart_scopes_at(source, "final outside");
        assert!(
            outside.contains(&"storage.modifier.dart".into()),
            "{outside:?}"
        );
        assert!(
            !outside
                .iter()
                .any(|value| value.starts_with("comment.") || value.starts_with("string."))
        );
    }

    #[test]
    fn dart_worker_renders_distinct_keyword_string_and_comment_colors() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let source = "final greeting = 'hello'; // a comment\n";
        let request = || {
            cache.layout(
                &ctx,
                711,
                1,
                source,
                "dart",
                14.0,
                Color32::WHITE,
                f32::INFINITY,
            )
        };
        let _ = request();
        settle(&cache, 711, source, "dart", 14.0);
        let job = request();
        let color_at = |needle: &str| {
            let offset = source.find(needle).unwrap();
            job.sections
                .iter()
                .find(|section| {
                    section.byte_range.start.0 <= offset && offset < section.byte_range.end.0
                })
                .unwrap()
                .format
                .color
        };
        assert_ne!(color_at("final"), color_at("hello"));
        assert_ne!(color_at("hello"), color_at("a comment"));
        assert_ne!(color_at("final"), color_at("a comment"));
    }

    fn settle(cache: &SyntaxCache, document: u64, text: &str, language: &str, font: f32) {
        let started = Instant::now();
        while !cache.is_ready(document, text, language, font) {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "syntax worker timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn layout(
        cache: &SyntaxCache,
        document: u64,
        revision: u64,
        text: &str,
        font: f32,
    ) -> LayoutJob {
        cache.layout(
            &egui::Context::default(),
            document,
            revision,
            text,
            "rs",
            font,
            Color32::WHITE,
            f32::INFINITY,
        )
    }

    fn galley_pass(
        ctx: &egui::Context,
        cache: &SyntaxCache,
        document: u64,
        text: &str,
        font: f32,
        width: f32,
    ) -> Arc<egui::Galley> {
        let mut result = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            result = Some(cache.galley(ui, document, 0, text, "", font, Color32::WHITE, width));
        });
        output.drop_without_applying_deltas();
        result.unwrap()
    }

    fn source_galley_pass(
        ctx: &egui::Context,
        cache: &SyntaxCache,
        text: &str,
        font: f32,
        width: f32,
    ) -> Arc<egui::Galley> {
        let mut result = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            result = Some(cache.galley(ui, 1, 0, text, "rs", font, Color32::WHITE, width));
        });
        output.drop_without_applying_deltas();
        result.unwrap()
    }

    fn assert_same_source_geometry(before: &egui::Galley, after: &egui::Galley) {
        assert_eq!(before.job.text, after.job.text);
        assert_eq!(before.size(), after.size());
        assert_eq!(
            visual_rows(before).collect::<Vec<_>>(),
            visual_rows(after).collect::<Vec<_>>()
        );
        for (before, after) in before.rows.iter().zip(&after.rows) {
            assert_eq!(before.pos, after.pos);
            assert_eq!(before.size, after.size);
        }
        // Cursor and range overlays must stay on the same glyphs when the
        // asynchronous result replaces the immediate source layout.
        for position in 0..=before.job.text.chars().count() {
            let cursor = egui::text::CCursor::new(position);
            assert_eq!(
                before.pos_from_cursor(cursor),
                after.pos_from_cursor(cursor)
            );
        }
        let mapped: Vec<_> = visual_rows(before).collect();
        for (row, mapped) in mapped.into_iter().enumerate() {
            assert_eq!(
                range_on_visual_row(before, row, mapped, mapped.start, mapped.end),
                range_on_visual_row(after, row, mapped, mapped.start, mapped.end)
            );
        }
    }

    #[test]
    fn source_rows_and_cursor_geometry_stay_stable_when_syntax_arrives() {
        let source = "// Grüße 🦀: comments stay calm\n\nfn main() {\n    let text = \"ä中🦀 and words that wrap\";\n}\n";
        for width in [f32::INFINITY, 100.0] {
            let ctx = egui::Context::default();
            let cache = SyntaxCache::new();
            let initial = source_galley_pass(&ctx, &cache, source, 14.0, width);
            assert_eq!(initial.job.sections.len(), 1);
            settle(&cache, 1, source, "rs", 14.0);
            let ready = source_galley_pass(&ctx, &cache, source, 14.0, width);
            assert!(ready.job.sections.len() > 1);
            assert!(ready.job.sections.iter().all(|section| {
                !section.format.italics && section.format.underline == egui::Stroke::NONE
            }));
            assert_same_source_geometry(&initial, &ready);
            let numbered: Vec<_> = visual_rows(&ready)
                .filter(|row| row.first_in_line)
                .map(|row| row.line)
                .collect();
            assert_eq!(numbered, [1, 2, 3, 4, 5, 6]);
            if width.is_finite() {
                assert!(ready.rows.len() > numbered.len());
            }
        }
    }

    #[test]
    fn document_tab_width_changes_geometry_without_changing_character_offsets() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::typography::fonts(crate::cli::EditorFont::default()));
        let cache = SyntaxCache::new();
        let text = "\tvalue\n\t\tsecond\n";
        let pass = |width| {
            let mut result = None;
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                result = Some(cache.galley_with_preferences(
                    ui,
                    801,
                    0,
                    text,
                    "txt",
                    14.0,
                    Color32::WHITE,
                    f32::INFINITY,
                    1.35,
                    width,
                ));
            });
            output.drop_without_applying_deltas();
            result.unwrap()
        };
        let narrow = pass(2);
        let wide = pass(8);
        assert_eq!(narrow.job.text, text);
        assert_eq!(wide.job.text, text);
        let cursor = egui::text::CCursor::new(1);
        assert!(wide.pos_from_cursor(cursor).left() > narrow.pos_from_cursor(cursor).left() * 3.0);
        assert_eq!(
            visual_rows(&narrow)
                .map(|row| row.start)
                .collect::<Vec<_>>(),
            visual_rows(&wide).map(|row| row.start).collect::<Vec<_>>()
        );
    }

    #[test]
    fn inserted_rows_keep_the_same_grid_before_and_after_recoloring() {
        let ctx = egui::Context::default();
        let cache = SyntaxCache::new();
        let source = "fn main() { let value = 1; }\n";
        source_galley_pass(&ctx, &cache, source, 15.0, 130.0);
        settle(&cache, 1, source, "rs", 15.0);
        let edited = "// inserted Unicode ä中🦀 and more words\n\nfn main() { let value = 1; }\n";
        let preview = source_galley_pass(&ctx, &cache, edited, 15.0, 130.0);
        assert!(preview.job.sections.len() > 1);
        assert_eq!(preview.job.sections[0].format.color, Color32::WHITE);
        assert!(
            preview
                .job
                .sections
                .iter()
                .any(|section| section.format.color != Color32::WHITE)
        );
        settle(&cache, 1, edited, "rs", 15.0);
        let ready = source_galley_pass(&ctx, &cache, edited, 15.0, 130.0);
        assert_same_source_geometry(&preview, &ready);
        assert_eq!(
            visual_rows(&ready)
                .filter(|row| row.first_in_line)
                .map(|row| row.line)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn resizing_source_font_updates_row_grid_without_reusing_old_glyphs() {
        let ctx = egui::Context::default();
        let cache = SyntaxCache::new();
        let source = "// ä🦀\n\nfn main() {}\n";
        source_galley_pass(&ctx, &cache, source, 13.0, f32::INFINITY);
        settle(&cache, 1, source, "rs", 13.0);
        let small = source_galley_pass(&ctx, &cache, source, 13.0, f32::INFINITY);
        let resized_plain = source_galley_pass(&ctx, &cache, source, 22.0, f32::INFINITY);
        assert!(!Arc::ptr_eq(&small, &resized_plain));
        assert!(resized_plain.rows[0].size.y > small.rows[0].size.y);
        assert_eq!(resized_plain.rows.len(), small.rows.len());
        settle(&cache, 1, source, "rs", 22.0);
        let resized = source_galley_pass(&ctx, &cache, source, 22.0, f32::INFINITY);
        assert_same_source_geometry(&resized_plain, &resized);
        assert!(
            resized
                .job
                .sections
                .iter()
                .all(|section| section.format.font_id.size == 22.0)
        );
        let returned = source_galley_pass(&ctx, &cache, source, 22.0, f32::INFINITY);
        assert!(Arc::ptr_eq(&resized, &returned));
    }

    #[test]
    fn wrapped_rows_preserve_logical_lines_and_half_open_range_ends() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let text = "abcdefghij klmnopqrst uvwxyz\n\n🦀final line\n";
        let galley = galley_pass(&ctx, &cache, 1, text, 14.0, 50.0);
        let mapped: Vec<_> = visual_rows(&galley).collect();
        assert!(mapped.len() > 4);
        let first_rows: Vec<_> = mapped.iter().filter(|row| row.first_in_line).collect();
        assert_eq!(
            first_rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(first_rows[1].start, text.find('\n').unwrap() + 1);
        assert_eq!(first_rows[2].start, first_rows[1].start + 1);
        assert_eq!(mapped.last().unwrap().end, text.chars().count());
        let boundary = mapped[0].end;
        assert!(boundary > 0);
        assert!(range_on_visual_row(&galley, 0, mapped[0], 0, boundary).is_some());
        assert!(
            range_on_visual_row(&galley, 1, mapped[1], 0, boundary).is_none(),
            "Exclusive range end must not spill into its next wrapped row"
        );
        assert!(range_on_visual_row(&galley, 0, mapped[0], boundary, boundary).is_none());
        assert!(range_on_visual_row(&galley, 1, mapped[1], boundary, boundary).is_some());
        let blank_index = mapped.iter().position(|row| row.line == 2).unwrap();
        let blank = mapped[blank_index];
        assert!(range_on_visual_row(&galley, blank_index, blank, blank.start, blank.end).is_some());
    }

    #[test]
    fn shaped_glyphs_survive_switching_between_tabs() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let first = galley_pass(&ctx, &cache, 1, "first document\n🦀", 14.0, f32::INFINITY);
        galley_pass(&ctx, &cache, 2, "second document", 14.0, f32::INFINITY);
        galley_pass(&ctx, &cache, 2, "second document", 14.0, f32::INFINITY);
        let returned = galley_pass(&ctx, &cache, 1, "first document\n🦀", 14.0, f32::INFINITY);
        assert!(Arc::ptr_eq(&first, &returned));
    }

    #[test]
    fn wrapped_split_layouts_survive_alternating_widths_across_frames() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let text = "Wrapped split source with words and 🦀 characters\n".repeat(20);
        let narrow = galley_pass(&ctx, &cache, 1, &text, 14.0, 120.0);
        let wide = galley_pass(&ctx, &cache, 1, &text, 14.0, 360.0);
        assert_ne!(narrow.rows.len(), wide.rows.len());
        for _ in 0..12 {
            assert!(Arc::ptr_eq(
                &narrow,
                &galley_pass(&ctx, &cache, 1, &text, 14.0, 120.0)
            ));
            assert!(Arc::ptr_eq(
                &wide,
                &galley_pass(&ctx, &cache, 1, &text, 14.0, 360.0)
            ));
        }
        let state = cache.shared.state.lock().unwrap();
        assert_eq!(state.documents[&1].galleys.len(), 2);
    }

    #[test]
    fn wrapped_layout_cache_is_bounded_and_discards_old_revisions_and_text() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        for width in [80.0, 100.0, 120.0, 140.0, 160.0, 180.0] {
            galley_pass(&ctx, &cache, 1, "a wrapping source document", 14.0, width);
        }
        {
            let state = cache.shared.state.lock().unwrap();
            let galleys = &state.documents[&1].galleys;
            assert_eq!(galleys.len(), MAX_GALLEYS_PER_DOCUMENT);
            assert!(
                galleys
                    .iter()
                    .all(|cached| cached.value.job.wrap.max_width >= 120.0)
            );
        }
        // The layouter can see changed text before its owner updates revision.
        galley_pass(&ctx, &cache, 1, "new buffer text", 14.0, 180.0);
        assert_eq!(
            cache.shared.state.lock().unwrap().documents[&1]
                .galleys
                .len(),
            1
        );
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            cache.galley(ui, 1, 1, "new buffer text", "", 14.0, Color32::WHITE, 120.0);
        });
        output.drop_without_applying_deltas();
        let state = cache.shared.state.lock().unwrap();
        let galleys = &state.documents[&1].galleys;
        assert_eq!(galleys.len(), 1);
        assert_eq!(galleys[0].revision, 1);
    }

    #[test]
    fn active_document_multiple_layouts_obey_the_global_memory_budget() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        for width in [80.0, 100.0, 120.0, 140.0] {
            galley_pass(&ctx, &cache, 1, "source", 14.0, width);
        }
        let mut state = cache.shared.state.lock().unwrap();
        for cached in &mut state.documents.get_mut(&1).unwrap().galleys {
            // Exercise accounting without allocating 128 MiB of test geometry.
            cached.bytes = MAX_GALLEY_BYTES;
        }
        trim(&mut state, 1);
        let entry = &state.documents[&1];
        assert_eq!(entry.galleys.len(), 2);
        assert!(document_bytes(entry) <= MAX_CACHE_BYTES);
        assert!(
            entry
                .galleys
                .iter()
                .all(|cached| cached.value.job.wrap.max_width >= 120.0)
        );
    }

    #[test]
    fn atlas_recreation_invalidates_every_wrapped_layout() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let narrow = galley_pass(&ctx, &cache, 1, "source text", 14.0, 80.0);
        let wide = galley_pass(&ctx, &cache, 1, "source text", 14.0, 160.0);
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .reverse();
        ctx.set_fonts(fonts);
        let replacement = galley_pass(&ctx, &cache, 1, "source text", 14.0, 80.0);
        assert!(!Arc::ptr_eq(&narrow, &replacement));
        assert_eq!(
            cache.shared.state.lock().unwrap().documents[&1]
                .galleys
                .len(),
            1
        );
        let replacement_wide = galley_pass(&ctx, &cache, 1, "source text", 14.0, 160.0);
        assert!(!Arc::ptr_eq(&wide, &replacement_wide));
    }

    #[test]
    fn shaped_glyphs_survive_frames_without_a_source_editor() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let initial = galley_pass(&ctx, &cache, 1, "source text\n🦀", 14.0, f32::INFINITY);
        for _ in 0..12 {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                cache.begin_frame(ui);
                ui.label("Rendered Markdown preview without source editor");
            });
            output.drop_without_applying_deltas();
        }
        let returned = galley_pass(&ctx, &cache, 1, "source text\n🦀", 14.0, f32::INFINITY);
        assert!(Arc::ptr_eq(&initial, &returned));
    }

    #[test]
    fn true_font_atlas_recreation_during_preview_invalidates_source_glyphs() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let initial = galley_pass(&ctx, &cache, 1, "source text", 14.0, f32::INFINITY);
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .reverse();
        ctx.set_fonts(fonts);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            cache.begin_frame(ui);
            ui.label("Preview while fonts change");
        });
        output.drop_without_applying_deltas();
        let returned = galley_pass(&ctx, &cache, 1, "source text", 14.0, f32::INFINITY);
        assert!(!Arc::ptr_eq(&initial, &returned));
    }

    #[test]
    fn shaped_glyphs_invalidate_for_changed_text_font_wrap_and_context() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let initial = galley_pass(&ctx, &cache, 1, "first document", 14.0, f32::INFINITY);
        let edited = galley_pass(&ctx, &cache, 1, "other document", 14.0, f32::INFINITY);
        assert!(!Arc::ptr_eq(&initial, &edited));
        let resized = galley_pass(&ctx, &cache, 1, "other document", 18.0, f32::INFINITY);
        assert!(!Arc::ptr_eq(&edited, &resized));
        let wrapped = galley_pass(&ctx, &cache, 1, "other document", 18.0, 40.0);
        assert!(!Arc::ptr_eq(&resized, &wrapped));
        let another_ctx = egui::Context::default();
        let recreated = galley_pass(&another_ctx, &cache, 1, "other document", 18.0, 40.0);
        assert!(!Arc::ptr_eq(&wrapped, &recreated));
    }

    #[test]
    fn shaped_glyphs_invalidate_when_font_atlas_is_recreated() {
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        let initial = galley_pass(&ctx, &cache, 1, "first document", 14.0, f32::INFINITY);
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .reverse();
        ctx.set_fonts(fonts);
        let recreated = galley_pass(&ctx, &cache, 1, "first document", 14.0, f32::INFINITY);
        assert!(!Arc::ptr_eq(&initial, &recreated));
    }

    #[test]
    fn indexed_lines_count_unicode_and_crlf_like_editor() {
        let index = LineIndex::default();
        let source = "ä🦀\r\n中\r\nx\n";
        let expected = [
            (1, 1),
            (1, 2),
            (1, 3),
            (1, 3),
            (2, 1),
            (2, 2),
            (2, 2),
            (3, 1),
            (3, 2),
            (4, 1),
        ];
        for (cursor, expected) in expected.into_iter().enumerate() {
            assert_eq!(
                index.line_col(source, 0, cursor),
                expected,
                "cursor {cursor}"
            );
        }
        assert_eq!(index.line_col(source, 0, usize::MAX), (4, 1));
        assert_eq!(index.line_col("", 1, 999), (1, 1));
    }

    #[test]
    fn line_index_rebuilds_on_revision_and_explicit_reload() {
        let index = LineIndex::default();
        assert_eq!(index.line_col("xx\nx", 0, 3), (2, 1));
        assert_eq!(index.line_col("x\nxx", 1, 3), (2, 2));
        index.invalidate();
        assert_eq!(index.line_col("xxxx", 1, 3), (1, 4));
    }

    #[test]
    fn indexed_positions_and_line_ends_are_unicode_and_break_aware() {
        let index = LineIndex::default();
        let text = "ä🦀\r\n中\nx\rq\n";
        assert_eq!(index.position(text, 0, 1, 1), Some(0));
        assert_eq!(index.position(text, 0, 1, 3), Some(2));
        assert_eq!(index.position(text, 0, 1, 4), None);
        assert_eq!(index.position(text, 0, 2, 1), Some(4));
        assert_eq!(index.position(text, 0, 2, 2), Some(5));
        assert_eq!(index.position(text, 0, 3, 4), Some(9));
        assert_eq!(index.position(text, 0, 4, 1), Some(10));
        assert_eq!(index.position(text, 0, 4, 2), None);
        assert_eq!(index.position(text, 0, 5, 1), None);
        assert_eq!(index.position(text, 0, 0, 1), None);
        assert_eq!(index.position(text, 0, 1, 0), None);
        assert_eq!(index.line_end(text, 0, 1), Some(2));
        assert_eq!(index.line_end(text, 0, 2), Some(5));
        assert_eq!(index.line_end(text, 0, 3), Some(9));
        assert_eq!(index.line_end(text, 0, 4), Some(10));
        assert_eq!(index.line_end(text, 0, 5), None);
        assert_eq!(index.line_end("", 1, 1), Some(0));
        assert_eq!(index.position("", 1, 1, 1), Some(0));
    }

    #[test]
    fn first_display_is_immediate_plain_text_then_syntax_arrives() {
        let cache = SyntaxCache::new();
        let source = "fn main() { println!(\"Grüße 🦀\"); }\n";
        let initial = layout(&cache, 1, 0, source, 16.0);
        assert_eq!(initial.text, source);
        assert_eq!(initial.sections.len(), 1);
        settle(&cache, 1, source, "rs", 16.0);
        let ready = layout(&cache, 1, 0, source, 16.0);
        assert!(ready.sections.len() > 1);
        assert!(
            ready
                .sections
                .iter()
                .all(|section| section.format.font_id.size == 16.0)
        );
    }

    #[test]
    fn colors_survive_inactive_tab_frames() {
        let cache = SyntaxCache::new();
        let first = "fn first() -> usize { 42 }\n";
        let second = "fn second() -> bool { true }\n";
        layout(&cache, 1, 0, first, 14.0);
        settle(&cache, 1, first, "rs", 14.0);
        layout(&cache, 2, 0, second, 14.0);
        settle(&cache, 2, second, "rs", 14.0);
        for _ in 0..20 {
            layout(&cache, 2, 0, second, 14.0);
        }
        assert!(layout(&cache, 1, 0, first, 14.0).sections.len() > 1);
        assert!(
            cache.shared.state.lock().unwrap().documents[&1]
                .pending
                .is_none()
        );
    }

    #[test]
    fn same_revision_changed_text_never_reuses_wrong_sections() {
        let cache = SyntaxCache::new();
        let first = "fn first() {}\n";
        let second = "// edited before revision increment\nfn second() {}\n";
        layout(&cache, 1, 0, first, 14.0);
        settle(&cache, 1, first, "rs", 14.0);
        let initial = layout(&cache, 1, 0, second, 14.0);
        assert_eq!(initial.text, second);
        assert!(
            initial
                .sections
                .iter()
                .all(|section| section.byte_range.end.0 <= second.len())
        );
        settle(&cache, 1, second, "rs", 14.0);
        assert_eq!(layout(&cache, 1, 1, second, 14.0).text, second);
    }

    #[test]
    fn only_latest_edit_is_installed_and_font_and_wrap_are_current() {
        let cache = SyntaxCache::new();
        for revision in 0..40 {
            layout(
                &cache,
                1,
                revision,
                &format!("fn value() -> u64 {{ {revision} }}\n"),
                14.0,
            );
        }
        let final_source = "fn value() -> u64 { 39 }\n";
        settle(&cache, 1, final_source, "rs", 14.0);
        let initial = layout(&cache, 1, 40, final_source, 20.0);
        assert!(
            initial
                .sections
                .iter()
                .all(|section| section.format.font_id.size == 20.0)
        );
        settle(&cache, 1, final_source, "rs", 20.0);
        let wrapped = cache.layout(
            &egui::Context::default(),
            1,
            40,
            final_source,
            "rs",
            20.0,
            Color32::WHITE,
            310.0,
        );
        assert_eq!(wrapped.wrap.max_width, 310.0);
        assert!(
            wrapped
                .sections
                .iter()
                .all(|section| section.format.font_id.size == 20.0)
        );
    }

    #[test]
    fn cache_and_work_queue_have_document_bounds() {
        let cache = SyntaxCache::new();
        for document in 0..100 {
            layout(&cache, document, 0, "fn main() {}\n", 14.0);
        }
        let state = cache.shared.state.lock().unwrap();
        assert!(state.documents.len() <= MAX_DOCUMENTS);
        assert!(state.documents.contains_key(&99));
    }

    #[test]
    fn huge_files_skip_parser_but_preserve_text_and_font() {
        let cache = SyntaxCache::new();
        let source = "x".repeat(MAX_DOCUMENT_BYTES + 1);
        let result = layout(&cache, 1, 0, &source, 17.0);
        assert_eq!(result.text, source);
        assert_eq!(result.sections.len(), 1);
        assert!(cache.shared.state.lock().unwrap().documents.is_empty());
    }

    #[test]
    fn typing_keeps_unchanged_token_colors_until_new_parse_arrives() {
        let cache = SyntaxCache::new();
        let first = "fn main() { let value = 1; }\n";
        let edited = "fn main() { let value = 12; }\n";
        layout(&cache, 1, 0, first, 14.0);
        settle(&cache, 1, first, "rs", 14.0);
        let old = layout(&cache, 1, 0, first, 14.0);
        let preview = layout(&cache, 1, 1, edited, 14.0);
        assert_eq!(preview.text, edited);
        assert_eq!(
            preview.sections[0].format.color,
            old.sections[0].format.color
        );
        assert_eq!(
            preview.sections.last().unwrap().format.color,
            old.sections.last().unwrap().format.color
        );
        assert!(preview.sections.len() > 1);
    }

    #[test]
    fn carried_colors_use_complete_current_utf8_ranges_for_inserts_and_deletes() {
        let source = "fn ä() { \"🦀\" }\n";
        let mut old = LayoutJob {
            text: source.into(),
            ..Default::default()
        };
        append_section(
            &mut old,
            0,
            3,
            egui::TextFormat {
                font_id: FontId::monospace(14.0),
                color: Color32::RED,
                ..Default::default()
            },
        );
        append_section(
            &mut old,
            3,
            source.len(),
            egui::TextFormat {
                font_id: FontId::monospace(14.0),
                color: Color32::BLUE,
                ..Default::default()
            },
        );
        for edited in [
            "fn ä() { \"🦀\"; }\n",
            "fn ä() { \"🐙\" }\n",
            "fn ö() { \"🦀\" }\n",
            "fn ä() { }\n",
            "🦀fn ä() { \"🦀\" }\n",
            "",
        ] {
            let preview = carry_colors(&old, edited, 14.0, Color32::WHITE, 250.0);
            assert_eq!(preview.text, edited);
            assert_eq!(preview.wrap.max_width, 250.0);
            let mut end = 0;
            for section in &preview.sections {
                assert_eq!(section.byte_range.start.0, end, "gap in {edited:?}");
                assert!(edited.is_char_boundary(section.byte_range.start.0));
                assert!(edited.is_char_boundary(section.byte_range.end.0));
                assert!(section.byte_range.start.0 < section.byte_range.end.0);
                end = section.byte_range.end.0;
            }
            assert_eq!(end, edited.len());
        }
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn rust_highlight_latency() {
        let source = include_str!("vim.rs");
        assert!(source.len() <= MAX_DOCUMENT_BYTES);
        let parser_ctx = egui::Context::default();
        let settings = SyntectSettings::default();
        let theme = CodeTheme::dark(14.0);
        let style = egui::Style {
            override_font_id: Some(FontId::monospace(14.0)),
            ..Default::default()
        };
        let started = Instant::now();
        for _ in 0..5 {
            let result = syntax_highlighting::highlight_with(
                &parser_ctx,
                &style,
                &theme,
                source,
                "rs",
                &settings,
            );
            assert_eq!(result.text, source);
            parser_ctx.memory_mut(|memory| memory.caches = Default::default());
        }
        let sync_parse = started.elapsed().as_secs_f64() * 1000.0 / 5.0;
        let cache = SyntaxCache::new();
        let ui_ctx = egui::Context::default();
        let started = Instant::now();
        cache.layout(
            &ui_ctx,
            1,
            0,
            source,
            "rs",
            14.0,
            Color32::WHITE,
            f32::INFINITY,
        );
        let initial = started.elapsed().as_secs_f64() * 1000.0;
        settle(&cache, 1, source, "rs", 14.0);
        let started = Instant::now();
        for _ in 0..100 {
            assert!(
                cache
                    .layout(
                        &ui_ctx,
                        1,
                        0,
                        source,
                        "rs",
                        14.0,
                        Color32::WHITE,
                        f32::INFINITY
                    )
                    .sections
                    .len()
                    > 1
            );
        }
        let cached = started.elapsed().as_secs_f64() * 1000.0 / 100.0;
        println!(
            "Rust source {} bytes: synchronous reparse {sync_parse:.3} ms, immediate display request {initial:.3} ms, cached tab return {cached:.3} ms",
            source.len()
        );
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn rust_tab_render_latency() {
        let source = include_str!("vim.rs");
        let cache = SyntaxCache::new();
        let ctx = egui::Context::default();
        cache.layout(
            &ctx,
            1,
            0,
            source,
            "rs",
            14.0,
            Color32::WHITE,
            f32::INFINITY,
        );
        settle(&cache, 1, source, "rs", 14.0);
        let job = cache.layout(
            &ctx,
            1,
            0,
            source,
            "rs",
            14.0,
            Color32::WHITE,
            f32::INFINITY,
        );
        let mut original = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            original = Some(ui.fonts_mut(|fonts| fonts.layout_job(job.clone())));
        });
        output.drop_without_applying_deltas();
        let original = original.unwrap();
        let mut uncached_ms = 0.0;
        for _ in 0..5 {
            galley_pass(
                &ctx,
                &SyntaxCache::new(),
                2,
                "another tab",
                14.0,
                f32::INFINITY,
            );
            galley_pass(
                &ctx,
                &SyntaxCache::new(),
                2,
                "another tab",
                14.0,
                f32::INFINITY,
            );
            let started = Instant::now();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let returned = ui.fonts_mut(|fonts| fonts.layout_job(job.clone()));
                assert!(!Arc::ptr_eq(&original, &returned));
            });
            output.drop_without_applying_deltas();
            uncached_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            cache.galley(ui, 1, 0, source, "rs", 14.0, Color32::WHITE, f32::INFINITY);
        });
        output.drop_without_applying_deltas();
        let mut cached_ms = 0.0;
        for _ in 0..20 {
            galley_pass(&ctx, &cache, 2, "another tab", 14.0, f32::INFINITY);
            galley_pass(&ctx, &cache, 2, "another tab", 14.0, f32::INFINITY);
            let started = Instant::now();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                cache.galley(ui, 1, 0, source, "rs", 14.0, Color32::WHITE, f32::INFINITY);
            });
            output.drop_without_applying_deltas();
            cached_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        println!(
            "Rust {} bytes, shaped glyph data {:.2} MiB: tab glyph reflow {:.3} ms, persistent cached tab return {:.3} ms (includes layout job lookup/clone, frame setup; excludes tessellation)",
            source.len(),
            galley_bytes(&original) as f64 / 1024.0 / 1024.0,
            uncached_ms / 5.0,
            cached_ms / 20.0
        );
    }
}
