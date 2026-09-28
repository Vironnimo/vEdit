//! User-owned excerpts for agents, separate from editor selection and review notes.
use super::*;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

pub(super) const AGENT_MARK_COLOR: Color32 = Color32::from_rgb(192, 159, 241);
const MARK_ROW_HEIGHT: f32 = 54.0;
const GROUP_HEIGHT: f32 = 30.0;

#[derive(Default)]
pub(super) struct AgentMarksUi {
    open: bool,
    query: String,
    selected: Option<u64>,
    focus: bool,
    editing: Option<MarkNoteEditor>,
    filter: MarkFilterCache,
    row_focus_ids: Vec<(u64, egui::Id)>,
}

#[derive(Default)]
struct MarkFilterCache {
    query: String,
    stamps: Vec<MarkSearchStamp>,
    matches: Vec<usize>,
}

#[derive(PartialEq, Eq)]
struct MarkSearchStamp {
    id: u64,
    source_hash: u64,
    note_hash: u64,
    workspace: PathBuf,
    path: Option<PathBuf>,
    title: String,
}

impl MarkFilterCache {
    fn matching(
        &mut self,
        query: &str,
        marks: &[crate::agent_marks::UserMark],
        rows: &[MarkRow],
    ) -> Vec<usize> {
        if query.is_empty() {
            return (0..marks.len()).collect();
        }
        let stamps: Vec<_> = marks
            .iter()
            .map(|mark| {
                let mut note_hash = std::collections::hash_map::DefaultHasher::new();
                mark.note.hash(&mut note_hash);
                MarkSearchStamp {
                    id: mark.id,
                    source_hash: mark.source_fingerprint.hash,
                    note_hash: note_hash.finish(),
                    workspace: mark.target.workspace.clone(),
                    path: mark.target.path.clone(),
                    title: mark.target.title.clone(),
                }
            })
            .collect();
        if self.query != query || self.stamps != stamps {
            let needle = CaseFoldNeedle::new(query);
            self.matches = marks
                .iter()
                .zip(rows)
                .enumerate()
                .filter_map(|(index, (mark, row))| {
                    (needle.matches(&row.file)
                        || needle.matches(&row.full_path)
                        || needle.matches(&row.workspace)
                        || needle.matches(&mark.note)
                        || needle.matches(&mark.snippet))
                    .then_some(index)
                })
                .collect();
            self.query = query.to_owned();
            self.stamps = stamps;
        }
        self.matches.clone()
    }
}

/// Search the complete snapshot without allocating a lowercase copy of it.
struct CaseFoldNeedle {
    chars: Vec<char>,
    fallback: Vec<usize>,
}

impl CaseFoldNeedle {
    fn new(query: &str) -> Self {
        let chars: Vec<_> = query.chars().flat_map(char::to_lowercase).collect();
        let mut fallback = vec![0; chars.len()];
        for index in 1..chars.len() {
            let mut prefix = fallback[index - 1];
            while prefix > 0 && chars[index] != chars[prefix] {
                prefix = fallback[prefix - 1];
            }
            if chars[index] == chars[prefix] {
                prefix += 1;
            }
            fallback[index] = prefix;
        }
        Self { chars, fallback }
    }

    fn matches(&self, text: &str) -> bool {
        if self.chars.is_empty() {
            return true;
        }
        let mut matched = 0;
        for ch in text.chars().flat_map(char::to_lowercase) {
            while matched > 0 && ch != self.chars[matched] {
                matched = self.fallback[matched - 1];
            }
            if ch == self.chars[matched] {
                matched += 1;
            }
            if matched == self.chars.len() {
                return true;
            }
        }
        false
    }
}

struct MarkNoteEditor {
    id: u64,
    text: String,
    focus: bool,
}

struct MarkRow {
    id: u64,
    file: String,
    full_path: String,
    workspace: String,
    location: String,
    range: Range<usize>,
    excerpt: String,
    note: String,
    tooltip: String,
    stale: bool,
}

