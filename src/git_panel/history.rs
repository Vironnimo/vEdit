//! Virtualized commit rows, history paging and graph presentation.

use super::navigation::{git_context_menu, git_menu_item, git_row_keyboard};
use super::*;

pub(super) fn show_history(
    ui: &mut egui::Ui,
    project: &Project,
    state: &mut GitViewState,
    panel: &PanelContext<'_>,
    keyboard_target: Option<egui::Id>,
    actions: &mut Vec<GitUiAction>,
) {
    let &PanelContext {
        root,
        busy,
        bindings,
        ..
    } = panel;
    let initial_history_loading = project.git_loading
        && project.git_history_snapshot.is_none()
        && project.recent_commits.is_empty();
    let history_rect = ui.available_rect_before_wrap();
    state.history_rect = Some(history_rect);
    ui.scope_builder(egui::UiBuilder::new().max_rect(history_rect), |ui| {
        ui.set_clip_rect(history_rect.intersect(ui.clip_rect()));
        let header = region_header(ui, "History", ()).on_hover_text(
            "The current branch's line is blue while its commits exist only locally and purple where they are on a remote branch, based on the last fetch. Merged side branches keep their own colors.",
        );
        state.keyboard_controls.push(header.id);
        if header.clicked() {
            state.section = GitSection::History;
            state.keyboard_focus_pending = true;
        }

        // The graph grows once per received page, never per painted
        // row. This also initializes synthetic screenshot fixtures.
        if state.graph.rows.len() > project.recent_commits.len() {
            state.graph.reset(
                project.git_branches.iter()
                    .find(|reference| reference.current)
                    .map(|reference| reference.hash.as_str()),
            );
        }
        state
            .graph
            .append(&project.recent_commits[state.graph.rows.len()..]);
        if let Some(error) = &project.git_history_error {
            error_note(ui, error);
            if design::ghost_button(ui, "Retry history", "Retry the failed history request")
                .clicked()
            {
                actions.push(GitUiAction::RetryHistory);
            }
        }
        if project.recent_commits.is_empty()
            && !initial_history_loading
            && !project.git_history_loading
            && project.git_history_error.is_none()
        {
            ui.add_space(20.0);
            ui.label(RichText::new("No commits yet").size(12.5).color(TEXT));
            ui.label(
                RichText::new("Stage changes and create your first commit.")
                    .size(11.0)
                    .color(MUTED),
            );
        }
        if initial_history_loading {
            ui.label(RichText::new("Loading commits…").size(11.0).color(MUTED));
        }
        let mut near_end = false;
        let upstream = project
            .git_tracking
            .as_ref()
            .and_then(|tracking| tracking.upstream.as_deref());
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt(("git-history", &project.root))
                .auto_shrink([false, false]);
            if let Some(offset) = state.history_scroll_pending.take() {
                scroll = scroll.vertical_scroll_offset(offset);
            }
            if state.section == GitSection::History
                && let Some(target) = keyboard_target
                && let Some(index) = state.keyboard_rows.iter().position(|id| *id == target)
            {
                scroll = scroll.vertical_scroll_offset(index as f32 * HISTORY_ROW_HEIGHT);
            }
            let history_scroll = scroll.show_rows(
                ui,
                HISTORY_ROW_HEIGHT,
                project.recent_commits.len(),
                |ui, range| {
                    near_end = history_near_end(range.end, project.recent_commits.len());
                    for index in range {
                        let commit = &project.recent_commits[index];
                        ui.push_id(&commit.hash, |ui| {
                            let response = history_row(
                                ui,
                                git_row_id(root, &commit.hash),
                                commit,
                                state.selected_commit.as_deref() == Some(&commit.hash),
                                &state.graph.rows[index],
                                state.graph.lane_count,
                                upstream,
                            );
                            history_hover(
                                &response,
                                commit,
                                state.hover_stats.get(&(root.to_path_buf(), commit.hash.clone())),
                                actions,
                            );
                            if response.clicked() {
                                state.history_click =
                                    Some((commit.hash.clone(), response.rect));
                                actions
                                    .push(GitUiAction::SelectCommit(commit.hash.clone()));
                            }
                            if response.double_clicked() {
                                actions
                                    .push(GitUiAction::ReviewCommit(commit.hash.clone()));
                            }
                            git_context_menu(&response, bindings, |ui, focus| {
                                if git_menu_item(
                                    ui,
                                    "Open commit",
                                    "",
                                    "Show metadata, message, and changed files",
                                    focus,
                                )
                                .clicked()
                                {
                                    actions.push(GitUiAction::SelectCommit(
                                        commit.hash.clone(),
                                    ));
                                    ui.close();
                                }
                                if git_menu_item(
                                    ui,
                                    "Review commit",
                                    "",
                                    "Read all changes of this commit in one view",
                                    false,
                                )
                                .clicked()
                                {
                                    actions.push(GitUiAction::ReviewCommit(
                                        commit.hash.clone(),
                                    ));
                                    ui.close();
                                }
                                if git_menu_item(
                                    ui,
                                    "Copy commit hash",
                                    "",
                                    "Copy the full commit identifier",
                                    false,
                                )
                                .clicked()
                                {
                                    ui.ctx().copy_text(commit.hash.clone());
                                    ui.close();
                                }
                            });
                        });
                    }
                },
            );
            state.history_scroll_offset = history_scroll.state.offset.y;
            if state.section == GitSection::History {
                state.keyboard_page_rows =
                    (history_scroll.inner_rect.intersect(ui.clip_rect()).height()
                        / HISTORY_ROW_HEIGHT)
                        .floor()
                        .max(1.0) as usize;
            }
        });
        // Page reads keep the existing viewport intact. A footer here
        // changes the parent's measured height and makes the sidebar
        // jump for a frame; reserving space for it wastes list height.
        if near_end
            && project.git_history_has_more
            && !project.git_history_loading
            && project.git_history_error.is_none()
            && !busy
        {
            actions.push(GitUiAction::LoadMore);
        }
    });
}

