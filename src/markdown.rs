//! Cached CommonMark preview. HTML stays inert and images never fetch remotely.

use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::{
    self, Align, Color32, FontFamily, FontId, Layout, RichText, Sense, Stroke, TextFormat, Vec2,
};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::palette::{
    ACCENT, BLUE, BORDER, BORDER_STRONG, HOVER, MUTED, PANEL as SURFACE, SURFACE as INLINE_CODE,
    TEXT as INK,
};
const CODE_PADDING: f32 = 2.0;
const MAX_DOCUMENTS: usize = 12;
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 1024 * 1024;

#[cfg(test)]
thread_local! {
    static INLINE_LAYOUT_REQUESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

// Keep the same UI IDs so persisted scroll and text-selection state stays attached.
fn with_reading_scrollbars<R>(ui: &mut egui::Ui, show: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let previous_scroll = ui.spacing().scroll;
    let previous_track = ui.visuals().extreme_bg_color;
    ui.spacing_mut().scroll = egui::style::ScrollStyle {
        foreground_color: true,
        ..egui::style::ScrollStyle::solid()
    };
    ui.visuals_mut().extreme_bg_color = BORDER;
    let result = show(ui);
    ui.spacing_mut().scroll = previous_scroll;
    ui.visuals_mut().extreme_bg_color = previous_track;
    result
}

#[derive(Default)]
pub(crate) struct PreviewResponse {
    pub open_file: Option<PathBuf>,
    pub source_line: Option<usize>,
}

#[derive(Default)]
pub(crate) struct MarkdownCache {
    documents: VecDeque<CachedDocument>,
    reveal: Option<(u64, usize)>,
    restore_scroll: HashMap<(egui::Id, u64), f32>,
}

#[derive(Clone)]
struct CachedDocument {
    id: u64,
    pane: egui::Id,
    revision: u64,
    text: Arc<str>,
    source_len: usize,
    document: Arc<Document>,
    heights: Vec<Option<f32>>,
    width: f32,
    font_size: f32,
    fragment: Option<String>,
    scroll_y: f32,
    content_height: f32,
    pending_g: Option<f64>,
    table_scroll: HashMap<usize, f32>,
    restore_tables: HashMap<usize, f32>,
    table_layouts: HashMap<usize, TableGeometry>,
    font_probe: Option<Arc<egui::Galley>>,
}

#[derive(Debug)]
struct Document {
    blocks: Vec<Block>,
    line_starts: Vec<usize>,
    truncated: bool,
}

#[derive(Debug)]
struct Block {
    source: Range<usize>,
    kind: BlockKind,
}

#[derive(Debug)]
enum BlockKind {
    Paragraph(Vec<Span>),
    Heading {
        level: u8,
        spans: Vec<Span>,
        anchor: String,
    },
    Code {
        language: String,
        text: String,
    },
    Quote(Vec<Block>),
    Footnote {
        anchor: String,
        children: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        alignments: Vec<Alignment>,
        headers: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Rule,
}

#[derive(Clone, Debug, Default)]
struct Span {
    text: String,
    strong: bool,
    emphasis: bool,
    strike: bool,
    code: bool,
    link: Option<String>,
    image: bool,
    task: Option<bool>,
}

impl MarkdownCache {
    /// Carry a moved view's measured layout and reading position to its new pane.
    /// The editor only calls this when the target does not already own that tab.
    pub(crate) fn transfer_view(&mut self, document: u64, source: egui::Id, destination: egui::Id) {
        if source == destination {
            return;
        }
        if let Some(mut entry) = self
            .documents
            .iter()
            .find(|entry| entry.id == document && entry.pane == source)
            .cloned()
        {
            self.documents
                .retain(|cached| cached.id != document || cached.pane != destination);
            entry.pane = destination;
            entry.pending_g = None;
            entry.restore_tables = entry.table_scroll.clone();
            self.restore_scroll
                .insert((destination, document), entry.scroll_y);
            self.documents.push_front(entry);
            self.trim_documents();
        }
    }
    pub(crate) fn view_scroll(&self, document: u64, pane: egui::Id) -> Option<f32> {
        self.documents
            .iter()
            .find(|entry| entry.id == document && entry.pane == pane)
            .map(|entry| entry.scroll_y)
    }
    pub(crate) fn restore_view_scroll(&mut self, document: u64, pane: egui::Id, offset: f32) {
        if offset.is_finite() && offset >= 0.0 {
            self.restore_scroll.insert((pane, document), offset);
        }
    }
    pub(crate) fn forget_document(&mut self, document: u64) {
        self.documents.retain(|entry| entry.id != document);
        self.restore_scroll.retain(|(_, id), _| *id != document);
        if self.reveal.is_some_and(|(id, _)| id == document) {
            self.reveal = None;
        }
    }
    pub(crate) fn forget_view_restore(&mut self, document: u64, pane: egui::Id) {
        self.restore_scroll.remove(&(pane, document));
    }
    pub(crate) fn forget_pane_restores(&mut self, pane: egui::Id) {
        self.restore_scroll.retain(|(id, _), _| *id != pane);
        // Cached layouts stay bounded by the LRU and can still supply a queued
        // tab transfer whose destination pane has not rendered yet.
    }
    #[cfg(test)]
    pub(crate) fn table_scroll(&self, document: u64, pane: egui::Id, source: usize) -> Option<f32> {
        self.documents
            .iter()
            .find(|entry| entry.id == document && entry.pane == pane)
            .and_then(|entry| entry.table_scroll.get(&source).copied())
    }
    fn trim_documents(&mut self) {
        while self.documents.len() > MAX_DOCUMENTS
            || (self.documents.len() > 1
                && self
                    .documents
                    .iter()
                    .map(|entry| {
                        entry.text.len()
                            + entry
                                .table_layouts
                                .values()
                                .map(|geometry| {
                                    (geometry.widths.capacity() + geometry.offsets.capacity())
                                        * std::mem::size_of::<f32>()
                                })
                                .sum::<usize>()
                    })
                    .sum::<usize>()
                    > MAX_CACHE_BYTES)
        {
            self.documents.pop_back();
        }
        // A restored inactive tab need not have a cached layout yet. Its
        // pending position belongs to the view lifecycle, not the LRU.
    }
    /// Reveal a one-based source line in the corresponding preview document.
    /// The request is consumed only when that document is rendered.
    pub(crate) fn reveal_line(&mut self, document: u64, line: usize) {
        self.reveal = Some((document, line.max(1)));
    }

    /// Preview owns a separate scroll position for each tab. Double-click any
    /// rendered block to return to its source; ordinary text remains selectable.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        revision: u64,
        text: &str,
        source_path: &Path,
        font_size: f32,
        smooth_scroll: bool,
        keyboard_active: bool,
        swap_jk: bool,
    ) -> PreviewResponse {
        let pane = ui.id();
        let existing = self
            .documents
            .iter()
            .position(|entry| entry.id == id && entry.pane == pane);
        let mut entry = existing.and_then(|index| self.documents.remove(index));
        let preview_text = preview_prefix(text);
        if entry.as_ref().is_none_or(|entry| {
            entry.revision != revision
                || entry.source_len != text.len()
                || entry.text.as_ref() != preview_text
        }) {
            let shared = self.documents.iter().find(|entry| {
                entry.id == id
                    && entry.revision == revision
                    && entry.source_len == text.len()
                    && entry.text.as_ref() == preview_text
            });
            let (cached_text, document) = if let Some(shared) = shared {
                (shared.text.clone(), shared.document.clone())
            } else {
                (Arc::<str>::from(preview_text), Arc::new(parse(text)))
            };
            entry = Some(CachedDocument {
                id,
                pane,
                revision,
                text: cached_text,
                source_len: text.len(),
                heights: vec![None; document.blocks.len()],
                document,
                width: 0.0,
                font_size: 0.0,
                fragment: None,
                scroll_y: 0.0,
                content_height: 0.0,
                pending_g: None,
                table_scroll: HashMap::new(),
                restore_tables: HashMap::new(),
                table_layouts: HashMap::new(),
                font_probe: None,
            });
        }
        let mut entry = entry.expect("created preview document");
        // Keep the probe alive for every preview frame, even while a particular
        // table is off screen, and invalidate measured heights after font/DPI changes.
        let probe = ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "vedit markdown font generation".into(),
                FontId::monospace(1.0),
                Color32::WHITE,
            )
        });
        if entry
            .font_probe
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &probe))
        {
            entry.heights.fill(None);
            entry.table_layouts.clear();
            entry.font_probe = Some(probe);
        }
        let restored_scroll = self.restore_scroll.remove(&(pane, id));
        if let Some(offset) = restored_scroll {
            entry.scroll_y = offset;
        }
        let reveal_target = self
            .reveal
            .filter(|(document, _)| *document == id)
            .and_then(|(_, line)| {
                self.reveal = None;
                let byte = entry
                    .document
                    .line_starts
                    .get(line.saturating_sub(1))
                    .copied()
                    .unwrap_or_else(|| {
                        entry
                            .document
                            .line_starts
                            .last()
                            .copied()
                            .unwrap_or_default()
                    });
                let index = entry
                    .document
                    .blocks
                    .iter()
                    .position(|block| byte < block.source.end)
                    .or_else(|| entry.document.blocks.len().checked_sub(1))?;
                Some((index, innermost_source(&entry.document.blocks[index], byte)))
            });
        let mut result = PreviewResponse::default();
        let available = ui.available_width();
        let width = (available - 48.0).clamp(120.0, 840.0).min(available);
        let font_size = (font_size + 1.5).clamp(14.0, 22.0);
        if (entry.width - width).abs() > 0.5 || entry.font_size != font_size {
            entry.heights.fill(None);
            entry.width = width;
            entry.font_size = font_size;
        }
        let pending_fragment = entry.fragment.take();
        let mut next_fragment = None;
        let viewport_height = ui.available_height();
        let requested_scroll = if keyboard_active
            && ui.is_enabled()
            && ui.ctx().memory(|memory| memory.focused().is_none())
            && !egui::Popup::is_any_open(ui.ctx())
        {
            ui.input_mut(|input| {
                let mut requested = None;
                let down = if swap_jk { egui::Key::K } else { egui::Key::J };
                let up = if swap_jk { egui::Key::J } else { egui::Key::K };
                if input.consume_key(egui::Modifiers::NONE, down) {
                    requested = Some(entry.scroll_y + font_size * 2.0);
                    entry.pending_g = None;
                }
                if input.consume_key(egui::Modifiers::NONE, up) {
                    requested = Some((entry.scroll_y - font_size * 2.0).max(0.0));
                    entry.pending_g = None;
                }
                if input.consume_key(egui::Modifiers::CTRL, egui::Key::D) {
                    requested = Some(entry.scroll_y + viewport_height * 0.5);
                    entry.pending_g = None;
                }
                if input.consume_key(egui::Modifiers::CTRL, egui::Key::U) {
                    requested = Some((entry.scroll_y - viewport_height * 0.5).max(0.0));
                    entry.pending_g = None;
                }
                if input.consume_key(egui::Modifiers::SHIFT, egui::Key::G) {
                    requested = Some((entry.content_height - viewport_height).max(0.0));
                    entry.pending_g = None;
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::G) {
                    if entry.pending_g.is_some_and(|time| input.time - time < 1.0) {
                        requested = Some(0.0);
                        entry.pending_g = None;
                    } else {
                        entry.pending_g = Some(input.time);
                    }
                }
                requested
            })
        } else {
            None
        };
        let scroll_salt = ("markdown_preview", pane, id);
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(scroll_salt)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .animated(smooth_scroll)
            .scroll_source(egui::scroll_area::ScrollSource {
                mouse_wheel: smooth_scroll,
                ..Default::default()
            })
            .auto_shrink([false, false]);
        if !smooth_scroll
            && let Some(offset) = crate::render::instant_scroll_offset_axes(
                ui,
                ui.make_persistent_id(egui::IdSalt::new(scroll_salt)),
                egui::Vec2b::new(false, true),
            )
        {
            scroll = scroll.scroll_offset(offset);
        }
        if let Some(offset) = restored_scroll {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        if let Some(offset) = requested_scroll {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        let output = with_reading_scrollbars(ui, |ui| {
            scroll.show(ui, |ui| {
                ui.add_space(26.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(((available - width) * 0.5).max(0.0));
                    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::LEFT), |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if entry.document.truncated {
                            egui::Frame::new().fill(SURFACE).corner_radius(6.0).inner_margin(12.0).show(ui, |ui| {
                                ui.label(RichText::new("Large document: preview shows the first 1 MiB. Open Source to read the complete file.").size(12.0).color(MUTED));
                            });
                            ui.add_space(18.0);
                        }
                        if entry.document.blocks.is_empty() {
                            ui.add_space(20.0);
                            ui.label(RichText::new("This Markdown document is empty.").size(15.0).color(MUTED));
                        }
                        for (index, block) in entry.document.blocks.iter().enumerate() {
                            let start = ui.cursor().top();
                            let anchored = pending_fragment.as_ref().is_some_and(|fragment| block_has_anchor(block, fragment));
                            let outside = entry.heights[index].is_some_and(|height| {
                                let rect = egui::Rect::from_min_size(ui.cursor().min, Vec2::new(width, height));
                                !ui.clip_rect().expand(120.0).intersects(rect)
                            });
                            if outside && !anchored && !reveal_target.is_some_and(|(target, _)| target == index) {
                                ui.allocate_exact_size(Vec2::new(width, entry.heights[index].unwrap_or_default()), Sense::hover());
                            } else {
                                ui.push_id(index, |ui| {
                                    let mut renderer = Renderer {
                                        source_path, font_size, line_starts: &entry.document.line_starts,
                                        document_start: index == 0 && !entry.document.truncated,
                                        result: &mut result, next_fragment: &mut next_fragment,
                                        pending_fragment: pending_fragment.as_deref(),
                                        reveal_source: reveal_target.map(|(_, source)| source),
                                        markdown_source: text,
                                        smooth_scroll,
                                        table_scroll: &mut entry.table_scroll,
                                        restore_tables: &mut entry.restore_tables,
                                        table_layouts: &mut entry.table_layouts,
                                    };
                                    renderer.block(ui, block, false);
                                });
                                entry.heights[index] = Some((ui.cursor().top() - start).max(0.0));
                            }
                        }
                        ui.add_space(44.0);
                    });
                });
            })
        });
        crate::render::remember_scroll_limits(ui.ctx(), &output);
        entry.scroll_y = output.state.offset.y;
        entry.content_height = output.content_size.y;
        entry.fragment = next_fragment;
        if entry.fragment.is_some() {
            ui.ctx().request_repaint();
        }
        self.documents.push_front(entry);
        self.trim_documents();
        result
    }
}

