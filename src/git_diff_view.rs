//! A cached, virtualized unified diff viewer. Parsing never touches editor buffers.

use crate::{design, design::HoverDetails, render};
use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, Stroke, Vec2};
use std::ops::Range;
use std::sync::Arc;

const FONT_SIZE: f32 = 12.0;
const TAB_WIDTH: usize = 4;
const ROW_PADDING: f32 = 7.0;
const TEXT_INSET: f32 = 10.0;

/// A resizable Git dialog with a real input-blocking backdrop. Parent dialogs
/// remain visible but disabled while a child diff owns the modal stack.
pub(crate) fn dialog(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    close_label: &str,
    preferred_size: Vec2,
    enabled: bool,
    content: impl FnOnce(&mut egui::Ui),
) -> egui::ModalResponse<bool> {
    let maximum = (ctx.content_rect().size() - Vec2::splat(64.0)).max(Vec2::new(260.0, 200.0));
    let minimum = Vec2::new(440.0, 280.0).min(maximum);
    let modal = egui::Modal::new(id).show(ctx, |ui| {
        if !enabled {
            ui.disable();
        }
        egui::Resize::default()
            .id(id.with("size"))
            .default_size(preferred_size.min(maximum))
            .min_size(minimum)
            .max_size(maximum)
            .with_stroke(false)
            .show(ui, |ui| {
                let mut close = false;
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new((ui.available_width() - 36.0).max(60.0), 26.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(title).size(14.0).strong())
                                    .truncate(),
                            )
                            .on_hover_details(title);
                            ui.allocate_space(Vec2::new(ui.available_width(), 0.0));
                        },
                    );
                    let response = ui
                        .add_sized([26.0, 26.0], egui::Button::new("×").frame(false))
                        .on_hover_text(format!("{close_label} · Esc or click outside"));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, close_label)
                    });
                    ctx.data_mut(|data| data.insert_temp(id.with("close_focus"), response.id));
                    if enabled && ctx.memory(|memory| memory.focused().is_none()) {
                        response.request_focus();
                    }
                    close |= response.clicked();
                });
                ui.separator();
                content(ui);
                close
            })
    });
    ctx.data_mut(|data| data.insert_temp(id.with("content_rect"), modal.response.rect));
    modal
}

#[derive(Default)]
pub(crate) struct DiffView {
    patch: ParsedPatch,
    title: String,
    generation: u64,
    wrapped: WrappedRows,
    details_open: bool,
    last_frame: Option<u64>,
    /// The clicked row, which Enter or o opens.
    selected: Option<usize>,
}

/// Where the reader asked to open the file of a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffOpen {
    /// Repository-relative path.
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) deleted: bool,
}

impl DiffView {
    /// Forget the focus and clicked row of the closed diff.
    pub(crate) fn reset_focus(&mut self) {
        self.last_frame = None;
        self.selected = None;
    }

    /// Show a read-only patch. The raw patch is also what the copy action
    /// returns. Double-click, Enter or o asks to open the file at a line.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, title: &str, text: &str) -> Option<DiffOpen> {
        let frame = ui.ctx().cumulative_frame_nr();
        let focus_contents = self
            .last_frame
            .is_none_or(|last| frame > last.saturating_add(1))
            || self.title != title
            || self.patch.source != text;
        self.last_frame = Some(frame);
        self.update(title, text);
        let palette = Palette::new(ui);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if self.patch.files > 1 {
                let noun = if self.patch.files == 1 {
                    "file"
                } else {
                    "files"
                };
                ui.label(
                    RichText::new(format!("{} {noun}", self.patch.files))
                        .size(12.0)
                        .color(palette.muted),
                );
            }
            if self.patch.file_details.len() == 1 {
                ui.label(
                    RichText::new(self.patch.file_details[0].status)
                        .size(12.0)
                        .color(palette.muted),
                );
            }
            ui.label(
                RichText::new(format!("+{}", self.patch.additions))
                    .monospace()
                    .size(12.0)
                    .color(palette.added),
            )
            .on_hover_text("Lines added; file headers are excluded.
Long lines wrap to fit the dialog. Scroll: j / k · Page Up / Down · Ctrl+F / B · Home / End. Esc closes the diff.");
            ui.label(
                RichText::new(format!("−{}", self.patch.deletions))
                    .monospace()
                    .size(12.0)
                    .color(palette.removed),
            )
            .on_hover_text("Lines removed; file headers are excluded");
            if self.patch.binary {
                ui.label(
                    RichText::new("Binary changes")
                        .size(12.0)
                        .color(palette.muted),
                )
                .on_hover_text("Binary files have no text diff or added/removed line counts");
            }
            if ui
                .add_enabled_ui(!text.is_empty(), |ui| {
                    design::ghost_button(ui, "Copy patch", "Copy the complete original patch")
                })
                .inner
                .clicked()
            {
                ui.ctx().copy_text(text.to_owned());
            }
            if !self.patch.details.is_empty()
                && design::ghost_button(
                    ui,
                    if self.details_open {
                        "Hide details"
                    } else {
                        "Details"
                    },
                    "File metadata and changed line ranges",
                )
                .clicked()
            {
                self.details_open = !self.details_open;
            }
        });
        if self.details_open {
            self.show_details(ui, &palette);
        }
        ui.separator();

        if self.patch.presentation.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(36.0);
                ui.label(RichText::new("No changes in this comparison").strong());
                ui.label(
                    RichText::new("There is no text patch to display.")
                        .size(12.0)
                        .color(palette.muted),
                );
            });
            return None;
        }

        // Reserve the vertical bar before measuring so its appearance cannot
        // change wrapping. Text never widens this scroll area horizontally.
        let content_width = (ui.available_width() - ui.spacing().scroll.allocated_width())
            .floor()
            .max(1.0);
        let metrics = RowMetrics::new(ui, self.patch.number_digits, content_width);
        let row_height = metrics.row_height;
        self.wrapped.update(ui, &self.patch, &metrics);
        let salt = ("git_unified_diff", title, self.generation);
        let output = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // ScrollArea wraps its salt in IdSalt before building the ID.
                // Use that same wrapper so wheel offsets and limits refer to
                // its persisted state rather than an unrelated empty state.
                let scroll_id = ui.make_persistent_id(egui::IdSalt::new(salt));
                let body_id = ui.make_persistent_id("diff_keyboard_surface");
                let body = ui.interact(
                    ui.available_rect_before_wrap().intersect(ui.clip_rect()),
                    body_id,
                    Sense::click(),
                );
                body.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Other,
                        ui.is_enabled(),
                        "Diff contents",
                    )
                });
                if focus_contents || body.clicked() {
                    body.request_focus();
                }
                let pointer = (body.clicked() || body.double_clicked())
                    .then(|| body.interact_pointer_pos())
                    .flatten();
                let double_clicked = body.double_clicked();
                let open_key = take_open_key(ui, body_id);
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt(salt)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .content_margin(egui::Margin::ZERO)
                    .auto_shrink([false, false])
                    .animated(false)
                    .horizontal_scroll_offset(0.0)
                    .scroll_source(egui::scroll_area::ScrollSource {
                        mouse_wheel: false,
                        ..Default::default()
                    });
                let wheel_offset = render::instant_scroll_offset_axes(
                    ui,
                    scroll_id,
                    egui::Vec2b::new(false, true),
                );
                let offset = keyboard_scroll_offset(
                    ui,
                    scroll_id,
                    body_id,
                    row_height,
                    self.wrapped.height(),
                    wheel_offset,
                )
                .or(wheel_offset)
                .unwrap_or_else(|| {
                    egui::scroll_area::State::load(ui.ctx(), scroll_id)
                        .unwrap_or_default()
                        .offset
                });
                // A resize can shorten wrapped rows while scrolled near the
                // end. Clamp immediately, before painting the new viewport.
                scroll = scroll.scroll_offset(Vec2::new(
                    0.0,
                    offset.y.clamp(
                        0.0,
                        (self.wrapped.height() - ui.available_height()).max(0.0),
                    ),
                ));
                let output = scroll.show_viewport(ui, |ui, viewport| {
                    ui.set_min_size(Vec2::new(content_width, self.wrapped.height()));
                    let origin = ui.max_rect().min;
                    let mut open = None;
                    if let Some(pointer) = pointer {
                        let y = pointer.y - origin.y;
                        if (0.0..self.wrapped.height()).contains(&y) {
                            let row = self.wrapped.visible(y..y + 0.01).start;
                            self.selected = Some(row);
                            if double_clicked {
                                open = Some(row);
                            }
                        }
                    }
                    if open_key {
                        open = self.selected.or_else(|| {
                            Some(
                                self.wrapped
                                    .visible(viewport.top()..viewport.top() + 1.0)
                                    .start,
                            )
                        });
                    }
                    for index in self.wrapped.visible(viewport.top()..viewport.bottom()) {
                        let rect = egui::Rect::from_min_max(
                            origin + Vec2::new(0.0, self.wrapped.offsets[index]),
                            origin + Vec2::new(content_width, self.wrapped.offsets[index + 1]),
                        );
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        paint_row(
                            ui,
                            &palette,
                            &self.patch,
                            index,
                            rect,
                            &metrics,
                            body_id.with(index),
                        );
                        if self.selected == Some(index) {
                            paint_selected_row(ui, rect);
                        }
                    }
                    open
                });
                render::remember_scroll_limits(ui.ctx(), &output);
                output.inner
            })
            .inner;
        let row = output?;
        let file = self.patch.file_at(row)?;
        Some(DiffOpen {
            path: file.path.to_owned(),
            line: self.patch.new_line_at(row),
            deleted: file.deleted,
        })
    }

    fn update(&mut self, title: &str, text: &str) {
        if self.title != title || self.patch.source != text {
            self.selected = None;
            self.patch = ParsedPatch::parse(text);
            self.title = title.to_owned();
            self.generation = self.generation.wrapping_add(1);
            self.wrapped = WrappedRows::default();
            self.details_open = false;
        }
    }

    fn show_details(&self, ui: &mut egui::Ui, palette: &Palette) {
        let height = (ui.available_height() * 0.35).clamp(36.0, 160.0);
        let font = FontId::monospace(11.0);
        let width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M'))
            * self.patch.detail_columns as f32
            + 16.0;
        egui::ScrollArea::both()
            .id_salt(("git_diff_details", self.generation))
            .max_height(height)
            .auto_shrink([false, true])
            .animated(false)
            .show_rows(ui, 18.0, self.patch.details.len(), |ui, visible| {
                ui.set_min_width(width.max(ui.available_width()));
                for index in visible {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&self.patch.details[index])
                                .font(font.clone())
                                .color(palette.muted),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                }
            });
    }
}

