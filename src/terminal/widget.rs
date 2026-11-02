//! egui terminal view; all positions are grid cells rather than document bytes.
use super::*;
use alacritty_terminal::{
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::cell::Flags,
    vte::ansi::{Color, CursorShape, NamedColor},
};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

impl TerminalSession {
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        widget_id: egui::Id,
        request_focus: bool,
        font_size: f32,
    ) -> egui::Response {
        // in_memory() has its own context until first attached to a UI fixture.
        self.ctx = ui.ctx().clone();
        let font = FontId::monospace(font_size.clamp(6.0, 40.0));
        let cell = ui
            .fonts_mut(|fonts| Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font)))
            .max(Vec2::splat(1.0));
        let size = ui.available_size().max(cell);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let response = ui.interact(rect, widget_id, Sense::click_and_drag());
        // egui only installs a lock filter after a widget had focus for a
        // complete frame. Reclaim a navigation key on that first transition;
        // App navigation has already consumed its own shortcuts before this.
        let retain_navigation_focus = response.lost_focus()
            && ui.input(|input| {
                !input.pointer.any_pressed()
                    && input.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key {
                                key: egui::Key::Tab
                                    | egui::Key::Escape
                                    | egui::Key::ArrowUp
                                    | egui::Key::ArrowDown
                                    | egui::Key::ArrowLeft
                                    | egui::Key::ArrowRight,
                                pressed: true,
                                ..
                            }
                        )
                    })
            });
        if ui.is_enabled()
            && (request_focus
                || response.clicked()
                || response.drag_started()
                || retain_navigation_focus)
        {
            response.request_focus();
            ui.ctx().request_repaint();
        }
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                widget_id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), "Terminal")
        });
        let content = rect.shrink(5.0);
        self.resize(PtySize {
            cols: (content.width() / cell.x).floor().clamp(2.0, 512.0) as u16,
            rows: (content.height() / cell.y).floor().clamp(1.0, 512.0) as u16,
            pixel_width: content.width().max(0.0).min(u16::MAX as f32) as u16,
            pixel_height: content.height().max(0.0).min(u16::MAX as f32) as u16,
        });
        let focused = response.has_focus() && ui.is_enabled();
        let (mode, display_offset) = {
            let mut term = self.term.lock().unwrap();
            let focus_changed = term.is_focused != focused;
            term.is_focused = focused;
            if focus_changed && term.mode().contains(TermMode::FOCUS_IN_OUT) {
                self.write(if focused { b"\x1b[I" } else { b"\x1b[O" }.to_vec());
            }
            (*term.mode(), term.grid().display_offset())
        };
        if ui.is_enabled() {
            self.pointer_input(ui, &response, content, cell, mode, display_offset);
        }
        if focused {
            let events = ui.input_mut(|input| {
                let mut events = Vec::new();
                input.events.retain(|event| {
                    if matches!(
                        event,
                        egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::Copy
                            | egui::Event::Cut
                            | egui::Event::Paste(_)
                            | egui::Event::Ime(_)
                    ) {
                        events.push(event.clone());
                        false
                    } else {
                        true
                    }
                });
                events
            });
            self.handle_events(&events);
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 0.0, crate::palette::BG);
        let term = self.term.lock().unwrap();
        let rendered = term.renderable_content();
        let cursor = rendered.cursor.point;
        let shape = rendered.cursor.shape;
        let cursor_row = cursor.line.0 + rendered.display_offset as i32;
        let cursor_rect = Rect::from_min_size(
            content.min + Vec2::new(cursor.column.0 as f32 * cell.x, cursor_row as f32 * cell.y),
            cell,
        );
        let selection = rendered.selection;
        for indexed in rendered.display_iter {
            let row = indexed.point.line.0 + rendered.display_offset as i32;
            let cell_rect = Rect::from_min_size(
                content.min
                    + Vec2::new(indexed.point.column.0 as f32 * cell.x, row as f32 * cell.y),
                cell,
            );
            if !cell_rect.intersects(painter.clip_rect()) {
                continue;
            }
            let flags = indexed.cell.flags;
            let mut foreground = color(
                indexed.cell.fg,
                rendered.colors,
                flags.contains(Flags::BOLD),
            );
            let mut background = color(indexed.cell.bg, rendered.colors, false);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if flags.contains(Flags::DIM) {
                foreground = foreground.gamma_multiply(0.66);
            }
            if selection.is_some_and(|selection| selection.contains(indexed.point)) {
                background = crate::palette::BLUE.gamma_multiply(0.35);
            }
            if background != crate::palette::BG {
                painter.rect_filled(cell_rect, 0.0, background);
            }
            let block_cursor = cursor == indexed.point
                && shape == CursorShape::Block
                && focused
                && rendered.display_offset == 0;
            if block_cursor {
                painter.rect_filled(cell_rect, 0.0, crate::palette::TEXT);
                foreground = crate::palette::BG;
            }
            if !flags.intersects(
                Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN,
            ) && indexed.cell.c != ' '
            {
                let mut text = indexed.cell.c.to_string();
                if let Some(combining) = indexed.cell.zerowidth() {
                    text.extend(combining);
                }
                painter.text(
                    cell_rect.min,
                    Align2::LEFT_TOP,
                    text,
                    font.clone(),
                    foreground,
                );
            }
            if flags.intersects(Flags::ALL_UNDERLINES) {
                painter.line_segment(
                    [
                        cell_rect.left_bottom() - Vec2::new(0.0, 1.5),
                        cell_rect.right_bottom() - Vec2::new(0.0, 1.5),
                    ],
                    Stroke::new(1.0, foreground),
                );
            }
            if flags.contains(Flags::STRIKEOUT) {
                painter.line_segment(
                    [cell_rect.left_center(), cell_rect.right_center()],
                    Stroke::new(1.0, foreground),
                );
            }
        }
        if rendered.display_offset == 0
            && shape != CursorShape::Hidden
            && cursor_rect.intersects(content)
        {
            let stroke = Stroke::new(1.0, crate::palette::TEXT);
            if !focused || shape == CursorShape::HollowBlock {
                painter.rect_stroke(cursor_rect, 0.0, stroke, egui::StrokeKind::Inside);
            } else if shape == CursorShape::Beam {
                painter.line_segment([cursor_rect.left_top(), cursor_rect.left_bottom()], stroke);
            } else if shape == CursorShape::Underline {
                painter.line_segment(
                    [cursor_rect.left_bottom(), cursor_rect.right_bottom()],
                    stroke,
                );
            }
        }
        if focused {
            ui.ctx().output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Normal,
                    rect,
                    cursor_rect,
                    should_interrupt_composition: false,
                })
            });
            if !self.preedit.is_empty() {
                let galley =
                    painter.layout_no_wrap(self.preedit.clone(), font, crate::palette::TEXT);
                let preedit_rect = Rect::from_min_size(cursor_rect.min, galley.size());
                painter.rect_filled(preedit_rect, 0.0, crate::palette::SURFACE);
                painter.galley(preedit_rect.min, galley, crate::palette::TEXT);
                painter.line_segment(
                    [preedit_rect.left_bottom(), preedit_rect.right_bottom()],
                    Stroke::new(1.0, crate::palette::ACCENT),
                );
            }
        }
        drop(term);
        if !mode.intersects(TermMode::MOUSE_MODE) || ui.input(|input| input.modifiers.shift) {
            response.context_menu(|ui| {
                let selected = self
                    .term
                    .lock()
                    .unwrap()
                    .selection_to_string()
                    .filter(|text| !text.is_empty());
                if ui
                    .add_enabled(selected.is_some(), egui::Button::new("Copy"))
                    .clicked()
                {
                    if let Some(text) = selected {
                        ui.ctx().copy_text(text);
                    }
                    ui.close();
                }
                if ui.button("Paste").clicked() {
                    response.request_focus();
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    ui.close();
                }
                if ui.button("Scroll to bottom").clicked() {
                    self.term.lock().unwrap().scroll_display(Scroll::Bottom);
                    ui.close();
                }
            });
        }
        response.on_hover_cursor(egui::CursorIcon::Text)
    }

    fn pointer_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        cell: Vec2,
        mode: TermMode,
        display_offset: usize,
    ) {
        let (modifiers, position, pressed, down, wheel) = ui.input(|input| {
            (
                input.modifiers,
                input.pointer.interact_pos(),
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
                input.smooth_scroll_delta.y,
            )
        });
        let mouse_reporting = mode.intersects(TermMode::MOUSE_MODE) && !modifiers.shift;
        if mouse_reporting {
            let events = ui.input(|input| input.events.clone());
            for event in events {
                match event {
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers,
                    } if response.rect.contains(pos) => {
                        let button = match button {
                            egui::PointerButton::Primary => 0,
                            egui::PointerButton::Middle => 1,
                            egui::PointerButton::Secondary => 2,
                            _ => continue,
                        };
                        self.write(mouse_bytes(
                            pos, rect, cell, button, pressed, modifiers, mode,
                        ));
                    }
                    egui::Event::PointerMoved(pos)
                        if response.rect.contains(pos)
                            && (mode.contains(TermMode::MOUSE_MOTION)
                                || mode.contains(TermMode::MOUSE_DRAG) && down) =>
                    {
                        let button = if down { 0 } else { 3 };
                        self.write(mouse_bytes(
                            pos,
                            rect,
                            cell,
                            32 + button,
                            true,
                            modifiers,
                            mode,
                        ));
                    }
                    _ => {}
                }
            }
        } else if let Some(position) = position {
            let point = Point::new(
                Line(
                    ((position.y - rect.top()) / cell.y)
                        .floor()
                        .clamp(0.0, self.size.rows.saturating_sub(1) as f32)
                        as i32
                        - display_offset as i32,
                ),
                Column(
                    ((position.x - rect.left()) / cell.x)
                        .floor()
                        .clamp(0.0, self.size.cols.saturating_sub(1) as f32)
                        as usize,
                ),
            );
            let side = if ((position.x - rect.left()) / cell.x).fract() < 0.5 {
                Side::Left
            } else {
                Side::Right
            };
            if pressed && response.rect.contains(position)
                || response.double_clicked()
                || response.triple_clicked()
            {
                let ty = if response.triple_clicked() {
                    SelectionType::Lines
                } else if response.double_clicked() {
                    SelectionType::Semantic
                } else if modifiers.alt {
                    SelectionType::Block
                } else {
                    SelectionType::Simple
                };
                self.term.lock().unwrap().selection = Some(Selection::new(ty, point, side));
            } else if response.dragged()
                && down
                && let Some(selection) = &mut self.term.lock().unwrap().selection
            {
                selection.update(point, side);
            }
        }
        if response.hovered() && !modifiers.ctrl && wheel.abs() > 0.01 {
            ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
            self.scroll_rows += wheel / cell.y;
            let lines = self.scroll_rows.trunc() as i32;
            self.scroll_rows -= lines as f32;
            if lines == 0 {
                return;
            }
            if mouse_reporting {
                if let Some(position) = position {
                    for _ in 0..lines.unsigned_abs().min(32) {
                        self.write(mouse_bytes(
                            position,
                            rect,
                            cell,
                            if lines > 0 { 64 } else { 65 },
                            true,
                            modifiers,
                            mode,
                        ));
                    }
                }
            } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                let sequence: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                self.write(sequence.repeat(lines.unsigned_abs().min(32) as usize));
            } else {
                self.term
                    .lock()
                    .unwrap()
                    .scroll_display(Scroll::Delta(lines));
            }
        }
    }
}