fn parse(text: &str) -> Document {
    let input = preview_prefix(text);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES;
    let events = Parser::new_ext(input, options)
        .into_offset_iter()
        .map(|(event, range)| (event.into_static(), range))
        .collect();
    let mut parser = Blocks { events, cursor: 0 };
    let mut blocks = parser.blocks(None);
    let mut anchors = std::collections::HashMap::<String, usize>::new();
    assign_anchors(&mut blocks, &mut anchors);
    let mut line_starts = vec![0];
    line_starts.extend(
        input
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    Document {
        blocks,
        line_starts,
        truncated: input.len() < text.len(),
    }
}

fn preview_prefix(text: &str) -> &str {
    let mut end = text.len().min(MAX_PREVIEW_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

struct Blocks {
    events: Vec<(Event<'static>, Range<usize>)>,
    cursor: usize,
}

impl Blocks {
    fn blocks(&mut self, until: Option<TagEnd>) -> Vec<Block> {
        let mut blocks = Vec::new();
        while self.cursor < self.events.len() {
            let (event, source) = self.events[self.cursor].clone();
            if let Event::End(end) = event
                && Some(end) == until
            {
                self.cursor += 1;
                break;
            }
            let kind = match event {
                Event::Start(Tag::Paragraph) => {
                    self.cursor += 1;
                    Some(BlockKind::Paragraph(self.inline(Some(TagEnd::Paragraph))))
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    self.cursor += 1;
                    Some(BlockKind::Heading {
                        level: heading_level(level),
                        spans: self.inline(Some(TagEnd::Heading(level))),
                        anchor: String::new(),
                    })
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.cursor += 1;
                    let language = match kind {
                        CodeBlockKind::Fenced(language) => language
                            .split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .into(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    let mut text = String::new();
                    while let Some((event, _)) = self.events.get(self.cursor) {
                        if *event == Event::End(TagEnd::CodeBlock) {
                            self.cursor += 1;
                            break;
                        }
                        if let Event::Text(value) | Event::Code(value) = event {
                            text.push_str(value);
                        }
                        self.cursor += 1;
                    }
                    Some(BlockKind::Code { language, text })
                }
                Event::Start(Tag::BlockQuote(kind)) => {
                    self.cursor += 1;
                    Some(BlockKind::Quote(
                        self.blocks(Some(TagEnd::BlockQuote(kind))),
                    ))
                }
                Event::Start(Tag::List(start)) => {
                    self.cursor += 1;
                    let mut items = Vec::new();
                    while self.cursor < self.events.len() {
                        match self.events[self.cursor].0 {
                            Event::Start(Tag::Item) => {
                                self.cursor += 1;
                                items.push(self.blocks(Some(TagEnd::Item)));
                            }
                            Event::End(TagEnd::List(_)) => {
                                self.cursor += 1;
                                break;
                            }
                            _ => {
                                self.cursor += 1;
                            }
                        }
                    }
                    Some(BlockKind::List { start, items })
                }
                Event::Start(Tag::Table(alignments)) => {
                    self.cursor += 1;
                    let mut headers = Vec::new();
                    let mut rows = Vec::new();
                    let mut row = Vec::new();
                    let mut in_header = false;
                    while self.cursor < self.events.len() {
                        match self.events[self.cursor].0 {
                            Event::Start(Tag::TableHead) => {
                                in_header = true;
                                self.cursor += 1;
                            }
                            Event::Start(Tag::TableCell) => {
                                self.cursor += 1;
                                row.push(self.inline(Some(TagEnd::TableCell)));
                            }
                            Event::End(TagEnd::TableHead) => {
                                headers = std::mem::take(&mut row);
                                in_header = false;
                                self.cursor += 1;
                            }
                            Event::End(TagEnd::TableRow) => {
                                if !in_header {
                                    rows.push(std::mem::take(&mut row));
                                }
                                self.cursor += 1;
                            }
                            Event::End(TagEnd::Table) => {
                                self.cursor += 1;
                                break;
                            }
                            _ => {
                                self.cursor += 1;
                            }
                        }
                    }
                    Some(BlockKind::Table {
                        alignments,
                        headers,
                        rows,
                    })
                }
                Event::Start(Tag::HtmlBlock) => {
                    self.cursor += 1;
                    let spans = self.inline(Some(TagEnd::HtmlBlock));
                    Some(BlockKind::Paragraph(spans))
                }
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    self.cursor += 1;
                    let mut children = self.blocks(Some(TagEnd::FootnoteDefinition));
                    children.insert(
                        0,
                        Block {
                            source: source.clone(),
                            kind: BlockKind::Paragraph(vec![Span {
                                text: format!("[{label}]"),
                                strong: true,
                                ..Default::default()
                            }]),
                        },
                    );
                    Some(BlockKind::Footnote {
                        anchor: format!("fn-{label}"),
                        children,
                    })
                }
                Event::Rule => {
                    self.cursor += 1;
                    Some(BlockKind::Rule)
                }
                Event::Text(_)
                | Event::Code(_)
                | Event::TaskListMarker(_)
                | Event::Start(
                    Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. },
                ) => Some(BlockKind::Paragraph(self.inline(None))),
                _ => {
                    self.cursor += 1;
                    None
                }
            };
            if let Some(kind) = kind {
                let end = self
                    .events
                    .get(self.cursor.saturating_sub(1))
                    .map_or(source.end, |(_, range)| range.end);
                blocks.push(Block {
                    source: source.start..end.max(source.end),
                    kind,
                });
            }
        }
        blocks
    }

    fn inline(&mut self, until: Option<TagEnd>) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut strong: usize = 0;
        let mut emphasis: usize = 0;
        let mut strike: usize = 0;
        let mut links = Vec::<String>::new();
        let mut image = false;
        while self.cursor < self.events.len() {
            let event = self.events[self.cursor].0.clone();
            if let Event::End(end) = event
                && Some(end) == until
            {
                self.cursor += 1;
                break;
            }
            let mut span = Span {
                strong: strong > 0,
                emphasis: emphasis > 0,
                strike: strike > 0,
                link: links.last().cloned(),
                image,
                ..Default::default()
            };
            match event {
                Event::Text(text) => span.text = text.into_string(),
                Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                    span.text = text.into_string();
                    span.code = true;
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    span.text = text.into_string();
                    span.code = true;
                }
                Event::FootnoteReference(label) => {
                    span.text = format!("[{label}]");
                    span.link = Some(format!("#fn-{label}"));
                }
                Event::SoftBreak => span.text = " ".into(),
                Event::HardBreak => span.text = "\n".into(),
                Event::TaskListMarker(checked) => {
                    span.task = Some(checked);
                    span.text = if checked { "☑  " } else { "☐  " }.into();
                }
                Event::Start(Tag::Strong) => strong += 1,
                Event::End(TagEnd::Strong) => strong = strong.saturating_sub(1),
                Event::Start(Tag::Emphasis) => emphasis += 1,
                Event::End(TagEnd::Emphasis) => emphasis = emphasis.saturating_sub(1),
                Event::Start(Tag::Strikethrough) => strike += 1,
                Event::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
                Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url.into_string()),
                Event::End(TagEnd::Link) => {
                    links.pop();
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    links.push(dest_url.into_string());
                    image = true;
                    spans.push(Span {
                        text: "Image: ".into(),
                        image: true,
                        link: links.last().cloned(),
                        ..Default::default()
                    });
                }
                Event::End(TagEnd::Image) => {
                    links.pop();
                    image = false;
                }
                _ => break,
            }
            self.cursor += 1;
            if !span.text.is_empty() {
                spans.push(span);
            }
        }
        spans
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn slug(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                Some(character)
            } else if character.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

fn assign_anchors(blocks: &mut [Block], seen: &mut std::collections::HashMap<String, usize>) {
    for block in blocks {
        match &mut block.kind {
            BlockKind::Heading { spans, anchor, .. } => {
                let base = slug(
                    &spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                );
                let mut count = seen.get(&base).copied().unwrap_or_default();
                let mut candidate = if count == 0 {
                    base.clone()
                } else {
                    format!("{base}-{count}")
                };
                while seen.contains_key(&candidate) {
                    count += 1;
                    candidate = format!("{base}-{count}");
                }
                seen.insert(base, count + 1);
                // Generated suffixes must also reserve their anchor, since a
                // later heading can contain that suffix in its own text.
                seen.entry(candidate.clone()).or_insert(1);
                *anchor = candidate;
            }
            BlockKind::Quote(children) | BlockKind::Footnote { children, .. } => {
                assign_anchors(children, seen)
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    assign_anchors(item, seen);
                }
            }
            _ => {}
        }
    }
}

fn block_has_anchor(block: &Block, fragment: &str) -> bool {
    match &block.kind {
        BlockKind::Heading { anchor, .. } => anchor == fragment,
        BlockKind::Footnote { anchor, children } => {
            anchor == fragment
                || children
                    .iter()
                    .any(|block| block_has_anchor(block, fragment))
        }
        BlockKind::Quote(children) => children
            .iter()
            .any(|block| block_has_anchor(block, fragment)),
        BlockKind::List { items, .. } => items
            .iter()
            .flatten()
            .any(|block| block_has_anchor(block, fragment)),
        _ => false,
    }
}

fn innermost_source(block: &Block, byte: usize) -> usize {
    let child = match &block.kind {
        BlockKind::Quote(children) | BlockKind::Footnote { children, .. } => children
            .iter()
            .find(|child| byte < child.source.end)
            .or_else(|| children.last()),
        BlockKind::List { items, .. } => items
            .iter()
            .flatten()
            .find(|child| byte < child.source.end)
            .or_else(|| items.iter().flatten().last()),
        _ => None,
    };
    child.map_or(block.source.start, |child| innermost_source(child, byte))
}

struct Renderer<'a> {
    source_path: &'a Path,
    font_size: f32,
    line_starts: &'a [usize],
    /// The first block sits directly below the page's own top padding.
    document_start: bool,
    result: &'a mut PreviewResponse,
    next_fragment: &'a mut Option<String>,
    pending_fragment: Option<&'a str>,
    reveal_source: Option<usize>,
    markdown_source: &'a str,
    smooth_scroll: bool,
    table_scroll: &'a mut HashMap<usize, f32>,
    restore_tables: &'a mut HashMap<usize, f32>,
    table_layouts: &'a mut HashMap<usize, TableGeometry>,
}

struct InlineLayout {
    galley: Arc<egui::Galley>,
    badges: Arc<Vec<egui::Rect>>,
    links: Vec<(Range<usize>, String)>,
}

#[derive(Clone)]
struct InlineGeometry {
    original: Arc<egui::Galley>,
    galley: Arc<egui::Galley>,
    badges: Arc<Vec<egui::Rect>>,
}

// Font layout already has a cache. Retain the small placement/decorations pass
// too, keyed by that exact layout so font changes cannot reuse stale geometry.
#[derive(Clone, Default)]
struct InlineGeometryCache {
    entries: VecDeque<InlineGeometry>,
    bytes: usize,
}

fn inline_geometry_bytes(entry: &InlineGeometry) -> usize {
    entry.original.num_vertices * std::mem::size_of::<egui::epaint::Vertex>()
        + entry.original.num_indices * std::mem::size_of::<u32>()
        + entry
            .original
            .rows
            .iter()
            .map(|row| row.glyphs.len() * std::mem::size_of::<egui::epaint::text::Glyph>())
            .sum::<usize>()
        + entry.original.rows.len() * std::mem::size_of::<egui::epaint::text::PlacedRow>() * 2
        + entry.original.job.text.len()
        + entry.badges.len() * std::mem::size_of::<egui::Rect>()
}

impl InlineGeometryCache {
    fn get_or_insert(
        &mut self,
        original: Arc<egui::Galley>,
        code: &[Range<usize>],
        line_height: f32,
    ) -> InlineGeometry {
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| Arc::ptr_eq(&entry.original, &original))
        {
            return entry.clone();
        }
        let entry = inline_geometry(original, code, line_height);
        let bytes = inline_geometry_bytes(&entry);
        if bytes <= 2 * 1024 * 1024 {
            self.bytes += bytes;
            self.entries.push_front(entry.clone());
            while self.entries.len() > 128 || self.bytes > 2 * 1024 * 1024 {
                if let Some(removed) = self.entries.pop_back() {
                    self.bytes -= inline_geometry_bytes(&removed);
                }
            }
        }
        entry
    }
}

