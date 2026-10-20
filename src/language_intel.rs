//! Hover, references and symbol navigation backed by language servers.
//! Requests run on the language client's threads. A result applies only when
//! its request, server instance and document revision still match the UI.
use super::*;
use crate::lsp::{Feature, HoverContents, Position, Reference, Symbol};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

pub(super) const HOVER_DELAY: Duration = Duration::from_millis(500);
const HOVER_MAX_WIDTH: f32 = 560.0;
const HOVER_MAX_HEIGHT: f32 = 340.0;
const SYMBOL_ROWS: usize = 300;
const CANCELLED: &str = "Language analysis restarted. The request was cancelled.";
const FINDING_REFERENCES: &str = "Finding references…";

/// The request and server instance a pending result belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Origin {
    pub(super) id: u64,
    root: PathBuf,
    language: String,
    instance: u64,
}

enum Lookup {
    Sent(Origin),
    Unsupported(String),
    Unavailable,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HoverSource {
    Pointer,
    Keyboard,
}

/// The source word under the pointer in the current frame.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PointerTarget {
    pub(super) tab_id: u64,
    pub(super) pane_id: u64,
    pub(super) revision: u64,
    pub(super) index: usize,
    pub(super) word: Range<usize>,
    pub(super) rect: egui::Rect,
    pub(super) pointer: egui::Pos2,
}

