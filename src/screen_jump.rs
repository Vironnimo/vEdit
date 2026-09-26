//! A modal screen-jump model and overlay. The caller owns focus and cursor changes.

use eframe::egui::{self, Color32, FontId, Rect, Stroke, StrokeKind, Vec2};
use regex::{Regex, RegexBuilder};
use std::collections::HashSet;

const MAX_QUERY_CHARS: usize = 3;
const LABEL_CELL: f32 = 8.0;

/// One visible source character. Omit off-screen characters rather than clamping
/// their rectangles: gaps in character indices prevent partially hidden matches.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VisibleGlyph {
    pub(crate) char_index: usize,
    pub(crate) character: char,
    pub(crate) rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JumpTarget {
    pub(crate) char_index: usize,
    pub(crate) char_end: usize,
    /// The first visual part of the match, used to anchor its jump label.
    pub(crate) rect: Rect,
    pub(crate) highlight_rects: Vec<Rect>,
    pub(crate) label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum JumpPhase {
    #[default]
    Query,
    Select,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum JumpAction {
    #[default]
    None,
    Jump(usize),
    Cancel,
}

#[derive(Default)]
pub(crate) struct ScreenJump {
    pub(crate) query: String,
    pub(crate) label_input: String,
    pub(crate) targets: Vec<JumpTarget>,
    pub(crate) phase: JumpPhase,
    matcher_query: String,
    matcher: Option<Regex>,
    explicit_selection: bool,
}

impl ScreenJump {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_query(&mut self, query: &str) {
        self.query = query
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_QUERY_CHARS)
            .collect();
        self.clear_selection();
        self.targets.clear();
    }

    /// Feed text input while the caller owns the jump modal. A changed query
    /// clears old targets: recompute visible glyphs before processing a later
    /// label key, including multiple text characters delivered in one frame.
    pub(crate) fn input_char(&mut self, character: char) -> JumpAction {
        if character.is_control() {
            return JumpAction::None;
        }
        if self.phase == JumpPhase::Select {
            return if character.is_ascii_digit() {
                self.select_digit(character)
            } else {
                JumpAction::None
            };
        }
        // Digits select a letter/punctuation query immediately. A purely numeric
        // query keeps accepting digits until Enter explicitly locks the query.
        if character.is_ascii_digit()
            && !self.query.is_empty()
            && !self
                .query
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return self.select_digit(character);
        }
        if self.query.chars().count() < MAX_QUERY_CHARS {
            let mut query = self.query.clone();
            query.push(character);
            self.set_query(&query);
        }
        JumpAction::None
    }

    pub(crate) fn enter(&mut self) -> JumpAction {
        if !self.query.is_empty() {
            self.phase = JumpPhase::Select;
            self.label_input.clear();
            self.explicit_selection = true;
        }
        JumpAction::None
    }

    pub(crate) fn backspace(&mut self) -> JumpAction {
        if self.label_input.pop().is_some() {
            if self.label_input.is_empty() && !self.explicit_selection {
                self.phase = JumpPhase::Query;
            }
        } else if self.phase == JumpPhase::Select {
            self.clear_selection();
        } else {
            self.query.pop();
            self.targets.clear();
        }
        JumpAction::None
    }

    pub(crate) fn cancel(&mut self) -> JumpAction {
        *self = Self::default();
        JumpAction::Cancel
    }

    /// Search only the supplied visible characters, including overlapping hits.
    /// Matching is literal and Unicode case-insensitive; nothing in the source
    /// buffer is copied, searched off-screen, or modified by this model.
    pub(crate) fn recompute(&mut self, visible: &[VisibleGlyph]) {
        if self.matcher_query != self.query {
            self.matcher = if self.query.is_empty() {
                None
            } else {
                RegexBuilder::new(&format!(r"\A(?:{})\z", regex::escape(&self.query)))
                    .case_insensitive(true)
                    .build()
                    .ok()
            };
            self.matcher_query.clone_from(&self.query);
        }
        let Some(matcher) = &self.matcher else {
            self.targets.clear();
            self.label_input.clear();
            return;
        };
        let query_len = self.query.chars().count();
        if query_len == 0 {
            self.targets.clear();
            return;
        }
        let mut glyphs: Vec<_> = visible
            .iter()
            .filter(|glyph| {
                glyph.rect.is_finite()
                    && glyph.rect.width() >= 0.0
                    && glyph.rect.height() > 0.0
                    && !glyph.character.is_control()
            })
            .copied()
            .collect();
        glyphs.sort_unstable_by_key(|glyph| glyph.char_index);
        glyphs.dedup_by_key(|glyph| glyph.char_index);
        let mut targets = Vec::new();
        let mut candidate = String::with_capacity(MAX_QUERY_CHARS * 4);
        for window in glyphs.windows(query_len) {
            if window
                .windows(2)
                .any(|pair| pair[0].char_index.checked_add(1) != Some(pair[1].char_index))
            {
                continue;
            }
            candidate.clear();
            candidate.extend(window.iter().map(|glyph| glyph.character));
            if !matcher.is_match(&candidate) {
                continue;
            }
            let highlight_rects = match_rects(window);
            targets.push(JumpTarget {
                char_index: window[0].char_index,
                char_end: window[window.len() - 1].char_index.saturating_add(1),
                rect: highlight_rects[0],
                highlight_rects,
                label: String::new(),
            });
        }
        let digits = targets.len().max(1).to_string().len();
        for (index, target) in targets.iter_mut().enumerate() {
            target.label = format!("{:0digits$}", index + 1);
        }
        if targets.len() != self.targets.len()
            || targets.iter().zip(&self.targets).any(|(new, old)| {
                new.char_index != old.char_index
                    || new.char_end != old.char_end
                    || new.label != old.label
            })
        {
            self.label_input.clear();
        }
        self.targets = targets;
    }

    /// Paint within the active source pane. Pass the caret rectangle to the
    /// variant below when available so labels also avoid its current position.
    #[allow(dead_code)] // A caller without a visible caret can use the simpler API.
    pub(crate) fn paint(&self, ui: &egui::Ui, clip: Rect) {
        self.paint_with_caret(ui, clip, None);
    }

    pub(crate) fn paint_with_caret(&self, ui: &egui::Ui, clip: Rect, caret: Option<Rect>) {
        let clip = clip.intersect(ui.clip_rect());
        if !clip.is_finite() || !clip.is_positive() {
            return;
        }
        let painter = ui.painter().with_clip_rect(clip);
        let accent = ui.visuals().selection.stroke.color;
        let muted = ui.visuals().weak_text_color();
        let highlight = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42);
        let label_fill = ui.visuals().extreme_bg_color.lerp_to_gamma(accent, 0.18);
        let font = FontId::monospace(11.0);
        let mut occupied = HashSet::new();
        if let Some(caret) = caret.filter(|caret| caret.is_finite() && caret.intersects(clip)) {
            occupy(caret.intersect(clip).expand(2.0), &mut occupied);
        }
        for target in &self.targets {
            let eligible = target.label.starts_with(&self.label_input);
            let fill = if eligible {
                highlight
            } else {
                highlight.gamma_multiply(0.35)
            };
            for rect in &target.highlight_rects {
                painter.rect_filled(*rect, 2.0, fill);
                if eligible {
                    painter.rect_stroke(
                        *rect,
                        2.0,
                        Stroke::new(0.75, accent.gamma_multiply(0.65)),
                        StrokeKind::Inside,
                    );
                }
            }
        }
        for target in &self.targets {
            if !target.rect.intersects(clip) {
                continue;
            }
            let eligible = target.label.starts_with(&self.label_input);
            let color = if eligible { accent } else { muted };
            let galley = ui
                .fonts_mut(|fonts| fonts.layout_no_wrap(target.label.clone(), font.clone(), color));
            let size = galley.size() + Vec2::new(8.0, 3.0);
            if size.x > clip.width() || size.y > clip.height() {
                continue;
            }
            let pill = place_label(target.rect, size, clip, &occupied);
            occupy(pill.expand(1.0), &mut occupied);
            painter.rect_filled(pill, 3.0, label_fill);
            painter.rect_stroke(
                pill,
                3.0,
                Stroke::new(
                    1.0,
                    if eligible {
                        accent
                    } else {
                        muted.gamma_multiply(0.5)
                    },
                ),
                StrokeKind::Inside,
            );
            painter.galley(pill.min + Vec2::new(4.0, 1.5), galley, color);
        }
    }

    fn clear_selection(&mut self) {
        self.label_input.clear();
        self.phase = JumpPhase::Query;
        self.explicit_selection = false;
    }

    fn select_digit(&mut self, character: char) -> JumpAction {
        let mut prefix = self.label_input.clone();
        prefix.push(character);
        if !self
            .targets
            .iter()
            .any(|target| target.label.starts_with(&prefix))
        {
            return JumpAction::None;
        }
        self.phase = JumpPhase::Select;
        self.label_input = prefix;
        match self
            .targets
            .iter()
            .find(|target| target.label == self.label_input)
        {
            Some(target) => JumpAction::Jump(target.char_index),
            None => JumpAction::None,
        }
    }
}