fn inline_geometry(
    original: Arc<egui::Galley>,
    code: &[Range<usize>],
    line_height: f32,
) -> InlineGeometry {
    let mut galley = (*original).clone();
    let inset = match galley.job.halign {
        Align::LEFT => CODE_PADDING,
        Align::Center => 0.0,
        Align::RIGHT => -CODE_PADDING,
    };
    let mut badges = Vec::new();
    let mut character = 0;
    let mut range_index = 0;
    let mut mesh_bounds = egui::Rect::NOTHING;
    for (index, row) in galley.rows.iter_mut().enumerate() {
        // Ascent-compensated native layout aligns glyphs AND decorations. A
        // code-only wrapped row is shorter; move that row, never its mesh, so
        // every line still has the same baseline and breathing room.
        row.pos.y = 1.0 + index as f32 * line_height + (line_height - row.size.y).max(0.0);
        row.pos.x += inset;
        let mut badge: Option<egui::Rect> = None;
        for glyph in &row.glyphs {
            while code
                .get(range_index)
                .is_some_and(|range| character >= range.end)
            {
                if let Some(rect) = badge.take() {
                    badges.push(rect.expand2(egui::vec2(CODE_PADDING, 1.0)));
                }
                range_index += 1;
            }
            if code
                .get(range_index)
                .is_some_and(|range| range.contains(&character))
            {
                let rect = egui::Rect::from_min_size(
                    row.pos
                        + egui::vec2(
                            glyph.pos.x,
                            glyph.pos.y
                                - glyph.font_face_ascent
                                - 0.5 * (glyph.font_height - glyph.font_face_height),
                        ),
                    egui::vec2(glyph.advance_width, glyph.font_height),
                );
                badge = Some(badge.map_or(rect, |previous| previous.union(rect)));
            } else if let Some(rect) = badge.take() {
                badges.push(rect.expand2(egui::vec2(CODE_PADDING, 1.0)));
            }
            character += 1;
        }
        if let Some(rect) = badge {
            badges.push(rect.expand2(egui::vec2(CODE_PADDING, 1.0)));
        }
        if row.ends_with_newline {
            character += 1;
        }
        mesh_bounds |= row.visuals.mesh_bounds.translate(row.pos.to_vec2());
    }
    let width = original.rect.width() + CODE_PADDING * 2.0;
    let left = -galley.job.halign.to_factor() * width;
    galley.rect = egui::Rect::from_min_size(
        egui::pos2(left, 0.0),
        egui::vec2(width, galley.rows.len() as f32 * line_height + 2.0),
    );
    galley.mesh_bounds = mesh_bounds;
    InlineGeometry {
        original,
        galley: Arc::new(galley),
        badges: Arc::new(badges),
    }
}

struct TableRowLayout {
    cells: Vec<InlineLayout>,
    height: f32,
}

/// Retain only small row geometry. Glyphs, links and selectable widgets belong
/// to the visible rows rather than every cell of a long table on every frame.
#[derive(Clone)]
struct TableGeometry {
    available: f32,
    size: f32,
    widths: Vec<f32>,
    offsets: Vec<f32>,
}

fn table_column_widths(
    headers: &[Vec<Span>],
    rows: &[Vec<Vec<Span>>],
    size: f32,
    available: f32,
) -> Vec<f32> {
    let columns = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or_default())
        .max(1);
    let mut minimums = Vec::with_capacity(columns);
    let mut preferred = Vec::with_capacity(columns);
    for column in 0..columns {
        let header_length = headers.get(column).map_or(0, |spans| {
            spans
                .iter()
                .map(|span| span.text.chars().count())
                .sum::<usize>()
        });
        let mut longest = header_length;
        let mut total = 0;
        let mut count = 0;
        for row in rows.iter().take(128) {
            if let Some(spans) = row.get(column) {
                let length = spans
                    .iter()
                    .map(|span| span.text.chars().count())
                    .sum::<usize>();
                longest = longest.max(length);
                total += length.min(400);
                count += 1;
            }
        }
        let minimum = (header_length as f32 * size * 0.58)
            .clamp(88.0, 170.0)
            .max(if longest > 70 { 140.0 } else { 88.0 });
        let average = if count == 0 {
            header_length as f32
        } else {
            total as f32 / count as f32
        };
        let desired = (average * size * 0.55).clamp(minimum, 460.0);
        minimums.push(minimum);
        preferred.push((desired - minimum).max(1.0));
    }
    let padding = columns as f32 * 24.0;
    let content = (available - padding - 2.0).max(minimums.iter().sum());
    let extra = (content - minimums.iter().sum::<f32>()).max(0.0);
    let total_preferred = preferred.iter().sum::<f32>();
    minimums
        .into_iter()
        .zip(preferred)
        .map(|(minimum, preferred)| minimum + extra * preferred / total_preferred)
        .collect()
}