enum ListEntry {
    Group { title: String, tooltip: String },
    Mark(usize),
}

impl ListEntry {
    fn height(&self) -> f32 {
        match self {
            Self::Group { .. } => GROUP_HEIGHT,
            Self::Mark(_) => MARK_ROW_HEIGHT,
        }
    }
}

#[derive(Clone)]
pub(super) struct AgentMarkOverlay {
    id: u64,
    range: Range<usize>,
    tooltip: String,
}

#[derive(Clone, Default)]
struct OverlayPointer {
    rects: Vec<egui::Rect>,
    owned: bool,
}

impl MorrowApp {
    pub(super) fn agent_marks_modal_open(&self) -> bool {
        self.agent_marks_ui.open
    }

    pub(super) fn open_agent_marks(&mut self) {
        self.cancel_navigation();
        self.palette = None;
        self.picker = None;
        self.completions = None;
        self.agent_marks_ui = AgentMarksUi {
            open: true,
            focus: true,
            ..Default::default()
        };
        self.ctx.memory_mut(|memory| {
            memory.request_focus(egui::Id::new("agent_marks_filter"));
        });
        self.ctx.request_repaint();
    }

    pub(super) fn agent_marks_toolbar(&mut self, ui: &mut egui::Ui) {
        let count = self.agent_marks.items.len();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(44.0, 28.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "For agent")
        });
        if response.hovered() || response.has_focus() || self.agent_marks_ui.open {
            ui.painter().rect_filled(rect, 4.0, HOVER);
        }
        let color = if count > 0 || self.agent_marks_ui.open {
            AGENT_MARK_COLOR
        } else {
            MUTED
        };
        let center = rect.left_center() + Vec2::new(13.0, 0.0);
        // A small highlighted excerpt, drawn as paths so no symbol font is required.
        for y in [-4.0, 0.0, 4.0] {
            ui.painter().line_segment(
                [center + Vec2::new(-4.0, y), center + Vec2::new(4.0, y)],
                Stroke::new(1.3, color),
            );
        }
        ui.painter().line_segment(
            [
                center + Vec2::new(-7.0, -6.0),
                center + Vec2::new(-7.0, 6.0),
            ],
            Stroke::new(2.0, color),
        );
        let badge = if count > 99 {
            "99+".into()
        } else {
            count.to_string()
        };
        ui.painter().text(
            rect.right_center() - Vec2::new(5.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            badge,
            FontId::proportional(10.5),
            color,
        );
        if response.clicked() {
            self.open_agent_marks();
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "For agent · {count} marked block{} across all projects\nSelect text, then Ctrl+Shift+A or Leader a to add it.\nOpen this collection to add notes, remove marks, or restore the most recently removed or agent-collected blocks.",
                block_suffix(count),
            ));
    }

    pub(super) fn agent_mark_overlay_rows(&self, document_id: u64) -> Vec<AgentMarkOverlay> {
        self.agent_marks
            .visible_marks(document_id)
            .map(|mark| AgentMarkOverlay {
                id: mark.id,
                range: mark.range.start..mark.range.end,
                tooltip: format!(
                    "For agent · {}\n{}{}\n{}",
                    mark_location(mark),
                    mark_exact_range(mark),
                    if mark.note.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", short_text(&mark.note, 400))
                    },
                    short_text(&mark.snippet, 500),
                ),
            })
            .collect()
    }

    pub(super) fn agent_marks_dialog(&mut self) {
        if !self.agent_marks_ui.open {
            return;
        }
        let mut state = std::mem::take(&mut self.agent_marks_ui);
        let rows = self
            .agent_marks
            .items
            .iter()
            .map(|mark| {
                let path = mark.target.path.as_deref();
                let file = path
                    .map(|path| {
                        crate::paths::display(
                            path.strip_prefix(&mark.target.workspace).unwrap_or(path),
                        )
                    })
                    .unwrap_or_else(|| mark.target.title.clone());
                let full_path = path
                    .map(crate::paths::display)
                    .unwrap_or_else(|| format!("{} · unsaved document", mark.target.title));
                let workspace = file_title(&mark.target.workspace);
                let location = mark_location(mark);
                MarkRow {
                    id: mark.id,
                    file,
                    full_path: full_path.clone(),
                    workspace,
                    location: location.clone(),
                    range: mark.range.start..mark.range.end,
                    excerpt: short_first_line(&mark.snippet, 180),
                    note: short_text(&mark.note, 150),
                    tooltip: format!(
                        "{full_path}\n{location}\n{}\n{}{}",
                        mark_exact_range(mark),
                        short_text(&mark.snippet, 700),
                        if mark.note.is_empty() {
                            String::new()
                        } else {
                            format!("\nNote: {}", short_text(&mark.note, 500))
                        }
                    ),
                    stale: mark.stale,
                }
            })
            .collect::<Vec<_>>();
        let query = state.query.to_lowercase();
        let matching = state
            .filter
            .matching(&query, &self.agent_marks.items, &rows);
        let mut groups = BTreeMap::<(String, String), Vec<usize>>::new();
        for &index in &matching {
            let row = &rows[index];
            groups
                .entry((row.workspace.clone(), row.full_path.clone()))
                .or_default()
                .push(index);
        }
        let mut entries = Vec::new();
        let mut entry_tops = Vec::new();
        let mut total_height = 0.0;
        for ((workspace, _), mut indices) in groups {
            indices.sort_by_key(|&index| {
                (
                    rows[index].range.start,
                    rows[index].range.end,
                    rows[index].id,
                )
            });
            let row = &rows[indices[0]];
            entry_tops.push(total_height);
            entries.push(ListEntry::Group {
                title: format!("{workspace} · {}", row.file),
                tooltip: row.full_path.clone(),
            });
            total_height += GROUP_HEIGHT;
            for index in indices {
                entry_tops.push(total_height);
                entries.push(ListEntry::Mark(index));
                total_height += MARK_ROW_HEIGHT;
            }
        }
        let ordered_ids: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                ListEntry::Mark(index) => Some(rows[*index].id),
                _ => None,
            })
            .collect();
        if !ordered_ids.contains(&state.selected.unwrap_or(0)) {
            state.selected = ordered_ids.first().copied();
        }
        let ctx = self.ctx.clone();
        let can_add = self.tab().is_some_and(|tab| {
            !tab.loading
                && tab.error.is_none()
                && tab.agent_view == crate::cli::DocumentView::Source
                && !tab.selection.is_empty()
        });
        let mut close = false;
        let mut add = false;
        let mut remove = None;
        let mut clear = false;
        let mut restore = false;
        let mut jump = None;
        let mut edit_note = None;
        let mut save_note = None;
        let mut query_changed = false;
        let restore_count = self.agent_marks.restore_count();
        let can_restore = self.agent_marks.can_restore();
        let restore_origin = self.agent_marks.restore_origin();
        let restore_detail = restore_summary(restore_count, restore_origin);
        let restore_tooltip = if can_restore {
            format!("Restore {restore_detail}, keeping new marks and notes")
        } else {
            "No removed or agent-collected blocks are missing from the current collection".into()
        };
        let mut next_row_focus_ids = Vec::new();
        let pending_batch = self
            .agent_marks
            .last_batch()
            .filter(|batch| !batch.acknowledged)
            .map(|batch| (batch.id, batch.items.len()));
        let focused_before = ctx.memory(|memory| memory.focused());
        let modal = egui::Modal::new(egui::Id::new("agent_marks_dialog"))
            .backdrop_color(Color32::from_black_alpha(80))
            .show(&ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 48.0).clamp(260.0, 680.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("For agent").size(20.0).strong());
                    ui.label(RichText::new(format!("{} block{}", rows.len(), block_suffix(rows.len()))).size(11.0).color(AGENT_MARK_COLOR));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let response = design::ghost_button(ui, "×", "Close marked blocks (Esc)");
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close marked blocks"));
                        close = response.clicked();
                    });
                });
                ui.add_space(8.0);
                ui.add_enabled_ui(state.editing.is_none(), |ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut state.query)
                        .id(egui::Id::new("agent_marks_filter"))
                        .hint_text("Find a file, excerpt, or note…")
                        .desired_width(f32::INFINITY));
                    if state.focus && !ui.is_sizing_pass() {
                        response.request_focus();
                        state.focus = false;
                    }
                    if response.changed() {
                        state.selected = None;
                        query_changed = true;
                        ctx.request_repaint();
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                let filter_id = egui::Id::new("agent_marks_filter");
                // Single-line TextEdit releases focus on Enter; keep ownership
                // from before it rendered, but never replace a focused action.
                let focused = ctx.memory(|memory| memory.focused()).or(focused_before);
                let focused_row = state.row_focus_ids.iter()
                    .find(|(_, id)| Some(*id) == focused).map(|(mark, _)| *mark);
                if let Some(id) = focused_row { state.selected = Some(id); }
                let keys_enabled = state.editing.is_none() && !egui::Popup::is_any_open(&ctx)
                    && (focused == Some(filter_id) || focused_row.is_some());
                let up = keys_enabled && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                let down = keys_enabled && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                let home = keys_enabled && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Home));
                let end = keys_enabled && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::End));
                if !ordered_ids.is_empty() && (up || down || home || end) {
                    let index = ordered_ids.iter().position(|id| Some(*id) == state.selected).unwrap_or(0);
                    let next = if home { 0 } else if end { ordered_ids.len() - 1 }
                        else if up { index.saturating_sub(1) } else { (index + 1).min(ordered_ids.len() - 1) };
                    state.selected = Some(ordered_ids[next]);
                    if focused_row.is_some() {
                        // The query remains the keyboard entry point when a
                        // chosen row needs to be virtualized into view.
                        ctx.memory_mut(|memory| memory.request_focus(filter_id));
                    }
                }
                if keys_enabled && focused == Some(filter_id) && !query_changed
                    && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    jump = state.selected;
                }
                let height = (ctx.content_rect().height() - if state.editing.is_some() { 355.0 } else { 285.0 }).clamp(100.0, 420.0);
                if rows.is_empty() {
                    ui.add_space(18.0);
                    ui.label(RichText::new("Choose the blocks you want an agent to see.").strong());
                    ui.label(RichText::new("Select text in any file, then Ctrl+Shift+A or Leader a. Add as many blocks as you need, across files and projects.").color(MUTED));
                    ui.add_space(22.0);
                } else if matching.is_empty() {
                    ui.add_space(16.0);
                    ui.label(RichText::new("No matching blocks. Try another file name, excerpt, or note.").color(MUTED));
                    ui.add_space(20.0);
                } else {
                    ui.scope(|ui| {
                        ui.spacing_mut().scroll.floating = false;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut scroll = egui::ScrollArea::vertical().id_salt("agent_marks_list")
                            .animated(false).max_height(height)
                            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
                        if (up || down || home || end) && let Some(index) = entries.iter().position(|entry|
                            matches!(entry, ListEntry::Mark(index) if Some(rows[*index].id) == state.selected)) {
                            scroll = scroll.vertical_scroll_offset((entry_tops[index] - height * 0.5).max(0.0));
                        }
                        scroll.show_viewport(ui, |ui, viewport| {
                            let first = entry_tops.partition_point(|top| *top < viewport.top())
                                .saturating_sub(1);
                            let last = entry_tops.partition_point(|top| *top < viewport.bottom()).min(entries.len());
                            ui.add_space(entry_tops.get(first).copied().unwrap_or(0.0));
                            for entry in entries.iter().take(last).skip(first) {
                                match entry {
                                    ListEntry::Group { title, tooltip } => {
                                        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), GROUP_HEIGHT), egui::Sense::hover());
                                        paint_single_line(ui, rect.shrink2(Vec2::new(8.0, 0.0)), title, FontId::proportional(11.0), MUTED);
                                        response.on_hover_text(tooltip);
                                    }
                                    ListEntry::Mark(index) => {
                                        let row = &rows[*index];
                                        let action = ui.push_id(row.id, |ui| mark_row(ui, row, state.selected == Some(row.id), state.editing.is_none())).inner;
                                        next_row_focus_ids.push((row.id, action.focus_id));
                                        if action.open { jump = Some(row.id); }
                                        if action.remove { remove = Some(row.id); }
                                        if action.note { edit_note = Some(row.id); }
                                    }
                                }
                            }
                            let end_y = entries.iter().take(last).map(ListEntry::height).sum::<f32>();
                            ui.add_space((total_height - end_y).max(0.0));
                        });
                    });
                }
                ui.add_space(8.0);
                ui.separator();
                if let Some(editor) = &mut state.editing {
                    ui.label(RichText::new("Note for the agent").size(11.0).color(AGENT_MARK_COLOR));
                    let response = ui.add(egui::TextEdit::singleline(&mut editor.text)
                        .id(egui::Id::new("agent_mark_note"))
                        .hint_text("What should the agent do with this block?")
                        .char_limit(4096).desired_width(f32::INFINITY));
                    if editor.focus && !ui.is_sizing_pass() { response.request_focus(); editor.focus = false; }
                    let owns_enter = response.has_focus()
                        || (focused_before == Some(response.id)
                            && ctx.memory(|memory| memory.focused().is_none()));
                    let enter = owns_enter
                        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
                    let escape = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
                    let mut cancel = escape;
                    ui.horizontal(|ui| {
                        if design::primary_button(ui, "Save note", Vec2::new(94.0, 28.0)).clicked() || enter {
                            save_note = Some((editor.id, editor.text.clone()));
                        }
                        cancel |= design::ghost_button(ui, "Cancel", "Keep the previous note").clicked();
                    });
                    if cancel { state.editing = None; state.focus = true; }
                } else {
                    ui.horizontal_wrapped(|ui| {
                        let response = ui.add_enabled_ui(can_add, |ui|
                            design::ghost_button(ui, "Add selection", "Add the current source selection · Ctrl+Shift+A / Leader a. Preview text cannot be marked; switch to source first.")).inner;
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, can_add, "Add selection"));
                        add = response.clicked();
                        restore = ui.add_enabled_ui(can_restore, |ui|
                            design::ghost_button(ui, "Restore last", &restore_tooltip)).inner.clicked();
                        ui.label(RichText::new(&restore_detail).size(10.5).color(MUTED));
                        clear = ui.add_enabled_ui(!rows.is_empty(), |ui|
                            design::ghost_button(ui, "Clear all", "Remove all current marks and make them the Restore last target. Source text stays unchanged.")).inner.clicked();
                    });
                    ui.label(RichText::new("↑ ↓ choose · Enter opens · Esc closes. Restoration keeps new marks and notes.")
                        .size(10.5).color(MUTED));
                    if let Some((batch, count)) = pending_batch {
                        ui.label(RichText::new(format!("Agent collected {count} block{} · awaiting acknowledgment", block_suffix(count)))
                            .size(10.5).color(AGENT_MARK_COLOR))
                            .on_hover_text(format!(
                                "The agent has not confirmed receiving every block and complete excerpt. This collection remains retained until a later agent collection replaces it.\n{}",
                                if restore_origin == crate::agent_marks::RestoreSource::Taken(batch) {
                                    "Restore last returns blocks from this collection that are not already marked."
                                } else {
                                    "Restore last currently targets the most recently removed blocks."
                                },
                            ));
                    }
                }
            });
        close |= modal.should_close()
            || modal
                .backdrop_response
                .clicked_by(egui::PointerButton::Secondary)
            || modal
                .backdrop_response
                .clicked_by(egui::PointerButton::Middle);
        if let Some(id) = remove {
            self.agent_marks_remove(id);
        }
        if clear {
            self.agent_marks_clear();
        }
        if restore {
            self.agent_marks_restore();
        }
        if add {
            self.capture_agent_mark();
        }
        if let Some(id) = edit_note
            && let Some(mark) = self.agent_marks.items.iter().find(|mark| mark.id == id)
        {
            state.editing = Some(MarkNoteEditor {
                id,
                text: mark.note.clone(),
                focus: true,
            });
        }
        if let Some((id, note)) = save_note
            && self.agent_marks_set_note(id, &note)
        {
            state.editing = None;
            state.focus = true;
        }
        if jump.is_some() {
            close = true;
        }
        state.open = !close;
        state.row_focus_ids = next_row_focus_ids;
        self.agent_marks_ui = state;
        if close {
            if let Some(id) = jump {
                if let Err(error) = self.navigate_agent_mark(id) {
                    self.status(error, true);
                }
            } else if let Some(tab) = self.tab_mut() {
                tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
            }
        }
    }
}