/// Prefix heights virtualize logical rows of different wrapped heights. Keep
/// only geometry, rather than every off-screen galley's glyphs and meshes.
#[derive(Default)]
pub(crate) struct WrappedRows {
    offsets: Vec<f32>,
    text_width: f32,
    character_width: f32,
    font_height: f32,
    pixels_per_point: f32,
    probe: Option<Arc<egui::Galley>>,
}

impl WrappedRows {
    pub(crate) fn update(&mut self, ui: &egui::Ui, patch: &ParsedPatch, metrics: &RowMetrics) {
        let font = &metrics.font;
        let (text_width, character_width, font_height) = (
            metrics.text_width,
            metrics.character_width,
            metrics.font_height,
        );
        // Refresh this probe even when geometry is cached: its identity also
        // detects replaced fonts whose ASCII metrics happen to be identical.
        let probe = ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "vedit diff font generation".into(),
                FontId::monospace(1.0),
                Color32::WHITE,
            )
        });
        let pixels_per_point = ui.ctx().pixels_per_point();
        if self.offsets.len() == patch.presentation.len() + 1
            && self.text_width == text_width
            && self.character_width == character_width
            && self.font_height == font_height
            && self.pixels_per_point == pixels_per_point
            && self
                .probe
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &probe))
        {
            return;
        }
        self.text_width = text_width;
        self.character_width = character_width;
        self.font_height = font_height;
        self.pixels_per_point = pixels_per_point;
        self.probe = Some(probe);
        self.offsets.clear();
        self.offsets.reserve(patch.presentation.len() + 1);
        self.offsets.push(0.0);
        let single_height = (font_height + ROW_PADDING).max(20.0);
        for index in 0..patch.presentation.len() {
            let display = patch.display(index);
            // Short ASCII monospace lines need no off-screen text layout. A
            // spare column covers fractional advances at different DPI values.
            let height = if display.is_ascii()
                && (display.len() + 1) as f32 * character_width <= text_width
            {
                single_height
            } else {
                let height = ui.fonts_mut(|fonts| {
                    // The public layout function uses the same shaping and
                    // wrapping as FontsView, without retaining off-screen
                    // galleys in egui's frame cache. Drop each row's temporary
                    // glyphs/meshes immediately after reading its exact height.
                    let job = egui::text::LayoutJob::simple(
                        display,
                        font.clone(),
                        Color32::PLACEHOLDER,
                        // Match FontsView's cache-key width normalization.
                        text_width.round(),
                    );
                    egui::epaint::text::layout(fonts.fonts, pixels_per_point, Arc::new(job))
                        .size()
                        .y
                });
                (height + ROW_PADDING).max(20.0)
            };
            self.offsets.push(self.height() + height);
        }
    }

    pub(crate) fn height(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    /// Top of row `index`; `index == len` is the bottom of the last row.
    pub(crate) fn offset(&self, index: usize) -> f32 {
        self.offsets
            .get(index)
            .copied()
            .unwrap_or_else(|| self.height())
    }

    pub(crate) fn visible(&self, viewport: Range<f32>) -> Range<usize> {
        let count = self.offsets.len().saturating_sub(1);
        let first = self
            .offsets
            .partition_point(|&y| y <= viewport.start)
            .saturating_sub(1)
            .min(count);
        let last = self
            .offsets
            .partition_point(|&y| y < viewport.end)
            .min(count)
            .max(first);
        first..last
    }
}

/// Enter or o on the focused diff opens the file.
fn take_open_key(ui: &egui::Ui, body_id: egui::Id) -> bool {
    let ctx = ui.ctx();
    if !ui.is_enabled()
        || egui::Popup::is_any_open(ctx)
        || !ctx.memory(|memory| memory.has_focus(body_id))
    {
        return false;
    }
    ctx.input_mut(|input| {
        let mut open = false;
        input.events.retain(|event| match event {
            egui::Event::Text(text) if text == "o" => {
                open = true;
                false
            }
            egui::Event::Key {
                key: egui::Key::Enter | egui::Key::O,
                pressed,
                modifiers,
                ..
            } if modifiers.is_none() => {
                open |= *pressed;
                false
            }
            _ => true,
        });
        open
    })
}

/// A quiet outline on the clicked row.
fn paint_selected_row(ui: &egui::Ui, rect: egui::Rect) {
    let accent = ui.visuals().selection.stroke.color;
    ui.painter().rect_filled(
        rect,
        0.0,
        ui.visuals().selection.bg_fill.gamma_multiply(0.35),
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
        0.0,
        accent,
    );
}

fn keyboard_scroll_offset(
    ui: &egui::Ui,
    scroll_id: egui::Id,
    body_id: egui::Id,
    row_height: f32,
    content_height: f32,
    wheel_offset: Option<Vec2>,
) -> Option<Vec2> {
    let ctx = ui.ctx();
    if !ui.is_enabled() || egui::Popup::is_any_open(ctx) {
        return None;
    }
    let top = ctx.memory(|memory| memory.areas().top_layer_id(ui.layer_id().order));
    if top.is_some_and(|top| top != ui.layer_id()) {
        return None;
    }
    // Opening the diff focuses its contents. A subsequent focus transfer to any
    // toolbar control or text widget must keep that control's own key handling.
    if let Some(focused) = ctx.memory(|memory| memory.focused())
        && focused != body_id
    {
        return None;
    }
    let viewport_height = ui.available_height().max(row_height);
    let maximum = (content_height - viewport_height).max(0.0);
    let mut offset = wheel_offset.unwrap_or_else(|| {
        egui::scroll_area::State::load(ctx, scroll_id)
            .unwrap_or_default()
            .offset
    });
    offset.x = 0.0;
    let mut changed = false;
    ctx.input_mut(|input| {
        let has_text_j = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Text(text) if text.contains('j')));
        let has_text_k = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Text(text) if text.contains('k')));
        let plain = input.modifiers.is_none();
        input.events.retain(|event| {
            if plain
                && let egui::Event::Text(text) = event
                && !text.is_empty()
                && text.chars().all(|ch| matches!(ch, 'j' | 'k'))
            {
                for ch in text.chars() {
                    offset.y = (offset.y + if ch == 'j' { row_height } else { -row_height })
                        .clamp(0.0, maximum);
                }
                changed = true;
                return false;
            }
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let destination = if modifiers.is_none() {
                match key {
                    egui::Key::J => Some(offset.y + if has_text_j { 0.0 } else { row_height }),
                    egui::Key::K => Some(offset.y - if has_text_k { 0.0 } else { row_height }),
                    egui::Key::PageDown => Some(offset.y + viewport_height * 0.9),
                    egui::Key::PageUp => Some(offset.y - viewport_height * 0.9),
                    egui::Key::Home => Some(0.0),
                    egui::Key::End => Some(maximum),
                    _ => None,
                }
            } else if modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.mac_cmd {
                match key {
                    egui::Key::F => Some(offset.y + viewport_height * 0.9),
                    egui::Key::B => Some(offset.y - viewport_height * 0.9),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(destination) = destination {
                offset.y = destination.clamp(0.0, maximum);
                changed = true;
                false
            } else {
                true
            }
        });
    });
    changed.then_some(offset)
}

/// Font and gutter geometry shared by every row of one diff surface.
pub(crate) struct RowMetrics {
    font: FontId,
    character_width: f32,
    font_height: f32,
    pub(crate) row_height: f32,
    number_width: f32,
    gutter_width: f32,
    text_width: f32,
}