fn match_rects(glyphs: &[VisibleGlyph]) -> Vec<Rect> {
    let mut rects: Vec<Rect> = Vec::new();
    for glyph in glyphs {
        let mut rect = glyph.rect;
        if rect.width() < 1.0 {
            rect.max.x = rect.min.x + 1.0;
        }
        if let Some(previous) = rects.last_mut()
            && (previous.top() - rect.top()).abs() < 1.0
            && (previous.bottom() - rect.bottom()).abs() < 1.0
            && rect.left() <= previous.right() + 1.0
            && rect.right() >= previous.left() - 1.0
        {
            *previous = previous.union(rect);
        } else {
            rects.push(rect);
        }
    }
    rects
}

fn cell_range(rect: Rect) -> (std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>) {
    (
        (rect.left() / LABEL_CELL).floor() as i32..=(rect.right() / LABEL_CELL).floor() as i32,
        (rect.top() / LABEL_CELL).floor() as i32..=(rect.bottom() / LABEL_CELL).floor() as i32,
    )
}

fn occupy(rect: Rect, occupied: &mut HashSet<(i32, i32)>) {
    let (columns, rows) = cell_range(rect);
    for column in columns {
        for row in rows.clone() {
            occupied.insert((column, row));
        }
    }
}

fn is_free(rect: Rect, occupied: &HashSet<(i32, i32)>) -> bool {
    let (columns, rows) = cell_range(rect);
    !columns
        .into_iter()
        .any(|column| rows.clone().any(|row| occupied.contains(&(column, row))))
}

