// Fixtures contain match ranges, not integer sequences.
#![allow(clippy::single_range_in_vec_init)]

use super::*;

const CURRENT_FILL: Color32 = Color32::from_rgba_unmultiplied_const(237, 191, 126, 85);
const OTHER_FILL: Color32 = Color32::from_rgba_unmultiplied_const(147, 174, 255, 38);

struct Painted {
    galley: Arc<egui::Galley>,
    position: egui::Pos2,
    clip: egui::Rect,
    backgrounds: Vec<egui::epaint::RectShape>,
    current_strokes: Vec<egui::epaint::RectShape>,
}

fn paint(text: &str, matches: &[Range<usize>], cursor: usize, scroll_column: usize) -> Painted {
    let ctx = egui::Context::default();
    let clip = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(80.0, 160.0));
    let mut drawn = None;
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let galley =
            ui.painter()
                .layout_no_wrap(text.into(), egui::FontId::monospace(14.0), Color32::WHITE);
        let first_row = &galley.rows[0];
        let scroll = first_row
            .glyphs
            .get(scroll_column)
            .map_or(first_row.size.x, |glyph| glyph.pos.x);
        let position = egui::pos2(clip.left() - scroll, clip.top() + 10.0);
        let visible: Vec<_> = crate::render::visual_rows(&galley).enumerate().collect();
        ui.set_clip_rect(clip);
        paint_matches(ui, &galley, position, &visible, matches, cursor);
        drawn = Some((galley, position));
    });
    let backgrounds = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.fill == CURRENT_FILL || rect.fill == OTHER_FILL => {
                Some(rect.clone())
            }
            _ => None,
        })
        .collect();
    let current_strokes = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.stroke.color == ORANGE.linear_multiply(0.8) => {
                Some(rect.clone())
            }
            _ => None,
        })
        .collect();
    output.drop_without_applying_deltas();
    let (galley, position) = drawn.unwrap();
    Painted {
        galley,
        position,
        clip,
        backgrounds,
        current_strokes,
    }
}

#[test]
fn horizontally_scrolled_dense_row_paints_only_visible_hits_and_current_style() {
    let text = "a".repeat(100_000);
    let matches: Vec<_> = (0..100_000).map(|index| index..index + 1).collect();
    let current = 25_005;
    let painted = paint(&text, &matches, current, 25_000);
    let mapped = crate::render::visual_rows(&painted.galley).next().unwrap();
    let columns = visible_match_columns(&painted.galley, 0, mapped, painted.position, painted.clip);
    assert!((24_998..=25_000).contains(&columns.start));
    assert!(columns.end - columns.start < 20);
    assert!((9..20).contains(&painted.backgrounds.len()));
    assert_eq!(matches.len(), 100_000);
    assert_eq!(
        painted
            .backgrounds
            .iter()
            .filter(|rect| rect.fill == CURRENT_FILL)
            .count(),
        1
    );
    assert_eq!(painted.current_strokes.len(), 1);
    assert!(
        painted
            .backgrounds
            .iter()
            .all(|rect| rect.rect.intersects(painted.clip))
    );
    let expected =
        crate::render::range_on_visual_row(&painted.galley, 0, mapped, current, current + 1)
            .unwrap()
            .translate(painted.position.to_vec2());
    assert_eq!(painted.current_strokes[0].rect, expected);
}

#[test]
fn horizontal_clip_keeps_spanning_and_partial_matches() {
    let text = "a".repeat(200);
    // The first hit begins far left of the viewport and crosses its left edge.
    // The second crosses the right edge, while the last is fully offscreen.
    let matches = [0..104, 108..150, 170..180];
    let painted = paint(&text, &matches, 108, 100);
    assert_eq!(painted.backgrounds.len(), 2);
    assert!(painted.backgrounds[0].rect.left() < painted.clip.left());
    assert!(painted.backgrounds[0].rect.right() > painted.clip.left());
    assert!(painted.backgrounds[1].rect.left() < painted.clip.right());
    assert!(painted.backgrounds[1].rect.right() > painted.clip.right());
    assert_eq!(painted.current_strokes.len(), 1);
}

#[test]
fn horizontal_scroll_retains_whitespace_and_newline_anchor_marks() {
    let text = format!("{}   \n", "a".repeat(100));
    let whitespace = paint(&text, &[100..103], 100, 100);
    assert_eq!(whitespace.backgrounds.len(), 1);
    assert_eq!(whitespace.current_strokes.len(), 1);
    let newline = paint(&text, &[103..103], 103, 103);
    assert_eq!(newline.backgrounds.len(), 1);
    assert_eq!(newline.backgrounds[0].rect.width(), 6.0);
    assert_eq!(newline.current_strokes.len(), 1);
}

#[test]
fn blank_rows_and_terminal_zero_width_anchors_are_visible() {
    let painted = paint("\n\n", &[0..0, 1..1, 2..2], 1, 0);
    assert_eq!(painted.backgrounds.len(), 3);
    assert!(
        painted
            .backgrounds
            .iter()
            .all(|rect| rect.rect.width() == 6.0)
    );
    assert_eq!(painted.current_strokes.len(), 1);
}