impl RowMetrics {
    pub(crate) fn new(ui: &egui::Ui, number_digits: usize, content_width: f32) -> Self {
        let font = FontId::monospace(FONT_SIZE);
        let (character_width, font_height) =
            ui.fonts_mut(|fonts| (fonts.glyph_width(&font, 'M'), fonts.row_height(&font)));
        let number_width = number_digits.max(3) as f32 * character_width + 16.0;
        let gutter_width = number_width * 2.0;
        Self {
            row_height: (font_height + ROW_PADDING).max(20.0),
            text_width: (content_width - gutter_width - TEXT_INSET * 2.0).max(1.0),
            font,
            character_width,
            font_height,
            number_width,
            gutter_width,
        }
    }
}

/// Paint one presentation row: tint, old/new line numbers, divider and text.
pub(crate) fn paint_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    patch: &ParsedPatch,
    index: usize,
    rect: egui::Rect,
    metrics: &RowMetrics,
    id: egui::Id,
) {
    let RowMetrics {
        font,
        font_height,
        number_width,
        gutter_width,
        text_width,
        ..
    } = metrics;
    let presentation = &patch.presentation[index];
    let row = presentation.raw_row.map(|index| &patch.rows[index]);
    let kind = presentation.kind;
    let response = ui.interact(rect, id, Sense::hover());
    if let Some(tooltip) = &presentation.tooltip {
        response.on_hover_details(tooltip);
    }
    if let Some(fill) = palette.background(kind) {
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    let gutter_color = match kind {
        RowKind::Added => palette.added.gamma_multiply(0.75),
        RowKind::Removed => palette.removed.gamma_multiply(0.75),
        _ => palette.muted,
    };
    for (number, right) in [
        (
            row.and_then(|row| row.old_line),
            rect.left() + number_width - 8.0,
        ),
        (
            row.and_then(|row| row.new_line),
            rect.left() + gutter_width - 8.0,
        ),
    ] {
        if let Some(number) = number {
            ui.painter().text(
                egui::pos2(right, rect.top() + ROW_PADDING * 0.5 + font_height * 0.5),
                Align2::RIGHT_CENTER,
                number.to_string(),
                font.clone(),
                gutter_color,
            );
        }
    }
    let divider = rect.left() + gutter_width;
    ui.painter().line_segment(
        [
            egui::pos2(divider, rect.top()),
            egui::pos2(divider, rect.bottom()),
        ],
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    let display = patch.display(index);
    let color = palette.foreground(kind);
    let galley = ui.fonts_mut(|fonts| match palette.marker(kind) {
        // Changed code stays readable in the text color; the
        // tint and the +/− marker carry the change.
        Some(marker) if !display.is_empty() => {
            let split = display.chars().next().map_or(0, char::len_utf8);
            let mut job = egui::text::LayoutJob::default();
            for (range, section_color) in [(0..split, marker), (split..display.len(), color)] {
                job.append(
                    &display[range],
                    0.0,
                    egui::TextFormat::simple(font.clone(), section_color),
                );
            }
            job.wrap.max_width = *text_width;
            fonts.layout_job(job)
        }
        _ => fonts.layout_delayed_color(display, font.clone(), *text_width),
    });
    let position = egui::pos2(divider + TEXT_INSET, rect.top() + ROW_PADDING * 0.5);
    ui.painter().galley(position, galley, color);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    FileHeader,
    Metadata,
    Hunk,
    Context,
    Added,
    Removed,
    Note,
    Binary,
}

#[derive(Debug)]
struct DiffRow {
    source: Range<usize>,
    kind: RowKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
}

struct PresentationRow {
    raw_row: Option<usize>,
    text: String,
    kind: RowKind,
    tooltip: Option<String>,
}

impl PresentationRow {
    fn text(text: impl Into<String>, kind: RowKind) -> Self {
        Self {
            raw_row: None,
            text: text.into(),
            kind,
            tooltip: None,
        }
    }
}

struct FileDetail {
    path: Option<String>,
    previous: Option<String>,
    status: &'static str,
    old_mode: Option<String>,
    new_mode: Option<String>,
    objects: Option<String>,
    similarity: Option<String>,
    ranges: Vec<String>,
    rows: Vec<PresentationRow>,
    additions: usize,
    deletions: usize,
    old_header_seen: bool,
    binary: bool,
}

impl FileDetail {
    fn new() -> Self {
        Self {
            path: None,
            previous: None,
            status: "Modified",
            old_mode: None,
            new_mode: None,
            objects: None,
            similarity: None,
            ranges: Vec::new(),
            rows: Vec::new(),
            additions: 0,
            deletions: 0,
            old_header_seen: false,
            binary: false,
        }
    }

    fn path(&self) -> &str {
        self.path
            .as_deref()
            .or(self.previous.as_deref())
            .unwrap_or("Current file")
    }

    fn metadata(&mut self, line: &str) {
        for (prefix, status, old) in [
            ("new file mode ", "Added", false),
            ("deleted file mode ", "Deleted", true),
            ("old mode ", "Mode changed", true),
            ("new mode ", "Mode changed", false),
        ] {
            if let Some(mode) = line.strip_prefix(prefix) {
                self.status = status;
                if old {
                    self.old_mode = Some(mode.into());
                } else {
                    self.new_mode = Some(mode.into());
                }
                return;
            }
        }
        for (prefix, status, old) in [
            ("rename from ", "Renamed", true),
            ("rename to ", "Renamed", false),
            ("copy from ", "Copied", true),
            ("copy to ", "Copied", false),
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                self.status = status;
                if old {
                    self.previous = Some(decode_git_path(path));
                } else {
                    self.path = Some(decode_git_path(path));
                }
                return;
            }
        }
        if let Some(value) = line.strip_prefix("index ") {
            let mut fields = value.split_ascii_whitespace();
            if let Some((old, new)) = fields.next().and_then(|objects| objects.split_once("..")) {
                self.objects = Some(format!("{old} → {new}"));
            }
            if let Some(mode) = fields.next() {
                self.old_mode.get_or_insert_with(|| mode.into());
                self.new_mode.get_or_insert_with(|| mode.into());
            }
        } else if let Some(value) = line.strip_prefix("similarity index ") {
            self.similarity = Some(value.into());
        }
    }
}

#[derive(Default)]
pub(crate) struct ParsedPatch {
    source: String,
    rows: Vec<DiffRow>,
    additions: usize,
    deletions: usize,
    files: usize,
    binary: bool,
    max_columns: usize,
    number_digits: usize,
    presentation: Vec<PresentationRow>,
    file_details: Vec<FileDetail>,
    /// First presentation row of each entry of `file_details`.
    file_starts: Vec<usize>,
    details: Vec<String>,
    detail_columns: usize,
}

/// One changed or context line of a patch, without its +/−/space marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiffLine<'a> {
    /// `'+'`, `'-'` or `' '`.
    pub(crate) marker: char,
    pub(crate) old_line: Option<usize>,
    pub(crate) new_line: Option<usize>,
    pub(crate) text: &'a str,
}

/// A file of a patch, as a presentation row belongs to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PatchFile<'a> {
    /// Repository-relative path; the previous path of a deleted file.
    pub(crate) path: &'a str,
    pub(crate) deleted: bool,
}

