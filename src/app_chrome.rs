//! Workspace chrome, status presentation and shared visual helpers.
use super::*;

impl VEditApp {
    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
        let options = crate::window_chrome::ChromeOptions {
            interaction_allowed: !self.chrome_blocked(),
            native_titlebar: self.ctx.data(|data| {
                data.get_temp::<bool>(egui::Id::new("native_window_chrome"))
                    .unwrap_or(false)
            }),
            native_maximize_state: self.ctx.data(|data| {
                data.get_temp::<(bool, bool)>(egui::Id::new("native_window_maximize_state"))
                    .unwrap_or_default()
            }),
        };
        let geometry = egui::Panel::top("header")
            .exact_size(crate::window_chrome::HEIGHT)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                crate::window_chrome::draw_top_bar(ui, options, |ui| {
                    if self.notifications.items_empty() {
                        self.workspace_header_button(ui);
                    } else {
                        let messages_width = 124.0_f32.min(ui.available_width() * 0.6);
                        let projects_width = (ui.available_width() - messages_width - 8.0).max(0.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(projects_width, 28.0),
                            Layout::left_to_right(Align::Center),
                            |ui| self.workspace_header_button(ui),
                        );
                        ui.add_space(8.0);
                        self.notifications_toolbar(ui);
                    }
                })
            })
            .inner;
        if geometry.close_requested {
            self.request_close(CloseAction::Exit);
        }
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new("vedit_window_chrome"), geometry);
        });
    }
    pub(super) fn chrome_blocked(&self) -> bool {
        egui::Popup::is_any_open(&self.ctx)
            || self.workspace_picker_open()
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.status_details.is_some()
            || self.notifications.is_open()
            || self.agent_marks_modal_open()
            || self.close_action.is_some()
            || self.context_file_dialog.is_some()
            || self.save_as.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
    }
    pub(super) fn activity(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("activity")
            .exact_size(49.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(5, 8)),
            )
            .show(ui, |ui| {
                self.main_menu_button(ui);
                ui.add_space(9.0);
                if !self.projects.is_empty() {
                    for (view, icon, hint) in [
                        (
                            SidebarView::Explorer,
                            Icon::Files,
                            format!(
                                "Explorer · {}",
                                self.shortcut_label(keybindings::Action::FocusExplorer)
                            ),
                        ),
                        (
                            SidebarView::Search,
                            Icon::Search,
                            format!(
                                "Search project · {}",
                                self.shortcut_label(keybindings::Action::SearchProject)
                            ),
                        ),
                        (
                            SidebarView::SourceControl,
                            Icon::Branch,
                            format!(
                                "Source control · {}",
                                self.shortcut_label(keybindings::Action::FocusGit)
                            ),
                        ),
                    ] {
                        if self.project().is_some_and(Project::is_other_files)
                            && view != SidebarView::Explorer
                        {
                            continue;
                        }
                        if icon_button(ui, icon, self.sidebar_view_visible(view), &hint).clicked() {
                            self.toggle_sidebar_view(view);
                        }
                        ui.add_space(6.0);
                    }
                }
                if self.review.is_some() {
                    if icon_button(
                        ui,
                        Icon::Review,
                        self.sidebar_view_visible(SidebarView::Review),
                        "Agent review - important places and notes",
                    )
                    .clicked()
                    {
                        self.toggle_sidebar_view(SidebarView::Review);
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    if icon_button(
                        ui,
                        Icon::Console,
                        self.output_visible,
                        &format!(
                            "Output · {}",
                            self.shortcut_label(keybindings::Action::ToggleOutput)
                        ),
                    )
                    .clicked()
                    {
                        self.output_visible = !self.output_visible;
                    }
                    ui.add_space(8.0);
                    if icon_button(ui, Icon::Settings, false, "Settings").clicked() {
                        self.settings_open = true;
                    }
                });
            });
    }
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let problems = self.problem_list();
        let statistics = self.tab().filter(|tab| !tab.loading).map(|tab| {
            self.text_statistics.display(
                ui.ctx(),
                tab.id,
                tab.revision,
                &tab.text,
                tab.lines.char_count(&tab.text, tab.revision),
                tab.selection.clone(),
            )
        });
        if statistics.is_none() {
            self.text_statistics.clear();
        }
        egui::Panel::bottom("status").exact_size(28.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 2)))
            .show(ui, |ui| {
                let compact = ui.available_width() < 820.0;
                let narrow = ui.available_width() < 360.0;
                let compact_statistics = ui.available_width() < 1100.0;
                let show_statistics = ui.available_width() >= 650.0;
                let status_error = self.status.as_ref().is_some_and(|(_, error, _)| *error);
                let right_budget = if narrow { 125.0 } else if !show_statistics { 300.0 } else if compact_statistics { 500.0 } else { 900.0 };
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.spacing_mut().interact_size.y = 20.0;
                ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
                ui.horizontal_centered(|ui| {
                    // Resting modes stay muted; color marks modes that change what keys do.
                    let (mode, color) = if self.tab().is_some_and(|t| t.vim.multi.placing()) { ("PLACE", ORANGE) } else if self.tab().is_some_and(|t| t.vim.multi.active()) { ("MULTI", BLUE) } else if self.screen_jump.is_some() { ("JUMP", ORANGE) } else if self.tab().is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview) { ("PREVIEW", MUTED) } else if self.settings.vim {
                        match self.tab().map(|t| t.vim.mode).unwrap_or(VimMode::Normal) {
                            VimMode::Normal => ("NORMAL", MUTED), VimMode::Insert => ("INSERT", BLUE),
                            VimMode::Visual => ("VISUAL", ORANGE), VimMode::VisualLine => ("V-LINE", ORANGE),
                            VimMode::VisualBlock => ("V-BLOCK", ORANGE),
                        }
                    } else { ("EDIT", MUTED) };
                    ui.label(RichText::new(format!(" {mode} ")).monospace().size(11.0).strong().color(color))
                        .on_hover_text(format!("PLACE: move only the active cursor; {} finishes placement. MULTI: edit every cursor; {} returns to one cursor. Vim: i to insert, v to select, Shift+V for lines, Esc for Normal", self.shortcut_label(keybindings::Action::FinishCursorPlacement), self.shortcut_label(keybindings::Action::ExitMulticursor)));
                    if let Some(tab) = self.tab().filter(|t| t.vim.multi.active()) {
                        let placing = tab.vim.multi.placing();
                        let count = tab.vim.multi.selections.len() - usize::from(placing);
                        let noun = if placing { "marked" } else if count == 1 { "cursor" } else { "cursors" };
                        ui.label(RichText::new(format!("{count} {noun}")).size(11.0).color(BLUE))
                            .on_hover_text(format!("Mark another position: {}. Finish placement and include the active cursor: {}. Undo last cursor selection: {}. Return to one cursor: {}.", self.shortcut_label(keybindings::Action::MarkCursor), self.shortcut_label(keybindings::Action::FinishCursorPlacement), self.shortcut_label(keybindings::Action::CursorUndo), self.shortcut_label(keybindings::Action::ExitMulticursor)));
                    }
                    if let Some(tab) = self.tab() {
                        let pending = tab.vim.pending_command();
                        if !pending.is_empty() { ui.label(RichText::new(pending).monospace().size(11.0).color(ORANGE)); }
                    }
                    if !narrow && !(compact && status_error) && let Some(branch) = self.project().and_then(|p| p.git_branch.clone()) {
                        let tracking = self.project().and_then(|p| p.git_tracking.as_ref());
                        let hint = tracking.map(|b| format!("{} - {} ahead, {} behind. Open source control.", b.upstream.as_deref().unwrap_or("Local branch"), b.ahead, b.behind)).unwrap_or_else(|| "Open source control".into());
                        let width = ui.fonts_mut(|fonts| fonts.layout_no_wrap(branch.clone(), FontId::proportional(11.0), TEXT)).size().x + 16.0;
                        let budget = (ui.available_width() - right_budget).clamp(30.0, 180.0);
                        if ui.add_sized([width.min(budget), 20.0], egui::Button::new(RichText::new(&branch).size(11.0).color(MUTED)).truncate().frame_when_inactive(false).corner_radius(3.0)).on_hover_text(&hint).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { self.show_sidebar(SidebarView::SourceControl); }
                    }
                    if !narrow && let Some(action) = &self.git_action && action.progress_ui(ui, 11.0) {
                        self.cancel_git_action();
                    }
                    if let Some(jump) = &self.screen_jump {
                        let hint = jump.state.hint();
                        ui.add(egui::Label::new(RichText::new(&hint).size(11.0).color(ORANGE)).truncate())
                            .on_hover_text("Type 1–3 characters to find visible text, then type its letter label to jump. Home-row letters come first; possible search continuations stay available for typing. Enter locks a shorter query or jumps to the only match. Three characters lock automatically. Backspace corrects a label, unlocks an Enter-confirmed query, or deletes a search character. Esc cancels.");
                    } else if !status_error && self.tab().is_some_and(|t| t.vim.multi.placing()) {
                        let hint = format!("Arrows: move · {}: mark · {}: edit · {}: cancel", self.shortcut_label(keybindings::Action::MarkCursor), self.shortcut_label(keybindings::Action::FinishCursorPlacement), self.shortcut_label(keybindings::Action::ExitMulticursor));
                        let width = (ui.available_width() - right_budget).clamp(24.0, 480.0);
                        ui.add_sized([width, 20.0], egui::Label::new(RichText::new(&hint).size(11.0).color(MUTED)).truncate()).on_hover_text(hint);
                    } else if let Some((_, error, time)) = &self.status
                        && (!compact || *error) && (*error || time.elapsed() < Duration::from_secs(6)) {
                        let width = (ui.available_width() - right_budget).clamp(24.0, 440.0);
                        self.status_message_ui(ui, width);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(t) = self.tab() {
                            let (line, col) = t.lines.line_col(&t.text, t.revision, t.vim.cursor);
                            let total = t.lines.line_count(&t.text, t.revision);
                            let position = if narrow { format!("{line}:{col}") } else { format!("Ln {line}/{total}, Col {col}") };
                            let mut position_hint = format!("Line {line}/{total} · Column {col}\nGo to line: {}", self.shortcut_label(keybindings::Action::GoToLine));
                            if !show_statistics && let Some(statistics) = &statistics {
                                position_hint.push_str(&format!("\n\n{}", statistics.tooltip()));
                            }
                            ui.label(RichText::new(position).size(11.0).color(MUTED)).on_hover_text(position_hint);
                            if show_statistics && let Some(statistics) = &statistics {
                                ui.label(RichText::new(statistics.label(compact_statistics)).size(11.0).color(MUTED)).on_hover_text(statistics.tooltip());
                            }
                            let details = format!("UTF-8{} - {} - {}", if t.bom { " with BOM" } else { "" }, t.line_ending, if t.agent_unsaved { "Agent draft: save to resume auto save" } else if self.settings.autosave { "Auto save on" } else { "Manual save" });
                            if !compact_statistics { ui.label(RichText::new(language_name(t.path.as_deref())).size(11.0).color(MUTED)).on_hover_text(details); }
                            if !compact_statistics && t.text.len() > crate::render::MAX_DOCUMENT_BYTES && !syntax_language(t.path.as_deref()).is_empty() {
                                ui.label(RichText::new("Plain text: file too large for highlighting").size(11.0).color(MUTED)).on_hover_text(format!(
                                    "Syntax colors are off for files larger than {} KiB so editing stays fast. This file has {} KiB. Editing, search and language analysis work as usual.",
                                    crate::render::MAX_DOCUMENT_BYTES / 1024,
                                    t.text.len().div_ceil(1024)
                                ));
                            }
                        }
                        if !compact_statistics { self.indentation_status_ui(ui); }
                        self.save_status_ui(ui, !narrow);
                        let problem_count = problems.counted();
                        if !compact_statistics && problem_count > 0 {
                            let label = if problem_count == 1 { "1 problem".to_owned() } else { format!("{problem_count} problems") };
                            let hint = format!("{}
Show Problems · {}
Next / previous error or warning: {} / {}", problems.summary(), self.shortcut_label(keybindings::Action::Problems), self.shortcut_label(keybindings::Action::NextProblem), self.shortcut_label(keybindings::Action::PreviousProblem));
                            if design::status_item(ui, &label, problems.badge_color(), &hint).clicked() { self.problems_visible = true; self.output_visible = true; }
                        }
                        if !compact_statistics { self.rust_analysis_status_ui(ui); }
                    });
                });
            });
    }
    pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
        let available_height = ui.available_height();
        let compact = available_height < 700.0;
        let space = if compact { 0.6 } else { 1.0 };
        egui::ScrollArea::vertical()
            .id_salt("welcome")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(if compact {
                        4.0
                    } else {
                        (available_height * 0.11).max(35.0)
                    });
                    brand(ui, if compact { 40.0 } else { 58.0 });
                    ui.add_space(if compact { 6.0 } else { 22.0 });
                    ui.label(
                        RichText::new("A quieter place to build.")
                            .size(if compact { 28.0 } else { 34.0 })
                            .strong()
                            .color(TEXT),
                    );
                    ui.add_space(10.0 * space);
                    ui.label(
                        RichText::new("Your projects. Your motions. Room to focus.")
                            .size(15.0)
                            .color(MUTED),
                    );
                    ui.add_space(if compact { 12.0 } else { 35.0 });
                    if accent_button(ui, "Open a project folder").clicked() {
                        self.pick_project();
                    }
                    ui.add_space(12.0 * space);
                    ui.label(
                        RichText::new(self.shortcut_label(keybindings::Action::OpenProject))
                            .monospace()
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(10.0 * space);
                    if ui
                        .button("Open a file")
                        .on_hover_text(self.shortcut_label(keybindings::Action::OpenFile))
                        .clicked()
                    {
                        self.pick_file();
                    }
                    ui.label(
                        RichText::new("Or drop files and folders here")
                            .small()
                            .color(MUTED),
                    );
                    ui.add_space(if compact { 12.0 } else { 20.0 });
                    egui::Frame::new()
                        .fill(PANEL)
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(10.0)
                        .inner_margin(if compact { 8 } else { 24 })
                        .show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(
                                RichText::new("Built around your flow")
                                    .size(11.5)
                                    .color(MUTED),
                            );
                            ui.add_space(if compact { 8.0 } else { 12.0 });
                            for (index, (label, keys)) in [
                                ("Start typing · return to motions", "i / Esc".to_owned()),
                                ("Vim motions & visual selections", "v / V".to_owned()),
                                (
                                    "Find a file",
                                    self.shortcut_label(keybindings::Action::QuickOpen),
                                ),
                                (
                                    "Search the whole project",
                                    self.shortcut_label(keybindings::Action::SearchProject),
                                ),
                                (
                                    "Commands at your fingertips",
                                    self.shortcut_label(keybindings::Action::CommandPalette),
                                ),
                                (
                                    "Editor & Vim settings",
                                    self.shortcut_label(keybindings::Action::Settings),
                                ),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                if index < 2 && !self.settings.vim {
                                    continue;
                                }
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(label).size(12.0).color(TEXT));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(keys).monospace().size(11.0).color(MUTED),
                                        );
                                    });
                                });
                                ui.add_space(if compact { 2.0 } else { 6.0 });
                            }
                        });
                    ui.add_space(if compact { 8.0 } else { 25.0 });
                    ui.label(
                        RichText::new(format!("VEDIT  /  {}", env!("CARGO_PKG_VERSION")))
                            .size(10.0)
                            .color(MUTED.linear_multiply(0.6)),
                    );
                });
            });
    }
}

