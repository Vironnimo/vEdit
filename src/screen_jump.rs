//! A modal screen-jump model and overlay. The caller owns focus and cursor changes.

use eframe::egui::{self, Color32, FontId, Rect, Stroke, StrokeKind, Vec2};
use regex::{Regex, RegexBuilder, RegexSet};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const MAX_QUERY_CHARS: usize = 3;
// Home row first, then nearby letters. No Shift or layout-specific punctuation.
const LABEL_KEYS: &str = "fjdkslaghrueiwoqptyxcvbnmz";
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
    previous_labels: HashMap<usize, String>,
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
        if self.query.chars().count() == MAX_QUERY_CHARS {
            self.phase = JumpPhase::Select;
        }
    }

    /// Feed text input while the caller owns the jump modal. A changed query
    /// clears old targets: recompute visible glyphs before processing a later
    /// label key, including multiple text characters delivered in one frame.
    pub(crate) fn input_char(&mut self, character: char) -> JumpAction {
        if character.is_control() {
            return JumpAction::None;
        }
        if self.phase == JumpPhase::Select {
            return self.select_label(character);
        }
        // Only labels that cannot continue any visible match are offered.
        // Digits and mixed alphanumeric text are ordinary query input.
        if self
            .targets
            .iter()
            .any(|target| target.label.starts_with(character.to_ascii_lowercase()))
        {
            return self.select_label(character);
        }
        if self.query.chars().count() < MAX_QUERY_CHARS {
            let mut query = self.query.clone();
            query.push(character);
            self.set_query(&query);
        }
        JumpAction::None
    }

    pub(crate) fn enter(&mut self) -> JumpAction {
        if self.targets.len() == 1 {
            return JumpAction::Jump(self.targets[0].char_index);
        }
        if !self.query.is_empty() && !self.targets.is_empty() && self.phase == JumpPhase::Query {
            self.phase = JumpPhase::Select;
            self.label_input.clear();
            self.explicit_selection = true;
        }
        JumpAction::None
    }

    pub(crate) fn backspace(&mut self) -> JumpAction {
        if self.label_input.pop().is_some() {
            if self.label_input.is_empty()
                && !self.explicit_selection
                && self.query.chars().count() < MAX_QUERY_CHARS
            {
                self.phase = JumpPhase::Query;
            }
        } else if self.explicit_selection {
            self.clear_selection();
        } else {
            let mut query = self.query.clone();
            query.pop();
            self.set_query(&query);
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
        let mut reserved = HashSet::new();
        let reserve_continuations = !self.explicit_selection && query_len < MAX_QUERY_CHARS;
        let mut candidate = String::with_capacity(MAX_QUERY_CHARS * 4);
        for (index, window) in glyphs.windows(query_len).enumerate() {
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
            if reserve_continuations
                && let Some(next) = glyphs.get(index + query_len)
                && window.last().unwrap().char_index.checked_add(1) == Some(next.char_index)
            {
                // Use the same Unicode simple case folding as the search:
                // e.g. ſ reserves s and K reserves k.
                let mut bytes = [0; 4];
                reserved.extend(label_matchers().matches(next.character.encode_utf8(&mut bytes)));
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
        let first_keys: Vec<_> = LABEL_KEYS
            .chars()
            .enumerate()
            .filter_map(|(index, key)| (!reserved.contains(&index)).then_some(key))
            .collect();
        assign_labels(&mut targets, &self.previous_labels, &first_keys);
        if targets.len() != self.targets.len()
            || targets.iter().zip(&self.targets).any(|(new, old)| {
                new.char_index != old.char_index
                    || new.char_end != old.char_end
                    || new.label != old.label
            })
        {
            self.label_input.clear();
            if !self.explicit_selection && query_len < MAX_QUERY_CHARS {
                self.phase = JumpPhase::Query;
            }
        }
        self.previous_labels = targets
            .iter()
            .filter(|target| !target.label.is_empty())
            .map(|target| (target.char_index, target.label.clone()))
            .collect();
        self.targets = targets;
    }

    pub(crate) fn hint(&self) -> String {
        if self.query.is_empty() {
            return "Type 1–3 characters · Esc cancels".into();
        }
        let count = self.targets.len();
        let matches = if count == 1 { "match" } else { "matches" };
        let action = if count == 0 {
            "No matches · Backspace corrects".to_owned()
        } else if !self.label_input.is_empty() {
            format!("Label {}… · type the next letter", self.label_input)
        } else if self.targets.iter().all(|target| target.label.is_empty()) {
            format!("{count} {matches} · keep typing or Enter for labels")
        } else if count == 1 {
            if self.phase == JumpPhase::Query {
                "1 match · keep typing, label or Enter".into()
            } else {
                "1 match · label or Enter".into()
            }
        } else if self.phase == JumpPhase::Select {
            format!("{count} {matches} · type a label")
        } else {
            format!("{count} {matches} · keep typing or choose a label · Enter locks")
        };
        format!("“{}” · {action} · Esc cancels", self.query)
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
        let mut occupied = OccupiedCells::new();
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
            if target.label.is_empty() || !target.rect.intersects(clip) {
                continue;
            }
            let eligible = target.label.starts_with(&self.label_input);
            let color = if eligible { accent } else { muted };
            let mut job = egui::text::LayoutJob::default();
            let prefix_len = if eligible { self.label_input.len() } else { 0 };
            for (text, color) in [
                (&target.label[..prefix_len], muted),
                (&target.label[prefix_len..], color),
            ] {
                job.append(text, 0.0, egui::TextFormat::simple(font.clone(), color));
            }
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let size = galley.size() + Vec2::new(8.0, 1.0);
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
            painter.galley(pill.min + Vec2::new(4.0, 0.5), galley, color);
        }
    }

    fn clear_selection(&mut self) {
        self.label_input.clear();
        self.phase = JumpPhase::Query;
        self.explicit_selection = false;
    }

    fn select_label(&mut self, character: char) -> JumpAction {
        let mut prefix = self.label_input.clone();
        prefix.push(character.to_ascii_lowercase());
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

fn label_matchers() -> &'static RegexSet {
    static MATCHERS: OnceLock<RegexSet> = OnceLock::new();
    MATCHERS.get_or_init(|| {
        RegexSet::new(LABEL_KEYS.chars().map(|key| format!(r"\A(?i:{key})\z")))
            .expect("literal label letters")
    })
}

/// Equal-length labels are prefix-free, so typing can never choose a shorter
/// target accidentally. Only the first key needs to avoid query continuations.
fn assign_labels(
    targets: &mut [JumpTarget],
    previous: &HashMap<usize, String>,
    first_keys: &[char],
) {
    if targets.is_empty() || first_keys.is_empty() {
        return;
    }
    let keys: Vec<_> = LABEL_KEYS.chars().collect();
    let mut width = 1;
    let mut capacity = first_keys.len();
    while capacity < targets.len() {
        capacity = capacity.saturating_mul(keys.len());
        width += 1;
    }
    let mut used = HashSet::new();
    for target in targets.iter_mut() {
        if let Some(label) = previous.get(&target.char_index)
            && label.len() == width
            && label
                .chars()
                .next()
                .is_some_and(|key| first_keys.contains(&key))
            && used.insert(label.clone())
        {
            target.label.clone_from(label);
        }
    }
    let mut ordinal = 0;
    for target in targets.iter_mut().filter(|target| target.label.is_empty()) {
        loop {
            let mut value = ordinal;
            let mut label = vec![' '; width];
            for key in label[1..].iter_mut().rev() {
                *key = keys[value % keys.len()];
                value /= keys.len();
            }
            label[0] = first_keys[value];
            ordinal += 1;
            let label: String = label.into_iter().collect();
            if used.insert(label.clone()) {
                target.label = label;
                break;
            }
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

type OccupiedCells = HashMap<(i32, i32), Vec<Rect>>;

fn occupy(rect: Rect, occupied: &mut OccupiedCells) {
    let (columns, rows) = cell_range(rect);
    for column in columns {
        for row in rows.clone() {
            occupied.entry((column, row)).or_default().push(rect);
        }
    }
}

fn is_free(rect: Rect, occupied: &OccupiedCells) -> bool {
    let (columns, rows) = cell_range(rect);
    !columns.into_iter().any(|column| {
        rows.clone().any(|row| {
            occupied
                .get(&(column, row))
                .is_some_and(|rects| rects.iter().any(|occupied| occupied.intersects(rect)))
        })
    })
}

fn place_label(anchor: Rect, size: Vec2, clip: Rect, occupied: &OccupiedCells) -> Rect {
    // Prefer the matching text itself: labels between densely packed rows are
    // otherwise hard to associate with the correct occurrence. Fall back to
    // nearby space for short/wrapped matches or when this would cover the caret.
    let inline = Rect::from_min_size(
        egui::pos2(anchor.left(), anchor.center().y - size.y * 0.5),
        size,
    );
    if anchor.contains_rect(inline) && clip.contains_rect(inline) && is_free(inline, occupied) {
        return inline;
    }
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
    fn home_row_labels_jump_without_enter() {
        let mut jump = with_query("gi", "git give giant");
        assert_eq!(
            jump.targets
                .iter()
                .map(|target| target.label.as_str())
                .collect::<Vec<_>>(),
            vec!["f", "j", "d"]
        );
        assert_eq!(jump.input_char('d'), JumpAction::Jump(9));
    }

    #[test]
    fn many_hits_use_short_equal_length_prefix_free_labels() {
        assert_eq!(LABEL_KEYS.len(), 26);
        assert_eq!(LABEL_KEYS.chars().collect::<HashSet<_>>().len(), 26);
        for count in [1, 9, 26, 27, 99, 676, 677] {
            let jump = with_query("a", &"a ".repeat(count));
            let width = if count <= 26 {
                1
            } else if count <= 676 {
                2
            } else {
                3
            };
            let labels: HashSet<_> = jump.targets.iter().map(|target| &target.label).collect();
            assert_eq!(labels.len(), count);
            assert!(labels.iter().all(|label| label.len() == width));
            // Every displayed label selects exactly its own target, including
            // the boundary where the first key alone is no longer sufficient.
            for target in &jump.targets {
                let mut selection = with_query("a", &"a ".repeat(count));
                for (index, key) in target.label.chars().enumerate() {
                    let expected = if index + 1 == width {
                        JumpAction::Jump(target.char_index)
                    } else {
                        JumpAction::None
                    };
                    assert_eq!(selection.input_char(key), expected);
                }
            }
        }
    }

    #[test]
    fn continuations_are_reserved_globally_with_unicode_case_folding() {
        let visible = glyphs("gif giJ giſ giK git give girl", 0);
        let mut jump = with_query("gi", "gif giJ giſ giK git give girl");
        for target in &jump.targets {
            assert!(!"fjsktvr".contains(target.label.chars().next().unwrap()));
        }
        assert_eq!(jump.input_char('J'), JumpAction::None);
        jump.recompute(&visible);
        assert_eq!(jump.query, "giJ");
        assert_eq!(jump.phase, JumpPhase::Select);
        assert_eq!(jump.targets.len(), 1);
    }

    #[test]
    fn three_characters_unlock_all_letters_and_backspace_edits_the_query() {
        let visible = glyphs("gifj gifd", 0);
        let mut jump = with_query("gi", "gifj gifd");
        assert!(jump.targets.iter().all(|target| target.label != "f"));
        assert_eq!(jump.input_char('f'), JumpAction::None);
        jump.recompute(&visible);
        assert_eq!(jump.phase, JumpPhase::Select);
        jump.backspace();
        jump.recompute(&visible);
        assert_eq!(jump.query, "gi");
        assert_eq!(jump.phase, JumpPhase::Query);
        jump.input_char('f');
        jump.recompute(&visible);
        // The j at the first hit can no longer extend the three-character query.
        assert_eq!(jump.input_char('j'), JumpAction::Jump(0));
    }

    #[test]
    fn labels_stay_at_the_same_positions_when_the_query_is_refined() {
        let visible = glyphs("go git give gap", 0);
        let mut jump = with_query("g", "go git give gap");
        let before = jump.previous_labels.clone();
        jump.input_char('i');
        jump.recompute(&visible);
        assert_eq!(jump.targets.len(), 2);
        for target in &jump.targets {
            assert_eq!(Some(&target.label), before.get(&target.char_index));
        }
        // The former f label becomes unsafe when it is a possible continuation.
        let mut changed = with_query("g", "gif git");
        assert_eq!(changed.targets[0].label, "f");
        changed.input_char('i');
        changed.recompute(&glyphs("gif git", 0));
        assert_ne!(changed.targets[0].label, "f");
        assert_eq!(changed.targets[1].label, "j");
    }

    #[test]
    fn enter_releases_reserved_letters_when_every_letter_is_a_continuation() {
        let document: String = LABEL_KEYS.chars().map(|key| format!("§{key} ")).collect();
        let visible = glyphs(&document, 0);
        let mut jump = with_query("§", &document);
        assert!(jump.targets.iter().all(|target| target.label.is_empty()));
        assert!(jump.hint().contains("Enter for labels"));
        assert_eq!(jump.enter(), JumpAction::None);
        jump.recompute(&visible);
        assert!(jump.targets.iter().all(|target| !target.label.is_empty()));
        assert_eq!(jump.input_char('F'), JumpAction::Jump(0));
    }

    #[test]
    fn only_the_first_key_of_a_pair_needs_to_be_safe() {
        let document: String = LABEL_KEYS
            .chars()
            .filter(|key| *key != 'f')
            .map(|key| format!("a{key} "))
            .collect();
        let mut jump = with_query("a", &document);
        assert!(
            jump.targets
                .iter()
                .all(|target| target.label.starts_with('f'))
        );
        assert_eq!(jump.targets[1].label, "fj");
        assert_eq!(jump.input_char('f'), JumpAction::None);
        jump.recompute(&glyphs(&document, 0));
        assert_eq!(jump.phase, JumpPhase::Select);
        assert_eq!(jump.input_char('j'), JumpAction::Jump(3));
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
        assert_eq!(jump.input_char('j'), JumpAction::Jump(4));
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
        assert_eq!(jump.query, "gi1");
        let mut jump = with_query("git", "git git");
        assert_eq!(jump.input_char('0'), JumpAction::None);
        assert!(jump.label_input.is_empty());
        assert_eq!(jump.input_char('9'), JumpAction::None);
        assert_eq!(jump.input_char('j'), JumpAction::Jump(4));
    }

    #[test]
    fn numbers_and_mixed_alphanumeric_queries_need_no_special_confirmation() {
        let visible = glyphs("12 12 13", 0);
        let mut jump = ScreenJump::new();
        jump.input_char('1');
        jump.recompute(&visible);
        jump.input_char('2');
        jump.recompute(&visible);
        assert_eq!(jump.query, "12");
        assert_eq!(jump.targets.len(), 2);
        assert_eq!(jump.input_char('j'), JumpAction::Jump(3));
        let mut mixed = with_query("a", "a1 a2 a1");
        mixed.input_char('1');
        mixed.recompute(&glyphs("a1 a2 a1", 0));
        assert_eq!(mixed.query, "a1");
        assert_eq!(mixed.input_char('d'), JumpAction::Jump(6));
    }

    #[test]
    fn enter_jumps_directly_to_the_only_visible_match() {
        for (query, document, position) in [("gi", "one git two", 4), ("12", "a 12 b", 2)] {
            let mut jump = with_query(query, document);
            assert_eq!(jump.enter(), JumpAction::Jump(position));
        }
        let mut missing = with_query("gi", "no match");
        assert_eq!(missing.enter(), JumpAction::None);
        assert_eq!(missing.phase, JumpPhase::Query);
        assert_eq!(missing.query, "gi");
    }

    #[test]
    fn backspace_corrects_a_label_prefix_then_refines_a_unicode_query() {
        let mut jump = with_query("ö", &"Ö ".repeat(30));
        jump.input_char('f');
        assert_eq!(jump.label_input, "f");
        jump.backspace();
        assert_eq!(jump.phase, JumpPhase::Query);
        assert_eq!(jump.query, "ö");
        jump.backspace();
        assert!(jump.query.is_empty());
        assert!(jump.targets.is_empty());
        let mut numeric = with_query("12", &"12 ".repeat(30));
        numeric.enter();
        numeric.recompute(&glyphs(&"12 ".repeat(30), 0));
        numeric.input_char('f');
        numeric.backspace();
        assert_eq!(numeric.phase, JumpPhase::Select);
        numeric.backspace();
        assert_eq!(numeric.phase, JumpPhase::Query);
        assert_eq!(numeric.query, "12");
    }

    #[test]
    fn changed_visible_targets_clear_stale_selection_prefixes() {
        let mut jump = with_query("a", &"a ".repeat(30));
        jump.input_char('f');
        jump.recompute(&glyphs("a a a", 500));
        assert!(jump.label_input.is_empty());
        assert_eq!(jump.phase, JumpPhase::Query);
        assert_eq!(jump.targets[0].label, "f");
        assert_eq!(jump.input_char('j'), JumpAction::Jump(502));
    }

    #[test]
    fn repeated_geometry_updates_preserve_a_valid_prefix_and_labels() {
        let mut jump = with_query("a", &"a ".repeat(30));
        jump.input_char('f');
        let mut visible = glyphs(&"a ".repeat(30), 0);
        for glyph in &mut visible {
            glyph.rect = glyph.rect.translate(Vec2::new(0.0, 12.0));
        }
        jump.recompute(&visible);
        assert_eq!(jump.label_input, "f");
        assert_eq!(jump.input_char('j'), JumpAction::Jump(2));
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
        let mut occupied = OccupiedCells::new();
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
    fn dense_rows_keep_labels_on_their_matches_without_false_cell_collisions() {
        let clip = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 400.0));
        let mut occupied = OccupiedCells::new();
        for row in 0..16 {
            let anchor = Rect::from_min_size(
                egui::pos2(100.0, 30.0 + row as f32 * 19.0),
                Vec2::new(25.0, 19.0),
            );
            let label = place_label(anchor, Vec2::new(21.0, 15.0), clip, &occupied);
            assert!(anchor.contains_rect(label));
            assert!(is_free(label, &occupied));
            occupy(label.expand(1.0), &mut occupied);
        }
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
                && ["f", "j"].contains(&text.galley.job.text.as_str())
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