impl ParsedPatch {
    /// The source line shown at presentation row `index`, when it is one.
    pub(crate) fn diff_line(&self, index: usize) -> Option<DiffLine<'_>> {
        let row = &self.rows[self.presentation.get(index)?.raw_row?];
        let marker = match row.kind {
            RowKind::Added => '+',
            RowKind::Removed => '-',
            RowKind::Context => ' ',
            _ => return None,
        };
        Some(DiffLine {
            marker,
            old_line: row.old_line,
            new_line: row.new_line,
            text: &self.source[row.source.start + 1..row.source.end],
        })
    }

    /// The file presentation row `index` belongs to.
    pub(crate) fn file_at(&self, index: usize) -> Option<PatchFile<'_>> {
        let file = self
            .file_starts
            .partition_point(|start| *start <= index)
            .checked_sub(1)?;
        let detail = self.file_details.get(file)?;
        Some(PatchFile {
            path: detail.path.as_deref().or(detail.previous.as_deref())?,
            deleted: detail.status == "Deleted",
        })
    }

    pub(crate) fn rows(&self) -> usize {
        self.presentation.len()
    }

    pub(crate) fn number_digits(&self) -> usize {
        self.number_digits
    }

    /// The new-side line shown at presentation row `index`; removed rows and
    /// hunk headers use the next new-side line, falling back to the previous.
    pub(crate) fn new_line_at(&self, index: usize) -> Option<usize> {
        let line = |index: usize| {
            self.presentation[index]
                .raw_row
                .and_then(|raw| self.rows[raw].new_line)
        };
        let index = index.min(self.presentation.len());
        (index..self.presentation.len())
            .find_map(line)
            .or_else(|| (0..index).rev().find_map(line))
    }

    fn display(&self, index: usize) -> String {
        let presentation = &self.presentation[index];
        match presentation.raw_row {
            Some(raw) => {
                let row = &self.rows[raw];
                display_line(&self.source[row.source.clone()], row.kind)
            }
            None => presentation.text.clone(),
        }
    }

    pub(crate) fn parse(text: &str) -> Self {
        let mut patch = Self {
            source: text.to_owned(),
            ..Self::default()
        };
        let mut offset = 0;
        let mut hunk: Option<Hunk> = None;
        let mut git_file_header = false;
        for piece in text.split_inclusive('\n') {
            let line = piece
                .trim_end_matches('\n')
                .strip_suffix('\r')
                .unwrap_or_else(|| piece.trim_end_matches('\n'));
            let source = offset..offset + line.len();
            offset += piece.len();
            let mut row = DiffRow {
                source,
                kind: RowKind::Metadata,
                old_line: None,
                new_line: None,
            };
            if line.starts_with("diff --git ") {
                row.kind = RowKind::FileHeader;
                patch.files += 1;
                git_file_header = true;
                hunk = None;
            } else if line.starts_with("@@") {
                row.kind = RowKind::Hunk;
                hunk = Hunk::parse(line).filter(|hunk| !hunk.finished());
            } else if line == "\\ No newline at end of file" {
                row.kind = RowKind::Note;
            } else if let Some(active) = &mut hunk {
                match line.as_bytes().first() {
                    Some(b' ') if active.old_remaining > 0 && active.new_remaining > 0 => {
                        row.kind = RowKind::Context;
                        row.old_line = Some(active.take_old());
                        row.new_line = Some(active.take_new());
                    }
                    Some(b'-') if active.old_remaining > 0 => {
                        row.kind = RowKind::Removed;
                        row.old_line = Some(active.take_old());
                        patch.deletions += 1;
                    }
                    Some(b'+') if active.new_remaining > 0 => {
                        row.kind = RowKind::Added;
                        row.new_line = Some(active.take_new());
                        patch.additions += 1;
                    }
                    _ => hunk = None,
                }
                if hunk.as_ref().is_some_and(Hunk::finished) {
                    hunk = None;
                }
            } else if line.starts_with("--- ") || line.starts_with("+++ ") {
                row.kind = RowKind::FileHeader;
                if line.starts_with("--- ") && !git_file_header {
                    patch.files += 1;
                }
            } else if (line.starts_with("Binary files ") && line.ends_with(" differ"))
                || line == "GIT binary patch"
            {
                row.kind = RowKind::Binary;
                patch.binary = true;
            }
            for number in [row.old_line, row.new_line].into_iter().flatten() {
                patch.number_digits = patch.number_digits.max(number.ilog10() as usize + 1);
            }
            let columns = if matches!(
                row.kind,
                RowKind::Added | RowKind::Removed | RowKind::Context
            ) {
                1 + display_columns(&line[1..])
            } else {
                display_columns(line)
            };
            patch.max_columns = patch.max_columns.max(columns);
            patch.rows.push(row);
        }
        patch.build_presentation();
        patch
    }

    fn build_presentation(&mut self) {
        let mut files: Vec<FileDetail> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let line = &self.source[row.source.clone()];
            if row.kind == RowKind::FileHeader && line.starts_with("diff --git ") {
                let mut file = FileDetail::new();
                if let Some((old, new)) = diff_paths(line.strip_prefix("diff --git ").unwrap()) {
                    file.previous = patch_path(&old);
                    file.path = patch_path(&new);
                }
                files.push(file);
                continue;
            }
            if files.is_empty() {
                if row.kind == RowKind::Metadata
                    && ![
                        "new file mode ",
                        "deleted file mode ",
                        "old mode ",
                        "new mode ",
                        "rename from ",
                        "rename to ",
                        "copy from ",
                        "copy to ",
                    ]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
                {
                    continue;
                }
                files.push(FileDetail::new());
            }
            if row.kind == RowKind::FileHeader
                && line.starts_with("--- ")
                && files.last().is_some_and(|file| file.old_header_seen)
            {
                files.push(FileDetail::new());
            }
            let file = files.last_mut().unwrap();
            match row.kind {
                RowKind::FileHeader => {
                    if let Some(path) = line.strip_prefix("--- ") {
                        file.old_header_seen = true;
                        file.previous = patch_path(path);
                        if file.previous.is_none() {
                            file.status = "Added";
                        }
                    } else if let Some(path) = line.strip_prefix("+++ ") {
                        file.path = patch_path(path);
                        if file.path.is_none() {
                            file.status = "Deleted";
                        }
                    }
                }
                RowKind::Metadata => file.metadata(line),
                RowKind::Hunk => {
                    if let Some(hunk) = Hunk::parse(line) {
                        let ranges = format!(
                            "Before: {}   After: {}",
                            line_range(hunk.old_next, hunk.old_remaining),
                            line_range(hunk.new_next, hunk.new_remaining)
                        );
                        file.ranges.push(ranges.clone());
                        let body = line.strip_prefix("@@ ").unwrap();
                        let context = body[body.find(" @@").unwrap() + 3..].trim();
                        if !context.is_empty() || !file.rows.is_empty() {
                            let mut display = PresentationRow::text(context, RowKind::Hunk);
                            display.tooltip = Some(ranges);
                            file.rows.push(display);
                        }
                    }
                }
                RowKind::Context | RowKind::Added | RowKind::Removed => {
                    file.additions += usize::from(row.kind == RowKind::Added);
                    file.deletions += usize::from(row.kind == RowKind::Removed);
                    file.rows.push(PresentationRow {
                        raw_row: Some(index),
                        text: String::new(),
                        kind: row.kind,
                        tooltip: None,
                    });
                }
                RowKind::Note => file.rows.push(PresentationRow::text(
                    "No newline at end of file",
                    RowKind::Note,
                )),
                RowKind::Binary => {
                    file.binary = true;
                    file.rows.push(PresentationRow::text(
                        "Binary file changed; text comparison unavailable.",
                        RowKind::Binary,
                    ));
                }
            }
        }
        let multiple = files.len() > 1;
        for file in &mut files {
            let description = format!("{} · {}", file.path(), file.status);
            self.details.push(description.clone());
            if let Some(previous) = &file.previous
                && previous != file.path()
            {
                self.details.push(format!("Previous path: {previous}"));
            }
            if let Some(objects) = &file.objects {
                self.details.push(format!("Objects: {objects}"));
            }
            if file.old_mode.is_some() || file.new_mode.is_some() {
                self.details.push(format!(
                    "File mode: {} → {}",
                    file.old_mode.as_deref().unwrap_or("absent"),
                    file.new_mode.as_deref().unwrap_or("absent")
                ));
            }
            if let Some(similarity) = &file.similarity {
                self.details.push(format!("Similarity: {similarity}"));
            }
            self.details.extend(file.ranges.iter().cloned());
            if file.binary {
                self.details
                    .push("Binary contents; no text line statistics.".into());
            }
            self.file_starts.push(self.presentation.len());
            if multiple {
                self.presentation.push(PresentationRow::text(
                    format!("{description}    +{}  −{}", file.additions, file.deletions),
                    RowKind::FileHeader,
                ));
            }
            if file.rows.is_empty() {
                let message = match file.status {
                    "Added" => "Empty file added.".into(),
                    "Deleted" => "Empty file removed.".into(),
                    "Renamed" | "Copied" => format!(
                        "{}: {} → {}",
                        file.status,
                        file.previous.as_deref().unwrap_or("Previous file"),
                        file.path()
                    ),
                    "Mode changed" => "Only the file mode changed.".into(),
                    _ => "No text changes in this comparison.".into(),
                };
                file.rows
                    .push(PresentationRow::text(message, RowKind::Metadata));
            }
            self.presentation.append(&mut file.rows);
        }
        self.file_details = files;
        self.max_columns = self
            .presentation
            .iter()
            .map(|display| match display.raw_row {
                Some(index) => {
                    1 + display_columns(
                        &self.source
                            [self.rows[index].source.start + 1..self.rows[index].source.end],
                    )
                }
                None => display_columns(&display.text),
            })
            .max()
            .unwrap_or(0);
        self.detail_columns = self
            .details
            .iter()
            .map(|line| display_columns(line))
            .max()
            .unwrap_or(0);
    }
}

fn line_range(start: usize, count: usize) -> String {
    match count {
        0 => "empty".into(),
        1 => start.to_string(),
        _ => format!("{start}–{}", start + (count - 1)),
    }
}

fn patch_path(value: &str) -> Option<String> {
    let decoded = decode_git_path(value);
    if decoded == "/dev/null" {
        return None;
    }
    Some(
        decoded
            .strip_prefix("a/")
            .or_else(|| decoded.strip_prefix("b/"))
            .unwrap_or(&decoded)
            .to_owned(),
    )
}

fn diff_paths(value: &str) -> Option<(String, String)> {
    if value.starts_with('"') {
        let mut escaped = false;
        for (index, character) in value.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Some((
                    value[..=index].into(),
                    value[index + 1..].trim_start().into(),
                ));
            }
        }
        return None;
    }
    if let Some(index) = value.rfind(" b/") {
        return Some((value[..index].into(), value[index + 1..].into()));
    }
    value
        .split_once(' ')
        .map(|(old, new)| (old.into(), new.into()))
}