struct Rest {
    target: PointerTarget,
    since: Instant,
    fired: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HoverTarget {
    pub(super) tab_id: u64,
    pub(super) revision: u64,
    pub(super) index: usize,
    pub(super) source: HoverSource,
    pub(super) anchor: egui::Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum HoverBody {
    /// Waiting until no other hover request is in flight.
    Pending,
    Requested(u64),
    Ready(Option<HoverDocument>),
    /// No hover provider; an explanation for explicit requests.
    Unavailable(Option<String>),
}

pub(super) struct HoverPopup {
    pub(super) target: HoverTarget,
    /// Severity and message of diagnostics at the hovered character.
    pub(super) diagnostics: Vec<(u8, String)>,
    pub(super) body: HoverBody,
}

impl HoverPopup {
    fn visible(&self) -> bool {
        !self.diagnostics.is_empty() || matches!(self.body, HoverBody::Ready(Some(_)))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct HoverSpan {
    pub(super) text: String,
    pub(super) code: bool,
    pub(super) strong: bool,
    pub(super) emphasis: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum HoverBlock {
    Text {
        spans: Vec<HoverSpan>,
        heading: bool,
        /// List depth for a bulleted item.
        bullet: Option<usize>,
    },
    Code {
        language: String,
        text: String,
    },
    Plain(String),
    Rule,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HoverDocument {
    pub(super) blocks: Vec<HoverBlock>,
}

#[derive(Debug, PartialEq)]
pub(super) enum ReferencesState {
    Loading,
    Loaded {
        groups: Vec<ReferenceGroup>,
        truncated: bool,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ReferenceGroup {
    pub(super) path: PathBuf,
    pub(super) label: String,
    pub(super) items: Vec<Reference>,
}

pub(super) struct ReferencesView {
    pub(super) request: Option<Origin>,
    pub(super) symbol: String,
    /// Tab, revision and cursor the request started from.
    origin: (u64, u64, usize),
    origin_path: PathBuf,
    root: PathBuf,
    pub(super) state: ReferencesState,
    pub(super) selected: usize,
    pub(super) focus_selected: bool,
    row_focused: bool,
}

impl ReferencesView {
    #[cfg(test)]
    pub(super) fn locations(&self) -> Vec<&Reference> {
        match &self.state {
            ReferencesState::Loaded { groups, .. } => {
                groups.iter().flat_map(|group| &group.items).collect()
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum SymbolState {
    Loading,
    Loaded(Vec<Symbol>),
    Failed(String),
}

pub(super) struct DocumentSymbols {
    tab_id: u64,
    revision: u64,
    path: PathBuf,
    pub(super) request: Option<Origin>,
    pub(super) state: SymbolState,
}

pub(super) struct WorkspaceSymbols {
    root: PathBuf,
    pub(super) request: Option<(Origin, String)>,
    loaded: Option<String>,
    pub(super) state: SymbolState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SymbolScope {
    Document,
    Workspace,
}

/// A palette row: label, detail, file and position to reveal.
pub(super) type PaletteRow = (String, String, Option<PathBuf>, Option<Position>);

pub(super) struct SymbolPalette {
    pub(super) rows: Vec<PaletteRow>,
    pub(super) loading: bool,
    pub(super) message: Option<String>,
}

#[derive(Default)]
pub(super) struct IntelState {
    /// Rendered this frame by the source editor.
    pub(super) pointer: Option<PointerTarget>,
    pub(super) caret: Option<egui::Rect>,
    pub(super) interactive: bool,
    rest: Option<Rest>,
    pub(super) hover: Option<HoverPopup>,
    hover_flight: Option<Origin>,
    hover_rect: Option<egui::Rect>,
    pub(super) references: Option<ReferencesView>,
    pub(super) document_symbols: Option<DocumentSymbols>,
    pub(super) workspace_symbols: Option<WorkspaceSymbols>,
    reveal: Option<(u64, Position)>,
}

impl IntelState {
    pub(super) fn begin_frame(&mut self) {
        self.pointer = None;
        self.caret = None;
        self.interactive = false;
    }

    pub(super) fn hover_visible(&self) -> bool {
        self.hover.as_ref().is_some_and(HoverPopup::visible)
    }

    pub(super) fn close_hover(&mut self) {
        self.hover = None;
        self.hover_rect = None;
        if let Some(rest) = &mut self.rest {
            rest.fired = true;
        }
    }

    #[cfg(test)]
    pub(super) fn hover_drawn(&self) -> bool {
        self.hover_rect.is_some()
    }

    #[cfg(test)]
    pub(super) fn hover_in_flight(&self) -> Option<u64> {
        self.hover_flight.as_ref().map(|origin| origin.id)
    }

    #[cfg(test)]
    pub(super) fn age_pointer_rest(&mut self, by: Duration) {
        if let Some(rest) = &mut self.rest {
            rest.since -= by;
        }
    }
}

pub(super) fn symbol_scope(query: &str) -> Option<(SymbolScope, &str)> {
    if let Some(rest) = query.strip_prefix('@') {
        Some((SymbolScope::Document, rest.trim()))
    } else {
        query
            .strip_prefix('#')
            .map(|rest| (SymbolScope::Workspace, rest.trim()))
    }
}

fn identifier(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The identifier (or single symbol character) at a Unicode scalar index.
pub(super) fn word_at(text: &str, index: usize) -> Option<(Range<usize>, String)> {
    let chars: Vec<char> = text
        .chars()
        .skip(index.saturating_sub(256))
        .take(512)
        .collect();
    let base = index.saturating_sub(256);
    let local = index - base;
    let at = |position: usize| chars.get(position).copied();
    let local = match at(local) {
        Some(ch) if identifier(ch) => local,
        _ if local > 0 && at(local - 1).is_some_and(identifier) => local - 1,
        Some(ch) if !ch.is_whitespace() => {
            return Some((index..index + 1, ch.to_string()));
        }
        _ => return None,
    };
    let mut start = local;
    while start > 0 && at(start - 1).is_some_and(identifier) {
        start -= 1;
    }
    let mut end = local + 1;
    while at(end).is_some_and(identifier) {
        end += 1;
    }
    Some((base + start..base + end, chars[start..end].iter().collect()))
}

/// The word under a galley-local pointer position on a visible row.
pub(super) fn pointer_word(
    galley: &egui::Galley,
    rows: &[(usize, crate::render::VisualRow)],
    local: egui::Pos2,
) -> Option<(usize, Range<usize>, egui::Rect)> {
    let &(row_index, mapped) = rows.iter().find(|(index, _)| {
        let row = &galley.rows[*index];
        row.min_y() <= local.y && local.y < row.max_y()
    })?;
    let row = &galley.rows[row_index];
    let glyphs = &row.glyphs;
    let x = local.x - row.pos.x;
    let column = glyphs
        .iter()
        .position(|glyph| glyph.pos.x <= x && x < glyph.pos.x + glyph.advance_width)?;
    let character = glyphs[column].chr;
    if character.is_whitespace() {
        return None;
    }
    let (mut start, mut end) = (column, column + 1);
    if identifier(character) {
        while start > 0 && identifier(glyphs[start - 1].chr) {
            start -= 1;
        }
        while end < glyphs.len() && identifier(glyphs[end].chr) {
            end += 1;
        }
    }
    let left = row.pos.x + glyphs[start].pos.x;
    let right = row.pos.x + glyphs[end - 1].pos.x + glyphs[end - 1].advance_width;
    Some((
        mapped.start + column,
        mapped.start + start..mapped.start + end,
        egui::Rect::from_min_max(
            egui::pos2(left, row.min_y()),
            egui::pos2(right, row.max_y()),
        ),
    ))
}

fn position_key(position: Position) -> (u32, u32) {
    (position.line, position.character)
}

/// Diagnostics whose range covers a UTF-16 position, errors first.
pub(super) fn diagnostics_at(
    diagnostics: &[crate::lsp::Diagnostic],
    position: Position,
) -> Vec<(u8, String)> {
    let at = position_key(position);
    let mut found: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| {
            let start = position_key(diagnostic.range.start);
            let end = position_key(diagnostic.range.end);
            start <= at && (at < end || start == end && at == start)
        })
        .map(|diagnostic| (diagnostic.severity, diagnostic.message.clone()))
        .collect();
    found.sort_by_key(|(severity, _)| *severity);
    found.dedup();
    found
}

/// Parses hover Markdown into calm blocks; plain text is never interpreted.
pub(super) fn hover_document(contents: &HoverContents) -> Option<HoverDocument> {
    if contents.text.trim().is_empty() {
        return None;
    }
    if !contents.markdown {
        return Some(HoverDocument {
            blocks: vec![HoverBlock::Plain(contents.text.trim_end().to_owned())],
        });
    }
    let mut blocks = Vec::new();
    let mut current: Option<HoverBlock> = None;
    let mut code: Option<(String, String)> = None;
    let (mut depth, mut strong, mut emphasis) = (0usize, 0usize, 0usize);
    let mut heading = false;
    let mut bullet = false;
    let flush = |current: &mut Option<HoverBlock>, blocks: &mut Vec<HoverBlock>| {
        if let Some(block) = current.take()
            && let HoverBlock::Text { spans, .. } = &block
            && spans.iter().any(|span| !span.text.trim().is_empty())
        {
            blocks.push(block);
        }
    };
    for event in Parser::new_ext(&contents.text, Options::ENABLE_STRIKETHROUGH) {
        if let Some((_, text)) = &mut code {
            match event {
                Event::Text(value) => text.push_str(&value),
                Event::End(TagEnd::CodeBlock) => {
                    let (language, text) = code.take().unwrap();
                    let text = text.trim_end_matches('\n').to_owned();
                    if !text.trim().is_empty() {
                        blocks.push(HoverBlock::Code { language, text });
                    }
                }
                _ => {}
            }
            continue;
        }
        let span = match event {
            Event::Start(Tag::Heading { .. }) => {
                flush(&mut current, &mut blocks);
                heading = true;
                None
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut current, &mut blocks);
                heading = false;
                None
            }
            Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                flush(&mut current, &mut blocks);
                None
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut current, &mut blocks);
                let language = match kind {
                    CodeBlockKind::Fenced(language) => language
                        .split(|ch: char| ch.is_whitespace() || ch == ',')
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((language, String::new()));
                None
            }
            Event::Start(Tag::List(_)) => {
                flush(&mut current, &mut blocks);
                depth += 1;
                None
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut current, &mut blocks);
                depth = depth.saturating_sub(1);
                None
            }
            Event::Start(Tag::Item) => {
                flush(&mut current, &mut blocks);
                bullet = true;
                None
            }
            Event::Start(Tag::Strong) => {
                strong += 1;
                None
            }
            Event::End(TagEnd::Strong) => {
                strong = strong.saturating_sub(1);
                None
            }
            Event::Start(Tag::Emphasis) => {
                emphasis += 1;
                None
            }
            Event::End(TagEnd::Emphasis) => {
                emphasis = emphasis.saturating_sub(1);
                None
            }
            Event::Rule => {
                flush(&mut current, &mut blocks);
                if blocks
                    .last()
                    .is_some_and(|block| !matches!(block, HoverBlock::Rule))
                {
                    blocks.push(HoverBlock::Rule);
                }
                None
            }
            Event::Text(text) => Some((text.into_string(), false)),
            Event::Code(text) => Some((text.into_string(), true)),
            Event::SoftBreak => Some((" ".to_owned(), false)),
            Event::HardBreak => Some(("\n".to_owned(), false)),
            _ => None,
        };
        let Some((text, is_code)) = span else {
            continue;
        };
        let block = current.get_or_insert_with(|| HoverBlock::Text {
            spans: Vec::new(),
            heading,
            bullet: std::mem::take(&mut bullet).then(|| depth.saturating_sub(1)),
        });
        if let HoverBlock::Text { spans, .. } = block {
            let span = HoverSpan {
                text,
                code: is_code,
                strong: strong > 0,
                emphasis: emphasis > 0,
            };
            match spans.last_mut() {
                Some(last)
                    if last.code == span.code
                        && last.strong == span.strong
                        && last.emphasis == span.emphasis =>
                {
                    last.text.push_str(&span.text);
                }
                _ => spans.push(span),
            }
        }
    }
    flush(&mut current, &mut blocks);
    while matches!(blocks.last(), Some(HoverBlock::Rule)) {
        blocks.pop();
    }
    (!blocks.is_empty()).then_some(HoverDocument { blocks })
}

/// Groups references by file: the current file first, then by relative path;
/// each file's references in source order.
pub(super) fn group_references(
    references: Vec<Reference>,
    current: &Path,
    root: &Path,
) -> Vec<ReferenceGroup> {
    let mut groups: Vec<ReferenceGroup> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    for reference in references {
        let slot = *index.entry(reference.path.clone()).or_insert_with(|| {
            groups.push(ReferenceGroup {
                label: crate::paths::display(
                    reference.path.strip_prefix(root).unwrap_or(&reference.path),
                ),
                path: reference.path.clone(),
                items: Vec::new(),
            });
            groups.len() - 1
        });
        groups[slot].items.push(reference);
    }
    for group in &mut groups {
        group
            .items
            .sort_by_key(|reference| position_key(reference.position));
    }
    groups.sort_by(|a, b| {
        (a.path != current)
            .cmp(&(b.path != current))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.label.cmp(&b.label))
    });
    groups
}

/// Fuzzy-filters symbols by name (or container and name), best first.
pub(super) fn filter_symbols<'a>(symbols: &'a [Symbol], query: &str) -> Vec<&'a Symbol> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return symbols.iter().take(SYMBOL_ROWS).collect();
    }
    let mut scored: Vec<_> = symbols
        .iter()
        .enumerate()
        .filter_map(|(order, symbol)| {
            let name = symbol.name.to_lowercase();
            navigation::fuzzy_score(&name, &query)
                .or_else(|| {
                    let qualified = format!(
                        "{} {}",
                        symbol.container.as_deref().unwrap_or_default(),
                        name
                    )
                    .to_lowercase();
                    navigation::fuzzy_score(&qualified, &query).map(|score| score + 1_000)
                })
                .map(|score| (score, order, symbol))
        })
        .collect();
    scored.sort_by_key(|(score, order, _)| (*score, *order));
    scored
        .into_iter()
        .take(SYMBOL_ROWS)
        .map(|(_, _, symbol)| symbol)
        .collect()
}

fn symbol_detail(symbol: &Symbol, root: Option<&Path>) -> String {
    let mut detail = crate::lsp::symbol_kind_label(symbol.kind).to_owned();
    if let Some(container) = symbol.container.as_deref().filter(|name| !name.is_empty()) {
        detail.push_str(" · ");
        detail.push_str(container);
    }
    if let (Some(root), Some(path)) = (root, &symbol.path) {
        detail.push_str(" · ");
        detail.push_str(&crate::paths::display(
            path.strip_prefix(root).unwrap_or(path),
        ));
    }
    detail
}

fn severity_label(severity: u8) -> (&'static str, Color32) {
    match severity {
        1 => ("Error", RED),
        2 => ("Warning", ORANGE),
        3 => ("Info", BLUE),
        _ => ("Hint", MUTED),
    }
}

fn inline_job(ui: &mut egui::Ui, spans: &[HoverSpan], heading: bool) -> egui::text::LayoutJob {
    let size = if heading { 13.5 } else { 12.5 };
    let strong_family = egui::FontFamily::Name("vedit-semibold".into());
    let has_strong = ui.fonts_mut(|fonts| fonts.families().contains(&strong_family));
    let mut job = egui::text::LayoutJob::default();
    for span in spans {
        let font_id = if span.code {
            FontId::monospace(size - 0.5)
        } else if (span.strong || heading) && has_strong {
            FontId::new(size, strong_family.clone())
        } else {
            FontId::proportional(size)
        };
        job.append(
            &span.text,
            0.0,
            egui::TextFormat {
                font_id,
                color: if span.code || span.strong || heading {
                    TEXT
                } else {
                    TEXT.gamma_multiply(0.9)
                },
                italics: span.emphasis,
                background: if span.code {
                    SURFACE
                } else {
                    Color32::TRANSPARENT
                },
                ..Default::default()
            },
        );
    }
    job
}

fn hover_document_ui(ui: &mut egui::Ui, document: &HoverDocument, font: f32) {
    for block in &document.blocks {
        match block {
            HoverBlock::Text {
                spans,
                heading,
                bullet,
            } => {
                let job = inline_job(ui, spans, *heading);
                if let Some(depth) = bullet {
                    ui.horizontal_top(|ui| {
                        ui.add_space(*depth as f32 * 12.0);
                        ui.label(RichText::new("•").size(12.5).color(MUTED));
                        ui.add(egui::Label::new(job).wrap().selectable(false));
                    });
                } else {
                    ui.add(egui::Label::new(job).wrap().selectable(false));
                }
            }
            HoverBlock::Code { language, text } => {
                egui::Frame::new()
                    .fill(BG)
                    .corner_radius(4.0)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let job = crate::render::highlight_snippet(ui.ctx(), text, language, font);
                        ui.add(egui::Label::new(job).wrap().selectable(false));
                    });
            }
            HoverBlock::Plain(text) => {
                ui.add(
                    egui::Label::new(RichText::new(text).monospace().size(font).color(TEXT))
                        .wrap()
                        .selectable(false),
                );
            }
            HoverBlock::Rule => {
                ui.separator();
            }
        }
    }
}

impl VEditApp {
    /// Opens a language-server location in the focused pane with the usual
    /// reveal feedback. Every reference and symbol jump goes through here.
    pub(super) fn open_language_location(&mut self, path: PathBuf, position: Position) {
        self.record_jump();
        self.open_file(path.clone());
        let Some(id) = self
            .tab()
            .filter(|tab| tab.path.as_deref() == Some(path.as_path()))
            .map(|tab| tab.id)
        else {
            return;
        };
        self.set_document_view(crate::cli::DocumentView::Source);
        self.intel.reveal = Some((id, position));
        self.apply_language_reveal();
    }

    pub(super) fn apply_language_reveal(&mut self) {
        let Some((id, position)) = self.intel.reveal else {
            return;
        };
        match self.tab_by_id_mut(id) {
            None => {
                self.intel.reveal = None;
                return;
            }
            Some(tab) if tab.loading => return,
            Some(_) => self.intel.reveal = None,
        }
        let Some(tab) = self.tab_by_id_mut(id) else {
            return;
        };
        let index = crate::lsp::position_to_char(&tab.text, position);
        let (line, column) = tab.lines.line_col(&tab.text, tab.revision, index);
        tab.pending_agent_location = Some(crate::cli::Location { line, column });
        tab.agent_location_error = None;
        tab.pending_navigation_feedback = true;
        self.apply_pending_agent_locations();
    }

    fn intel_tab(&self, id: u64) -> Option<&EditorTab> {
        self.project()?.tabs.iter().find(|tab| tab.id == id)
    }

    fn intel_request(
        &self,
        path: Option<&Path>,
        feature: Feature,
        send: impl FnOnce(&crate::lsp::LanguageServer) -> u64,
    ) -> Lookup {
        let Some(project) = self.project() else {
            return Lookup::Unavailable;
        };
        let mut sessions: Vec<_> = project
            .languages
            .iter()
            .filter(|(_, session)| {
                session.config.enabled
                    && path.is_none_or(|path| session.config.matches(path))
                    && (path.is_some()
                        || session.active()
                            && session
                                .server
                                .as_ref()
                                .is_some_and(|server| server.supports(feature)))
            })
            .collect();
        sessions.sort_by(|a, b| a.0.cmp(b.0));
        let Some((language, session)) = sessions.into_iter().next() else {
            return Lookup::Unavailable;
        };
        let Some(server) = session.server.as_ref().filter(|_| session.active()) else {
            return Lookup::Unavailable;
        };
        if !server.supports(feature) {
            let owner = if session.config.name.is_empty() {
                "This language server".to_owned()
            } else {
                format!("{} analysis", session.config.name)
            };
            return Lookup::Unsupported(format!("{owner} does not provide {}.", feature.label()));
        }
        match send(server) {
            0 => Lookup::Busy,
            id => Lookup::Sent(Origin {
                id,
                root: project.root.clone(),
                language: language.clone(),
                instance: server.instance(),
            }),
        }
    }

    fn origin_alive(&self, origin: &Origin) -> bool {
        self.projects
            .iter()
            .find(|project| project.root == origin.root)
            .and_then(|project| project.languages.get(&origin.language))
            .and_then(|session| session.server.as_ref())
            .is_some_and(|server| server.instance() == origin.instance && !server.is_stopped())
    }

    /// Resolves requests whose server restarted or stopped as cancelled.
    pub(super) fn check_language_intel_requests(&mut self) {
        if let Some(origin) = &self.intel.hover_flight
            && !self.origin_alive(origin)
        {
            let id = origin.id;
            self.intel.hover_flight = None;
            if let Some(popup) = &mut self.intel.hover
                && popup.body == HoverBody::Requested(id)
            {
                popup.body = HoverBody::Ready(None);
            }
        }
        if let Some(view) = &self.intel.references
            && let Some(origin) = &view.request
            && !self.origin_alive(origin)
        {
            let view = self.intel.references.as_mut().unwrap();
            view.request = None;
            view.state = ReferencesState::Failed(CANCELLED.into());
            self.clear_status(FINDING_REFERENCES);
        }
        if let Some(symbols) = &self.intel.document_symbols
            && let Some(origin) = &symbols.request
            && !self.origin_alive(origin)
        {
            let symbols = self.intel.document_symbols.as_mut().unwrap();
            symbols.request = None;
            symbols.state = SymbolState::Failed(CANCELLED.into());
        }
        if let Some(symbols) = &self.intel.workspace_symbols
            && let Some((origin, _)) = &symbols.request
            && !self.origin_alive(origin)
        {
            let symbols = self.intel.workspace_symbols.as_mut().unwrap();
            if let Some((_, query)) = symbols.request.take() {
                symbols.loaded = Some(query);
            }
            symbols.state = SymbolState::Failed(CANCELLED.into());
        }
    }

    fn clear_status(&mut self, text: &str) {
        if self
            .status
            .as_ref()
            .is_some_and(|(message, _, _)| message == text)
        {
            self.status = None;
        }
    }

    /// Handles a failed hover, references or symbol request. Returns false
    /// for requests this module does not own.
    pub(super) fn language_intel_failed(&mut self, request_id: u64, error: Option<String>) -> bool {
        if self
            .intel
            .hover_flight
            .as_ref()
            .is_some_and(|origin| origin.id == request_id)
        {
            self.intel.hover_flight = None;
            if let Some(popup) = &mut self.intel.hover
                && popup.body == HoverBody::Requested(request_id)
            {
                popup.body = HoverBody::Ready(None);
            }
            return true;
        }
        if let Some(view) = &mut self.intel.references
            && view
                .request
                .as_ref()
                .is_some_and(|origin| origin.id == request_id)
        {
            view.request = None;
            view.state = ReferencesState::Failed(
                error.unwrap_or_else(|| "Language analysis could not find references.".into()),
            );
            self.clear_status(FINDING_REFERENCES);
            return true;
        }
        if let Some(symbols) = &mut self.intel.document_symbols
            && symbols
                .request
                .as_ref()
                .is_some_and(|origin| origin.id == request_id)
        {
            symbols.request = None;
            symbols.state = SymbolState::Failed(
                error.unwrap_or_else(|| "Language analysis could not list symbols.".into()),
            );
            return true;
        }
        if let Some(symbols) = &mut self.intel.workspace_symbols
            && symbols
                .request
                .as_ref()
                .is_some_and(|(origin, _)| origin.id == request_id)
        {
            let (_, query) = symbols.request.take().unwrap();
            symbols.loaded = Some(query);
            symbols.state = SymbolState::Failed(
                error.unwrap_or_else(|| "Language analysis could not search symbols.".into()),
            );
            return true;
        }
        false
    }

    pub(super) fn hover_result(&mut self, request_id: u64, contents: Option<HoverContents>) {
        if self
            .intel
            .hover_flight
            .as_ref()
            .is_none_or(|origin| origin.id != request_id)
        {
            return;
        }
        self.intel.hover_flight = None;
        let current = self.intel.hover.as_ref().is_some_and(|popup| {
            popup.body == HoverBody::Requested(request_id)
                && self
                    .intel_tab(popup.target.tab_id)
                    .is_some_and(|tab| tab.revision == popup.target.revision)
        });
        if !current {
            return;
        }
        let document = contents.as_ref().and_then(hover_document);
        if let Some(popup) = &mut self.intel.hover {
            popup.body = HoverBody::Ready(document);
        }
        self.settle_keyboard_hover();
    }

    /// An explicit hover with nothing to show explains why instead.
    fn settle_keyboard_hover(&mut self) {
        let Some(popup) = &self.intel.hover else {
            return;
        };
        if popup.target.source != HoverSource::Keyboard || popup.visible() {
            return;
        }
        let message = match &popup.body {
            HoverBody::Ready(None) => Some("No hover information here".to_owned()),
            HoverBody::Unavailable(message) => message.clone(),
            _ => return,
        };
        self.intel.close_hover();
        match message {
            Some(message) => self.status(message, false),
            None => self.language_request_unavailable(),
        }
    }

    /// Shows hover information for the character at the cursor.
    pub(super) fn show_hover(&mut self) {
        let Some(tab) = self.tab().filter(|tab| !tab.loading) else {
            return;
        };
        if tab.agent_view != crate::cli::DocumentView::Source {
            return;
        }
        let target = HoverTarget {
            tab_id: tab.id,
            revision: tab.revision,
            index: tab.vim.cursor,
            source: HoverSource::Keyboard,
            anchor: self
                .intel
                .caret
                .or(self.editor_cursor_rect)
                .unwrap_or(egui::Rect::NOTHING),
        };
        self.completions = None;
        self.open_hover(target);
        self.pump_hover();
        self.settle_keyboard_hover();
    }

    fn open_hover(&mut self, target: HoverTarget) {
        let Some(project) = self.project() else {
            return;
        };
        let Some(tab) = project.tabs.iter().find(|tab| tab.id == target.tab_id) else {
            return;
        };
        let diagnostics = tab
            .path
            .as_ref()
            .and_then(|path| project.diagnostics.get(path))
            .map(|list| diagnostics_at(list, crate::lsp::char_to_position(&tab.text, target.index)))
            .unwrap_or_default();
        self.intel.hover = Some(HoverPopup {
            target,
            diagnostics,
            body: HoverBody::Pending,
        });
        self.intel.hover_rect = None;
    }

    /// Sends the pending hover once no other hover request is in flight.
    fn pump_hover(&mut self) {
        if self.intel.hover_flight.is_some() {
            return;
        }
        let Some(target) = self
            .intel
            .hover
            .as_ref()
            .filter(|popup| popup.body == HoverBody::Pending)
            .map(|popup| popup.target.clone())
        else {
            return;
        };
        let Some(tab) = self
            .intel_tab(target.tab_id)
            .filter(|tab| tab.revision == target.revision)
        else {
            self.intel.close_hover();
            return;
        };
        let Some(path) = tab.path.clone() else {
            if let Some(popup) = &mut self.intel.hover {
                popup.body = HoverBody::Unavailable(None);
            }
            return;
        };
        let position = crate::lsp::char_to_position(&tab.text, target.index);
        self.sync_languages(true);
        let body = match self.intel_request(Some(&path), Feature::Hover, |server| {
            server.hover(&path, position)
        }) {
            Lookup::Sent(origin) => {
                let id = origin.id;
                self.intel.hover_flight = Some(origin);
                HoverBody::Requested(id)
            }
            Lookup::Unsupported(message) => HoverBody::Unavailable(Some(message)),
            Lookup::Unavailable => HoverBody::Unavailable(None),
            Lookup::Busy => HoverBody::Ready(None),
        };
        if let Some(popup) = &mut self.intel.hover {
            popup.body = body;
        }
    }

    /// Whether pointer hover is available for the tab's language server.
    pub(super) fn pointer_hover_available(&self, tab: &EditorTab) -> bool {
        tab.path.as_ref().is_some_and(|path| {
            self.project()
                .and_then(|project| project.language_for(path))
                .is_some_and(|session| {
                    session.active()
                        && session
                            .server
                            .as_ref()
                            .is_some_and(|server| server.supports(Feature::Hover))
                })
        })
    }

    pub(super) fn hover_escape(&mut self) {
        if !self.intel.hover_visible() {
            return;
        }
        if !self
            .ctx
            .input(|input| input.key_pressed(Key::Escape) && input.modifiers.is_none())
        {
            return;
        }
        self.intel.close_hover();
        if !self.settings.vim {
            self.ctx
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
        }
    }

    /// Tracks pointer rest, keeps requests flowing and draws the hover popup.
    pub(super) fn hover_ui(&mut self) {
        self.update_hover();
        self.hover_popup_ui();
    }

    pub(super) fn update_hover(&mut self) {
        let blocked = !self.intel.interactive
            || self.palette.is_some()
            || self.completions.is_some()
            || self.screen_jump.is_some();
        if blocked {
            self.intel.close_hover();
            self.intel.rest = None;
            return;
        }
        // An explicit hover follows its caret and closes when the cursor moves.
        let cursor = self.tab().map(|tab| (tab.id, tab.revision, tab.vim.cursor));
        if let Some(popup) = &mut self.intel.hover
            && popup.target.source == HoverSource::Keyboard
        {
            let current = cursor
                == Some((
                    popup.target.tab_id,
                    popup.target.revision,
                    popup.target.index,
                ));
            match self.intel.caret {
                Some(caret) if current => popup.target.anchor = caret,
                _ => self.intel.close_hover(),
            }
        }
        self.track_pointer_rest();
        self.pump_hover();
    }

    fn track_pointer_rest(&mut self) {
        let pointer = self.intel.pointer.take();
        let hover_pos = self.ctx.input(|input| input.pointer.hover_pos());
        let pointer_popup = self
            .intel
            .hover
            .as_ref()
            .filter(|popup| popup.target.source == HoverSource::Pointer);
        let keep_open = pointer_popup.is_some_and(|popup| {
            hover_pos.is_some_and(|position| {
                popup.target.anchor.expand(2.0).contains(position)
                    || self
                        .intel
                        .hover_rect
                        .is_some_and(|rect| rect.expand(8.0).contains(position))
            })
        });
        match pointer {
            Some(target) => {
                let same_word = self.intel.rest.as_ref().is_some_and(|rest| {
                    rest.target.tab_id == target.tab_id
                        && rest.target.pane_id == target.pane_id
                        && rest.target.revision == target.revision
                        && rest.target.word == target.word
                });
                if !same_word {
                    if pointer_popup.is_some() {
                        self.intel.hover = None;
                        self.intel.hover_rect = None;
                    }
                    self.intel.rest = Some(Rest {
                        target,
                        since: Instant::now(),
                        fired: false,
                    });
                } else if let Some(rest) = &mut self.intel.rest
                    && !rest.fired
                    && rest.target.pointer != target.pointer
                {
                    // Moving cancels a pending hover; only a resting pointer asks.
                    rest.since = Instant::now();
                    rest.target = target;
                }
            }
            None if keep_open => {}
            None => {
                if pointer_popup.is_some() {
                    self.intel.hover = None;
                    self.intel.hover_rect = None;
                }
                self.intel.rest = None;
            }
        }
        let Some(rest) = &mut self.intel.rest else {
            return;
        };
        if rest.fired {
            return;
        }
        let elapsed = rest.since.elapsed();
        if elapsed < HOVER_DELAY {
            self.ctx.request_repaint_after(HOVER_DELAY - elapsed);
            return;
        }
        rest.fired = true;
        let target = HoverTarget {
            tab_id: rest.target.tab_id,
            revision: rest.target.revision,
            index: rest.target.index,
            source: HoverSource::Pointer,
            anchor: rest.target.rect,
        };
        self.open_hover(target);
    }

    fn hover_popup_ui(&mut self) {
        let Some(popup) = self.intel.hover.as_ref().filter(|popup| popup.visible()) else {
            self.intel.hover_rect = None;
            return;
        };
        let ctx = self.ctx.clone();
        let screen = ctx.content_rect();
        let anchor = popup.target.anchor;
        // A caret anchor has no width; it only needs to be on screen.
        if !anchor.is_finite() || !screen.intersects(anchor.expand(1.0)) {
            return;
        }
        let width = HOVER_MAX_WIDTH.min(screen.width() - 24.0).max(160.0);
        let previous_height = self
            .intel
            .hover_rect
            .map_or(HOVER_MAX_HEIGHT, |rect| rect.height());
        let below = screen.bottom() - anchor.bottom();
        let above = anchor.top() - screen.top();
        let (position, pivot) = if below >= previous_height + 8.0 || below >= above {
            (
                egui::pos2(anchor.left(), anchor.bottom() + 2.0),
                egui::Align2::LEFT_TOP,
            )
        } else {
            (
                egui::pos2(anchor.left(), anchor.top() - 2.0),
                egui::Align2::LEFT_BOTTOM,
            )
        };
        let font = (self.settings.font_size - 1.0).clamp(11.0, 16.0);
        // Auto-sized areas keep their previous size; new content gets a fresh
        // area so its sizing pass fits the scroll view to the whole text.
        let identity = (
            popup.target.tab_id,
            popup.target.revision,
            popup.target.index,
            popup.diagnostics.len(),
            matches!(popup.body, HoverBody::Ready(Some(_))),
        );
        let response = egui::Area::new(egui::Id::new(("language_hover", identity)))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .pivot(pivot)
            .constrain(true)
            .interactable(true)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0, crate::palette::BORDER_STRONG))
                    .corner_radius(6.0)
                    .inner_margin(Margin::symmetric(10, 8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 14,
                        spread: 0,
                        color: Color32::from_black_alpha(70),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(width);
                        egui::ScrollArea::vertical()
                            .id_salt("language_hover_scroll")
                            .max_height(HOVER_MAX_HEIGHT)
                            .auto_shrink([true, true])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                for (severity, message) in &popup.diagnostics {
                                    let (label, color) = severity_label(*severity);
                                    ui.horizontal_top(|ui| {
                                        ui.label(
                                            RichText::new(label).size(11.0).strong().color(color),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(message).size(12.5).color(TEXT),
                                            )
                                            .wrap()
                                            .selectable(false),
                                        );
                                    });
                                }
                                if let HoverBody::Ready(Some(document)) = &popup.body {
                                    if !popup.diagnostics.is_empty() {
                                        ui.separator();
                                    }
                                    hover_document_ui(ui, document, font);
                                }
                            });
                    });
            });
        self.intel.hover_rect = Some(response.response.rect);
    }

    /// Requests references to the symbol at the cursor and shows them in the
    /// Search sidebar.
    pub(super) fn find_references(&mut self) {
        let Some(tab) = self.tab().filter(|tab| !tab.loading) else {
            return;
        };
        let Some(path) = tab.path.clone() else {
            self.language_request_unavailable();
            return;
        };
        let origin = (tab.id, tab.revision, tab.vim.cursor);
        let position = crate::lsp::char_to_position(&tab.text, tab.vim.cursor);
        let symbol = word_at(&tab.text, tab.vim.cursor)
            .map(|(_, word)| word)
            .unwrap_or_default();
        self.sync_languages(true);
        let request = match self.intel_request(Some(&path), Feature::References, |server| {
            server.references(&path, position)
        }) {
            Lookup::Sent(request) => request,
            Lookup::Unsupported(message) => {
                self.status(message, false);
                return;
            }
            Lookup::Unavailable => {
                self.language_request_unavailable();
                return;
            }
            Lookup::Busy => {
                self.status(
                    "Language analysis is busy or unavailable. Try again or restart language analysis.",
                    true,
                );
                return;
            }
        };
        let root = self
            .project()
            .map(|project| project.root.clone())
            .unwrap_or_default();
        self.intel.references = Some(ReferencesView {
            request: Some(request),
            symbol,
            origin,
            origin_path: path,
            root,
            state: ReferencesState::Loading,
            selected: 0,
            focus_selected: false,
            row_focused: false,
        });
        self.show_sidebar(SidebarView::Search);
        self.status(FINDING_REFERENCES, false);
    }

    pub(super) fn references_result(
        &mut self,
        request_id: u64,
        references: Vec<Reference>,
        truncated: bool,
    ) {
        let unchanged = self.tab().map(|tab| (tab.id, tab.revision, tab.vim.cursor));
        let Some(view) = &mut self.intel.references else {
            return;
        };
        if view
            .request
            .as_ref()
            .is_none_or(|origin| origin.id != request_id)
        {
            return;
        }
        view.request = None;
        let count = references.len();
        let groups = group_references(references, &view.origin_path, &view.root);
        view.state = ReferencesState::Loaded { groups, truncated };
        view.selected = 0;
        // Move to the list only when the user still waits at the same place.
        view.focus_selected = count > 0 && unchanged == Some(view.origin);
        self.clear_status(FINDING_REFERENCES);
        if count == 0 {
            self.status("No references found", false);
        }
        self.ctx.request_repaint();
    }

    pub(super) fn close_references(&mut self) {
        let focused = self
            .intel
            .references
            .take()
            .is_some_and(|view| view.row_focused);
        self.clear_status(FINDING_REFERENCES);
        if focused {
            self.focus_editor();
        }
    }

    pub(super) fn references_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut view) = self.intel.references.take() else {
            return;
        };
        let mut close = false;
        let mut open = None;
        let mut escape = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("References").size(12.5).strong().color(TEXT));
            if !view.symbol.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(&view.symbol)
                            .monospace()
                            .size(12.0)
                            .color(MUTED),
                    )
                    .truncate(),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = small_button(ui, "×", "Close references and return to project search")
                    .clicked();
            });
        });
        ui.add_space(4.0);
        match &view.state {
            ReferencesState::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(FINDING_REFERENCES).size(11.0).color(MUTED));
                });
            }
            ReferencesState::Failed(message) => {
                ui.add(egui::Label::new(RichText::new(message).size(11.5).color(ORANGE)).wrap());
            }
            ReferencesState::Loaded { groups, .. } if groups.is_empty() => {
                ui.label(RichText::new("No references").size(12.0).color(MUTED));
            }
            ReferencesState::Loaded { groups, truncated } => {
                let total: usize = groups.iter().map(|group| group.items.len()).sum();
                ui.label(
                    RichText::new(format!(
                        "{total} {} in {} {}",
                        if total == 1 {
                            "reference"
                        } else {
                            "references"
                        },
                        groups.len(),
                        if groups.len() == 1 { "file" } else { "files" },
                    ))
                    .size(11.0)
                    .color(MUTED),
                );
                if *truncated {
                    ui.colored_label(ORANGE, "Showing the first references only.");
                }
                let mut moved = None;
                let mut row_focused = false;
                let pending_focus = view.focus_selected.then_some(view.selected);
                egui::ScrollArea::vertical()
                    .id_salt("references_results")
                    .show(ui, |ui| {
                        let mut flat = 0;
                        for group in groups {
                            ui.add_space(9.0);
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&group.label).size(12.0).strong().color(TEXT),
                                    )
                                    .truncate(),
                                );
                                ui.label(
                                    RichText::new(group.items.len().to_string())
                                        .size(10.5)
                                        .color(MUTED),
                                );
                            });
                            for reference in &group.items {
                                let index = flat;
                                flat += 1;
                                let preview = if reference.preview.is_empty() {
                                    format!("Line {}", reference.line)
                                } else {
                                    reference.preview.clone()
                                };
                                let response = ui
                                    .push_id(("reference", index), |ui| {
                                        design::left_row(
                                            ui,
                                            &preview,
                                            Some((&reference.line.to_string(), MUTED)),
                                            index == view.selected,
                                            &format!(
                                                "{}:{}:{}",
                                                crate::paths::display(&reference.path),
                                                reference.line,
                                                reference.column
                                            ),
                                        )
                                    })
                                    .inner;
                                if pending_focus == Some(index) {
                                    response.request_focus();
                                    response.scroll_to_me(Some(Align::Center));
                                }
                                if response.clicked() {
                                    view.selected = index;
                                    open = Some(reference.clone());
                                }
                                if response.has_focus() {
                                    if pending_focus.is_none() {
                                        view.selected = index;
                                    }
                                    row_focused = true;
                                    self.register_region_focus_widget(response.id);
                                    ui.memory_mut(|memory| {
                                        memory.set_focus_lock_filter(
                                            response.id,
                                            egui::EventFilter {
                                                vertical_arrows: true,
                                                escape: true,
                                                ..Default::default()
                                            },
                                        )
                                    });
                                    if crate::design::focus_visible(ui.ctx()) {
                                        ui.painter().rect_stroke(
                                            response.rect.shrink(1.0),
                                            4.0,
                                            Stroke::new(1.0, ACCENT.gamma_multiply(0.7)),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    let keys = ui.input_mut(|input| {
                                        [
                                            Key::ArrowDown,
                                            Key::ArrowUp,
                                            Key::Home,
                                            Key::End,
                                            Key::Escape,
                                        ]
                                        .map(|key| input.consume_key(egui::Modifiers::NONE, key))
                                    });
                                    moved = match keys {
                                        [true, ..] => Some(index + 1),
                                        [_, true, ..] => Some(index.saturating_sub(1)),
                                        [_, _, true, ..] => Some(0),
                                        [_, _, _, true, _] => Some(usize::MAX),
                                        _ => moved,
                                    };
                                    escape |= keys[4];
                                }
                            }
                        }
                    });
                view.focus_selected = false;
                view.row_focused = row_focused;
                if let Some(next) = moved {
                    let last = total.saturating_sub(1);
                    view.selected = next.min(last);
                    view.focus_selected = true;
                    ui.ctx().request_repaint();
                }
            }
        }
        if !close {
            self.intel.references = Some(view);
        }
        if close {
            self.close_references();
        } else if escape {
            self.focus_editor();
        }
        if let Some(reference) = open {
            self.open_language_location(reference.path, reference.position);
        }
    }

    /// Rows for the `@` and `#` modes of Go to file, or None for file search.
    pub(super) fn symbol_palette(&mut self, scope: SymbolScope, query: &str) -> SymbolPalette {
        match scope {
            SymbolScope::Document => self.document_symbol_palette(query),
            SymbolScope::Workspace => self.workspace_symbol_palette(query),
        }
    }

    fn document_symbol_palette(&mut self, query: &str) -> SymbolPalette {
        let message = |text: &str| SymbolPalette {
            rows: Vec::new(),
            loading: false,
            message: Some(text.into()),
        };
        let Some(tab) = self.tab().filter(|tab| !tab.loading) else {
            return message("Open a file to list its symbols.");
        };
        let Some(path) = tab.path.clone() else {
            return message("Save the file to list its symbols.");
        };
        let (tab_id, revision) = (tab.id, tab.revision);
        let stale = self.intel.document_symbols.as_ref().is_none_or(|symbols| {
            symbols.tab_id != tab_id || symbols.revision != revision || symbols.path != path
        });
        let idle = self
            .intel
            .document_symbols
            .as_ref()
            .is_none_or(|symbols| symbols.request.is_none() || symbols.tab_id != tab_id);
        if stale && idle {
            self.sync_languages(true);
            let lookup = self.intel_request(Some(&path), Feature::DocumentSymbols, |server| {
                server.document_symbols(&path)
            });
            let keep = self
                .intel
                .document_symbols
                .take()
                .filter(|symbols| symbols.tab_id == tab_id && symbols.path == path)
                .map(|symbols| symbols.state)
                .filter(|state| matches!(state, SymbolState::Loaded(_)));
            let (request, state) = match lookup {
                Lookup::Sent(origin) => (Some(origin), keep.unwrap_or(SymbolState::Loading)),
                Lookup::Unsupported(message) => (None, SymbolState::Failed(message)),
                Lookup::Unavailable => (
                    None,
                    SymbolState::Failed("Language analysis is not available for this file.".into()),
                ),
                Lookup::Busy => (
                    None,
                    SymbolState::Failed("Language analysis is busy. Try again.".into()),
                ),
            };
            self.intel.document_symbols = Some(DocumentSymbols {
                tab_id,
                revision,
                path: path.clone(),
                request,
                state,
            });
        }
        let symbols = self.intel.document_symbols.as_ref().unwrap();
        let loading = symbols.request.is_some();
        match &symbols.state {
            SymbolState::Loading => SymbolPalette {
                rows: Vec::new(),
                loading: true,
                message: None,
            },
            SymbolState::Failed(text) => message(text),
            SymbolState::Loaded(list) => {
                let filtering = !query.trim().is_empty();
                let rows: Vec<PaletteRow> = filter_symbols(list, query)
                    .into_iter()
                    .map(|symbol| {
                        let indent = if filtering { 0 } else { symbol.depth.min(6) };
                        (
                            format!("{}{}", "    ".repeat(indent), symbol.name),
                            symbol_detail(symbol, None),
                            Some(path.clone()),
                            Some(symbol.position),
                        )
                    })
                    .collect();
                let message = rows.is_empty().then(|| {
                    if list.is_empty() {
                        "No symbols in this file".to_owned()
                    } else {
                        "No matching symbols".to_owned()
                    }
                });
                SymbolPalette {
                    rows,
                    loading,
                    message,
                }
            }
        }
    }

    fn workspace_symbol_palette(&mut self, query: &str) -> SymbolPalette {
        let message = |text: &str| SymbolPalette {
            rows: Vec::new(),
            loading: false,
            message: Some(text.into()),
        };
        let Some(root) = self.project().map(|project| project.root.clone()) else {
            return message("Open a project to search its symbols.");
        };
        if self
            .intel
            .workspace_symbols
            .as_ref()
            .is_none_or(|symbols| symbols.root != root)
        {
            self.intel.workspace_symbols = Some(WorkspaceSymbols {
                root: root.clone(),
                request: None,
                loaded: None,
                state: SymbolState::Loaded(Vec::new()),
            });
        }
        if query.is_empty() {
            return message("Type a symbol name to search this workspace.");
        }
        let symbols = self.intel.workspace_symbols.as_ref().unwrap();
        if symbols.request.is_none() && symbols.loaded.as_deref() != Some(query) {
            let path = self.tab().and_then(|tab| tab.path.clone()).filter(|path| {
                self.project()
                    .and_then(|project| project.language_for(path))
                    .is_some_and(|session| {
                        session.active()
                            && session
                                .server
                                .as_ref()
                                .is_some_and(|server| server.supports(Feature::WorkspaceSymbols))
                    })
            });
            self.sync_languages(true);
            let lookup = self.intel_request(path.as_deref(), Feature::WorkspaceSymbols, |server| {
                server.workspace_symbols(query)
            });
            let symbols = self.intel.workspace_symbols.as_mut().unwrap();
            match lookup {
                Lookup::Sent(origin) => symbols.request = Some((origin, query.to_owned())),
                Lookup::Unsupported(text) => {
                    symbols.loaded = Some(query.to_owned());
                    symbols.state = SymbolState::Failed(text);
                }
                Lookup::Unavailable => {
                    symbols.loaded = Some(query.to_owned());
                    symbols.state = SymbolState::Failed(
                        "No language server in this project provides workspace symbols.".into(),
                    );
                }
                Lookup::Busy => {
                    symbols.loaded = Some(query.to_owned());
                    symbols.state =
                        SymbolState::Failed("Language analysis is busy. Try again.".into());
                }
            }
        }
        let symbols = self.intel.workspace_symbols.as_ref().unwrap();
        let loading = symbols.request.is_some();
        match &symbols.state {
            SymbolState::Failed(text) if !loading => message(text),
            SymbolState::Loaded(list) => {
                let rows: Vec<PaletteRow> = filter_symbols(list, query)
                    .into_iter()
                    .filter(|symbol| symbol.path.is_some())
                    .map(|symbol| {
                        (
                            symbol.name.clone(),
                            symbol_detail(symbol, Some(&root)),
                            symbol.path.clone(),
                            Some(symbol.position),
                        )
                    })
                    .collect();
                let message = (rows.is_empty() && !loading).then(|| "No matching symbols".into());
                SymbolPalette {
                    rows,
                    loading,
                    message,
                }
            }
            _ => SymbolPalette {
                rows: Vec::new(),
                loading,
                message: None,
            },
        }
    }

    pub(super) fn symbols_result(&mut self, request_id: u64, symbols: Vec<Symbol>) {
        if let Some(document) = &mut self.intel.document_symbols
            && document
                .request
                .as_ref()
                .is_some_and(|origin| origin.id == request_id)
        {
            document.request = None;
            document.state = SymbolState::Loaded(symbols);
            self.ctx.request_repaint();
            return;
        }
        if let Some(workspace) = &mut self.intel.workspace_symbols
            && workspace
                .request
                .as_ref()
                .is_some_and(|(origin, _)| origin.id == request_id)
        {
            let (_, query) = workspace.request.take().unwrap();
            workspace.loaded = Some(query);
            workspace.state = SymbolState::Loaded(symbols);
            self.ctx.request_repaint();
        }
    }

    /// Opens Go to file in one of its symbol modes.
    pub(super) fn open_symbol_palette(&mut self, scope: SymbolScope) {
        self.open_palette(Palette::Files);
        self.palette_query = match scope {
            SymbolScope::Document => "@".into(),
            SymbolScope::Workspace => "#".into(),
        };
    }
}