fn mouse_bytes(
    position: Pos2,
    rect: Rect,
    cell: Vec2,
    button: u8,
    pressed: bool,
    modifiers: egui::Modifiers,
    mode: TermMode,
) -> Vec<u8> {
    let column = ((position.x - rect.left()) / cell.x).floor().max(0.0) as u32 + 1;
    let row = ((position.y - rect.top()) / cell.y).floor().max(0.0) as u32 + 1;
    let button = button
        + 4 * u8::from(modifiers.shift)
        + 8 * u8::from(modifiers.alt)
        + 16 * u8::from(modifiers.ctrl);
    if mode.contains(TermMode::SGR_MOUSE) {
        format!(
            "\x1b[<{button};{column};{row}{}",
            if pressed { 'M' } else { 'm' }
        )
        .into_bytes()
    } else if column <= 223 && row <= 223 {
        vec![
            0x1b,
            b'[',
            b'M',
            32 + if pressed { button } else { 3 },
            32 + column as u8,
            32 + row as u8,
        ]
    } else {
        Vec::new()
    }
}

fn color(value: Color, colors: &alacritty_terminal::term::color::Colors, bold: bool) -> Color32 {
    let rgb = match value {
        Color::Spec(rgb) => rgb,
        Color::Indexed(index) => {
            colors[index as usize].unwrap_or_else(|| default_color(index as usize))
        }
        Color::Named(named) => {
            let named = if bold { named.to_bright() } else { named };
            colors[named].unwrap_or_else(|| default_color(named as usize))
        }
    };
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

pub(super) fn default_color(index: usize) -> Rgb {
    let color = match index {
        0..=15 => [
            [0x26, 0x23, 0x1e],
            [0xd9, 0x68, 0x68],
            [0x78, 0xbd, 0x78],
            [0xd8, 0xbc, 0x70],
            [0x74, 0xad, 0xf2],
            [0xbc, 0x8e, 0xd2],
            [0x71, 0xbf, 0xc3],
            [0xd8, 0xd2, 0xc6],
            [0x82, 0x79, 0x6e],
            [0xf2, 0x8b, 0x82],
            [0x5f, 0xcf, 0x8f],
            [0xe5, 0xb5, 0x3e],
            [0x9c, 0xc5, 0xf7],
            [0xd5, 0xaa, 0xe8],
            [0x8e, 0xdc, 0xdf],
            [0xf1, 0xed, 0xe6],
        ][index],
        16..=231 => {
            let level = |value: usize| if value == 0 { 0 } else { 55 + value as u8 * 40 };
            let index = index - 16;
            [level(index / 36), level(index / 6 % 6), level(index % 6)]
        }
        232..=255 => [8 + (index as u8 - 232) * 10; 3],
        index if index == NamedColor::Background as usize => {
            crate::palette::BG.to_array()[..3].try_into().unwrap()
        }
        259..=266 => {
            let rgb = default_color(index - 259);
            [rgb.r / 2, rgb.g / 2, rgb.b / 2]
        }
        index if index == NamedColor::DimForeground as usize => [0xb3, 0xaa, 0x9e],
        _ => [0xf1, 0xed, 0xe6],
    };
    Rgb {
        r: color[0],
        g: color[1],
        b: color[2],
    }
}