fn decode_git_path(value: &str) -> String {
    let Some(quoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.split('\t').next().unwrap_or(value).to_owned();
    };
    let bytes = quoted.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 == bytes.len() {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        let character = bytes[index];
        if (b'0'..=b'7').contains(&character) {
            let mut number = 0u16;
            for _ in 0..3 {
                if index == bytes.len() || !(b'0'..=b'7').contains(&bytes[index]) {
                    break;
                }
                number = number * 8 + u16::from(bytes[index] - b'0');
                index += 1;
            }
            decoded.push(number as u8);
        } else if character == b'u'
            && bytes.get(index + 1) == Some(&b'{')
            && let Some(end) = quoted[index + 2..].find('}')
            && let Ok(number) = u32::from_str_radix(&quoted[index + 2..index + 2 + end], 16)
            && let Some(character) = char::from_u32(number)
        {
            let mut encoded = [0; 4];
            decoded.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            index += end + 3;
        } else {
            let escaped = match character {
                b'\\' => b'\\',
                b'"' => b'"',
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                b'a' => 7,
                b'b' => 8,
                b'v' => 11,
                b'f' => 12,
                _ => {
                    decoded.push(b'\\');
                    character
                }
            };
            decoded.push(escaped);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[derive(Debug, PartialEq, Eq)]
struct Hunk {
    old_next: usize,
    new_next: usize,
    old_remaining: usize,
    new_remaining: usize,
}

impl Hunk {
    fn parse(line: &str) -> Option<Self> {
        let body = line.strip_prefix("@@ ")?;
        let closing = body.find(" @@")?;
        let suffix = &body[closing + 3..];
        if !suffix.is_empty() && !suffix.starts_with(' ') {
            return None;
        }
        let mut ranges = body[..closing].split_whitespace();
        let (old_next, old_remaining) = parse_range(ranges.next()?.strip_prefix('-')?)?;
        let (new_next, new_remaining) = parse_range(ranges.next()?.strip_prefix('+')?)?;
        if ranges.next().is_some() {
            return None;
        }
        Some(Self {
            old_next,
            new_next,
            old_remaining,
            new_remaining,
        })
    }

    fn take_old(&mut self) -> usize {
        let number = self.old_next;
        self.old_next = self.old_next.saturating_add(1);
        self.old_remaining -= 1;
        number
    }

    fn take_new(&mut self) -> usize {
        let number = self.new_next;
        self.new_next = self.new_next.saturating_add(1);
        self.new_remaining -= 1;
        number
    }

    fn finished(&self) -> bool {
        self.old_remaining == 0 && self.new_remaining == 0
    }
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    let number = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        value.parse::<usize>().ok()
    };
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (number(start)?, number(count)?),
        None => (number(range)?, 1),
    };
    if count != 0 && (start == 0 || start.checked_add(count - 1).is_none()) {
        return None;
    }
    Some((start, count))
}

fn display_columns(text: &str) -> usize {
    text.chars().fold(0, |columns, character| {
        columns
            + if character == '\t' {
                TAB_WIDTH - columns % TAB_WIDTH
            } else {
                1
            }
    })
}

fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    let mut columns = 0;
    for character in text.chars() {
        if character == '\t' {
            let spaces = TAB_WIDTH - columns % TAB_WIDTH;
            output.extend(std::iter::repeat_n(' ', spaces));
            columns += spaces;
        } else {
            output.push(character);
            columns += 1;
        }
    }
    output
}

fn display_line(text: &str, kind: RowKind) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    if matches!(kind, RowKind::Added | RowKind::Removed | RowKind::Context) {
        let mut output = text[..1].to_owned();
        output.push_str(&expand_tabs(&text[1..]));
        output
    } else {
        expand_tabs(text)
    }
}

pub(crate) struct Palette {
    normal: Color32,
    muted: Color32,
    added: Color32,
    removed: Color32,
    hunk: Color32,
    file_fill: Color32,
}

impl Palette {
    pub(crate) fn new(ui: &egui::Ui) -> Self {
        let dark = ui.visuals().dark_mode;
        Self {
            normal: ui.visuals().text_color(),
            muted: ui.visuals().weak_text_color(),
            added: if dark {
                crate::palette::GREEN
            } else {
                Color32::from_rgb(25, 115, 75)
            },
            removed: if dark {
                crate::palette::RED
            } else {
                Color32::from_rgb(183, 45, 63)
            },
            hunk: if dark {
                crate::palette::BLUE
            } else {
                Color32::from_rgb(62, 87, 175)
            },
            file_fill: ui.visuals().widgets.noninteractive.weak_bg_fill,
        }
    }

    fn foreground(&self, kind: RowKind) -> Color32 {
        match kind {
            RowKind::Added | RowKind::Removed => self.normal,
            RowKind::Hunk => self.hunk,
            RowKind::Context | RowKind::FileHeader => self.normal,
            RowKind::Metadata | RowKind::Note | RowKind::Binary => self.muted,
        }
    }

    /// The leading +/− of a changed line keeps the change color.
    fn marker(&self, kind: RowKind) -> Option<Color32> {
        match kind {
            RowKind::Added => Some(self.added),
            RowKind::Removed => Some(self.removed),
            _ => None,
        }
    }