impl Renderer<'_> {
    fn table_row(
        &self,
        ui: &egui::Ui,
        widths: &[f32],
        alignments: &[Alignment],
        row: &[Vec<Span>],
        size: f32,
        header: bool,
    ) -> TableRowLayout {
        let cells = widths
            .iter()
            .enumerate()
            .map(|(column, width)| {
                let align = match alignments.get(column) {
                    Some(Alignment::Center) => Align::Center,
                    Some(Alignment::Right) => Align::RIGHT,
                    _ => Align::LEFT,
                };
                self.shape_inline(
                    ui,
                    row.get(column).map_or(&[], Vec::as_slice),
                    size,
                    header,
                    *width,
                    align,
                )
            })
            .collect::<Vec<_>>();
        let height = cells
            .iter()
            .map(|cell| cell.galley.size().y)
            .fold(size * 1.55, f32::max)
            + 24.0;
        TableRowLayout { cells, height }
    }

    fn table(
        &mut self,
        ui: &mut egui::Ui,
        alignments: &[Alignment],
        headers: &[Vec<Span>],
        rows: &[Vec<Vec<Span>>],
        source: usize,
    ) {
        let size = (self.font_size - 1.0).max(12.0);
        let available = ui.available_width();
        let geometry = self
            .table_layouts
            .remove(&source)
            .filter(|cached| cached.available == available && cached.size == size)
            .unwrap_or_else(|| {
                let widths = table_column_widths(headers, rows, size, available);
                let mut offsets = Vec::with_capacity(rows.len() + 2);
                offsets.push(0.0);
                for (index, row) in std::iter::once(headers)
                    .chain(rows.iter().map(Vec::as_slice))
                    .enumerate()
                {
                    let layout = self.table_row(ui, &widths, alignments, row, size, index == 0);
                    offsets.push(offsets.last().copied().unwrap_or_default() + layout.height);
                }
                TableGeometry {
                    available,
                    size,
                    widths,
                    offsets,
                }
            });
        let widths = &geometry.widths;
        let table_width = widths.iter().sum::<f32>() + widths.len() as f32 * 24.0 + 2.0;
        let table_height = geometry.offsets.last().copied().unwrap_or_default();
        let salt = ("markdown_table", ui.id(), source);
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new(salt));
        let mut scroll = egui::ScrollArea::horizontal()
            .id_salt(salt)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .auto_shrink([false, true])
            .animated(self.smooth_scroll)
            .scroll_source(egui::scroll_area::ScrollSource {
                mouse_wheel: self.smooth_scroll,
                ..Default::default()
            });
        if let Some(offset) = self.restore_tables.remove(&source) {
            // Tables use nested UI IDs, which change when a view moves. Seed
            // their own scroll state before wheel handling in the new pane.
            let mut state = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
            state.offset.x = offset;
            state.store(ui.ctx(), scroll_id);
            scroll = scroll.horizontal_scroll_offset(offset);
        }
        if !self.smooth_scroll {
            // The centered Markdown column starts with a zero requested height.
            // Its content grows during layout, so use the measured table extent
            // when deciding who owns an immediate horizontal wheel event.
            let wheel_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("table_wheel", source))
                    .max_rect(egui::Rect::from_min_size(
                        ui.cursor().min,
                        Vec2::new(ui.available_width(), table_height + 14.0),
                    )),
            );
            if let Some(offset) = crate::render::instant_scroll_offset_axes(
                &wheel_ui,
                scroll_id,
                egui::Vec2b::new(true, false),
            ) {
                scroll = scroll.scroll_offset(offset);
            }
        }
        let output = with_reading_scrollbars(ui, |ui| {
            scroll.show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let (table_rect, _) =
                    ui.allocate_exact_size(Vec2::new(table_width, table_height), Sense::hover());
                let first = geometry
                    .offsets
                    .partition_point(|offset| table_rect.top() + offset <= ui.clip_rect().top())
                    .saturating_sub(1)
                    .min(rows.len() + 1);
                let last = geometry
                    .offsets
                    .partition_point(|offset| table_rect.top() + offset < ui.clip_rect().bottom())
                    .min(rows.len() + 1);
                for row_index in first..last {
                    let height = geometry.offsets[row_index + 1] - geometry.offsets[row_index];
                    let row_rect = egui::Rect::from_min_size(
                        table_rect.left_top() + Vec2::new(0.0, geometry.offsets[row_index]),
                        Vec2::new(table_width, height),
                    );
                    if !ui.is_rect_visible(row_rect) {
                        continue;
                    }
                    let row = if row_index == 0 {
                        headers
                    } else {
                        &rows[row_index - 1]
                    };
                    let cells = self
                        .table_row(ui, widths, alignments, row, size, row_index == 0)
                        .cells;
                    let fill = if row_index == 0 {
                        crate::palette::SURFACE
                    } else if row_index % 2 == 1 {
                        SURFACE
                    } else {
                        Color32::TRANSPARENT
                    };
                    if fill != Color32::TRANSPARENT {
                        ui.painter().rect_filled(
                            row_rect,
                            if row_index == 0 {
                                egui::CornerRadius {
                                    nw: 5,
                                    ne: 5,
                                    sw: 0,
                                    se: 0,
                                }
                            } else {
                                egui::CornerRadius::ZERO
                            },
                            fill,
                        );
                    }
                    if row_index > 0 {
                        ui.painter().line_segment(
                            [row_rect.left_top(), row_rect.right_top()],
                            Stroke::new(1.0, BORDER.gamma_multiply(0.75)),
                        );
                    }
                    let mut left = row_rect.left() + 13.0;
                    for (column, (width, cell)) in widths.iter().zip(cells).enumerate() {
                        let cell_rect = egui::Rect::from_min_size(
                            egui::pos2(left, row_rect.top() + 12.0),
                            Vec2::new(*width, height - 24.0),
                        );
                        let align = cell.galley.job.halign;
                        // A row's selectable text keeps its identity when rows
                        // above it enter or leave the virtualized viewport.
                        let cell_id =
                            ui.make_persistent_id(("table_cell", source, row_index, column));
                        let mut cell_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .id(cell_id)
                                .max_rect(cell_rect)
                                .layout(Layout::top_down(align)),
                        );
                        cell_ui.set_clip_rect(cell_rect.intersect(ui.clip_rect()));
                        self.paint_inline(&mut cell_ui, cell, source);
                        left += width + 24.0;
                    }
                }
                ui.painter().rect_stroke(
                    table_rect,
                    6.0,
                    Stroke::new(1.0, BORDER),
                    egui::StrokeKind::Inside,
                );
            })
        });
        crate::render::remember_scroll_limits(ui.ctx(), &output);
        self.table_scroll.insert(source, output.state.offset.x);
        self.table_layouts.insert(source, geometry);
    }

    fn block(&mut self, ui: &mut egui::Ui, block: &Block, compact: bool) {
        let footnote_target = matches!(&block.kind, BlockKind::Footnote { anchor, .. }
            if self.pending_fragment == Some(anchor.as_str()));
        if self.reveal_source == Some(block.source.start) || footnote_target {
            let rendered = ui.scope(|ui| self.block_contents(ui, block, compact));
            rendered.response.scroll_to_me(Some(Align::TOP));
        } else {
            self.block_contents(ui, block, compact);
        }
    }

    fn block_contents(&mut self, ui: &mut egui::Ui, block: &Block, compact: bool) {
        match &block.kind {
            BlockKind::Paragraph(spans) => {
                self.inline(ui, spans, self.font_size, false, block.source.start);
                ui.add_space(if compact { 5.0 } else { 14.0 });
            }
            BlockKind::Heading {
                level,
                spans,
                anchor,
            } => {
                // Headings structure the text without dominating it: a modest
                // scale in the semibold face, and a rule only below the title.
                if !self.document_start {
                    ui.add_space(12.0);
                }
                let size = self.font_size
                    * match level {
                        1 => 1.5,
                        2 => 1.25,
                        3 => 1.1,
                        _ => 1.0,
                    };
                let response = self.inline(ui, spans, size, true, block.source.start);
                if self.pending_fragment == Some(anchor.as_str()) {
                    response.scroll_to_me(Some(Align::TOP));
                }
                if *level == 1 {
                    ui.add_space(4.0);
                    ui.separator();
                }
                ui.add_space(6.0);
            }
            BlockKind::Code { language, text } => {
                let frame = egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(7.0)
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.set_width((ui.available_width() - 1.0).max(1.0));
                        if !language.is_empty() {
                            ui.label(RichText::new(language).size(10.5).color(MUTED));
                            ui.add_space(7.0);
                        }
                        let response = ui.add(
                            egui::Label::new(
                                RichText::new(text.trim_end_matches('\n'))
                                    .monospace()
                                    .size((self.font_size - 1.0).max(12.0))
                                    .color(INK),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                        self.source_response(&response, block.source.start, None);
                    });
                code_copy_button(ui, frame.response.rect, block.source.start, text);
                ui.add_space(16.0);
            }
            BlockKind::Quote(children) | BlockKind::Footnote { children, .. } => {
                let frame = egui::Frame::new()
                    .fill(SURFACE)
                    .corner_radius(5.0)
                    .inner_margin(egui::Margin {
                        left: 17,
                        right: 13,
                        top: 10,
                        bottom: 5,
                    });
                let response = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width().max(1.0));
                        for (index, child) in children.iter().enumerate() {
                            ui.push_id(index, |ui| self.block(ui, child, true));
                        }
                    })
                    .response;
                ui.painter().line_segment(
                    [
                        response.rect.left_top() + Vec2::new(2.0, 5.0),
                        response.rect.left_bottom() + Vec2::new(2.0, -5.0),
                    ],
                    Stroke::new(3.0, BORDER_STRONG),
                );
                ui.add_space(15.0);
            }
            BlockKind::List { start, items } => {
                for (index, item) in items.iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 9.0;
                            let is_task = item.first().is_some_and(|block| matches!(&block.kind, BlockKind::Paragraph(spans) if spans.first().is_some_and(|span| span.task.is_some())));
                            let marker = if is_task { String::new() } else { start.map_or_else(|| "•".into(), |start| format!("{}.", start + index as u64)) };
                            ui.add_sized([25.0, self.font_size * 1.5], egui::Label::new(RichText::new(marker).size(self.font_size).color(MUTED)));
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                for (child_index, child) in item.iter().enumerate() { ui.push_id(child_index, |ui| self.block(ui, child, true)); }
                            });
                        });
                    });
                }
                ui.add_space(if compact { 5.0 } else { 12.0 });
            }
            BlockKind::Table {
                alignments,
                headers,
                rows,
            } => {
                self.table(ui, alignments, headers, rows, block.source.start);
                ui.add_space(18.0);
            }
            BlockKind::Rule => {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(18.0);
            }
        }
    }

    fn inline(
        &mut self,
        ui: &mut egui::Ui,
        spans: &[Span],
        size: f32,
        bold: bool,
        source: usize,
    ) -> egui::Response {
        let inline = self.shape_inline(ui, spans, size, bold, ui.available_width(), Align::LEFT);
        self.paint_inline(ui, inline, source)
    }

    fn shape_inline(
        &self,
        ui: &egui::Ui,
        spans: &[Span],
        size: f32,
        bold: bool,
        width: f32,
        align: Align,
    ) -> InlineLayout {
        #[cfg(test)]
        INLINE_LAYOUT_REQUESTS.with(|requests| requests.set(requests.get() + 1));
        let mut job = egui::text::LayoutJob::default();
        let has_code = spans.iter().any(|span| span.code);
        job.wrap.max_width = (width - if has_code { CODE_PADDING * 2.0 } else { 0.0 }).max(1.0);
        job.halign = align;
        let strong_family = FontFamily::Name("vedit-semibold".into());
        let has_strong = ui.fonts_mut(|fonts| fonts.families().contains(&strong_family));
        let mut links = Vec::new();
        let mut code = Vec::new();
        let mut character = 0;
        let mut previous_code = false;
        let mut formats = Vec::with_capacity(spans.len());
        let mut metrics = Vec::<(FontId, f32, f32)>::new();
        for span in spans {
            let strong = bold || span.strong;
            let family = if span.code {
                FontFamily::Monospace
            } else if strong && has_strong {
                strong_family.clone()
            } else {
                FontFamily::Proportional
            };
            let color = if span.link.is_some() || span.task == Some(true) {
                BLUE
            } else if span.strike || span.image || span.task == Some(false) {
                MUTED
            } else {
                INK
            };
            let text_size = if span.code {
                (size - 1.0).max(11.0)
            } else if strong && !has_strong {
                size + 0.4
            } else {
                size
            };
            let font_id = FontId::new(text_size, family);
            if !metrics.iter().any(|(font, _, _)| *font == font_id) {
                let (ascent, height) = ui.fonts_mut(|fonts| {
                    let measured = fonts.layout_no_wrap("M".into(), font_id.clone(), INK);
                    let glyph = &measured.rows[0].glyphs[0];
                    (glyph.font_ascent, glyph.font_height)
                });
                metrics.push((font_id.clone(), ascent, height));
            }
            let format = TextFormat {
                font_id,
                color,
                italics: span.emphasis,
                strikethrough: if span.strike {
                    Stroke::new(1.0, MUTED)
                } else {
                    Stroke::NONE
                },
                ..Default::default()
            };
            formats.push(format);
        }
        let max_ascent = metrics
            .iter()
            .map(|(_, ascent, _)| *ascent)
            .fold(0.0, f32::max);
        let max_height = metrics
            .iter()
            .map(|(_, _, height)| *height)
            .fold(0.0, f32::max);
        let pixels = ui.ctx().pixels_per_point();
        // Enlarged headings use a tighter leading than reading text.
        let leading = if bold && size > self.font_size {
            1.3
        } else {
            1.55
        };
        let line_height = ((size * leading).max(max_height + 2.0) * pixels).ceil() / pixels;
        for (span, mut format) in spans.iter().zip(formats) {
            let ascent = metrics
                .iter()
                .find(|(font, _, _)| *font == format.font_id)
                .map_or(max_ascent, |(_, ascent, _)| *ascent);
            // Equal line heights make egui's vertical alignment inert: each
            // family then uses its own ascent, raising monospace above prose.
            // Compensate the native line height instead, keeping glyphs,
            // strikethrough and fallback fonts together on one baseline.
            format.line_height = Some(if has_code {
                line_height + ascent - max_ascent
            } else {
                line_height
            });
            // A little breathing room keeps inline code badges separate from
            // prose without changing the copied text or link coordinates.
            let leading = if !job.text.is_empty() && (span.code || previous_code) {
                2.0
            } else {
                0.0
            };
            job.append(&span.text, leading, format);
            previous_code = span.code;
            let end = character + span.text.chars().count();
            if let Some(link) = &span.link {
                links.push((character..end, link.clone()));
            }
            if span.code {
                code.push(character..end);
            }
            character = end;
        }
        let original = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let geometry = if has_code {
            ui.ctx().data_mut(|data| {
                data.get_temp_mut_or_default::<InlineGeometryCache>(egui::Id::new(
                    "markdown_inline_geometry",
                ))
                .get_or_insert(original, &code, line_height)
            })
        } else {
            InlineGeometry {
                galley: original.clone(),
                original,
                badges: Arc::new(Vec::new()),
            }
        };
        InlineLayout {
            galley: geometry.galley,
            badges: geometry.badges,
            links,
        }
    }

    fn paint_inline(
        &mut self,
        ui: &mut egui::Ui,
        inline: InlineLayout,
        source: usize,
    ) -> egui::Response {
        let InlineLayout {
            galley,
            badges,
            links,
        } = inline;
        // Reserve the background before Label paints its selection, then place
        // rounded badges using the Label's actual alignment and position.
        let badge_shape = ui.painter().add(egui::Shape::Noop);
        let mut response = ui.add(
            egui::Label::new(galley.clone())
                .selectable(true)
                .sense(Sense::click_and_drag() - Sense::FOCUSABLE),
        );
        let origin = match galley.job.halign {
            Align::LEFT => response.rect.left_top(),
            Align::Center => response.rect.center_top(),
            Align::RIGHT => response.rect.right_top(),
        };
        ui.painter().set(
            badge_shape,
            egui::Shape::Vec(
                badges
                    .iter()
                    .map(|rect| {
                        egui::Shape::rect_filled(rect.translate(origin.to_vec2()), 3.0, INLINE_CODE)
                    })
                    .collect(),
            ),
        );
        let mut hovered_link = None;
        let mut link_details = String::new();
        if let Some(pointer) = response.hover_pos() {
            let character = galley.cursor_from_pos(pointer - origin).index.0;
            if let Some((_, link)) = links.iter().find(|(range, _)| range.contains(&character)) {
                hovered_link = Some(link.clone());
                let action = link_action(link, self.source_path);
                link_details = match action {
                    LinkAction::Blocked => format!("Inactive link: {link}"),
                    LinkAction::File(ref path) => crate::paths::display(path),
                    _ => link.clone(),
                };
                if !matches!(action, LinkAction::Blocked) {
                    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        match action {
                            LinkAction::External(url) => {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(url))
                            }
                            LinkAction::File(path) => self.result.open_file = Some(path),
                            LinkAction::Fragment(fragment) => *self.next_fragment = Some(fragment),
                            LinkAction::Blocked => {}
                        }
                    }
                }
            }
        }
        self.source_response(&response, source, hovered_link.as_deref());
        if link_details.is_empty() {
            response
        } else {
            response.on_hover_text(link_details)
        }
    }

    fn source_response(&mut self, response: &egui::Response, byte: usize, link: Option<&str>) {
        let line = self
            .line_starts
            .partition_point(|start| *start <= byte)
            .max(1);
        if response.double_clicked() {
            self.result.source_line = Some(line);
        }
        let link_id = response.id.with("preview_context_link");
        if response.secondary_clicked() {
            response
                .ctx
                .data_mut(|data| data.insert_temp(link_id, link.map(str::to_owned)));
        }
        let context_link = if response.context_menu_opened() {
            response
                .ctx
                .data(|data| data.get_temp::<Option<String>>(link_id))
                .flatten()
        } else {
            link.map(str::to_owned)
        };
        response.context_menu(|ui| {
            if crate::design::menu_item(
                ui,
                "Open source here",
                "",
                "Switch to Source at this part of the document",
            )
            .clicked()
            {
                self.result.source_line = Some(line);
                ui.close();
            }
            ui.separator();
            if crate::design::menu_item(
                ui,
                "Copy document source",
                "",
                "Copy the complete Markdown document, including its formatting",
            )
            .clicked()
            {
                ui.ctx().copy_text(self.markdown_source.into());
                ui.close();
            }
            if let Some(link) = context_link.as_deref()
                && crate::design::menu_item(
                    ui,
                    "Copy link target",
                    "",
                    "Copy this link's destination",
                )
                .clicked()
            {
                ui.ctx().copy_text(link.into());
                ui.close();
            }
            if crate::design::menu_item(ui, "Copy document path", "", "Copy the full document path")
                .clicked()
            {
                ui.ctx().copy_text(crate::paths::display(self.source_path));
                ui.close();
            }
        });
    }
}