struct MarkRowAction {
    open: bool,
    note: bool,
    remove: bool,
    focus_id: egui::Id,
}

fn mark_row(ui: &mut egui::Ui, row: &MarkRow, selected: bool, enabled: bool) -> MarkRowAction {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), MARK_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let remove_rect = egui::Rect::from_center_size(
        rect.right_center() - Vec2::new(16.0, 0.0),
        Vec2::splat(24.0),
    );
    let note_rect = remove_rect.translate(Vec2::new(-28.0, 0.0));
    let open_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(note_rect.left() - 4.0, rect.bottom()));
    let response = ui.interact(
        open_rect,
        ui.id().with("open"),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            enabled,
            selected,
            format!(
                "Marked block {}: {} · {} · {}{}{}",
                row.location,
                row.excerpt,
                row.workspace,
                row.file,
                if row.stale {
                    " · changed; saved excerpt kept"
                } else {
                    ""
                },
                if row.note.is_empty() {
                    String::new()
                } else {
                    format!(" · note: {}", row.note)
                }
            ),
        )
    });
    if selected || response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            4.0,
            if selected {
                Color32::from_rgb(48, 41, 65)
            } else {
                SURFACE
            },
        );
    }
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            rect.left_top() + Vec2::new(0.0, 7.0),
            rect.left_bottom() + Vec2::new(2.0, -7.0),
        ),
        1.0,
        AGENT_MARK_COLOR,
    );
    let first = egui::Rect::from_min_max(
        open_rect.min + Vec2::new(10.0, 4.0),
        open_rect.max - Vec2::new(2.0, 27.0),
    );
    let label = format!(
        "{}{}{}",
        row.location,
        if row.stale {
            " · changed; saved excerpt kept"
        } else {
            ""
        },
        if row.note.is_empty() {
            String::new()
        } else {
            format!(" · {}", row.note)
        }
    );
    paint_single_line(
        ui,
        first,
        &label,
        FontId::proportional(11.5),
        if row.stale { ORANGE } else { TEXT },
    );
    let second = egui::Rect::from_min_max(
        open_rect.min + Vec2::new(10.0, 26.0),
        open_rect.max - Vec2::new(2.0, 6.0),
    );
    paint_single_line(ui, second, &row.excerpt, FontId::monospace(11.0), MUTED);
    let note = ui.interact(
        note_rect,
        ui.id().with("note"),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    note.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            enabled,
            format!(
                "Edit note for mark {} · {} · {} · {}",
                row.id, row.workspace, row.file, row.location
            ),
        )
    });
    if note.hovered() || note.has_focus() {
        ui.painter().rect_filled(note_rect, 4.0, HOVER);
    }
    let c = note_rect.center();
    ui.painter().line_segment(
        [c + Vec2::new(-4.0, 4.0), c + Vec2::new(4.0, -4.0)],
        Stroke::new(
            1.6,
            if row.note.is_empty() {
                MUTED
            } else {
                AGENT_MARK_COLOR
            },
        ),
    );
    ui.painter().line_segment(
        [c + Vec2::new(-5.0, 1.0), c + Vec2::new(-5.0, 5.0)],
        Stroke::new(1.2, MUTED),
    );
    ui.painter().line_segment(
        [c + Vec2::new(-5.0, 5.0), c + Vec2::new(-1.0, 5.0)],
        Stroke::new(1.2, MUTED),
    );
    let remove = ui.interact(
        remove_rect,
        ui.id().with("remove"),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    remove.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            enabled,
            format!(
                "Remove mark {} · {} · {} · {}",
                row.id, row.workspace, row.file, row.location
            ),
        )
    });
    paint_cross(
        ui,
        remove_rect,
        remove.hovered() || remove.has_focus(),
        MUTED,
    );
    let action = MarkRowAction {
        open: response.clicked(),
        note: note.clicked(),
        remove: remove.clicked(),
        focus_id: response.id,
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(&row.tooltip);
    note.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Add or edit a note for the agent");
    remove
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Remove this marked block; the source text stays unchanged");
    action
}