    fn background(&self, kind: RowKind) -> Option<Color32> {
        let tint = match kind {
            RowKind::Added => self.added,
            RowKind::Removed => self.removed,
            RowKind::Hunk => self.hunk,
            RowKind::FileHeader => return Some(self.file_fill),
            _ => return None,
        };
        Some(Color32::from_rgba_unmultiplied(
            tint.r(),
            tint.g(),
            tint.b(),
            if kind == RowKind::Hunk { 24 } else { 34 },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(patch: &ParsedPatch) -> Vec<(RowKind, Option<usize>, Option<usize>)> {
        patch
            .rows
            .iter()
            .filter(|row| row.old_line.is_some() || row.new_line.is_some())
            .map(|row| (row.kind, row.old_line, row.new_line))
            .collect()
    }

    #[test]
    fn replacement_has_separate_line_numbers_and_excludes_file_headers_from_counts() {
        let patch = ParsedPatch::parse(
            "diff --git a/main.rs b/main.rs\n--- a/main.rs\n+++ b/main.rs\n@@ -8,3 +8,4 @@ fn main()\n same\n-old\n+new\n+++source beginning with pluses\n last\n",
        );
        assert_eq!((patch.files, patch.additions, patch.deletions), (1, 2, 1));
        assert_eq!(
            numbered(&patch),
            vec![
                (RowKind::Context, Some(8), Some(8)),
                (RowKind::Removed, Some(9), None),
                (RowKind::Added, None, Some(9)),
                (RowKind::Added, None, Some(10)),
                (RowKind::Context, Some(10), Some(11)),
            ]
        );
    }

    #[test]
    fn root_commit_and_new_file_start_at_one_without_old_line_numbers() {
        let patch = ParsedPatch::parse(
            "diff --git a/new.md b/new.md\nnew file mode 100644\n--- /dev/null\n+++ b/new.md\n@@ -0,0 +1,2 @@\n+# Plan\n+Hello\n",
        );
        assert_eq!((patch.additions, patch.deletions), (2, 0));
        assert_eq!(
            numbered(&patch),
            vec![
                (RowKind::Added, None, Some(1)),
                (RowKind::Added, None, Some(2)),
            ]
        );
    }

    #[test]
    fn deleted_file_has_only_old_numbers_and_no_new_zero() {
        let patch = ParsedPatch::parse(
            "diff --git a/old.rs b/old.rs\ndeleted file mode 100644\n--- a/old.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n",
        );
        assert_eq!((patch.additions, patch.deletions), (0, 2));
        assert_eq!(
            numbered(&patch),
            vec![
                (RowKind::Removed, Some(1), None),
                (RowKind::Removed, Some(2), None),
            ]
        );
    }

    #[test]
    fn unchanged_rename_is_metadata_without_fake_text_changes() {
        let patch = ParsedPatch::parse(
            "diff --git a/old.rs b/new.rs\nsimilarity index 100%\nrename from old.rs\nrename to new.rs\n",
        );
        assert_eq!((patch.files, patch.additions, patch.deletions), (1, 0, 0));
        assert!(numbered(&patch).is_empty());
        assert!(
            patch
                .rows
                .iter()
                .skip(1)
                .all(|row| row.kind == RowKind::Metadata)
        );
    }

    #[test]
    fn no_newline_notes_do_not_advance_either_hunk_counter() {
        let patch = ParsedPatch::parse(
            "@@ -3 +3 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file",
        );
        assert_eq!(
            numbered(&patch),
            vec![
                (RowKind::Removed, Some(3), None),
                (RowKind::Added, None, Some(3)),
            ]
        );
        assert_eq!(
            patch
                .rows
                .iter()
                .filter(|row| row.kind == RowKind::Note)
                .count(),
            2
        );
    }

    #[test]
    fn multiple_files_and_hunks_reset_numbers_and_counts() {
        let patch = ParsedPatch::parse(
            "diff --git a/a b/a\n@@ -2 +2 @@\n-old\n+new\n@@ -90,0 +91 @@\n+tail\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n keep\n",
        );
        assert_eq!((patch.files, patch.additions, patch.deletions), (2, 2, 1));
        assert_eq!(
            numbered(&patch),
            vec![
                (RowKind::Removed, Some(2), None),
                (RowKind::Added, None, Some(2)),
                (RowKind::Added, None, Some(91)),
                (RowKind::Context, Some(1), Some(1)),
            ]
        );
    }

    #[test]
    fn malformed_hunk_never_fabricates_numbers_or_counts_metadata_as_source() {
        for hunk in [
            "@@ -x +1 @@",
            "@@ -1,2 + @@",
            "@@@ -1 +1 @@@",
            "@@ -0 +1 @@",
            "@@ -1 +1 extra @@",
            "@@ -184467440737095516160 +1 @@",
            "@@ -+1 +1 @@",
            "@@ -1 +1 @@broken",
        ] {
            let patch = ParsedPatch::parse(&format!("{hunk}\n--- a/file\n+++ b/file\n+unknown\n"));
            assert!(numbered(&patch).is_empty(), "{hunk}");
            assert_eq!((patch.additions, patch.deletions), (0, 0), "{hunk}");
        }
    }

    #[test]
    fn hunk_counts_stop_an_unexpected_header_from_consuming_line_numbers() {
        let patch = ParsedPatch::parse("@@ -1 +1 @@\n-old\n+new\n--- a/next\n+++ b/next\n");
        assert_eq!((patch.additions, patch.deletions), (1, 1));
        assert_eq!(patch.rows[3].kind, RowKind::FileHeader);
        assert_eq!(patch.rows[4].kind, RowKind::FileHeader);
        assert!(
            patch.rows[3..]
                .iter()
                .all(|row| row.old_line.is_none() && row.new_line.is_none())
        );
    }

    #[test]
    fn binary_patch_is_marked_without_counting_encoded_lines() {
        let patch = ParsedPatch::parse(
            "diff --git a/image.png b/image.png\nBinary files a/image.png and b/image.png differ\n",
        );
        assert!(patch.binary);
        assert_eq!((patch.additions, patch.deletions), (0, 0));
        assert_eq!(patch.rows[1].kind, RowKind::Binary);
        let encoded = ParsedPatch::parse(
            "diff --git a/image.png b/image.png\nGIT binary patch\nliteral 3\n+encoded\n-encoded\n",
        );
        assert!(encoded.binary);
        assert_eq!((encoded.additions, encoded.deletions), (0, 0));
    }

    #[test]
    fn crlf_unicode_tabs_and_last_line_preserve_original_patch_and_ranges() {
        let text = "@@ -0,0 +1,2 @@\r\n+über\t界\r\n+last";
        let patch = ParsedPatch::parse(text);
        assert_eq!(patch.source, text);
        assert_eq!(&patch.source[patch.rows[1].source.clone()], "+über\t界");
        assert_eq!(&patch.source[patch.rows[2].source.clone()], "+last");
        assert_eq!(expand_tabs("+a\tb\tc"), "+a  b   c");
        assert_eq!(display_columns("+a\tb\tc"), 9);
        assert_eq!(display_line("+\tindented", RowKind::Added), "+    indented");
        assert_eq!(patch.additions, 2);
    }

    #[test]
    fn cache_changes_only_for_new_content_or_comparison() {
        let mut view = DiffView::default();
        view.update("file", "@@ -1 +1 @@\n-a\n+b\n");
        let generation = view.generation;
        let allocation = view.patch.source.as_ptr();
        view.update("file", "@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(view.generation, generation);
        assert_eq!(view.patch.source.as_ptr(), allocation);
        view.update("file", "@@ -1 +1 @@\n-a\n+c\n");
        assert_ne!(view.generation, generation);
        assert_eq!(&view.patch.source[view.patch.rows[2].source.clone()], "+c");
        let changed = view.generation;
        view.update("another file", "@@ -1 +1 @@\n-a\n+c\n");
        assert_ne!(view.generation, changed);
    }

    #[test]
    fn empty_patch_has_no_rows_or_changes() {
        let patch = ParsedPatch::parse("");
        assert!(patch.rows.is_empty());
        assert_eq!((patch.files, patch.additions, patch.deletions), (0, 0, 0));
    }

    fn collect_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, result: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => result.push((
                    text.galley.job.text.clone(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, result);
                    }
                }
                _ => {}
            }
        }
        let mut result = Vec::new();
        for shape in shapes {
            collect(&shape.shape, &mut result);
        }
        result
    }

    fn show_pass(
        context: &egui::Context,
        view: &mut DiffView,
        text: &str,
        size: Vec2,
        events: Vec<egui::Event>,
        time: f64,
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::OutputCommand>) {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Default::default(), size)),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    view.show(ui, "Review", text);
                });
            },
        );
        let result = (
            collect_text(&output.shapes),
            output.platform_output.commands.clone(),
        );
        output.drop_without_applying_deltas();
        result
    }

    #[test]
    fn ten_thousand_line_patch_paints_only_the_viewport_and_wheel_reaches_last_line() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let patch = format!(
            "@@ -0,0 +1,10000 @@\n{}",
            (0..10000)
                .map(|index| format!("+payload {index}\n"))
                .collect::<String>()
        );
        let size = Vec2::new(500.0, 260.0);
        let (initial, _) = show_pass(&context, &mut view, &patch, size, vec![], 0.0);
        let visible = initial
            .iter()
            .filter(|(text, _)| text.starts_with("+payload "))
            .count();
        assert!(visible > 0 && visible < 30, "painted {visible} source rows");
        assert_eq!(view.patch.rows.len(), 10001);
        let pointer = egui::pos2(180.0, 140.0);
        let (last, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -1_000_000.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
            0.1,
        );
        assert!(
            last.iter().any(|(text, _)| text == "+payload 9999"),
            "visible text: {last:?}"
        );
        assert!(!last.iter().any(|(text, _)| text == "+payload 0"));
        assert!(
            last.iter()
                .filter(|(text, _)| text.starts_with("+payload "))
                .count()
                < 30
        );
        let wheel = |delta| {
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, delta),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            }]
        };
        let (away, _) = show_pass(&context, &mut view, &patch, size, wheel(42.0), 0.2);
        assert!(!away.iter().any(|(text, _)| text == "+payload 9999"));
        assert!(away.iter().any(|(text, _)| text.starts_with("+payload 99")));
        let (returned, _) = show_pass(&context, &mut view, &patch, size, wheel(-42.0), 0.3);
        assert!(returned.iter().any(|(text, _)| text == "+payload 9999"));
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn diff_keyboard_scrolls_once_per_key_text_pair_and_reaches_both_ends() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let patch = format!(
            "@@ -0,0 +1,1000 @@\n{}",
            (0..1000)
                .map(|index| format!("+payload {index}\n"))
                .collect::<String>()
        );
        let size = Vec2::new(550.0, 260.0);
        let (initial, _) = show_pass(&context, &mut view, &patch, size, vec![], 0.0);
        let before = initial
            .iter()
            .find(|(text, _)| text == "+payload 2")
            .unwrap()
            .1
            .top();
        let (down, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![
                key_event(egui::Key::J, egui::Modifiers::NONE),
                egui::Event::Text("j".into()),
            ],
            0.1,
        );
        let after = down
            .iter()
            .find(|(text, _)| text == "+payload 2")
            .unwrap()
            .1
            .top();
        assert!(
            (15.0..30.0).contains(&(before - after)),
            "moved {} points",
            before - after
        );
        let (last, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![key_event(egui::Key::End, egui::Modifiers::NONE)],
            0.2,
        );
        assert!(last.iter().any(|(text, _)| text == "+payload 999"));
        let (first, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![key_event(egui::Key::Home, egui::Modifiers::NONE)],
            0.3,
        );
        assert!(first.iter().any(|(text, _)| text == "+payload 0"));
        let (page, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![key_event(egui::Key::PageDown, egui::Modifiers::NONE)],
            0.4,
        );
        assert!(!page.iter().any(|(text, _)| text == "+payload 0"));
        let (back, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![key_event(egui::Key::B, egui::Modifiers::CTRL)],
            0.5,
        );
        assert!(back.iter().any(|(text, _)| text == "+payload 0"));
    }

    #[test]
    fn focused_text_field_retains_its_input_and_diff_does_not_scroll() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let mut query = String::new();
        let patch = format!(
            "@@ -0,0 +1,1000 @@\n{}",
            (0..1000)
                .map(|index| format!("+payload {index}\n"))
                .collect::<String>()
        );
        let mut pass = |events, time| {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        Vec2::new(550.0, 300.0),
                    )),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let field = ui.text_edit_singleline(&mut query);
                        view.show(ui, "Review", &patch);
                        field.request_focus();
                    });
                },
            );
            let painted = collect_text(&output.shapes);
            output.drop_without_applying_deltas();
            painted
        };
        let initial = pass(vec![], 0.0);
        let typed = pass(
            vec![
                egui::Event::Text("j".into()),
                key_event(egui::Key::PageDown, egui::Modifiers::NONE),
            ],
            0.1,
        );
        let before = initial
            .iter()
            .find(|(text, _)| text == "+payload 2")
            .unwrap()
            .1
            .top();
        let after = typed
            .iter()
            .find(|(text, _)| text == "+payload 2")
            .unwrap()
            .1
            .top();
        assert_eq!(before, after);
        assert_eq!(query, "j");
    }

    #[test]
    fn diff_wraps_complete_source_to_viewport_width_and_reflows_on_resize() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let source = format!("+{}END", "long_identifier ".repeat(100));
        let patch = format!("@@ -0,0 +1 @@\n{source}\n");
        let mut heights = Vec::new();
        for (pass, width) in [300.0, 950.0, 300.0].into_iter().enumerate() {
            let (painted, _) = show_pass(
                &context,
                &mut view,
                &patch,
                Vec2::new(width, 260.0),
                vec![],
                pass as f64 * 0.1,
            );
            let (_, rect) = painted
                .iter()
                .find(|(text, _)| text == &source)
                .expect("complete source line");
            assert!(rect.width() <= view.wrapped.text_width + 1.0);
            assert!(
                rect.left() > 60.0,
                "text must leave both number gutters free"
            );
            assert!(
                rect.right() < width - 8.0,
                "wrapped text escaped the viewport: {rect:?}"
            );
            assert!(rect.height() > 25.0, "long source must wrap");
            heights.push(rect.height());
        }
        assert!(
            heights[0] > heights[1] * 2.0,
            "wider dialogs must reflow the text: {heights:?}"
        );
        assert_eq!(
            heights[0], heights[2],
            "returning to the narrow width must reflow again"
        );
    }

    #[test]
    fn unbroken_unicode_and_tabs_wrap_without_elision_or_horizontal_scrolling() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let raw = format!("+{}\tTAIL", "über界🙂e\u{301}".repeat(120));
        let displayed = display_line(&raw, RowKind::Added);
        let patch = format!("@@ -0,0 +1 @@\n{raw}\n");
        let size = Vec2::new(320.0, 300.0);
        let (initial, _) = show_pass(&context, &mut view, &patch, size, vec![], 0.0);
        let before = initial
            .iter()
            .find(|(text, _)| text == &displayed)
            .unwrap()
            .1;
        assert!(before.height() > 100.0);
        assert!(before.width() <= view.wrapped.text_width + 1.0);
        assert!(before.right() < size.x - 8.0);
        let (scrolled, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![
                egui::Event::PointerMoved(egui::pos2(180.0, 130.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(-10_000.0, 0.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
            0.1,
        );
        let after = scrolled
            .iter()
            .find(|(text, _)| text == &displayed)
            .unwrap()
            .1;
        assert_eq!(
            before, after,
            "a horizontal wheel must not move wrapped diff text"
        );
        assert_eq!(
            view.patch.source, patch,
            "wrapping must preserve the copied raw patch"
        );
    }

    #[test]
    fn wrapped_rows_have_one_number_on_the_first_visual_line_and_full_tinted_background() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let removed = format!("-{}", "previous_value ".repeat(12));
        let added = format!("+{}", "replacement_value ".repeat(12));
        let patch = format!("@@ -321 +654 @@\n{removed}\n{added}\n");
        let mut fills = (Color32::TRANSPARENT, Color32::TRANSPARENT);
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Default::default(),
                    Vec2::new(360.0, 1100.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let palette = Palette::new(ui);
                    fills = (
                        palette.background(RowKind::Removed).unwrap(),
                        palette.background(RowKind::Added).unwrap(),
                    );
                    view.show(ui, "Review", &patch);
                });
            },
        );
        let text = collect_text(&output.shapes);
        let mut backgrounds = Vec::new();
        fn collect_rects(shape: &egui::Shape, result: &mut Vec<(egui::Rect, Color32)>) {
            match shape {
                egui::Shape::Rect(rect) => result.push((rect.rect, rect.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_rects(shape, result);
                    }
                }
                _ => {}
            }
        }
        for shape in &output.shapes {
            collect_rects(&shape.shape, &mut backgrounds);
        }
        for (source, number, fill) in [(&removed, "321", fills.0), (&added, "654", fills.1)] {
            let source_rect = text
                .iter()
                .find(|(painted, _)| painted == source)
                .unwrap()
                .1;
            assert!(source_rect.height() > 25.0);
            let numbers: Vec<_> = text
                .iter()
                .filter(|(painted, _)| painted == number)
                .collect();
            assert_eq!(
                numbers.len(),
                1,
                "wrapped rows must not duplicate line numbers"
            );
            assert!(
                (numbers[0].1.top() - source_rect.top()).abs() <= 2.0,
                "number must align with the first visual row"
            );
            let tinted: Vec<_> = backgrounds
                .iter()
                .filter(|(_, color)| *color == fill)
                .collect();
            assert_eq!(tinted.len(), 1);
            let background = tinted[0].0;
            assert!(background.top() < source_rect.top());
            assert!(background.bottom() > source_rect.bottom());
            assert!(background.height() >= source_rect.height() + ROW_PADDING - 1.0);
            assert!(background.left() < numbers[0].1.left());
            assert!(background.right() > source_rect.right());
        }
        output.drop_without_applying_deltas();
    }

    #[test]
    fn wrapped_diff_stays_virtualized_and_end_remains_visible_after_resize() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let patch = format!(
            "@@ -0,0 +1,1000 @@\n{}",
            (0..1000)
                .map(|index| {
                    let suffix = if index % 4 == 3 {
                        "long_value ".repeat(30)
                    } else {
                        "short".into()
                    };
                    format!("+payload {index} {suffix}\n")
                })
                .collect::<String>()
        );
        let size = Vec2::new(360.0, 280.0);
        let (initial, _) = show_pass(&context, &mut view, &patch, size, vec![], 0.0);
        assert!(initial.iter().any(|(text, _)| text == "+payload 0 short"));
        assert!(
            initial
                .iter()
                .filter(|(text, _)| text.starts_with("+payload "))
                .count()
                < 20
        );
        let narrow_height = view.wrapped.height();
        let (last, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![key_event(egui::Key::End, egui::Modifiers::NONE)],
            0.1,
        );
        assert!(
            last.iter()
                .any(|(text, _)| text.starts_with("+payload 999 "))
        );
        let (resized, _) = show_pass(
            &context,
            &mut view,
            &patch,
            Vec2::new(950.0, 280.0),
            vec![],
            0.2,
        );
        assert!(view.wrapped.height() < narrow_height);
        assert!(
            resized
                .iter()
                .any(|(text, _)| text.starts_with("+payload 999 ")),
            "resize must clamp to the new bottom before painting"
        );
        assert!(
            resized
                .iter()
                .filter(|(text, _)| text.starts_with("+payload "))
                .count()
                < 20
        );
        let (first, _) = show_pass(
            &context,
            &mut view,
            &patch,
            Vec2::new(950.0, 280.0),
            vec![key_event(egui::Key::Home, egui::Modifiers::NONE)],
            0.3,
        );
        assert!(first.iter().any(|(text, _)| text == "+payload 0 short"));
    }

    #[test]
    fn large_wrapped_unicode_diff_keeps_offscreen_galleys_out_of_font_cache() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        // Every unique row remains long enough to wrap at the widest viewport.
        let source = "über界🙂e\u{301}_long_identifier ".repeat(6);
        let patch = format!(
            "@@ -0,0 +1,2000 @@\n{}",
            (0..2000)
                .map(|index| format!("+row {index} {source}\n"))
                .collect::<String>()
        );
        for (pass, width) in [360.0, 950.0, 360.0].into_iter().enumerate() {
            let (painted, _) = show_pass(
                &context,
                &mut view,
                &patch,
                Vec2::new(width, 280.0),
                vec![],
                pass as f64 * 0.1,
            );
            assert!(painted.iter().any(|(text, _)| text.starts_with("+row 0 ")));
            let cache_size = context.fonts_mut(|fonts| fonts.num_galleys_in_cache());
            assert!(
                cache_size < 80,
                "only visible rows and UI labels may remain cached; got {cache_size} galleys"
            );

            // The uncached public API must agree exactly with the cached paint
            // API, including Unicode shaping and fractional width rounding.
            let single_row = context
                .fonts_mut(|fonts| fonts.row_height(&FontId::monospace(FONT_SIZE)))
                + ROW_PADDING;
            for index in [0, 997, 1999] {
                let display = view.patch.display(index);
                let expected = context.fonts_mut(|fonts| {
                    fonts
                        .layout_delayed_color(
                            display,
                            FontId::monospace(FONT_SIZE),
                            view.wrapped.text_width,
                        )
                        .size()
                        .y
                }) + ROW_PADDING;
                let measured = view.wrapped.offsets[index + 1] - view.wrapped.offsets[index];
                assert!(
                    measured > single_row,
                    "row {index} must wrap at viewport width {width}"
                );
                assert!(
                    (measured - expected).abs() < 0.1,
                    "off-screen height changed at row {index}: measured {measured}, paint {expected}"
                );
            }
        }
        let (last, _) = show_pass(
            &context,
            &mut view,
            &patch,
            Vec2::new(360.0, 280.0),
            vec![key_event(egui::Key::End, egui::Modifiers::NONE)],
            0.3,
        );
        assert!(last.iter().any(|(text, _)| text.starts_with("+row 1999 ")));
    }

    #[test]
    fn copy_patch_button_returns_exact_original_unicode_crlf_patch() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let patch = "diff --git \"a/\\303\\234ber Plan.md\" \"b/\\303\\234ber Plan.md\"\r\nnew file mode 100644\r\nindex 0000000..abc1234\r\n--- /dev/null\r\n+++ \"b/\\303\\234ber Plan.md\"\r\n@@ -0,0 +1 @@\r\n+über\t界\r\n\\ No newline at end of file";
        let size = Vec2::new(550.0, 260.0);
        let (painted, _) = show_pass(&context, &mut view, patch, size, vec![], 0.0);
        let position = painted
            .iter()
            .find(|(text, _)| text == "Copy patch")
            .expect("copy button")
            .1
            .center();
        show_pass(
            &context,
            &mut view,
            patch,
            size,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            0.1,
        );
        let (_, commands) = show_pass(
            &context,
            &mut view,
            patch,
            size,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            0.2,
        );
        assert!(commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == patch)
        ));
    }

    #[test]
    fn code_begins_with_function_context_and_technical_details_require_a_click() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let patch = "diff --git a/src/git_panel.rs b/src/git_panel.rs\nindex 82594cb..faefadc 100644\n--- a/src/git_panel.rs\n+++ b/src/git_panel.rs\n@@ -45,2 +45,2 @@ impl RepositoryView {\n-old\n+new\n keep\n";
        let size = Vec2::new(650.0, 420.0);
        let (painted, _) = show_pass(&context, &mut view, patch, size, vec![], 0.0);
        assert!(
            painted
                .iter()
                .any(|(text, _)| text == "impl RepositoryView {")
        );
        assert!(painted.iter().any(|(text, _)| text == "45"));
        assert!(painted.iter().any(|(text, _)| text == "-old"));
        assert!(painted.iter().any(|(text, _)| text == "+new"));
        assert!(
            !painted
                .iter()
                .any(|(text, _)| text.starts_with("diff --git")
                    || text.starts_with("index ")
                    || text.starts_with("@@")
                    || text.starts_with("--- ")
                    || text.starts_with("+++ ")
                    || text.starts_with("Objects:")
                    || text.starts_with("Before:"))
        );
        assert!(!painted.iter().any(|(text, _)| text == "src/git_panel.rs"));
        let position = painted
            .iter()
            .find(|(text, _)| text == "Details")
            .unwrap()
            .1
            .center();
        show_pass(
            &context,
            &mut view,
            patch,
            size,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            0.1,
        );
        let (expanded, _) = show_pass(
            &context,
            &mut view,
            patch,
            size,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            0.2,
        );
        assert!(
            expanded
                .iter()
                .any(|(text, _)| text == "Objects: 82594cb → faefadc")
        );
        assert!(
            expanded
                .iter()
                .any(|(text, _)| text == "Before: 45–46   After: 45–46")
        );
        assert!(
            expanded
                .iter()
                .any(|(text, _)| text == "impl RepositoryView {")
        );
    }

    #[test]
    fn root_commit_shows_readable_file_sections_and_added_status_without_patch_headers() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let patch = "diff --git \"a/docs/\\303\\234ber Plan.md\" \"b/docs/\\303\\234ber Plan.md\"\nnew file mode 100644\nindex 0000000..abc1234\n--- /dev/null\n+++ \"b/docs/\\303\\234ber Plan.md\"\n@@ -0,0 +1 @@\n+# Plan\ndiff --git a/src/main.rs b/src/main.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/main.rs\n@@ -0,0 +1 @@\n+fn main() {}\n";
        let (painted, _) = show_pass(
            &context,
            &mut view,
            patch,
            Vec2::new(750.0, 500.0),
            vec![],
            0.0,
        );
        assert!(
            painted
                .iter()
                .any(|(text, _)| text.starts_with("docs/Über Plan.md · Added"))
        );
        assert!(
            painted
                .iter()
                .any(|(text, _)| text.starts_with("src/main.rs · Added"))
        );
        assert!(painted.iter().any(|(text, _)| text == "+# Plan"));
        assert!(painted.iter().any(|(text, _)| text == "+fn main() {}"));
        assert_eq!((view.patch.additions, view.patch.deletions), (2, 0));
        assert!(
            !painted
                .iter()
                .any(|(text, _)| text.starts_with("diff --git")
                    || text.starts_with("index ")
                    || text.starts_with("@@")
                    || text.contains("/dev/null")
                    || text.contains("\\303")
                    || text == "0")
        );
    }

    #[test]
    fn metadata_only_changes_have_readable_states_without_fake_code() {
        for (patch, expected) in [
            (
                "diff --git a/empty b/empty\nnew file mode 100644\nindex 0000000..e69de29\n",
                "Empty file added.",
            ),
            (
                "diff --git a/empty b/empty\ndeleted file mode 100644\nindex e69de29..0000000\n",
                "Empty file removed.",
            ),
            (
                "diff --git a/old b/new\nsimilarity index 100%\nrename from \"old plan.md\"\nrename to \"\\303\\234ber Plan.md\"\n",
                "Renamed: old plan.md → Über Plan.md",
            ),
            (
                "diff --git a/script b/script\nold mode 100644\nnew mode 100755\n",
                "Only the file mode changed.",
            ),
            (
                "diff --git a/image.png b/image.png\nBinary files a/image.png and b/image.png differ\n",
                "Binary file changed; text comparison unavailable.",
            ),
        ] {
            let context = egui::Context::default();
            let mut view = DiffView::default();
            let (painted, _) = show_pass(
                &context,
                &mut view,
                patch,
                Vec2::new(650.0, 300.0),
                vec![],
                0.0,
            );
            assert!(
                painted.iter().any(|(text, _)| text == expected),
                "{expected}"
            );
            assert!(
                !painted
                    .iter()
                    .any(|(text, _)| text.starts_with("diff --git")
                        || text.starts_with("index ")
                        || text.starts_with("@@")
                        || text.starts_with("rename from")
                        || text.starts_with("Binary files"))
            );
            assert!(numbered(&view.patch).is_empty());
        }
    }

    #[test]
    fn hidden_long_paths_and_object_ids_do_not_create_horizontal_code_scrolling() {
        let context = egui::Context::default();
        let mut view = DiffView::default();
        let path = format!("docs/{}plan.md", "long_folder/".repeat(100));
        let patch = format!(
            "diff --git a/{path} b/{path}\nindex {}..{} 100644\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new\n",
            "a".repeat(1500),
            "b".repeat(1500)
        );
        let size = Vec2::new(550.0, 260.0);
        let (initial, _) = show_pass(&context, &mut view, &patch, size, vec![], 0.0);
        let before = initial
            .iter()
            .find(|(text, _)| text == "+new")
            .unwrap()
            .1
            .left();
        let (after, _) = show_pass(
            &context,
            &mut view,
            &patch,
            size,
            vec![
                egui::Event::PointerMoved(egui::pos2(180.0, 100.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(-1_000_000.0, 0.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
            0.1,
        );
        let after = after
            .iter()
            .find(|(text, _)| text == "+new")
            .unwrap()
            .1
            .left();
        assert!((after - before).abs() < 1.0);
    }

    #[test]
    fn git_c_quoted_and_rust_debug_paths_decode_without_losing_spaces_or_unicode() {
        assert_eq!(
            decode_git_path(r#""docs/\303\234ber Plan.md""#),
            "docs/Über Plan.md"
        );
        assert_eq!(
            decode_git_path(r#""C:\\Dev\\Über Plan.md""#),
            "C:\\Dev\\Über Plan.md"
        );
        assert_eq!(
            decode_git_path(r#""docs/\u{dc}ber Plan.md""#),
            "docs/Über Plan.md"
        );
        assert_eq!(
            decode_git_path(r#""a/quote\" and\\slash.md""#),
            "a/quote\" and\\slash.md"
        );
        let paths = diff_paths(r#""a/old plan.md" "b/\303\234ber Plan.md""#).unwrap();
        assert_eq!(patch_path(&paths.0).as_deref(), Some("old plan.md"));
        assert_eq!(patch_path(&paths.1).as_deref(), Some("Über Plan.md"));
    }

    #[test]
    fn commit_preamble_does_not_create_a_fake_current_file_section() {
        let patch = ParsedPatch::parse(
            "commit abc123\nAuthor: Someone\n\n    Subject\n\ndiff --git a/file b/file\n@@ -1 +1 @@\n-old\n+new\n",
        );
        assert_eq!(patch.file_details.len(), 1);
        assert!(
            !patch
                .presentation
                .iter()
                .any(|row| row.kind == RowKind::FileHeader)
        );
    }
}