pub(super) fn annotation_color(severity: crate::cli::AnnotationSeverity) -> Color32 {
    match severity {
        crate::cli::AnnotationSeverity::Info => BLUE,
        crate::cli::AnnotationSeverity::Warning => ORANGE,
        crate::cli::AnnotationSeverity::Critical => RED,
    }
}

pub(super) fn set_style(ctx: &egui::Context) {
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    ctx.set_fonts(crate::typography::fonts(crate::cli::EditorFont::default()));
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = SURFACE;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.weak_text_color = Some(MUTED);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.warn_fg_color = ORANGE;
    style.visuals.error_fg_color = RED;
    style.visuals.code_bg_color = SURFACE;
    style.visuals.window_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.inactive.weak_bg_fill = SURFACE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.hovered.bg_fill = HOVER;
    style.visuals.widgets.hovered.weak_bg_fill = HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, crate::palette::BORDER_STRONG);
    style.visuals.widgets.active.bg_fill = HOVER;
    style.visuals.widgets.active.weak_bg_fill = HOVER;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.noninteractive.bg_fill = SURFACE;
    style.visuals.widgets.noninteractive.weak_bg_fill = SURFACE;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.window_corner_radius = egui::CornerRadius::same(8);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(6);
    style.visuals.selection.bg_fill = HOVER;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.spacing.item_spacing = Vec2::new(7.0, 5.0);
    style.spacing.button_padding = Vec2::new(9.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    // Floating scrollbars expand on hover without flashing bright white.
    style.spacing.scroll.interact_handle_opacity = 0.45;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(12.5));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.5));
    style.interaction.tooltip_delay = 0.45;
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::Theme::Dark);
}