fn place_label(anchor: Rect, size: Vec2, clip: Rect, occupied: &HashSet<(i32, i32)>) -> Rect {
    let candidates = [
        egui::pos2(anchor.left(), anchor.top() - size.y - 2.0),
        egui::pos2(anchor.left(), anchor.bottom() + 2.0),
        egui::pos2(anchor.right() + 3.0, anchor.center().y - size.y * 0.5),
        egui::pos2(
            anchor.left() - size.x - 3.0,
            anchor.center().y - size.y * 0.5,
        ),
        egui::pos2(anchor.right() - size.x, anchor.top() - size.y - 2.0),
        egui::pos2(anchor.right() - size.x, anchor.bottom() + 2.0),
        egui::pos2(anchor.left() + size.x + 4.0, anchor.top() - size.y - 2.0),
        egui::pos2(anchor.left() - size.x - 4.0, anchor.top() - size.y - 2.0),
        egui::pos2(
            anchor.left() + size.x * 2.0 + 8.0,
            anchor.top() - size.y - 2.0,
        ),
        egui::pos2(anchor.left() + size.x + 4.0, anchor.bottom() + 2.0),
        egui::pos2(anchor.left() - size.x * 2.0 - 8.0, anchor.bottom() + 2.0),
        egui::pos2(anchor.left(), anchor.bottom() + size.y + 4.0),
    ];
    let clamp = |position: egui::Pos2| {
        Rect::from_min_size(
            egui::pos2(
                position.x.clamp(clip.left(), clip.right() - size.x),
                position.y.clamp(clip.top(), clip.bottom() - size.y),
            ),
            size,
        )
    };
    candidates
        .into_iter()
        .map(clamp)
        .find(|candidate| is_free(*candidate, occupied))
        .unwrap_or_else(|| clamp(candidates[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(text: &str, start: usize) -> Vec<VisibleGlyph> {
        text.chars()
            .enumerate()
            .map(|(index, character)| VisibleGlyph {
                char_index: start + index,
                character,
                rect: Rect::from_min_size(
                    egui::pos2(20.0 + index as f32 * 8.0, 40.0),
                    Vec2::new(8.0, 16.0),
                ),
            })
            .collect()
    }

    fn with_query(query: &str, text: &str) -> ScreenJump {
        let mut jump = ScreenJump::new();
        jump.set_query(query);
        jump.recompute(&glyphs(text, 0));
        jump
    }

    #[test]
    fn one_to_three_query_characters_refine_hits_without_changing_source() {
        let visible = glyphs("git give giant go", 20);
        let mut jump = ScreenJump::new();
        assert_eq!(jump.input_char('g'), JumpAction::None);
        jump.recompute(&visible);
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.char_index)
                .collect::<Vec<_>>(),
            vec![20, 24, 29, 35]
        );
        jump.input_char('i');
        jump.recompute(&visible);
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.char_index)
                .collect::<Vec<_>>(),
            vec![20, 24, 29]
        );
        jump.input_char('t');
        jump.recompute(&visible);
        assert_eq!(jump.targets.len(), 1);
        assert_eq!(
            (jump.targets[0].char_index, jump.targets[0].char_end),
            (20, 23)
        );
        jump.input_char('x');
        assert_eq!(jump.query, "git");
        assert_eq!(visible[0].character, 'g');
    }

    #[test]
    fn a_single_digit_jumps_to_the_requested_occurrence() {
        let mut jump = with_query("gi", "git give giant");
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.label.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2", "3"]
        );
        assert_eq!(jump.input_char('3'), JumpAction::Jump(9));
    }

    #[test]
    fn ten_to_ninety_nine_hits_use_fixed_two_digit_prefix_free_labels() {
        let mut jump = with_query("a", &"a ".repeat(12));
        assert_eq!(jump.targets[0].label, "01");
        assert_eq!(jump.targets[11].label, "12");
        assert_eq!(jump.input_char('1'), JumpAction::None);
        assert_eq!(jump.label_input, "1");
        assert_eq!(jump.input_char('2'), JumpAction::Jump(22));
        let mut ninety_nine = with_query("a", &"a ".repeat(99));
        assert_eq!(ninety_nine.targets[98].label, "99");
        assert_eq!(ninety_nine.input_char('0'), JumpAction::None);
        assert_eq!(ninety_nine.input_char('1'), JumpAction::Jump(0));
    }

    #[test]
    fn three_digit_labels_never_jump_on_a_short_prefix() {
        let mut jump = with_query("a", &"a ".repeat(100));
        assert_eq!(jump.targets[0].label, "001");
        assert_eq!(jump.targets[99].label, "100");
        assert_eq!(jump.input_char('0'), JumpAction::None);
        assert_eq!(jump.input_char('0'), JumpAction::None);
        assert_eq!(jump.input_char('1'), JumpAction::Jump(0));
    }

    #[test]
    fn unicode_simple_case_folding_and_character_offsets_are_correct() {
        let mut jump = with_query("ö", "éÖ xö");
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.char_index)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
        assert_eq!(jump.input_char('2'), JumpAction::Jump(4));
        let greek = with_query("σ", "Σσς");
        assert_eq!(greek.targets.len(), 3);
    }

    #[test]
    fn overlapping_occurrences_all_receive_labels() {
        let jump = with_query("aa", "aaaa");
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.char_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(
            jump.targets
                .iter()
                .all(|target| target.char_end - target.char_index == 2)
        );
    }

    #[test]
    fn missing_visible_characters_exclude_partial_and_offscreen_matches() {
        let mut visible = glyphs("gi gi gi", 50);
        visible.retain(|glyph| glyph.char_index != 51 && glyph.char_index != 56);
        let mut jump = ScreenJump::new();
        jump.set_query("gi");
        jump.recompute(&visible);
        assert_eq!(jump.targets.len(), 1);
        assert_eq!(jump.targets[0].char_index, 53);
    }

    #[test]
    fn wrapped_match_highlights_each_visual_row_without_filling_the_gap() {
        let mut visible = glyphs("git", 100);
        visible[0].rect = Rect::from_min_size(egui::pos2(180.0, 40.0), Vec2::new(8.0, 16.0));
        visible[1].rect = Rect::from_min_size(egui::pos2(188.0, 40.0), Vec2::new(8.0, 16.0));
        visible[2].rect = Rect::from_min_size(egui::pos2(20.0, 56.0), Vec2::new(8.0, 16.0));
        let mut jump = ScreenJump::new();
        jump.set_query("git");
        jump.recompute(&visible);
        assert_eq!(jump.targets[0].highlight_rects.len(), 2);
        assert_eq!(jump.targets[0].rect, visible[0].rect.union(visible[1].rect));
        assert_eq!(jump.targets[0].highlight_rects[1], visible[2].rect);
    }

    #[test]
    fn no_hits_and_invalid_label_keys_never_return_a_jump() {
        let mut jump = with_query("gi", "no matches");
        assert!(jump.targets.is_empty());
        assert_eq!(jump.input_char('1'), JumpAction::None);
        assert_eq!(jump.phase, JumpPhase::Query);
        let mut jump = with_query("gi", "git give");
        assert_eq!(jump.input_char('0'), JumpAction::None);
        assert!(jump.label_input.is_empty());
        assert_eq!(jump.input_char('9'), JumpAction::None);
        assert_eq!(jump.input_char('2'), JumpAction::Jump(4));
    }

    #[test]
    fn numeric_query_is_locked_with_enter_before_digits_select_labels() {
        let visible = glyphs("12 12 13", 0);
        let mut jump = ScreenJump::new();
        jump.input_char('1');
        jump.recompute(&visible);
        jump.input_char('2');
        jump.recompute(&visible);
        assert_eq!(jump.query, "12");
        assert_eq!(jump.targets.len(), 2);
        assert_eq!(jump.enter(), JumpAction::None);
        assert_eq!(jump.phase, JumpPhase::Select);
        assert_eq!(jump.input_char('2'), JumpAction::Jump(3));
    }

    #[test]
    fn backspace_corrects_a_label_prefix_then_refines_a_unicode_query() {
        let mut jump = with_query("ö", &"Ö ".repeat(12));
        jump.input_char('1');
        assert_eq!(jump.label_input, "1");
        jump.backspace();
        assert_eq!(jump.phase, JumpPhase::Query);
        assert_eq!(jump.query, "ö");
        jump.backspace();
        assert!(jump.query.is_empty());
        assert!(jump.targets.is_empty());
        let mut numeric = with_query("12", &"12 ".repeat(12));
        numeric.enter();
        numeric.input_char('0');
        numeric.backspace();
        assert_eq!(numeric.phase, JumpPhase::Select);
        numeric.backspace();
        assert_eq!(numeric.phase, JumpPhase::Query);
        assert_eq!(numeric.query, "12");
    }

    #[test]
    fn changed_visible_targets_clear_stale_selection_prefixes() {
        let mut jump = with_query("a", &"a ".repeat(12));
        jump.input_char('1');
        jump.recompute(&glyphs("a a a", 500));
        assert!(jump.label_input.is_empty());
        assert_eq!(jump.targets[0].label, "1");
        assert_eq!(jump.input_char('2'), JumpAction::Jump(502));
    }

    #[test]
    fn repeated_geometry_updates_preserve_a_valid_prefix_and_labels() {
        let mut jump = with_query("a", &"a ".repeat(12));
        jump.input_char('1');
        let mut visible = glyphs(&"a ".repeat(12), 0);
        for glyph in &mut visible {
            glyph.rect = glyph.rect.translate(Vec2::new(0.0, 12.0));
        }
        jump.recompute(&visible);
        assert_eq!(jump.label_input, "1");
        assert_eq!(jump.input_char('2'), JumpAction::Jump(22));
    }

    #[test]
    fn escaped_punctuation_matches_literally_and_unsorted_glyphs_are_stable() {
        let mut visible = glyphs("a.b ++ a.b", 70);
        visible.reverse();
        let mut jump = ScreenJump::new();
        jump.set_query(".");
        jump.recompute(&visible);
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.char_index)
                .collect::<Vec<_>>(),
            vec![71, 78]
        );
        jump.set_query("++");
        jump.recompute(&visible);
        assert_eq!(jump.targets[0].char_index, 74);
    }

    #[test]
    fn cancellation_resets_only_the_model_and_returns_no_cursor_change() {
        let visible = glyphs("git", 50);
        let mut jump = ScreenJump::new();
        jump.set_query("gi");
        jump.recompute(&visible);
        assert_eq!(jump.cancel(), JumpAction::Cancel);
        assert!(jump.query.is_empty());
        assert!(jump.targets.is_empty());
        assert_eq!(jump.phase, JumpPhase::Query);
        assert_eq!(visible[0].char_index, 50);
    }

    #[test]
    fn label_placement_stays_in_the_pane_and_avoids_the_caret_when_space_exists() {
        let clip = Rect::from_min_size(egui::pos2(10.0, 10.0), Vec2::new(200.0, 100.0));
        let anchor = Rect::from_min_size(egui::pos2(12.0, 12.0), Vec2::new(16.0, 16.0));
        let size = Vec2::new(24.0, 15.0);
        let caret = Rect::from_min_size(egui::pos2(12.0, 30.0), Vec2::new(2.0, 16.0));
        let mut occupied = HashSet::new();
        occupy(caret.expand(2.0), &mut occupied);
        let pill = place_label(anchor, size, clip, &occupied);
        assert!(clip.contains_rect(pill));
        assert!(!pill.intersects(caret));
        occupy(pill.expand(1.0), &mut occupied);
        let other = place_label(anchor.translate(Vec2::new(8.0, 0.0)), size, clip, &occupied);
        assert!(clip.contains_rect(other));
        assert!(!other.intersects(pill));
    }

    #[test]
    fn painted_labels_are_clipped_to_the_source_pane_and_leave_the_caret_visible() {
        let context = egui::Context::default();
        let clip = Rect::from_min_size(egui::pos2(50.0, 50.0), Vec2::new(220.0, 130.0));
        let caret = Rect::from_min_size(egui::pos2(54.0, 52.0), Vec2::new(2.0, 16.0));
        let mut visible = glyphs("gi gi", 0);
        for (index, glyph) in visible.iter_mut().enumerate() {
            glyph.rect = Rect::from_min_size(
                egui::pos2(52.0 + index as f32 * 36.0, 52.0),
                Vec2::new(8.0, 16.0),
            );
        }
        let mut jump = ScreenJump::new();
        jump.set_query("gi");
        jump.recompute(&visible);
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Default::default(),
                    Vec2::new(500.0, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    jump.paint_with_caret(ui, clip, Some(caret));
                });
            },
        );
        let mut labels = 0;
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape
                && ["1", "2"].contains(&text.galley.job.text.as_str())
            {
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                assert_eq!(shape.clip_rect, clip);
                assert!(clip.contains_rect(bounds));
                assert!(!bounds.intersects(caret));
                labels += 1;
            }
        }
        assert_eq!(labels, 2);
        output.drop_without_applying_deltas();
    }
}
