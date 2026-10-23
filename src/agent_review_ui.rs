//! Agent draft review and annotation presentation.
use super::*;

impl VEditApp {
    pub(super) fn review_ui(&mut self, ui: &mut egui::Ui) {
        let Some(review) = &self.review else {
            ui.label(RichText::new("No review to show").color(MUTED));
            return;
        };
        let title = review.title.clone();
        let note = review.note.clone();
        let files = review.files.clone();
        ui.label(RichText::new(title).size(17.0).strong().color(TEXT));
        if !note.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(note).size(12.0).color(MUTED));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} files", files.len()))
                    .size(10.5)
                    .color(MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if design::ghost_button(
                    ui,
                    "Clear",
                    "Remove review notes and markings; keep all documents open",
                )
                .clicked()
                {
                    for tab in self.projects.iter_mut().flat_map(|p| &mut p.tabs) {
                        tab.annotations.clear();
                        tab.annotations_revision = None;
                        tab.agent_location_error = None;
                    }
                    self.review = None;
                    self.clear_review_sidebar();
                }
            });
        });
        ui.separator();
        let mut jump = None;
        egui::ScrollArea::vertical()
            .id_salt("agent_review")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for id in files {
                    let Some((project_index, tab_index)) =
                        self.projects.iter().enumerate().find_map(|(pi, p)| {
                            p.tabs.iter().position(|t| t.id == id).map(|ti| (pi, ti))
                        })
                    else {
                        continue;
                    };
                    let project = &self.projects[project_index];
                    let tab = &project.tabs[tab_index];
                    let active =
                        self.active_project == project_index && project.active_tab == tab_index;
                    let path = tab
                        .path
                        .as_ref()
                        .map(|p| crate::paths::display(p.strip_prefix(&project.root).unwrap_or(p)))
                        .unwrap_or_else(|| tab.title.clone());
                    let label = if self.projects.len() > 1 {
                        format!("{} · {path}", project.display_name())
                    } else {
                        path
                    };
                    let tooltip = tab.path.as_ref().map_or_else(
                        || format!("{}\nOpen this review document", project.display_name()),
                        |path| {
                            format!("{}\nOpen this review document", crate::paths::display(path))
                        },
                    );
                    ui.add_space(5.0);
                    if ui
                        .push_id(id, |ui| {
                            design::left_row(
                                ui,
                                &label,
                                Some((
                                    file_badge(tab.path.as_deref().unwrap_or(Path::new(""))),
                                    file_color(tab.path.as_deref()),
                                )),
                                active,
                                &tooltip,
                            )
                        })
                        .inner
                        .clicked()
                    {
                        jump = Some((project_index, tab_index, None));
                    }
                    if tab
                        .annotations_revision
                        .is_some_and(|revision| revision != tab.revision)
                    {
                        ui.label(
                            RichText::new("Edited since review. Check the marked locations.")
                                .size(10.5)
                                .color(ORANGE),
                        );
                    }
                    if let Some(error) = &tab.agent_location_error {
                        ui.label(RichText::new(error).size(10.5).color(ORANGE));
                    }
                    for annotation in &tab.annotations {
                        let location = annotation
                            .end
                            .as_ref()
                            .map(|end| format!("Lines {}-{}", annotation.start.line, end.line))
                            .unwrap_or_else(|| format!("Line {}", annotation.start.line));
                        let severity = match annotation.severity {
                            crate::cli::AnnotationSeverity::Info => "Info",
                            crate::cli::AnnotationSeverity::Warning => "Warning",
                            crate::cli::AnnotationSeverity::Critical => "Critical",
                        };
                        let detail = format!("{severity} · {location}");
                        let label = if annotation.note.is_empty() {
                            "Marked location"
                        } else {
                            &annotation.note
                        };
                        let response = ui
                            .push_id((id, &annotation.id), |ui| {
                                design::detail_row(
                                    ui,
                                    label,
                                    &detail,
                                    false,
                                    &format!("{label} - {detail}. Click to reveal in source."),
                                )
                            })
                            .inner;
                        let color = annotation_color(annotation.severity);
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                response.rect.min + Vec2::new(0.0, 8.0),
                                Vec2::new(2.0, 32.0),
                            ),
                            1.0,
                            color,
                        );
                        if response.clicked() {
                            jump = Some((project_index, tab_index, Some(annotation.start.clone())));
                        }
                    }
                }
            });
        if let Some((pi, ti, location)) = jump {
            self.activate_tab(pi, ti);
            if let Some(location) = location {
                self.set_document_view(crate::cli::DocumentView::Source);
                if let Some(tab) = self.tab_mut() {
                    tab.pending_agent_location = Some(location);
                    tab.agent_location_error = None;
                }
                self.apply_pending_agent_locations();
            }
        }
    }
}