/// Selected code keeps its own color on the blue wash, instead of egui's
/// accent-colored selection glyphs.
pub(super) fn editor_selection(visuals: &mut egui::Visuals) {
    visuals.selection.bg_fill = EDITOR_SELECTION;
    visuals.selection.stroke.color = TEXT;
}

pub(super) fn brand(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let c = rect.center();
    let points = |r: f32| {
        vec![
            c + Vec2::new(0.0, -r),
            c + Vec2::new(r, 0.0),
            c + Vec2::new(0.0, r),
            c + Vec2::new(-r, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points(size * 0.44),
        Color32::TRANSPARENT,
        Stroke::new((size / 18.0).max(1.3), ACCENT),
    ));
    ui.painter().add(egui::Shape::convex_polygon(
        points(size * 0.13),
        ACCENT,
        Stroke::NONE,
    ));
}

pub(super) fn small_button(ui: &mut egui::Ui, label: &str, hint: &str) -> egui::Response {
    design::ghost_button(ui, label, hint)
}

pub(super) fn accent_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    design::primary_button(ui, label, Vec2::new(210.0, 38.0))
}

pub(super) fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    selected: bool,
    hint: &str,
) -> egui::Response {
    let response = ui.add_sized(
        Vec2::splat(35.0),
        egui::Button::new("").frame_when_inactive(false),
    );
    let rect = response.rect;
    let label = match icon {
        Icon::Menu => "Main menu",
        Icon::Files => "Explorer",
        Icon::Search => "Search project",
        Icon::Branch => "Source control",
        Icon::Console => "Output",
        Icon::Settings => "Settings",
        Icon::Review => "Agent review",
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    let c = rect.center();
    let painter = ui.painter();
    let color = if selected || response.hovered() {
        TEXT
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.5, color);
    if response.hovered() {
        painter.rect_filled(rect, 5.0, HOVER);
    }
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() - 4.0, rect.top() + 7.0),
                egui::pos2(rect.left() - 2.0, rect.bottom() - 7.0),
            ),
            1.0,
            ACCENT,
        );
    }
    if design::keyboard_focus(&response) {
        painter.rect_stroke(
            rect.shrink(1.0),
            5.0,
            Stroke::new(1.5, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let line = |a: Vec2, b: Vec2| {
        painter.line_segment([c + a, c + b], stroke);
    };
    match icon {
        Icon::Menu => {
            for y in [-6.0, 0.0, 6.0] {
                line(Vec2::new(-7.0, y), Vec2::new(7.0, y));
            }
        }
        Icon::Files => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c + Vec2::new(2.0, 2.0), Vec2::new(12.0, 16.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(Vec2::new(-8.0, 4.0), Vec2::new(-8.0, -9.0));
            line(Vec2::new(-8.0, -9.0), Vec2::new(2.0, -9.0));
        }
        Icon::Search => {
            painter.circle_stroke(c - Vec2::splat(2.0), 6.0, stroke);
            line(Vec2::new(2.5, 2.5), Vec2::new(8.0, 8.0));
        }
        Icon::Branch => {
            for p in [
                Vec2::new(-5.0, -7.0),
                Vec2::new(-5.0, 7.0),
                Vec2::new(6.0, -6.0),
            ] {
                painter.circle_stroke(c + p, 2.3, stroke);
            }
            line(Vec2::new(-5.0, -4.7), Vec2::new(-5.0, 4.7));
            line(Vec2::new(-5.0, 2.0), Vec2::new(6.0, -2.0));
            line(Vec2::new(6.0, -2.0), Vec2::new(6.0, -3.7));
        }
        Icon::Console => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(21.0, 16.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(Vec2::new(-6.0, -3.0), Vec2::new(-2.0, 0.0));
            line(Vec2::new(-2.0, 0.0), Vec2::new(-6.0, 3.0));
            line(Vec2::new(1.0, 4.0), Vec2::new(6.0, 4.0));
        }
        Icon::Settings => {
            painter.circle_stroke(c, 6.5, stroke);
            painter.circle_stroke(c, 2.0, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                let v = Vec2::angled(angle);
                line(v * 6.5, v * 9.0);
            }
        }
        Icon::Review => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(16.0, 21.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            for y in [-5.0, 1.0, 7.0] {
                line(Vec2::new(-4.0, y), Vec2::new(-2.0, y + 2.0));
                line(Vec2::new(-2.0, y + 2.0), Vec2::new(1.0, y - 2.0));
                line(Vec2::new(3.0, y), Vec2::new(5.0, y));
            }
        }
    }
    response
        .on_hover_text(hint)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Code selection: a blue wash that leaves syntax colors readable.
pub(super) const EDITOR_SELECTION: Color32 =
    Color32::from_rgba_premultiplied(0x20, 0x30, 0x43, 0x47);

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Menu,
    Files,
    Search,
    Branch,
    Console,
    Settings,
    Review,
}
