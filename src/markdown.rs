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

const ACCENT: Color32 = Color32::from_rgb(126, 211, 182);
const MUTED: Color32 = Color32::from_rgb(139, 150, 172);
const INK: Color32 = Color32::from_rgb(224, 230, 240);
const SURFACE: Color32 = Color32::from_rgb(22, 26, 34);
const BORDER: Color32 = Color32::from_rgb(43, 50, 63);
const MAX_DOCUMENTS: usize = 12;
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 1024 * 1024;

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
    #[cfg(test)]
    pub(crate) fn view_scroll(&self, document: u64, pane: egui::Id) -> Option<f32> {
        self.documents
            .iter()
            .find(|entry| entry.id == document && entry.pane == pane)
            .map(|entry| entry.scroll_y)
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
                    .map(|entry| entry.text.len())
                    .sum::<usize>()
                    > MAX_CACHE_BYTES)
        {
            self.documents.pop_back();
        }
        self.restore_scroll.retain(|(pane, id), _| {
            self.documents
                .iter()
                .any(|entry| entry.pane == *pane && entry.id == *id)
        });
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
            });
        }
        let mut entry = entry.expect("created preview document");
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
                if input.consume_key(egui::Modifiers::NONE, egui::Key::J) {
                    requested = Some(entry.scroll_y + font_size * 2.0);
                    entry.pending_g = None;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::K) {
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
        let output = scroll.show(ui, |ui| {
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
                                        result: &mut result, next_fragment: &mut next_fragment,
                                        pending_fragment: pending_fragment.as_deref(),
                                        reveal_source: reveal_target.map(|(_, source)| source),
                                        markdown_source: text,
                                        smooth_scroll,
                                        table_scroll: &mut entry.table_scroll,
                                        restore_tables: &mut entry.restore_tables,
                                    };
                                    renderer.block(ui, block, false);
                                });
                                entry.heights[index] = Some((ui.cursor().top() - start).max(0.0));
                            }
                        }
                        ui.add_space(44.0);
                    });
                });
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
                    Some(BlockKind::Quote(children))
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
                let count = seen.entry(base.clone()).or_default();
                *anchor = if *count == 0 {
                    base
                } else {
                    format!("{base}-{count}")
                };
                *count += 1;
            }
            BlockKind::Quote(children) => assign_anchors(children, seen),
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
        BlockKind::Quote(children) => children
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
    result: &'a mut PreviewResponse,
    next_fragment: &'a mut Option<String>,
    pending_fragment: Option<&'a str>,
    reveal_source: Option<usize>,
    markdown_source: &'a str,
    smooth_scroll: bool,
    table_scroll: &'a mut HashMap<usize, f32>,
    restore_tables: &'a mut HashMap<usize, f32>,
}

struct InlineLayout {
    galley: Arc<egui::Galley>,
    links: Vec<(Range<usize>, String)>,
}