/// A code block's Copy action appears while the block is hovered. It stays in
/// the keyboard order and becomes visible whenever it holds focus.
fn code_copy_button(ui: &mut egui::Ui, block: egui::Rect, source: usize, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap("Copy".into(), FontId::proportional(12.0), INK);
    let size = Vec2::new(galley.size().x + 14.0, 22.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(block.right() - 7.0 - size.x, block.top() + 6.0),
        size,
    );
    let response = ui.interact(
        rect,
        ui.id().with(("markdown_code_copy", source)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Copy")
    });
    if ui.rect_contains_pointer(block) || response.has_focus() {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, 4.0, HOVER);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                4.0,
                Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let color = if response.hovered() || response.has_focus() {
            INK
        } else {
            MUTED
        };
        painter.galley_with_override_text_color(rect.center() - galley.size() * 0.5, galley, color);
    }
    if response.clicked() {
        ui.ctx().copy_text(text.to_owned());
        response.surrender_focus();
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Copy this code block");
}

#[derive(Debug, PartialEq, Eq)]
enum LinkAction {
    External(String),
    File(PathBuf),
    Fragment(String),
    Blocked,
}

fn link_action(link: &str, source_path: &Path) -> LinkAction {
    let lower = link.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
    {
        return LinkAction::External(link.into());
    }
    if let Some(fragment) = link.strip_prefix('#') {
        return LinkAction::Fragment(decode_url_path(fragment));
    }
    let native_drive = link.as_bytes().get(1) == Some(&b':')
        && link.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && link
            .as_bytes()
            .get(2)
            .is_some_and(|byte| *byte == b'\\' || *byte == b'/');
    if (link.contains(':') && !native_drive) || link.starts_with("//") || link.is_empty() {
        return LinkAction::Blocked;
    }
    let path = decode_url_path(link.split(['#', '?']).next().unwrap_or_default());
    if path.is_empty() {
        return LinkAction::Blocked;
    }
    let path = PathBuf::from(path);
    LinkAction::File(if path.is_absolute() {
        path
    } else {
        source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    })
}