fn block_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn restore_summary(count: usize, origin: crate::agent_marks::RestoreSource) -> String {
    if count == 0 {
        return "Nothing to restore".into();
    }
    let origin = match origin {
        crate::agent_marks::RestoreSource::Removed => "removed",
        crate::agent_marks::RestoreSource::Taken(_) => "agent-collected",
    };
    format!("{count} {origin} block{}", block_suffix(count))
}

fn mark_location(mark: &crate::agent_marks::UserMark) -> String {
    // A next-line boundary includes the previous line's ending, not that next line.
    let last_line = if mark.end.column == 1 && mark.end.line > mark.start.line {
        mark.end.line - 1
    } else {
        mark.end.line
    };
    let lines = if last_line == mark.start.line {
        format!("Line {}", mark.start.line)
    } else {
        format!("Lines {}–{last_line}", mark.start.line)
    };
    let count = mark.snippet_chars();
    format!(
        "{lines} · {count} character{}",
        if count == 1 { "" } else { "s" }
    )
}

fn mark_exact_range(mark: &crate::agent_marks::UserMark) -> String {
    format!(
        "Exact range: {}:{} → {}:{} (end excluded)",
        mark.start.line, mark.start.column, mark.end.line, mark.end.column,
    )
}

fn short_text(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn short_first_line(text: &str, limit: usize) -> String {
    let mut chars = text.chars().take_while(|ch| *ch != '\n' && *ch != '\r');
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn paint_single_line(ui: &egui::Ui, rect: egui::Rect, text: &str, font: FontId, color: Color32) {
    let mut job = egui::text::LayoutJob::simple(text.into(), font, color, rect.width().max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
}

fn paint_cross(ui: &egui::Ui, rect: egui::Rect, hovered: bool, color: Color32) {
    if hovered {
        ui.painter().rect_filled(rect, 4.0, HOVER);
    }
    let c = rect.center();
    ui.painter().line_segment(
        [c - Vec2::splat(3.0), c + Vec2::splat(3.0)],
        Stroke::new(1.3, color),
    );
    ui.painter().line_segment(
        [c + Vec2::new(-3.0, 3.0), c + Vec2::new(3.0, -3.0)],
        Stroke::new(1.3, color),
    );
}

/// Resolve ownership before TextEdit processes input. Cached badge rectangles
/// let a remove click keep the ordinary selection and Vim drag anchor intact.
pub(super) fn marker_pointer_owns_editor(ctx: &egui::Context, widget: egui::Id) -> bool {
    let key = widget.with("agent_mark_pointer");
    let mut state = ctx
        .data(|data| data.get_temp::<OverlayPointer>(key))
        .unwrap_or_default();
    let (pressed, down, released, point) = ctx.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
            input.pointer.interact_pos(),
        )
    });
    if pressed {
        state.owned =
            point.is_some_and(|point| state.rects.iter().any(|rect| rect.contains(point)));
    }
    let owned = state.owned && (down || released);
    if !down {
        state.owned = false;
    }
    ctx.data_mut(|data| data.insert_temp(key, state));
    owned
}