struct TableRowLayout {
    cells: Vec<InlineLayout>,
    height: f32,
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
    fn table(
        &mut self,
        ui: &mut egui::Ui,
        alignments: &[Alignment],
        headers: &[Vec<Span>],
        rows: &[Vec<Vec<Span>>],
        source: usize,
    ) {
        let size = (self.font_size - 1.0).max(12.0);
        let widths = table_column_widths(headers, rows, size, ui.available_width());
        let table_width = widths.iter().sum::<f32>() + widths.len() as f32 * 24.0 + 2.0;
        let prepared = std::iter::once(headers)
            .chain(rows.iter().map(Vec::as_slice))
            .enumerate()
            .map(|(row_index, row)| {
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
                            row_index == 0,
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
            })
            .collect::<Vec<_>>();
        let table_height = prepared.iter().map(|row| row.height).sum::<f32>();
        let salt = ("markdown_table", ui.id(), source);
        let scroll_id = ui.make_persistent_id(egui::IdSalt::new(salt));
        let mut scroll = egui::ScrollArea::horizontal()
            .id_salt(salt)
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
        let output = scroll.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let table_start = ui.cursor().min;
            for (row_index, TableRowLayout { cells, height }) in prepared.into_iter().enumerate() {
                let (row_rect, _) =
                    ui.allocate_exact_size(Vec2::new(table_width, height), Sense::hover());
                if !ui.is_rect_visible(row_rect) {
                    continue;
                }
                let fill = if row_index == 0 {
                    Color32::from_rgb(29, 34, 44)
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
                    let mut cell_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(("table_cell", source, row_index, column))
                            .max_rect(cell_rect)
                            .layout(Layout::top_down(align)),
                    );
                    cell_ui.set_clip_rect(cell_rect.intersect(ui.clip_rect()));
                    self.paint_inline(&mut cell_ui, cell, source);
                    left += width + 24.0;
                }
            }
            let table_rect = egui::Rect::from_min_max(
                table_start,
                egui::pos2(table_start.x + table_width, ui.cursor().top()),
            );
            ui.painter().rect_stroke(
                table_rect,
                6.0,
                Stroke::new(1.0, BORDER),
                egui::StrokeKind::Inside,
            );
        });
        crate::render::remember_scroll_limits(ui.ctx(), &output);
        self.table_scroll.insert(source, output.state.offset.x);
    }

    fn block(&mut self, ui: &mut egui::Ui, block: &Block, compact: bool) {
        if self.reveal_source == Some(block.source.start) {
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
                ui.add_space(if *level == 1 { 4.0 } else { 16.0 });
                let size = self.font_size
                    * match level {
                        1 => 2.1,
                        2 => 1.6,
                        3 => 1.3,
                        _ => 1.1,
                    };
                let response = self.inline(ui, spans, size, true, block.source.start);
                if self.pending_fragment == Some(anchor.as_str()) {
                    response.scroll_to_me(Some(Align::TOP));
                }
                ui.add_space(9.0);
                if *level <= 2 {
                    ui.separator();
                    ui.add_space(14.0);
                }
            }
            BlockKind::Code { language, text } => {
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(7.0)
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.set_width((ui.available_width() - 1.0).max(1.0));
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if language.is_empty() {
                                    "Code"
                                } else {
                                    language
                                })
                                .size(10.5)
                                .color(MUTED),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let copy =
                                    crate::design::ghost_button(ui, "Copy", "Copy this code block");
                                if copy.clicked() {
                                    ui.ctx().copy_text(text.clone());
                                    copy.surrender_focus();
                                }
                            });
                        });
                        ui.add_space(7.0);
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
                ui.add_space(16.0);
            }
            BlockKind::Quote(children) => {
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
                    Stroke::new(3.0, ACCENT.gamma_multiply(0.7)),
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
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = width.max(1.0);
        job.halign = align;
        let strong_family = FontFamily::Name("morrow-semibold".into());
        let has_strong = ui.fonts_mut(|fonts| fonts.families().contains(&strong_family));
        let mut links = Vec::new();
        let mut character = 0;
        let mut previous_code = false;
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
                ACCENT
            } else if span.strike || span.image {
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
            let format = TextFormat {
                font_id: FontId::new(text_size, family),
                color,
                italics: span.emphasis,
                background: if span.code {
                    Color32::from_rgb(31, 38, 48)
                } else {
                    Color32::TRANSPARENT
                },
                strikethrough: if span.strike {
                    Stroke::new(1.0, MUTED)
                } else {
                    Stroke::NONE
                },
                line_height: Some(size * 1.55),
                ..Default::default()
            };
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
            character = end;
        }
        InlineLayout {
            galley: ui.fonts_mut(|fonts| fonts.layout_job(job)),
            links,
        }
    }

    fn paint_inline(
        &mut self,
        ui: &mut egui::Ui,
        inline: InlineLayout,
        source: usize,
    ) -> egui::Response {
        let InlineLayout { galley, links } = inline;
        let mut response = ui.add(
            egui::Label::new(galley.clone())
                .selectable(true)
                .sense(Sense::click_and_drag() - Sense::FOCUSABLE),
        );
        let mut hovered_link = None;
        if let Some(pointer) = response.hover_pos() {
            let origin = match galley.job.halign {
                Align::LEFT => response.rect.left_top(),
                Align::Center => response.rect.center_top(),
                Align::RIGHT => response.rect.right_top(),
            };
            let character = galley.cursor_from_pos(pointer - origin).index.0;
            if let Some((_, link)) = links.iter().find(|(range, _)| range.contains(&character)) {
                hovered_link = Some(link.clone());
                let action = link_action(link, self.source_path);
                response = response.on_hover_text(match action {
                    LinkAction::Blocked => format!("Inactive link: {link}"),
                    LinkAction::File(ref path) => crate::paths::display(path),
                    _ => link.clone(),
                });
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
        response
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
                    cache.show(ui, 7, 3, text, Path::new("README.md"), 14.0, false, true);
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
                    cache.show(ui, 1, 0, &text, Path::new("README.md"), 14.0, false, true);
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
                cache.show(ui, 1, 0, text, Path::new("README.md"), 14.0, false, true);
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
                            cache.show(ui, 1, 0, &text, Path::new("README.md"), 14.0, false, true);
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
                        cache.show(ui, 1, 0, text, Path::new("README.md"), 14.0, false, true);
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
                        cache.show(ui, 1, 0, text, Path::new("README.md"), 14.0, false, true);
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
}