fn decode_url_path(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let input = text.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%'
            && index + 2 < input.len()
            && let (Some(hi), Some(lo)) = (
                (input[index + 1] as char).to_digit(16),
                (input[index + 2] as char).to_digit(16),
            )
        {
            bytes.push((hi * 16 + lo) as u8);
            index += 3;
        } else {
            bytes.push(input[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_preserve_headings_tasks_nested_lists_and_tables() {
        let document = parse(
            "# Plan\n\n- [x] **Done**\n- [ ] Next\n  1. nested\n\n| File | Status |\n| :--- | ---: |\n| `app.rs` | ready |\n",
        );
        assert_eq!(document.blocks.len(), 3);
        assert!(
            matches!(&document.blocks[0].kind, BlockKind::Heading { level: 1, anchor, .. } if anchor == "plan")
        );
        let BlockKind::List { items, .. } = &document.blocks[1].kind else {
            panic!("list missing")
        };
        assert_eq!(items.len(), 2);
        assert!(
            matches!(&items[0][0].kind, BlockKind::Paragraph(spans) if spans[0].task == Some(true) && spans.iter().any(|span| span.strong))
        );
        assert!(
            items[1]
                .iter()
                .any(|block| matches!(&block.kind, BlockKind::List { start: Some(1), .. }))
        );
        assert!(
            matches!(&document.blocks[2].kind, BlockKind::Table { alignments, headers, rows } if alignments == &[Alignment::Left, Alignment::Right] && headers.len() == 2 && rows.len() == 1 && rows[0][0][0].code)
        );
    }

    #[test]
    fn fences_html_and_escaped_markup_remain_inert() {
        let document = parse(
            "```rust\nlet x = \"<script>\";\n```\n\n<script>alert('bad')</script>\n\n\\*literal\\* &amp; ~~removed~~\n",
        );
        assert!(
            matches!(&document.blocks[0].kind, BlockKind::Code { language, text } if language == "rust" && text.contains("<script>"))
        );
        assert!(
            matches!(&document.blocks[1].kind, BlockKind::Paragraph(spans) if spans.iter().all(|span| span.code) && spans[0].text.contains("<script>"))
        );
        assert!(
            matches!(&document.blocks[2].kind, BlockKind::Paragraph(spans) if spans.iter().any(|span| span.strike) && spans.iter().map(|span| span.text.as_str()).collect::<String>().starts_with("*literal* &"))
        );
    }

    #[test]
    fn links_are_explicit_and_dangerous_schemes_disabled() {
        let source = Path::new("C:/project/docs/plan.md");
        assert_eq!(
            link_action("https://example.com", source),
            LinkAction::External("https://example.com".into())
        );
        assert_eq!(
            link_action("#next-step", source),
            LinkAction::Fragment("next-step".into())
        );
        assert_eq!(
            link_action("../README%20new.md#intro", source),
            LinkAction::File(source.parent().unwrap().join("../README new.md"))
        );
        for blocked in [
            "javascript:alert(1)",
            "data:image/png;base64,a",
            "file:///C:/secrets.txt",
            "//example.com",
        ] {
            assert_eq!(link_action(blocked, source), LinkAction::Blocked);
        }
    }

    #[test]
    fn unicode_source_lines_and_repeated_heading_anchors_are_correct() {
        let document = parse("é\r\n\r\n# Next\r\n\r\n# Next\r\n");
        assert_eq!(
            document
                .line_starts
                .partition_point(|start| *start <= document.blocks[1].source.start),
            3
        );
        assert!(
            matches!(&document.blocks[2].kind, BlockKind::Heading { anchor, .. } if anchor == "next-1")
        );
    }

    #[test]
    fn generated_heading_anchors_do_not_collide_with_explicit_suffixes() {
        for (text, expected) in [
            ("# A\n\n# A-1\n\n# A\n", ["a", "a-1", "a-2"]),
            ("# A\n\n# A\n\n# A-1\n", ["a", "a-1", "a-1-1"]),
        ] {
            let document = parse(text);
            let anchors = document
                .blocks
                .iter()
                .filter_map(|block| match &block.kind {
                    BlockKind::Heading { anchor, .. } => Some(anchor.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(anchors, expected);
        }
    }

    #[test]
    fn large_preview_truncates_on_a_utf8_boundary_with_notice() {
        let text = "é".repeat(MAX_PREVIEW_BYTES);
        let document = parse(&text);
        assert!(document.truncated);
        assert!(!document.blocks.is_empty());
    }

    #[test]
    fn image_is_a_link_and_never_silently_downloaded() {
        let document = parse("![Architecture](https://example.com/diagram.png)");
        assert!(
            matches!(&document.blocks[0].kind, BlockKind::Paragraph(spans) if spans.iter().any(|span| span.image && span.link.as_deref() == Some("https://example.com/diagram.png")))
        );
    }

    #[test]
    fn cache_reuses_document_but_handles_changed_text_at_same_revision() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let mut original = None;
        for text in ["# First", "# First", "# Changed"] {
            let output = context.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    cache.show(
                        ui,
                        7,
                        3,
                        text,
                        Path::new("README.md"),
                        14.0,
                        false,
                        true,
                        false,
                    );
                });
            });
            output.drop_without_applying_deltas();
            assert_eq!(cache.documents.len(), 1);
            if text == "# First" {
                if let Some(original) = &original {
                    assert!(Arc::ptr_eq(original, &cache.documents[0].text));
                } else {
                    original = Some(cache.documents[0].text.clone());
                }
            }
        }
        assert_eq!(cache.documents[0].text.as_ref(), "# Changed");
    }

    #[test]
    fn closing_a_preview_view_clears_only_its_pending_restore() {
        let mut cache = MarkdownCache::default();
        let left = egui::Id::new("left");
        let right = egui::Id::new("right");
        cache.restore_view_scroll(1, left, 120.0);
        cache.restore_view_scroll(1, right, 240.0);
        cache.restore_view_scroll(2, left, 360.0);
        cache.restore_view_scroll(2, right, 480.0);
        cache.forget_view_restore(1, left);
        assert!(!cache.restore_scroll.contains_key(&(left, 1)));
        assert_eq!(cache.restore_scroll.get(&(right, 1)), Some(&240.0));
        cache.forget_pane_restores(left);
        assert!(!cache.restore_scroll.contains_key(&(left, 2)));
        assert_eq!(cache.restore_scroll.get(&(right, 2)), Some(&480.0));
        cache.forget_document(1);
        assert!(!cache.restore_scroll.contains_key(&(right, 1)));
        assert_eq!(cache.restore_scroll.get(&(right, 2)), Some(&480.0));
    }

    #[test]
    fn preview_vim_scrolls_and_respects_another_widgets_focus() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "# Section\n\nA paragraph with enough words to read.\n\n".repeat(40);
        let mut frame = 0;
        let mut render = |cache: &mut MarkdownCache, key: Option<(egui::Key, egui::Modifiers)>| {
            frame += 1;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Default::default(),
                    Vec2::new(640.0, 340.0),
                )),
                time: Some(frame as f64 * 0.1),
                ..Default::default()
            };
            if let Some((key, modifiers)) = key {
                input.events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
            }
            let output = context.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    cache.show(
                        ui,
                        1,
                        0,
                        &text,
                        Path::new("README.md"),
                        14.0,
                        false,
                        true,
                        false,
                    );
                });
            });
            output.drop_without_applying_deltas();
        };
        render(&mut cache, None);
        render(&mut cache, Some((egui::Key::J, egui::Modifiers::NONE)));
        let after_j = cache.documents[0].scroll_y;
        assert!(after_j > 20.0);
        render(&mut cache, Some((egui::Key::D, egui::Modifiers::CTRL)));
        assert!(cache.documents[0].scroll_y > after_j + 100.0);
        render(&mut cache, Some((egui::Key::G, egui::Modifiers::SHIFT)));
        assert!(cache.documents[0].scroll_y > 1000.0);
        render(&mut cache, Some((egui::Key::G, egui::Modifiers::NONE)));
        render(&mut cache, Some((egui::Key::G, egui::Modifiers::NONE)));
        assert_eq!(cache.documents[0].scroll_y, 0.0);
        context.memory_mut(|memory| memory.request_focus(egui::Id::new("command_palette_input")));
        render(&mut cache, Some((egui::Key::J, egui::Modifiers::NONE)));
        assert_eq!(cache.documents[0].scroll_y, 0.0);
    }

    fn preview_pass(
        context: &egui::Context,
        cache: &mut MarkdownCache,
        text: &str,
        time: f64,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Default::default(),
                Vec2::new(640.0, 340.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let output = context.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                cache.show(
                    ui,
                    1,
                    0,
                    text,
                    Path::new("README.md"),
                    14.0,
                    false,
                    true,
                    false,
                );
            });
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn clicking_selectable_preview_text_does_not_disable_vim_scroll() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "# Section\n\nA paragraph with enough words to read.\n\n".repeat(40);
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        let pos = egui::pos2(80.0, 60.0);
        preview_pass(
            &context,
            &mut cache,
            &text,
            0.1,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        preview_pass(
            &context,
            &mut cache,
            &text,
            0.2,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(context.memory(|memory| memory.focused().is_none()));
        preview_pass(
            &context,
            &mut cache,
            &text,
            0.3,
            vec![egui::Event::Key {
                key: egui::Key::J,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(cache.documents[0].scroll_y > 20.0);
    }

    #[test]
    fn source_line_request_reveals_a_paragraph_below_the_fold() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = (1..=70)
            .map(|line| format!("Paragraph {line}: review this step.\n\n"))
            .collect::<String>();
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        cache.reveal_line(1, 100);
        for pass in 1..=8 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        let entry = &cache.documents[0];
        let target_byte = entry.document.line_starts[99];
        let target = entry
            .document
            .blocks
            .iter()
            .position(|block| target_byte < block.source.end)
            .unwrap();
        let expected_top = 26.0
            + entry.heights[..target]
                .iter()
                .map(|height| height.unwrap())
                .sum::<f32>();
        assert!(entry.scroll_y > 1000.0);
        assert!(
            (entry.scroll_y - expected_top).abs() < 60.0,
            "scroll {} versus target {expected_top}",
            entry.scroll_y
        );
        assert!(cache.reveal.is_none());
    }

    #[test]
    fn source_line_request_reveals_an_item_inside_a_long_list() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = (1..=70)
            .map(|line| format!("- Item {line}: review this step.\n"))
            .collect::<String>();
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        cache.reveal_line(1, 50);
        for pass in 1..=8 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        assert!(cache.documents[0].scroll_y > 1000.0);
        assert!(cache.reveal.is_none());
    }

    #[test]
    fn footnote_references_reveal_definitions_below_the_fold() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = format!(
            "Reference[^note].\n\n{}\n[^note]: Footnote content.\n",
            "Another paragraph.\n\n".repeat(50)
        );
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        let entry = &mut cache.documents[0];
        assert!(
            entry
                .document
                .blocks
                .iter()
                .any(|block| block_has_anchor(block, "fn-note"))
        );
        entry.fragment = Some("fn-note".into());
        for pass in 1..=8 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        assert!(cache.documents[0].scroll_y > 1000.0);
        assert!(cache.documents[0].fragment.is_none());
    }

    fn text_bounds(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, output: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => output.push((
                    text.galley.job.text.clone(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, output);
                    }
                }
                _ => {}
            }
        }
        let mut output = Vec::new();
        for shape in shapes {
            collect(&shape.shape, &mut output);
        }
        output
    }

    fn reading_harness(
        size: [f32; 2],
        text: &str,
    ) -> egui_kittest::Harness<'static, MarkdownCache> {
        let text = text.to_owned();
        let screen = egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui_state(
                move |ui, cache: &mut MarkdownCache| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        cache.show(
                            ui,
                            1,
                            0,
                            &text,
                            Path::new("README.md"),
                            14.0,
                            false,
                            true,
                            false,
                        );
                    });
                },
                MarkdownCache::default(),
            );
        screen.ctx.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        screen
    }

    #[test]
    fn prose_has_no_tooltip_and_links_show_their_destination() {
        for (text, tooltip) in [("Ordinary prose", false), ("[Link](target.md)", true)] {
            let mut screen = reading_harness([700.0, 500.0], text);
            screen.run();
            let bounds = text_bounds(&screen.output().shapes)
                .into_iter()
                .find(|(painted, _)| painted == if tooltip { "Link" } else { text })
                .unwrap()
                .1;
            screen.hover_at(bounds.center());
            screen
                .ctx
                .all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
            screen.run();
            let has_tooltip = screen.ctx.memory(|memory| {
                memory
                    .areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == egui::Order::Tooltip)
            });
            assert_eq!(has_tooltip, tooltip, "{text}");
        }
    }

    #[test]
    fn code_copy_action_appears_on_block_hover_and_keyboard_focus() {
        use egui_kittest::kittest::Queryable;
        let mut screen = reading_harness([700.0, 500.0], "Intro\n\n```\nlet value = 1;\n```\n");
        let copy_painted = |screen: &egui_kittest::Harness<'_, MarkdownCache>| {
            text_bounds(&screen.output().shapes)
                .iter()
                .any(|(text, _)| text == "Copy")
        };
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(2);
        assert!(screen.query_by_label("Copy").is_some());
        assert!(!copy_painted(&screen), "Copy stays hidden at rest");
        let code = text_bounds(&screen.output().shapes)
            .into_iter()
            .find_map(|(text, rect)| (text == "let value = 1;").then_some(rect))
            .expect("code text is painted");
        screen.hover_at(code.center());
        screen.run_steps(2);
        assert!(copy_painted(&screen), "Hovering the block reveals Copy");
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(2);
        assert!(!copy_painted(&screen));

        for _ in 0..8 {
            if screen.get_by_label("Copy").is_focused() {
                break;
            }
            screen.key_press(egui::Key::Tab);
            screen.run_steps(2);
        }
        assert!(screen.get_by_label("Copy").is_focused());
        assert!(copy_painted(&screen), "Keyboard focus reveals Copy");
        // Press and release in one frame, so its output still holds the copy.
        screen
            .input_mut()
            .events
            .extend([true, false].map(|pressed| egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }));
        screen.step();
        let copied = screen
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.trim_end().to_owned()),
                _ => None,
            });
        assert_eq!(copied.as_deref(), Some("let value = 1;"));
    }

    fn reading_scroll_tracks(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
        fn collect(shape: &egui::Shape, clip: egui::Rect, tracks: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == BORDER => {
                    assert!(
                        clip.expand(0.1).contains_rect(rect.rect),
                        "the track must be painted within its viewport"
                    );
                    tracks.push(rect.rect);
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, clip, tracks);
                    }
                }
                _ => {}
            }
        }
        let mut tracks = Vec::new();
        for shape in shapes {
            collect(&shape.shape, shape.clip_rect, &mut tracks);
        }
        tracks
    }

    #[test]
    fn overflowing_preview_paints_a_compact_vertical_track_without_hover_and_keeps_scroll() {
        let text = "# Section\n\nA paragraph with enough words to read.\n\n".repeat(40);
        let mut screen = reading_harness([640.0, 340.0], &text);
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(4);
        let tracks = reading_scroll_tracks(&screen.output().shapes);
        assert_eq!(tracks.len(), 1);
        let track = tracks[0];
        assert!((5.0..=7.0).contains(&track.width()));
        assert!(track.height() > 250.0);
        assert!(
            track.right() > 620.0,
            "the track should stay beside the reading viewport"
        );
        let pane = screen.state().documents[0].pane;
        let document = screen.state().documents[0].document.clone();
        screen.state_mut().restore_view_scroll(1, pane, 92.0);
        screen.run_steps(3);
        assert_eq!(screen.state().view_scroll(1, pane), Some(92.0));
        assert!(Arc::ptr_eq(
            &document,
            &screen.state().documents[0].document
        ));
        assert_eq!(reading_scroll_tracks(&screen.output().shapes), tracks);
    }

    #[test]
    fn overflowing_table_paints_a_horizontal_track_without_hover_and_leaves_prose_below() {
        let text = "| Risk | Countermeasure | DoD |\n| --- | --- | --- |\n| A long risk description that needs room to wrap in a narrow pane. | A long countermeasure description that needs enough space too. | H1 |\n\nAfter the table.\n";
        let mut screen = reading_harness([320.0, 900.0], text);
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(4);
        let tracks = reading_scroll_tracks(&screen.output().shapes);
        assert_eq!(tracks.len(), 1);
        let track = tracks[0];
        assert!((5.0..=7.0).contains(&track.height()));
        assert!(track.width() > 200.0 && track.width() < 320.0);
        let after = text_bounds(&screen.output().shapes)
            .into_iter()
            .find(|(text, _)| text == "After the table.")
            .unwrap()
            .1;
        assert!(
            after.top() > track.bottom() + 10.0,
            "prose must follow the track, without an oversized gutter"
        );
        assert!(after.top() < track.bottom() + 45.0);
        assert_eq!(screen.state().documents[0].scroll_y, 0.0);
        screen.state_mut().documents[0]
            .restore_tables
            .insert(0, 60.0);
        screen.run_steps(3);
        assert_eq!(
            screen.state().documents[0].table_scroll.get(&0),
            Some(&60.0)
        );
        assert_eq!(screen.state().documents[0].scroll_y, 0.0);
        assert_eq!(reading_scroll_tracks(&screen.output().shapes), tracks);
    }

    #[test]
    fn fitting_preview_and_table_do_not_paint_scroll_tracks() {
        for text in [
            "# Brief plan\n\nRead this short plan.\n",
            "| File | Status |\n| --- | --- |\n| plan.md | Ready |\n\nAfter the table.\n",
        ] {
            let mut screen = reading_harness([960.0, 900.0], text);
            screen.hover_at(egui::pos2(-20.0, -20.0));
            screen.run_steps(4);
            assert!(reading_scroll_tracks(&screen.output().shapes).is_empty());
            assert_eq!(screen.state().documents[0].scroll_y, 0.0);
        }
    }

    #[test]
    fn wrapped_table_cells_fit_their_row_and_following_rows_never_overlap() {
        let risk = format!(
            "First risk: {}",
            "many words need room to wrap correctly. ".repeat(10)
        );
        let counter = format!(
            "Countermeasure: {}",
            "**Strong prose** with `a_long_inline_identifier` and more details. ".repeat(14)
        );
        let text = format!(
            "| Risk | Countermeasure | DoD |\n| :--- | :--- | ---: |\n| {risk} | {counter} | H1 |\n| SECOND ROW | Next action | H2 |\n"
        );
        for width in [320.0, 960.0] {
            let context = egui::Context::default();
            let mut cache = MarkdownCache::default();
            for pass in 0..2 {
                let output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            Default::default(),
                            Vec2::new(width, 2400.0),
                        )),
                        time: Some(pass as f64 * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            cache.show(
                                ui,
                                1,
                                0,
                                &text,
                                Path::new("README.md"),
                                14.0,
                                false,
                                true,
                                false,
                            );
                        });
                    },
                );
                let bounds = text_bounds(&output.shapes);
                let first = bounds
                    .iter()
                    .find(|(text, _)| text.starts_with("First risk:"))
                    .unwrap()
                    .1;
                let counter = bounds
                    .iter()
                    .find(|(text, _)| text.starts_with("Countermeasure:"))
                    .unwrap()
                    .1;
                let next = bounds
                    .iter()
                    .find(|(text, _)| text == "SECOND ROW")
                    .unwrap()
                    .1;
                assert!(first.height() > 45.0 && counter.height() > 100.0);
                assert!(
                    (first.top() - counter.top()).abs() < 1.0,
                    "cells must align at the top"
                );
                assert!(
                    next.top() >= first.bottom().max(counter.bottom()) + 16.0,
                    "width {width}: following row {:?} overlaps wrapped cells {:?} {:?}",
                    next,
                    first,
                    counter
                );
                let row_background = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect) if rect.fill == SURFACE => Some(rect.rect),
                        _ => None,
                    })
                    .find(|rect| rect.contains(first.center()))
                    .unwrap();
                assert!(row_background.top() <= first.top() - 10.0);
                assert!(
                    row_background.bottom() >= first.bottom().max(counter.bottom()) + 10.0,
                    "row stripe must contain all cell text"
                );
                if width == 320.0 {
                    assert!(
                        row_background.width() > width,
                        "narrow panes must scroll the table instead of crushing cells"
                    );
                }
                output.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn split_previews_share_parsing_but_keep_width_scroll_and_keyboard_independent() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "# Section\n\nA paragraph with enough words to read.\n\n".repeat(40);
        for pass in 0..3 {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        Vec2::new(950.0, 340.0),
                    )),
                    time: Some(pass as f64 * 0.1),
                    events: if pass == 2 {
                        vec![egui::Event::Key {
                            key: egui::Key::J,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let origin = ui.cursor().min;
                        for (label, left, width, active) in [
                            ("left_preview", 0.0, 320.0, false),
                            ("right_preview", 330.0, 600.0, true),
                        ] {
                            let mut pane = ui.new_child(
                                egui::UiBuilder::new()
                                    .id_salt(label)
                                    .max_rect(egui::Rect::from_min_size(
                                        origin + Vec2::new(left, 0.0),
                                        Vec2::new(width, 320.0),
                                    ))
                                    .layout(Layout::top_down(Align::LEFT)),
                            );
                            pane.set_clip_rect(pane.max_rect());
                            cache.show(
                                &mut pane,
                                1,
                                0,
                                &text,
                                Path::new("README.md"),
                                14.0,
                                false,
                                active,
                                false,
                            );
                        }
                    });
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(cache.documents.len(), 2);
        assert!(Arc::ptr_eq(
            &cache.documents[0].document,
            &cache.documents[1].document
        ));
        assert!(Arc::ptr_eq(
            &cache.documents[0].text,
            &cache.documents[1].text
        ));
        assert_ne!(cache.documents[0].pane, cache.documents[1].pane);
        let right = cache
            .documents
            .iter()
            .find(|entry| entry.width > 400.0)
            .unwrap();
        let left = cache
            .documents
            .iter()
            .find(|entry| entry.width < 400.0)
            .unwrap();
        assert!(right.scroll_y > 20.0);
        assert_eq!(left.scroll_y, 0.0);
    }

    #[test]
    fn preview_wheel_moves_on_the_same_frame_without_an_animation_tail() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "# Section\n\nA paragraph with enough words to read.\n\n".repeat(40);
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        preview_pass(
            &context,
            &mut cache,
            &text,
            0.1,
            vec![
                egui::Event::PointerMoved(egui::pos2(160.0, 180.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -90.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
        );
        assert_eq!(cache.documents[0].scroll_y, 90.0);
        for pass in 2..6 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        assert_eq!(cache.documents[0].scroll_y, 90.0);
    }

    #[test]
    fn narrow_table_claims_horizontal_wheel_without_moving_the_document() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "| Risk | Countermeasure | DoD |\n| --- | --- | --- |\n| Long risk description which should have room to wrap safely in a narrow pane. | Some extensive countermeasure details which also require enough width. | H1 |\n";
        let mut positions = Vec::new();
        for pass in 0..2 {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        Vec2::new(320.0, 900.0),
                    )),
                    time: Some(pass as f64 * 0.1),
                    events: if pass == 1 {
                        vec![
                            egui::Event::PointerMoved(egui::pos2(160.0, 120.0)),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0.0, -90.0),
                                modifiers: egui::Modifiers::SHIFT,
                                phase: egui::TouchPhase::Move,
                            },
                        ]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        cache.show(
                            ui,
                            1,
                            0,
                            text,
                            Path::new("README.md"),
                            14.0,
                            false,
                            true,
                            false,
                        );
                    });
                },
            );
            positions.push(
                text_bounds(&output.shapes)
                    .iter()
                    .find(|(text, _)| text == "Countermeasure")
                    .unwrap()
                    .1
                    .left(),
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            (positions[0] - positions[1] - 90.0).abs() < 1.0,
            "header positions {positions:?}"
        );
        assert_eq!(cache.documents[0].scroll_y, 0.0);
    }

    #[test]
    fn preview_link_context_menu_keeps_its_target_and_can_open_source() {
        use egui_kittest::{Harness, kittest::Queryable};
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
        };
        let cache = Rc::new(RefCell::new(MarkdownCache::default()));
        let opened = Rc::new(Cell::new(None));
        let result = opened.clone();
        let mut screen = Harness::builder()
            .with_size([640.0, 340.0])
            .build_ui(move |ui| {
                let response = cache.borrow_mut().show(
                    ui,
                    1,
                    0,
                    "[Reference](https://example.com/plan)",
                    Path::new("README.md"),
                    14.0,
                    false,
                    true,
                    false,
                );
                if let Some(line) = response.source_line {
                    result.set(Some(line));
                }
            });
        screen.run_steps(2);
        screen.get_by_label("Reference").click_secondary();
        screen.run_steps(2);
        screen.get_by_label_contains("Copy link target").hover();
        screen.run_steps(2);
        assert!(screen.query_by_label_contains("Copy link target").is_some());
        screen.get_by_label_contains("Open source here").click();
        screen.run_steps(2);
        assert_eq!(opened.get(), Some(1));
    }

    #[test]
    fn preview_wheel_accumulates_across_separate_frames_and_reverses_immediately() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "# Section\n\nA paragraph with enough words to read.\n\n".repeat(40);
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        let mut expected = 0.0;
        for (index, delta) in [-90.0, -55.0, 30.0, -75.0, 65.0].into_iter().enumerate() {
            let time = (index * 2 + 1) as f64 * 0.1;
            preview_pass(
                &context,
                &mut cache,
                &text,
                time,
                vec![
                    egui::Event::PointerMoved(egui::pos2(160.0, 180.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, delta),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                ],
            );
            expected -= delta;
            assert_eq!(
                cache.documents[0].scroll_y, expected,
                "wheel frame {index} must start at the previous frame's position"
            );
            preview_pass(&context, &mut cache, &text, time + 0.1, vec![]);
            assert_eq!(
                cache.documents[0].scroll_y, expected,
                "idle frame after wheel frame {index} must not continue moving"
            );
        }
    }

    #[test]
    fn preview_wheel_clamps_to_both_ends_and_can_leave_them_immediately() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = (1..=80)
            .map(|line| format!("Paragraph {line}: review this step.\n\n"))
            .collect::<String>();
        preview_pass(&context, &mut cache, &text, 0.0, vec![]);
        let mut frame = 0;
        let mut wheel = |cache: &mut MarkdownCache, delta| {
            frame += 1;
            preview_pass(
                &context,
                cache,
                &text,
                frame as f64 * 0.1,
                vec![
                    egui::Event::PointerMoved(egui::pos2(160.0, 180.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, delta),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                ],
            );
        };
        wheel(&mut cache, -1_000_000.0);
        let bottom = cache.documents[0].scroll_y;
        assert!(bottom > 1_000.0 && bottom < 10_000.0);
        wheel(&mut cache, -1_000_000.0);
        assert_eq!(cache.documents[0].scroll_y, bottom);
        wheel(&mut cache, 31.0);
        assert_eq!(cache.documents[0].scroll_y, bottom - 31.0);
        wheel(&mut cache, 1_000_000.0);
        assert_eq!(cache.documents[0].scroll_y, 0.0);
        wheel(&mut cache, 1_000_000.0);
        assert_eq!(cache.documents[0].scroll_y, 0.0);
        wheel(&mut cache, -47.0);
        assert_eq!(cache.documents[0].scroll_y, 47.0);
        preview_pass(&context, &mut cache, &text, 1.0, vec![]);
        assert_eq!(cache.documents[0].scroll_y, 47.0);
    }

    #[test]
    fn table_wheel_accumulates_and_clamps_without_vertical_motion() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "| Risk | Countermeasure | DoD |\n| --- | --- | --- |\n| Long risk description which should have room to wrap safely in a narrow pane. | Some extensive countermeasure details which also require enough width. | H1 |\n";
        let mut frame = 0;
        let mut render = |cache: &mut MarkdownCache, delta: Option<f32>| {
            frame += 1;
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        Vec2::new(320.0, 900.0),
                    )),
                    time: Some(frame as f64 * 0.1),
                    events: delta.map_or_else(Vec::new, |delta| {
                        vec![
                            egui::Event::PointerMoved(egui::pos2(160.0, 120.0)),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0.0, delta),
                                modifiers: egui::Modifiers::SHIFT,
                                phase: egui::TouchPhase::Move,
                            },
                        ]
                    }),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        cache.show(
                            ui,
                            1,
                            0,
                            text,
                            Path::new("README.md"),
                            14.0,
                            false,
                            true,
                            false,
                        );
                    });
                },
            );
            let header_x = text_bounds(&output.shapes)
                .iter()
                .find(|(text, _)| text == "Countermeasure")
                .expect("countermeasure header remains visible while the table scrolls")
                .1
                .left();
            output.drop_without_applying_deltas();
            assert_eq!(cache.documents[0].scroll_y, 0.0);
            header_x
        };
        let start = render(&mut cache, None);
        let mut expected = 0.0;
        for delta in [-30.0, -35.0, 20.0, -10.0] {
            expected -= delta;
            let header_x = render(&mut cache, Some(delta));
            assert!((start - header_x - expected).abs() < 1.0);
            let idle_x = render(&mut cache, None);
            assert!((idle_x - header_x).abs() < 1.0);
        }
        let right_end = render(&mut cache, Some(-1_000_000.0));
        assert!(start - right_end > expected);
        let at_right_end = render(&mut cache, Some(-1_000_000.0));
        assert!((at_right_end - right_end).abs() < 1.0);
        let reversed = render(&mut cache, Some(19.0));
        assert!((reversed - right_end - 19.0).abs() < 1.0);
        let left_end = render(&mut cache, Some(1_000_000.0));
        assert!((left_end - start).abs() < 1.0);
        let at_left_end = render(&mut cache, Some(1_000_000.0));
        assert!((at_left_end - start).abs() < 1.0);
        let forward = render(&mut cache, Some(-13.0));
        assert!((start - forward - 13.0).abs() < 1.0);
        let idle = render(&mut cache, None);
        assert!((idle - forward).abs() < 1.0);
    }

    fn measured_inline(
        context: &egui::Context,
        text: &str,
        width: f32,
        size: f32,
        align: Align,
    ) -> InlineLayout {
        let document = parse(text);
        let BlockKind::Paragraph(spans) = &document.blocks[0].kind else {
            panic!("paragraph required")
        };
        let mut result = PreviewResponse::default();
        let mut next = None;
        let mut tables = HashMap::new();
        let mut restore = HashMap::new();
        let mut table_layouts = HashMap::new();
        let mut measured = None;
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            let renderer = Renderer {
                source_path: Path::new("README.md"),
                font_size: size,
                line_starts: &document.line_starts,
                document_start: false,
                result: &mut result,
                next_fragment: &mut next,
                pending_fragment: None,
                reveal_source: None,
                markdown_source: text,
                smooth_scroll: false,
                table_scroll: &mut tables,
                restore_tables: &mut restore,
                table_layouts: &mut table_layouts,
            };
            measured = Some(renderer.shape_inline(ui, spans, size, false, width, align));
        });
        output.drop_without_applying_deltas();
        measured.unwrap()
    }

    fn effective_baseline(
        row: &egui::epaint::text::PlacedRow,
        glyph: &egui::epaint::text::Glyph,
    ) -> f32 {
        row.pos.y + glyph.pos.y
            - glyph.font_face_ascent
            - 0.5 * (glyph.font_height - glyph.font_face_height)
            + glyph.font_ascent
    }

    #[test]
    fn inline_code_shares_the_prose_baseline_and_wraps_without_tall_backgrounds() {
        let text = "Before **bold prose** and `é🙂漢字 very_long_inline_identifier_without_breaks` then ~~`old_value`~~ after.\n\n";
        for size in [12.0, 18.0, 24.0] {
            for width in [100.0, 240.0, 900.0] {
                let context = egui::Context::default();
                let inline = measured_inline(&context, text, width, size, Align::LEFT);
                let row_height =
                    (inline.galley.rect.height() - 2.0) / inline.galley.rows.len() as f32;
                let first_row = &inline.galley.rows[0];
                let baseline = effective_baseline(first_row, &first_row.glyphs[0]);
                for (index, row) in inline.galley.rows.iter().enumerate() {
                    for glyph in &row.glyphs {
                        assert!(
                            (effective_baseline(row, glyph) - baseline - index as f32 * row_height)
                                .abs()
                                <= 1.1,
                            "mixed font baseline drift: size {size}, width {width}, row {index}, glyph {}",
                            glyph.chr
                        );
                    }
                }
                assert!(!inline.badges.is_empty());
                for badge in inline.badges.iter() {
                    assert!(
                        inline.galley.rect.expand(0.6).contains_rect(*badge),
                        "badge clips outside the text allocation: {badge:?} vs {:?}",
                        inline.galley.rect
                    );
                    assert!(
                        badge.height() < row_height,
                        "code background must fit its font instead of filling the whole line"
                    );
                }
                assert_eq!(
                    inline.galley.job.text,
                    "Before bold prose and é🙂漢字 very_long_inline_identifier_without_breaks then old_value after."
                );
                assert!(
                    inline
                        .galley
                        .job
                        .sections
                        .iter()
                        .all(|section| section.format.background == Color32::TRANSPARENT)
                );
            }
        }
    }

    #[test]
    fn code_at_cell_edges_keeps_padding_in_left_center_and_right_aligned_cells() {
        for align in [Align::LEFT, Align::Center, Align::RIGHT] {
            let context = egui::Context::default();
            let inline = measured_inline(
                &context,
                "`path/é🙂/漢字.rs` and **details** then `last_value`",
                130.0,
                15.0,
                align,
            );
            assert!(inline.galley.rows.len() > 1);
            assert!(inline.galley.size().x <= 130.5);
            for badge in inline.badges.iter() {
                assert!(
                    inline.galley.rect.expand(0.6).contains_rect(*badge),
                    "edge padding clipped with alignment {align:?}"
                );
            }
        }
    }

    #[test]
    fn inline_geometry_cache_reuses_placements_without_reusing_link_destinations() {
        let context = egui::Context::default();
        let first = measured_inline(
            &context,
            "See [`é🙂 target`](first.md) now.",
            400.0,
            14.0,
            Align::LEFT,
        );
        let second = measured_inline(
            &context,
            "See [`é🙂 target`](second.md) now.",
            400.0,
            14.0,
            Align::LEFT,
        );
        assert!(Arc::ptr_eq(&first.galley, &second.galley));
        assert!(Arc::ptr_eq(&first.badges, &second.badges));
        assert_eq!(first.links, vec![(4..13, "first.md".into())]);
        assert_eq!(second.links, vec![(4..13, "second.md".into())]);
        let row = &second.galley.rows[0];
        let pointer = row.pos + row.glyphs[6].pos.to_vec2();
        let character = second.galley.cursor_from_pos(pointer.to_vec2()).index.0;
        assert!(second.links[0].0.contains(&character));
        let retained = context
            .data(|data| {
                data.get_temp::<InlineGeometryCache>(egui::Id::new("markdown_inline_geometry"))
            })
            .unwrap();
        assert_eq!(retained.entries.len(), 1);
        assert!(retained.bytes > retained.entries[0].original.job.text.len());
    }

    #[test]
    fn swapped_preview_jk_moves_in_the_users_configured_direction() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "A reading paragraph with `inline code`.\n\n".repeat(80);
        for (pass, key) in [None, Some(egui::Key::K), Some(egui::Key::J)]
            .into_iter()
            .enumerate()
        {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        Vec2::new(700.0, 300.0),
                    )),
                    time: Some(pass as f64 * 0.1),
                    events: key
                        .map(|key| {
                            vec![egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            }]
                        })
                        .unwrap_or_default(),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        cache.show(
                            ui,
                            1,
                            0,
                            &text,
                            Path::new("README.md"),
                            14.0,
                            false,
                            true,
                            true,
                        );
                    });
                },
            );
            output.drop_without_applying_deltas();
            if pass == 1 {
                assert!(cache.documents[0].scroll_y > 20.0);
            }
            if pass == 2 {
                assert!(cache.documents[0].scroll_y < 1.0);
            }
        }
    }

    #[test]
    fn selecting_across_wrapped_inline_code_copies_the_original_unicode_text() {
        const SOURCE: &str = "Start **bold** and `é🙂漢字 a_long_identifier_without_breaks` then ~~`old_value`~~ end.";
        const EXPECTED: &str =
            "Start bold and é🙂漢字 a_long_identifier_without_breaks then old_value end.";
        let mut screen = egui_kittest::Harness::builder()
            .with_size([290.0, 600.0])
            .build_ui_state(
                |ui, cache: &mut MarkdownCache| {
                    cache.show(
                        ui,
                        1,
                        0,
                        SOURCE,
                        Path::new("README.md"),
                        18.0,
                        false,
                        true,
                        false,
                    );
                },
                MarkdownCache::default(),
            );
        screen.run_steps(2);
        let mut shapes = Vec::new();
        fn collect<'a>(shape: &'a egui::Shape, output: &mut Vec<&'a egui::epaint::TextShape>) {
            match shape {
                egui::Shape::Text(text) => output.push(text),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, output);
                    }
                }
                _ => {}
            }
        }
        for shape in &screen.output().shapes {
            collect(&shape.shape, &mut shapes);
        }
        let shape = shapes
            .into_iter()
            .find(|shape| shape.galley.job.text == EXPECTED)
            .unwrap();
        assert!(shape.galley.rows.len() > 2);
        let start = shape.pos
            + shape
                .galley
                .pos_from_cursor(egui::text::CCursor::new(0))
                .center()
                .to_vec2();
        let end = shape.pos
            + shape
                .galley
                .pos_from_cursor(egui::text::CCursor::new(EXPECTED.chars().count()))
                .center()
                .to_vec2();
        screen.input_mut().events.extend([
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        screen.step();
        screen
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(end));
        screen.step();
        screen.input_mut().events.push(egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        screen.step();
        screen.input_mut().events.push(egui::Event::Copy);
        screen.step();
        let copied = screen
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            });
        assert_eq!(copied, Some(EXPECTED));
    }

    const INLINE_PREVIEW: &str = "# Review notes\n\nRead **important references** beside `src/editor.rs`, follow [`docs/plan.md`](docs/plan.md), and keep `:w` close to the surrounding text.\n\n- `src/markdown.rs` — inline code now shares the prose baseline.\n- **Unicode stays readable:** `café/Ω/☀.md`, ~~`old_value`~~ and *ordinary emphasis*.\n\n## Wrapped references\n\nA longer paragraph keeps **bold text**, `very_long_inline_identifier_without_any_spaces_or_breaks_at_all`, and normal words together across several lines. Code with spaces such as `review this plan before continuing` wraps gently within the same paragraph.\n\n| Left aligned | Centered | Right aligned |\n| :--- | :---: | ---: |\n| `src/main.rs` and **notes** | `éΩ ☀` | `last_value` |\n| A longer explanation with `inline_code_value` in the middle and enough words to wrap. | `a_long_reference_with_no_breaks` | **Ready** |\n\nThe paragraph after the table remains separate and readable.\n";

    fn inline_preview_harness(width: f32) -> egui_kittest::Harness<'static, MarkdownCache> {
        let mut setup = egui_wgpu::WgpuSetup::without_display_handle();
        if let egui_wgpu::WgpuSetup::CreateNew(options) = &mut setup {
            options.instance_descriptor.backends = egui_wgpu::wgpu::Backends::GL;
        }
        let screen = egui_kittest::Harness::builder()
            .with_size([width, 1300.0])
            .with_pixels_per_point(1.0)
            .with_theme(egui::Theme::Dark)
            .with_os(egui::os::OperatingSystem::Windows)
            .wgpu_setup(setup)
            .build_ui_state(
                |ui, cache: &mut MarkdownCache| {
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::new()
                                .fill(crate::palette::BG)
                                .inner_margin(12.0),
                        )
                        .show(ui, |ui| {
                            cache.show(
                                ui,
                                1,
                                0,
                                INLINE_PREVIEW,
                                Path::new("README.md"),
                                18.0,
                                false,
                                true,
                                false,
                            );
                        });
                },
                MarkdownCache::default(),
            );
        let mut fonts = egui::FontDefinitions::default();
        for (name, file, family) in [
            ("preview-ui", "segoeui.ttf", FontFamily::Proportional),
            ("preview-code", "CascadiaMono.ttf", FontFamily::Monospace),
            (
                "vedit-semibold",
                "seguisb.ttf",
                FontFamily::Name("vedit-semibold".into()),
            ),
        ] {
            if let Ok(bytes) = std::fs::read(
                PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:/Windows".into()))
                    .join("Fonts")
                    .join(file),
            ) {
                fonts
                    .font_data
                    .insert(name.into(), egui::FontData::from_owned(bytes).into());
                let mut family_fonts = vec![name.into()];
                family_fonts.extend(
                    fonts
                        .families
                        .get(&family)
                        .cloned()
                        .unwrap_or_else(|| fonts.families[&FontFamily::Proportional].clone()),
                );
                fonts.families.insert(family, family_fonts);
            }
        }
        screen.ctx.set_fonts(fonts);
        screen
    }

    fn inline_snapshot(screen: &mut egui_kittest::Harness<'_, MarkdownCache>, name: &str) {
        screen.hover_at(egui::pos2(-20.0, -20.0));
        screen.run_steps(3);
        if let Some(directory) = std::env::var_os("VEDIT_SNAPSHOT_DIR") {
            screen.snapshot_options(
                name,
                &egui_kittest::SnapshotOptions::new().output_path(PathBuf::from(directory)),
            );
        } else {
            screen.snapshot(name);
        }
    }

    #[test]
    fn markdown_inline_code_screenshot() {
        inline_snapshot(
            &mut inline_preview_harness(980.0),
            "vedit_markdown_inline_code",
        );
    }

    #[test]
    fn narrow_markdown_inline_code_screenshot() {
        inline_snapshot(
            &mut inline_preview_harness(420.0),
            "vedit_narrow_markdown_inline_code",
        );
    }

    #[test]
    #[ignore = "manual headless preview performance measurement"]
    fn markdown_table_preview_benchmark() {
        let text = format!("| File | Change | Status |\n| --- | --- | --- |\n{}", (0..500).map(|row|
            format!("| `src/file_{row}.rs` | **Important:** review this change, which contains several words and an `inline_code_value`. | Ready |\n")
        ).collect::<String>());
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        for pass in 0..3 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        let started = std::time::Instant::now();
        for pass in 3..23 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        println!(
            "Cached 500-row Markdown table headless UI: {:.2} ms/frame (debug build, includes layout and UI widget generation, excludes GPU)",
            started.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
    }

    #[test]
    fn cached_large_table_shapes_only_visible_cells() {
        let text = format!(
            "| File | Description |\n| --- | --- |\n{}",
            (0..500)
                .map(|row| format!(
                    "| `file_{row}.rs` | Unicode Grüße 🦀 and **wrapped words** for row {row}. |\n"
                ))
                .collect::<String>()
        );
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        for pass in 0..3 {
            preview_pass(&context, &mut cache, &text, pass as f64 * 0.1, vec![]);
        }
        INLINE_LAYOUT_REQUESTS.with(|requests| requests.set(0));
        preview_pass(&context, &mut cache, &text, 0.4, vec![]);
        let layouts = INLINE_LAYOUT_REQUESTS.with(std::cell::Cell::get);
        assert!(
            layouts < 40,
            "a 340-point viewport should shape only its visible cells, got {layouts} layouts"
        );
        INLINE_LAYOUT_REQUESTS.with(|requests| requests.set(0));
        preview_pass(
            &context,
            &mut cache,
            &text,
            0.5,
            vec![egui::Event::Key {
                key: egui::Key::G,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
        );
        assert!(cache.documents[0].scroll_y > 10_000.0);
        let layouts = INLINE_LAYOUT_REQUESTS.with(std::cell::Cell::get);
        assert!(
            layouts < 40,
            "scrolling to the end must also shape only visible cells, got {layouts}"
        );
    }

    #[test]
    fn cached_table_geometry_tracks_width_font_and_same_revision_text_changes() {
        let context = egui::Context::default();
        let mut cache = MarkdownCache::default();
        let text = "| File | Description |\n| --- | --- |\n| first | These are enough words to wrap repeatedly at a narrow width. |\n| last | End |\n";
        let mut pass = |width, font_size, text: &str| {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        Vec2::new(width, 640.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        cache.show(
                            ui,
                            1,
                            0,
                            text,
                            Path::new("README.md"),
                            font_size,
                            false,
                            true,
                            false,
                        );
                    });
                },
            );
            let bounds = text_bounds(&output.shapes);
            assert!(bounds.iter().any(|(text, _)| text == "last"));
            output.drop_without_applying_deltas();
            cache.documents[0].table_layouts[&0]
                .offsets
                .last()
                .copied()
                .unwrap()
        };
        let wide = pass(960.0, 14.0, text);
        let narrow = pass(320.0, 14.0, text);
        assert!(narrow > wide);
        assert_eq!(pass(960.0, 14.0, text), wide);
        assert!(pass(960.0, 22.0, text) > wide);
        let changed = text.replace("| last | End |", "| middle | Added |\n| last | End |");
        assert!(pass(960.0, 14.0, &changed) > wide);
    }

    #[test]
    fn table_text_selection_survives_scrolling_past_preceding_rows() {
        const SELECTED: &str = "Selected Grüße 🦀";
        let source = format!(
            "| Column |\n| --- |\n| Before |\n| {SELECTED} |\n{}",
            "| More content |\n".repeat(20)
        );
        let mut screen = egui_kittest::Harness::builder()
            .with_size([640.0, 340.0])
            .build_ui_state(
                move |ui, cache: &mut MarkdownCache| {
                    cache.show(
                        ui,
                        1,
                        0,
                        &source,
                        Path::new("README.md"),
                        14.0,
                        false,
                        true,
                        false,
                    );
                },
                MarkdownCache::default(),
            );
        screen.run_steps(3);
        let bounds = text_bounds(&screen.output().shapes);
        let rect = bounds.iter().find(|(text, _)| text == SELECTED).unwrap().1;
        let start = rect.left_center();
        let end = rect.right_center();
        screen.input_mut().events.extend([
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        screen.step();
        screen
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(end));
        screen.step();
        screen.input_mut().events.push(egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        screen.step();
        screen.input_mut().events.extend([
            egui::Event::PointerMoved(egui::pos2(300.0, 180.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -90.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ]);
        screen.step();
        assert!(screen.state().documents[0].scroll_y >= 90.0);
        screen.input_mut().events.push(egui::Event::Copy);
        screen.step();
        let copied = screen
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.as_str())
                } else {
                    None
                }
            });
        assert_eq!(copied, Some(SELECTED));
    }
}
