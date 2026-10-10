//! Brief, non-destructive feedback for a copied range or a revealed source line.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Yank,
    Navigation,
}

pub(super) struct Highlight {
    revision: u64,
    ranges: Vec<Range<usize>>,
    kind: Kind,
    full_line: bool,
    started: Instant,
}

impl Highlight {
    fn duration(&self) -> Duration {
        Duration::from_millis(if self.kind == Kind::Yank { 550 } else { 850 })
    }

    pub(super) fn paint(
        &self,
        ui: &egui::Ui,
        galley: &egui::Galley,
        position: egui::Pos2,
        visible: &[(usize, crate::render::VisualRow)],
        revision: u64,
    ) {
        let progress = self.started.elapsed().as_secs_f32() / self.duration().as_secs_f32();
        if self.revision != revision || progress >= 1.0 {
            return;
        }
        let alpha = (48.0 * (1.0 - progress).powi(2)) as u8;
        let color = if self.kind == Kind::Yank {
            ACCENT
        } else {
            BLUE
        };
        let clip = ui.clip_rect();
        let painter = ui.painter().with_clip_rect(clip);
        for &(row, mapped) in visible {
            for range in &self.ranges {
                if let Some(segment) =
                    crate::render::range_on_visual_row(galley, row, mapped, range.start, range.end)
                {
                    let mut rect = segment.translate(position.to_vec2());
                    if self.full_line || (range.start <= mapped.start && range.end > mapped.end) {
                        rect.min.x = position.x;
                        rect.max.x = clip.right();
                    }
                    painter.rect_filled(
                        rect.intersect(clip),
                        2.0,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
                    );
                }
            }
        }
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

impl EditorTab {
    pub(super) fn flash_yank(&mut self, ranges: Vec<Range<usize>>) {
        if ranges.is_empty() {
            return;
        }
        self.feedback = Some(Highlight {
            revision: self.revision,
            ranges,
            kind: Kind::Yank,
            full_line: false,
            started: Instant::now(),
        });
    }

    pub(super) fn flash_line_yank(&mut self, ranges: Vec<Range<usize>>) {
        self.flash_yank(ranges);
        if let Some(feedback) = &mut self.feedback {
            feedback.full_line = true;
        }
    }

    pub(super) fn flash_navigation(&mut self) {
        let line = self
            .lines
            .line_col(&self.text, self.revision, self.vim.cursor)
            .0;
        let start = self
            .lines
            .position(&self.text, self.revision, line, 1)
            .unwrap_or(0);
        let end = self
            .lines
            .line_end(&self.text, self.revision, line)
            .unwrap_or(start);
        self.feedback = Some(Highlight {
            revision: self.revision,
            ranges: std::iter::once(start..end).collect(),
            kind: Kind::Navigation,
            full_line: true,
            started: Instant::now(),
        });
        self.center_on_reveal = true;
        self.scroll_to_cursor = true;
    }
}

impl VEditApp {
    pub(super) fn reveal_search_result(&mut self, path: PathBuf, line: usize, column: usize) {
        self.open_file(path);
        self.set_document_view(crate::cli::DocumentView::Source);
        if let Some(tab) = self.tab_mut() {
            tab.pending_agent_location = Some(crate::cli::Location { line, column });
            tab.agent_location_error = None;
            tab.pending_navigation_feedback = true;
        }
        self.apply_pending_agent_locations();
    }
}