/// One compact line per commit: graph, subject, author and ref badges.
pub(super) const HISTORY_ROW_HEIGHT: f32 = 24.0;
/// The HEAD line before and after the commits reachable from a remote.
const GRAPH_LOCAL: Color32 = BLUE;
const GRAPH_REMOTE: Color32 = Color32::from_rgb(0xb4, 0x8e, 0xe8);

fn history_near_end(visible_end: usize, loaded: usize) -> bool {
    loaded > 0 && visible_end.saturating_add(4) >= loaded
}

/// Side branches avoid the HEAD line's blue and purple.
fn graph_color(index: usize) -> Color32 {
    [
        ORANGE,
        Color32::from_rgb(0xe0, 0x6c, 0x9f),
        GREEN,
        Color32::from_rgb(0x6d, 0xc7, 0xde),
        ACCENT,
        RED,
    ][index % 6]
}

fn track_color(color: usize, head: bool, remote: bool) -> Color32 {
    match (head, remote) {
        (false, _) => graph_color(color),
        (true, false) => GRAPH_LOCAL,
        (true, true) => GRAPH_REMOTE,
    }
}

/// Filled badges mark the checked-out branch and its upstream; other refs
/// are outlined so the row stays calm when a commit has many decorations.
fn ref_badge_style(reference: &git::CommitRef, upstream: Option<&str>) -> (Color32, bool) {
    match reference.kind {
        _ if reference.current => (GRAPH_LOCAL, true),
        git::RefKind::Remote => (GRAPH_REMOTE, upstream == Some(reference.name.as_str())),
        git::RefKind::Local => (GRAPH_LOCAL, false),
        git::RefKind::Tag | git::RefKind::Head => (MUTED, false),
    }
}

