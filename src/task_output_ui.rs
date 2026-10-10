//! Retain the output label's shaped text across unchanged frames. The label
//! itself still owns selection, copy and its full horizontal/vertical extent.

use eframe::egui::{self, Color32, FontId, FontSelection, RichText, WidgetText, text::LayoutJob};
use std::sync::Arc;

const MAX_CACHED_GALLEY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
pub(super) struct TaskOutputCache {
    entry: Option<Entry>,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    misses: usize,
    #[cfg(test)]
    last_galley_bytes: usize,
}

struct Entry {
    ctx: egui::Context,
    pixels_per_point: f32,
    atlas_probe: Arc<egui::Galley>,
    template: LayoutJob,
    galley: Arc<egui::Galley>,
}

impl TaskOutputCache {
    pub(super) fn clear(&mut self) {
        self.entry = None;
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
        let galley = self.galley(ui, text, color);
        let intrinsic_size = galley.intrinsic_size();
        let mut response = ui.add(egui::Label::new(galley).selectable(true).extend());
        response.set_intrinsic_size(intrinsic_size);
        response
    }

    fn galley(&mut self, ui: &egui::Ui, text: &str, color: Color32) -> Arc<egui::Galley> {
        let atlas_probe = ui.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "vedit output atlas generation".into(),
                FontId::monospace(1.0),
                Color32::WHITE,
            )
        });
        // Resolve the exact label formatting with empty text. Comparing this
        // tiny job catches UI style/alignment changes without hashing or copying
        // up to one MiB of output on every spinner/caret/background repaint.
        let mut template = Arc::unwrap_or_clone(
            WidgetText::from(RichText::new("").monospace().size(12.0).color(color))
                .into_layout_job(ui.style(), FontSelection::Default, ui.text_valign()),
        );
        template.wrap.max_width = f32::INFINITY;
        // Output is a plain scroll area, never an egui Grid.
        template.halign = ui.layout().horizontal_placement();
        template.justify = ui.layout().horizontal_justify();
        let pixels_per_point = ui.ctx().pixels_per_point();
        if let Some(entry) = &self.entry
            && entry.ctx == *ui.ctx()
            && entry.pixels_per_point == pixels_per_point
            && Arc::ptr_eq(&entry.atlas_probe, &atlas_probe)
            && entry.template == template
            && entry.galley.job.text == text
        {
            #[cfg(test)]
            {
                self.hits += 1;
            }
            return Arc::clone(&entry.galley);
        }
        #[cfg(test)]
        {
            self.misses += 1;
        }
        let mut job = template.clone();
        job.text = text.to_owned();
        for section in &mut job.sections {
            section.byte_range = egui::text::ByteIndex(0)..egui::text::ByteIndex(text.len());
        }
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        self.retain(Entry {
            ctx: ui.ctx().clone(),
            pixels_per_point,
            atlas_probe,
            template,
            galley: Arc::clone(&galley),
        });
        galley
    }

    fn retain(&mut self, entry: Entry) {
        // Text bytes alone severely undercount a shaped log: every glyph also
        // owns row metadata and mesh vertices. Oversized output still renders
        // normally, but must not remain pinned by our persistent cache.
        let bytes = galley_bytes(&entry.galley);
        #[cfg(test)]
        {
            self.last_galley_bytes = bytes;
        }
        self.entry = (bytes <= MAX_CACHED_GALLEY_BYTES).then_some(entry);
    }
}