/// Paint against the shaped source rows, including wrapping and horizontal scroll.
/// Buttons live in a dedicated gutter outside source glyphs and line numbers.
pub(super) fn paint_agent_mark_overlays(
    ui: &mut egui::Ui,
    widget: egui::Id,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    visible_rows: &[(usize, crate::render::VisualRow)],
    marks: &[AgentMarkOverlay],
) -> Option<u64> {
    let clip = ui.clip_rect();
    let mut remove = None;
    let mut badges: Vec<egui::Rect> = Vec::new();
    for mark in marks {
        let mut first_rect = None;
        for &(row_index, mapped) in visible_rows {
            let Some(segment) = crate::render::range_on_visual_row(
                galley,
                row_index,
                mapped,
                mark.range.start,
                mark.range.end,
            ) else {
                continue;
            };
            let source_rect = segment.translate(galley_pos.to_vec2());
            // Keep the remove action attached to a visible source row even
            // when its selected text has scrolled horizontally out of view.
            if source_rect.bottom() > clip.top() && source_rect.top() < clip.bottom() {
                first_rect.get_or_insert(source_rect.intersect(egui::Rect::from_x_y_ranges(
                    source_rect.x_range(),
                    clip.y_range(),
                )));
            }
            let rect = source_rect.intersect(clip);
            if !rect.is_positive() {
                continue;
            }
            ui.painter().rect_filled(
                rect,
                2.0,
                Color32::from_rgba_unmultiplied(
                    AGENT_MARK_COLOR.r(),
                    AGENT_MARK_COLOR.g(),
                    AGENT_MARK_COLOR.b(),
                    28,
                ),
            );
            ui.painter().line_segment(
                [
                    rect.left_bottom() - Vec2::new(0.0, 1.0),
                    rect.right_bottom() - Vec2::new(0.0, 1.0),
                ],
                Stroke::new(1.0, AGENT_MARK_COLOR.gamma_multiply(0.6)),
            );
            ui.interact(
                rect,
                widget.with(("agent_mark_hover", mark.id, row_index)),
                egui::Sense::hover(),
            )
            .on_hover_text(&mark.tooltip);
        }
        let Some(first) = first_rect else {
            continue;
        };
        let mut center = egui::pos2(
            clip.left() + 16.0,
            first
                .center()
                .y
                .max(clip.top() + 9.0)
                .min(clip.bottom() - 9.0),
        );
        let mut badge = egui::Rect::from_center_size(center, Vec2::splat(18.0));
        while badges.iter().any(|rect| rect.intersects(badge)) {
            center.y += 19.0;
            badge = egui::Rect::from_center_size(center, Vec2::splat(18.0));
        }
        if !clip.contains_rect(badge) {
            continue;
        }
        let response = ui.interact(
            badge,
            widget.with(("agent_mark_remove", mark.id)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("Remove marked block {}", mark.id),
            )
        });
        ui.painter().rect_filled(
            badge,
            4.0,
            if response.hovered() {
                Color32::from_rgb(62, 47, 84)
            } else {
                Color32::from_rgb(38, 32, 51)
            },
        );
        paint_cross(ui, badge, false, AGENT_MARK_COLOR);
        if response.clicked() {
            remove = Some(mark.id);
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "Remove this block from For agent\n{}",
                mark.tooltip
            ));
        badges.push(badge);
    }
    let key = widget.with("agent_mark_pointer");
    let mut pointer = ui
        .ctx()
        .data(|data| data.get_temp::<OverlayPointer>(key))
        .unwrap_or_default();
    pointer.rects = badges;
    ui.ctx().data_mut(|data| data.insert_temp(key, pointer));
    remove
}

#[cfg(test)]
#[path = "agent_marks_ui_tests.rs"]
mod tests;