fn history_row(
    ui: &mut egui::Ui,
    row_id: egui::Id,
    commit: &git::CommitSummary,
    selected: bool,
    graph: &crate::git_graph::GraphRow,
    lanes: usize,
    upstream: Option<&str>,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), HISTORY_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(rect, row_id, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            &commit.subject,
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let background = if selected {
            ui.visuals().selection.bg_fill
        } else if response.hovered() || crate::design::keyboard_focus(&response) {
            HOVER
        } else {
            PANEL
        };
        if background != PANEL {
            painter.rect_filled(rect, 4.0, background);
        }
        let lanes = lanes.max(graph.lane_count).max(1);
        let graph_width = (18.0 + lanes.saturating_sub(1) as f32 * 11.0)
            .min((rect.width() * 0.3).clamp(18.0, 90.0));
        let pitch = if lanes > 1 {
            (graph_width - 14.0) / (lanes - 1) as f32
        } else {
            0.0
        };
        let lane_x = |lane: usize| rect.left() + 8.0 + lane as f32 * pitch;
        let middle = rect.center().y;
        for (edges, top, bottom) in [
            (&graph.incoming, rect.top(), middle),
            (&graph.outgoing, middle, rect.bottom()),
        ] {
            for edge in edges {
                let from = egui::pos2(lane_x(edge.from_lane), top);
                let to = egui::pos2(lane_x(edge.to_lane), bottom);
                let bend = (top + bottom) * 0.5;
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [from, egui::pos2(from.x, bend), egui::pos2(to.x, bend), to],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(1.5, track_color(edge.color, edge.head, edge.remote)),
                ));
            }
        }
        let node = egui::pos2(lane_x(graph.node_lane), middle);
        let color = track_color(graph.node_color, graph.node_head, commit.on_remote);
        let head = commit.refs.iter().any(|reference| reference.current);
        if head {
            painter.circle_filled(node, 6.0, background);
            painter.circle_stroke(node, 4.5, Stroke::new(2.0, color));
        } else if commit.parents.len() > 1 {
            painter.circle_filled(node, 5.0, background);
            painter.circle_stroke(node, 3.6, Stroke::new(1.5, color));
            painter.circle_filled(node, 1.4, color);
        } else {
            painter.circle_filled(node, 4.5, background);
            painter.circle_filled(node, 3.0, color);
        }

        let left = rect.left() + graph_width + 4.0;
        let right = rect.right() - 4.0;
        let badge_room = ((right - left) * 0.5).max(0.0);
        let mut used = 0.0;
        let mut hidden = 0;
        for (index, reference) in commit.refs.iter().enumerate() {
            let (badge_color, filled) = ref_badge_style(reference, upstream);
            let mut job = egui::text::LayoutJob::single_section(
                reference.name.clone(),
                egui::TextFormat {
                    font_id: FontId::proportional(10.5),
                    color: if filled { BG } else { badge_color },
                    ..Default::default()
                },
            );
            // Only the first badge shortens; later ones collapse into "+N".
            job.wrap.max_width = (badge_room - 10.0).max(12.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let width = galley.size().x + 10.0;
            if index > 0 && used + width > badge_room {
                hidden = commit.refs.len() - index;
                break;
            }
            let badge = egui::Rect::from_min_size(
                egui::pos2(right - used - width, middle - 8.0),
                Vec2::new(width, 16.0),
            );
            if filled {
                painter.rect_filled(badge, 8.0, badge_color);
            } else {
                painter.rect_stroke(
                    badge,
                    8.0,
                    Stroke::new(1.0, badge_color.gamma_multiply(0.7)),
                    egui::StrokeKind::Inside,
                );
            }
            painter.galley(
                egui::pos2(badge.left() + 5.0, middle - galley.size().y * 0.5),
                galley,
                badge_color,
            );
            used += width + 4.0;
        }
        if hidden > 0 {
            let more = ui.painter().layout_no_wrap(
                format!("+{hidden}"),
                FontId::proportional(10.5),
                MUTED,
            );
            let width = more.size().x;
            painter.galley(
                egui::pos2(right - used - width, middle - more.size().y * 0.5),
                more,
                MUTED,
            );
            used += width + 4.0;
        }

        let mut line = egui::text::LayoutJob::default();
        line.wrap.max_rows = 1;
        line.wrap.max_width = (right - used - left - 2.0).max(1.0);
        line.wrap.break_anywhere = true;
        let strong = egui::FontFamily::Name("vedit-semibold".into());
        let family = if head && ui.fonts_mut(|fonts| fonts.families().contains(&strong)) {
            strong
        } else {
            egui::FontFamily::Proportional
        };
        line.append(
            &commit.subject,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(12.0, family),
                color: TEXT,
                ..Default::default()
            },
        );
        line.append(
            &format!("  {}", commit.author),
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(11.0),
                color: MUTED,
                ..Default::default()
            },
        );
        let line = ui.fonts_mut(|fonts| fonts.layout_job(line));
        painter.galley(egui::pos2(left, middle - line.size().y * 0.5), line, TEXT);
    }
    git_row_keyboard(ui, &response);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn history_hover(
    response: &egui::Response,
    commit: &git::CommitSummary,
    stats: Option<&Result<git::CommitStats, String>>,
    actions: &mut Vec<GitUiAction>,
) {
    response.clone().on_hover_details_ui(|ui| {
        ui.ctx().request_repaint_after(Duration::from_secs(30));
        ui.label(RichText::new(&commit.subject).strong());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        // Snapshot tests inject a fixed clock; production uses wall-clock time.
        #[cfg(test)]
        let now = ui
            .data(|data| data.get_temp::<i64>(egui::Id::new("git-history-test-clock")))
            .unwrap_or(now);
        ui.label(
            RichText::new(format!(
                "{} · {}",
                commit.author,
                relative_time(commit.timestamp, now)
            ))
            .color(MUTED),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(short_hash(&commit.hash))
                    .monospace()
                    .size(10.5)
                    .color(MUTED),
            );
            ui.label(
                RichText::new(if commit.on_remote {
                    "on remote"
                } else {
                    "local only"
                })
                .size(10.5)
                .color(if commit.on_remote {
                    GRAPH_REMOTE
                } else {
                    GRAPH_LOCAL
                }),
            );
            ui.label(
                RichText::new(commit.date.replace('T', " "))
                    .size(10.5)
                    .color(MUTED),
            );
        });
        if !commit.refs.is_empty() {
            ui.label(
                commit
                    .refs
                    .iter()
                    .map(|reference| {
                        if reference.current {
                            format!("HEAD → {}", reference.name)
                        } else {
                            reference.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        ui.separator();
        match stats {
            Some(Ok(stats)) => {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} {} changed",
                        stats.files_changed,
                        if stats.files_changed == 1 {
                            "file"
                        } else {
                            "files"
                        }
                    ));
                    ui.label(RichText::new(format!("+{}", stats.lines.added)).color(GREEN));
                    ui.label(RichText::new(format!("−{}", stats.lines.removed)).color(RED));
                });
                if stats.lines.binary {
                    ui.label(
                        RichText::new("Includes binary changes")
                            .size(10.5)
                            .color(MUTED),
                    );
                }
            }
            Some(Err(error)) => {
                ui.label(RichText::new("Change statistics unavailable").color(MUTED));
                ui.label(RichText::new(error).size(10.5).color(MUTED));
            }
            None => {
                actions.push(GitUiAction::LoadCommitStats(commit.hash.clone()));
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0).color(MUTED));
                    ui.label(RichText::new("Loading changes…").color(MUTED));
                });
            }
        }
        let mut copy = format!(
            "{}\n{} · {}\n{}\n{}\n{}",
            commit.subject,
            commit.author,
            relative_time(commit.timestamp, now),
            commit.date,
            commit.hash,
            commit
                .refs
                .iter()
                .map(|r| if r.current {
                    format!("HEAD → {}", r.name)
                } else {
                    r.name.clone()
                })
                .collect::<Vec<_>>()
                .join(" · ")
        );
        copy.push_str(if commit.on_remote {
            "\non remote"
        } else {
            "\nlocal only"
        });
        match stats {
            Some(Ok(stats)) => copy.push_str(&format!(
                "\n{} files changed · +{} −{}{}",
                stats.files_changed,
                stats.lines.added,
                stats.lines.removed,
                if stats.lines.binary {
                    " · Includes binary changes"
                } else {
                    ""
                }
            )),
            Some(Err(error)) => copy.push_str(&format!("\nChange statistics unavailable: {error}")),
            None => copy.push_str("\nLoading changes…"),
        }
        copy
    });
}

pub(super) fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

pub(super) fn format_date(date: &str) -> String {
    date.replace('T', " ")
}