fn galley_bytes(galley: &egui::Galley) -> usize {
    galley.job.text.len()
        + galley.job.sections.len() * size_of::<egui::text::LayoutSection>()
        + galley.rows.len() * size_of::<egui::epaint::text::PlacedRow>()
        + galley
            .rows
            .iter()
            .map(|row| row.glyphs.len() * size_of::<egui::epaint::text::Glyph>())
            .sum::<usize>()
        + galley.num_vertices * size_of::<egui::epaint::Vertex>()
        + galley.num_indices * size_of::<u32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(
        ctx: &egui::Context,
        cache: &mut TaskOutputCache,
        text: &str,
        color: Color32,
    ) -> Arc<egui::Galley> {
        let mut result = None;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            result = Some(cache.galley(ui, text, color));
        })
        .drop_without_applying_deltas();
        result.unwrap()
    }

    #[test]
    #[ignore = "manual before/after task-output frame benchmark; no timing assertions"]
    fn benchmark_task_output_cache() {
        use std::{hint::black_box, time::Instant};

        fn frame(ctx: &egui::Context, mut cache: Option<&mut TaskOutputCache>, text: &str) {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_max_width(800.0);
                egui::ScrollArea::both()
                    .max_height(240.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(cache) = cache.as_deref_mut() {
                            cache.show(ui, black_box(text), Color32::WHITE);
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(black_box(text))
                                        .monospace()
                                        .size(12.0)
                                        .color(Color32::WHITE),
                                )
                                .selectable(true)
                                .extend(),
                            );
                        }
                    });
            })
            .drop_without_applying_deltas();
        }

        fn measure(
            ctx: &egui::Context,
            mut cache: Option<&mut TaskOutputCache>,
            text: &str,
        ) -> f64 {
            let started = Instant::now();
            for _ in 0..20 {
                frame(ctx, cache.as_deref_mut(), text);
            }
            started.elapsed().as_secs_f64() * 1000.0 / 20.0
        }

        fn median(samples: &mut [f64]) -> f64 {
            samples.sort_by(f64::total_cmp);
            samples[samples.len() / 2]
        }

        let line = "🦀 Grüße output: checking module_name and another value = 42\r\n";
        for kib in [128_usize, 256, 561] {
            let requested_bytes = kib * 1024;
            let mut text = line.repeat(requested_bytes.div_ceil(line.len()));
            let mut end = requested_bytes;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            let before_ctx = egui::Context::default();
            let after_ctx = egui::Context::default();
            let mut cache = TaskOutputCache::default();
            for _ in 0..10 {
                frame(&before_ctx, None, &text);
                frame(&after_ctx, Some(&mut cache), &text);
            }
            let initial_hits = cache.hits;
            let initial_misses = cache.misses;
            let mut before = Vec::new();
            let mut after = Vec::new();
            for pair in 0..11 {
                if pair % 2 == 0 {
                    before.push(measure(&before_ctx, None, &text));
                    after.push(measure(&after_ctx, Some(&mut cache), &text));
                } else {
                    after.push(measure(&after_ctx, Some(&mut cache), &text));
                    before.push(measure(&before_ctx, None, &text));
                }
            }
            println!(
                "output_cache bytes={} lines={} galley_bytes={} budget_bytes={} retained={} cache_hits={} cache_misses={} before_ms={:.6} after_ms={:.6}",
                text.len(),
                text.lines().count(),
                cache.last_galley_bytes,
                MAX_CACHED_GALLEY_BYTES,
                cache.entry.is_some(),
                cache.hits - initial_hits,
                cache.misses - initial_misses,
                median(&mut before),
                median(&mut after),
            );
        }
    }

    #[test]
    fn cached_output_preserves_original_label_geometry_and_selection_sense() {
        let ctx = egui::Context::default();
        let mut cache = TaskOutputCache::default();
        let text = format!("🦀 Grüße\r\n\t{}\nlast line", "long line ".repeat(150));
        ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_width(300.0);
            let available_width = ui.available_width();
            let cached = cache.galley(ui, &text, Color32::WHITE);
            let (_, original, original_response) = egui::Label::new(
                RichText::new(&text)
                    .monospace()
                    .size(12.0)
                    .color(Color32::WHITE),
            )
            .selectable(true)
            .extend()
            .layout_in_ui(ui);
            assert_eq!(cached.job, original.job);
            assert_eq!(cached.size(), original.size());
            assert_eq!(cached.rows.len(), original.rows.len());
            assert!(cached.size().x > available_width);
            let response = cache.show(ui, &text, Color32::WHITE);
            assert_eq!(response.sense, original_response.sense);
            assert!(response.sense.senses_drag());
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn clearing_hidden_output_releases_its_cached_layout() {
        let ctx = egui::Context::default();
        let mut cache = TaskOutputCache::default();
        pass(&ctx, &mut cache, "retained output", Color32::WHITE);
        assert!(cache.entry.is_some());
        cache.clear();
        assert!(cache.entry.is_none());
        let restored = pass(&ctx, &mut cache, "retained output", Color32::WHITE);
        assert_eq!(restored.job.text, "retained output");
        assert!(cache.entry.is_some());
    }

    #[test]
    fn cache_budget_counts_glyph_meshes_and_drops_previous_entries() {
        let ctx = egui::Context::default();
        let mut cache = TaskOutputCache::default();
        pass(&ctx, &mut cache, "small output", Color32::WHITE);
        let mut oversized = cache.entry.take().unwrap();
        // Exercise the same budget accounting without allocating a huge mesh
        // during every full test run. Text remains small; shaped geometry wins.
        Arc::make_mut(&mut oversized.galley).num_vertices =
            MAX_CACHED_GALLEY_BYTES / size_of::<egui::epaint::Vertex>() + 1;
        assert!(galley_bytes(&oversized.galley) > MAX_CACHED_GALLEY_BYTES);
        pass(&ctx, &mut cache, "previous cached output", Color32::WHITE);
        assert!(cache.entry.is_some());
        cache.retain(oversized);
        assert!(cache.entry.is_none());
    }

    #[test]
    fn output_cache_reuses_idle_frames_but_invalidates_content_color_and_font_atlas() {
        let ctx = egui::Context::default();
        let mut cache = TaskOutputCache::default();
        let first = pass(&ctx, &mut cache, "same length", Color32::WHITE);
        for _ in 0..5 {
            assert!(Arc::ptr_eq(
                &first,
                &pass(&ctx, &mut cache, "same length", Color32::WHITE)
            ));
        }
        let changed = pass(&ctx, &mut cache, "edit length", Color32::WHITE);
        assert!(!Arc::ptr_eq(&first, &changed));
        let recolored = pass(&ctx, &mut cache, "edit length", Color32::YELLOW);
        assert!(!Arc::ptr_eq(&changed, &recolored));
        ctx.set_pixels_per_point(2.0);
        let scaled = pass(&ctx, &mut cache, "edit length", Color32::YELLOW);
        assert!(!Arc::ptr_eq(&recolored, &scaled));
        ctx.set_fonts(egui::FontDefinitions::empty());
        let replaced = pass(&ctx, &mut cache, "edit length", Color32::YELLOW);
        assert!(!Arc::ptr_eq(&scaled, &replaced));
        assert_eq!(replaced.job.text, "edit length");
    }
}
