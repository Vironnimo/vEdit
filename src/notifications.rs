//! Persistent messages agents send with `vedit-cli notify`: unread indicators
//! stay until the user expands an entry or opens its file, project or review.
use super::*;
use crate::cli::{NotificationLevel, NotificationOpen, NotificationReview, OpenFile, Response};
use serde_json::{Value, json};

/// Bound the read history without discarding messages the user has not read.
const MAX_READ_ENTRIES: usize = 100;
/// The taskbar flashes at most this often, however many messages arrive.
const FLASH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Notification {
    id: u64,
    root: Option<PathBuf>,
    level: NotificationLevel,
    title: Option<String>,
    message: String,
    open: Option<NotificationOpen>,
    review: Option<NotificationReview>,
    /// Unix seconds of the latest repeat.
    time: i64,
    count: u32,
    read: bool,
}

impl Notification {
    fn same_message(&self, other: &Notification) -> bool {
        self.root == other.root
            && self.level == other.level
            && self.title == other.title
            && self.message == other.message
            && self.open == other.open
            && self.review == other.review
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Inbox {
    /// Oldest first.
    items: Vec<Notification>,
    next_id: u64,
    #[serde(skip)]
    open: bool,
    #[serde(skip)]
    expanded: Option<u64>,
    #[serde(skip)]
    last_flash: Option<Instant>,
}

impl Inbox {
    pub(super) fn unread(&self) -> usize {
        self.items.iter().filter(|item| !item.read).count()
    }

    pub(super) fn unread_for(&self, root: &Path) -> usize {
        self.items
            .iter()
            .filter(|item| !item.read && item.root.as_deref() == Some(root))
            .count()
    }

    /// The most severe unread level of a project, for its tab's dot.
    pub(super) fn unread_level(&self, root: &Path) -> Option<NotificationLevel> {
        self.items
            .iter()
            .filter(|item| !item.read && item.root.as_deref() == Some(root))
            .map(|item| item.level)
            .max_by_key(|level| severity(*level))
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn items_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(super) fn status_json(&self, projects: &[Project]) -> Value {
        let elsewhere =
            self.items
                .iter()
                .filter(|item| {
                    !item.read
                        && !item.root.as_ref().is_some_and(|root| {
                            projects.iter().any(|project| &project.root == root)
                        })
                })
                .count();
        json!({"unread":self.unread(),"total":self.items.len(),"unread_outside_open_projects":elsewhere})
    }

    /// Add a message, or count a repeat of an identical unread one.
    /// Returns its id, whether it was a repeat, and its repeat count.
    fn push(&mut self, mut notification: Notification) -> (u64, bool, u32) {
        if let Some(position) = self
            .items
            .iter()
            .position(|item| !item.read && item.same_message(&notification))
        {
            let mut item = self.items.remove(position);
            item.count = item.count.saturating_add(1);
            item.time = notification.time;
            let result = (item.id, true, item.count);
            self.items.push(item);
            return result;
        }
        self.next_id += 1;
        notification.id = self.next_id;
        let id = notification.id;
        self.items.push(notification);
        (id, false, 1)
    }

    fn set_read(&mut self, id: u64, read: bool) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if item.read == read {
            return false;
        }
        item.read = read;
        // Drop older read entries, but keep the entry just opened so that its
        // body cannot disappear while the user is reading it.
        let mut excess = self
            .items
            .iter()
            .filter(|item| item.read)
            .count()
            .saturating_sub(MAX_READ_ENTRIES);
        self.items.retain(|item| {
            if excess > 0 && item.read && item.id != id {
                excess -= 1;
                false
            } else {
                true
            }
        });
        true
    }

    /// Whether the taskbar may flash now; records the flash.
    fn take_flash(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_flash
            .is_some_and(|last| now.duration_since(last) < FLASH_INTERVAL)
        {
            return false;
        }
        self.last_flash = Some(now);
        true
    }
}

fn severity(level: NotificationLevel) -> u8 {
    match level {
        NotificationLevel::Info => 0,
        NotificationLevel::Success => 1,
        NotificationLevel::Warning => 2,
        NotificationLevel::Error => 3,
    }
}

pub(super) fn level_color(level: NotificationLevel) -> Color32 {
    match level {
        NotificationLevel::Info => BLUE,
        NotificationLevel::Success => GREEN,
        NotificationLevel::Warning => ORANGE,
        NotificationLevel::Error => RED,
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

impl VEditApp {
    /// `vedit-cli notify`: record the message for the user; never changes
    /// the visible project, focus or documents.
    pub(super) fn control_notify(
        &mut self,
        message: String,
        title: Option<String>,
        level: NotificationLevel,
        workspace: Option<PathBuf>,
        open: Option<NotificationOpen>,
        review: Option<NotificationReview>,
    ) -> Response {
        let (project, source) = if workspace.is_some() {
            match self.control_project_index(workspace.as_deref()) {
                Ok((index, source)) => (Some(index), source),
                Err(response) => return response,
            }
        } else if let Some(index) = self
            .control_caller
            .as_deref()
            .and_then(|caller| control::containing_project(&self.projects, caller))
        {
            (Some(index), "caller_directory")
        } else if let Some(index) = open
            .as_ref()
            .and_then(|open| control::containing_project(&self.projects, &open.path))
        {
            (Some(index), "open_path")
        } else if self.active_folder().is_some() {
            (Some(self.active_project), "active_project")
        } else {
            (None, "none")
        };
        if review.is_some() {
            match project {
                None => {
                    return Response::failure(
                        "no_workspace",
                        "A review notification needs an open project. Nothing was recorded.",
                    );
                }
                Some(index) if self.projects[index].git_root.is_none() => {
                    return Response::failure(
                        "no_repository",
                        "This workspace has no loaded repository, so --review has nothing to open. Nothing was recorded; send the notification without --review.",
                    );
                }
                Some(_) => {}
            }
        }
        let root = project.map(|index| self.projects[index].root.clone());
        let (id, coalesced, count) = self.notifications.push(Notification {
            id: 0,
            root: root.clone(),
            level,
            title,
            message,
            open,
            review,
            time: unix_now(),
            count: 1,
            read: false,
        });
        self.notification_state_changed();
        let focused = self.ctx.input(|input| input.focused);
        let flashed = !focused && self.notifications.take_flash();
        if flashed {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
        }
        self.ctx.request_repaint();
        Response::success(json!({
            "applied":true,"id":id,
            "workspace":root.as_deref().map(crate::paths::display),"workspace_source":source,
            "coalesced":coalesced,"repeat_count":count,
            "unread":self.notifications.unread(),
            "unread_in_workspace":root.as_deref().map(|root| self.notifications.unread_for(root)),
            "window_focused":focused,"flashed":flashed
        }))
    }

    pub(super) fn open_notifications(&mut self) {
        self.cancel_navigation();
        self.palette = None;
        self.picker = None;
        self.completions = None;
        self.notifications.open = true;
        self.notifications.expanded = None;
        self.ctx.request_repaint();
    }

    fn notification_state_changed(&mut self) {
        self.session_dirty = true;
        self.ctx.request_repaint();
    }

    fn set_notification_read(&mut self, id: u64, read: bool) {
        if self.notifications.set_read(id, read) {
            self.notification_state_changed();
        }
    }

    /// The inbox button beside "For agent"; drawn only once a message arrived.
    pub(super) fn notifications_toolbar(&mut self, ui: &mut egui::Ui) {
        if self.notifications.items.is_empty() {
            return;
        }
        let unread = self.notifications.unread();
        let width = if unread > 0 { 40.0 } else { 28.0 };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), egui::Sense::click());
        let label = if unread == 1 {
            "1 unread agent notification".to_owned()
        } else {
            format!("{unread} unread agent notifications")
        };
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label)
        });
        if response.hovered() || response.has_focus() || self.notifications.open {
            ui.painter().rect_filled(rect, 4.0, HOVER);
        }
        let color = self
            .notifications
            .items
            .iter()
            .filter(|item| !item.read)
            .map(|item| item.level)
            .max_by_key(|level| severity(*level))
            .map_or(MUTED, level_color);
        let center = if unread > 0 {
            rect.left_center() + Vec2::new(12.0, 0.0)
        } else {
            rect.center()
        };
        // A bell drawn as paths so no symbol font is required.
        let stroke = Stroke::new(1.3, color);
        let bell = [
            center + Vec2::new(-5.5, 4.0),
            center + Vec2::new(-4.0, 2.0),
            center + Vec2::new(-4.0, -2.0),
            center + Vec2::new(-2.0, -5.0),
            center + Vec2::new(2.0, -5.0),
            center + Vec2::new(4.0, -2.0),
            center + Vec2::new(4.0, 2.0),
            center + Vec2::new(5.5, 4.0),
            center + Vec2::new(-5.5, 4.0),
        ];
        ui.painter().add(egui::Shape::line(bell.to_vec(), stroke));
        ui.painter()
            .circle_filled(center + Vec2::new(0.0, 6.0), 1.4, color);
        if unread > 0 {
            let badge = if unread > 99 {
                "99+".into()
            } else {
                unread.to_string()
            };
            ui.painter().text(
                rect.right_center() - Vec2::new(5.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                badge,
                FontId::proportional(10.5),
                color,
            );
        }
        if response.clicked() {
            self.open_notifications();
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "Agent notifications · {label}\nMessages stay unread until you expand them or open their action."
            ));
    }

    pub(super) fn notifications_dialog(&mut self) {
        if !self.notifications.open {
            return;
        }
        let ctx = self.ctx.clone();
        let now = unix_now();
        let mut close = false;
        let mut clear = false;
        let mut run = None;
        let mut expand = None;
        let mut mark_unread = None;
        let items = &self.notifications.items;
        let modal = egui::Modal::new(egui::Id::new("agent_notifications_dialog"))
            .backdrop_color(Color32::from_black_alpha(80))
            .show(&ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 48.0).clamp(260.0, 620.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Agent notifications").size(20.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let response = design::ghost_button(ui, "×", "Close notifications (Esc)");
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                "Close notifications",
                            )
                        });
                        close |= response.clicked();
                        if !items.is_empty()
                            && design::ghost_button(
                                ui,
                                "Clear all",
                                "Remove all saved notifications, including unread messages",
                            )
                            .clicked()
                        {
                            clear = true;
                        }
                    });
                });
                ui.add_space(6.0);
                if items.is_empty() {
                    ui.label(
                        RichText::new("No notifications. Agents send them with vedit-cli notify.")
                            .color(MUTED),
                    );
                    return;
                }
                ui.separator();
                let height = (ctx.content_rect().height() - 180.0).clamp(120.0, 480.0);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for item in items.iter().rev() {
                            let expanded = self.notifications.expanded == Some(item.id);
                            ui.push_id(item.id, |ui| {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    let (dot, _) = ui.allocate_exact_size(
                                        Vec2::new(12.0, 16.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(dot.center(), 3.5, level_color(item.level));
                                    let heading = item.title.clone()
                                        .unwrap_or_else(|| short_line(&item.message, 90));
                                    let response = ui.add(egui::Button::selectable(expanded,
                                        RichText::new(heading).strong().color(TEXT)).wrap());
                                    if response.on_hover_text(if expanded { "Collapse message" } else { "Read message" }).clicked() {
                                        expand = Some((item.id, !expanded));
                                    }
                                    if item.count > 1 {
                                        ui.label(RichText::new(format!("×{}", item.count)).size(11.0).color(MUTED));
                                    }
                                });
                                let project = item.root.as_deref().map(file_title)
                                    .unwrap_or_else(|| "No project".into());
                                let level = match item.level {
                                    NotificationLevel::Info => "info",
                                    NotificationLevel::Success => "success",
                                    NotificationLevel::Warning => "warning",
                                    NotificationLevel::Error => "error",
                                };
                                let detail = format!("{} · {project} · {level} · {}",
                                    if item.read { "Read" } else { "Unread" },
                                    git_panel::relative_time(item.time, now));
                                ui.label(RichText::new(detail).size(11.0).color(MUTED));
                                if expanded {
                                    ui.add(egui::Label::new(RichText::new(&item.message).color(TEXT)).wrap());
                                }
                                ui.horizontal(|ui| {
                                    if let Some(action) = notification_action(item)
                                        && design::ghost_button(ui, &action, "Switch to this project and open it").clicked()
                                    {
                                        run = Some(item.id);
                                    }
                                    if expanded && design::ghost_button(ui, "Mark unread",
                                        "Keep this message's project indicator until you read it again").clicked() {
                                        mark_unread = Some(item.id);
                                    }
                                });
                                ui.add_space(4.0);
                                ui.separator();
                            });
                        }
                    });
            });
        close |= modal.should_close()
            || modal
                .backdrop_response
                .clicked_by(egui::PointerButton::Secondary)
            || modal
                .backdrop_response
                .clicked_by(egui::PointerButton::Middle);
        if clear {
            self.notifications.items.clear();
            self.notifications.expanded = None;
            self.notification_state_changed();
        }
        if let Some((id, expanded)) = expand {
            self.notifications.expanded = expanded.then_some(id);
            if expanded {
                self.set_notification_read(id, true);
            }
        }
        if let Some(id) = mark_unread {
            self.notifications.expanded = None;
            self.set_notification_read(id, false);
        }
        if close || clear {
            self.notifications.open = false;
        }
        if let Some(id) = run {
            self.notifications.open = false;
            self.run_notification(id);
        }
    }

    /// Switch to the notification's project and open what it points at.
    pub(super) fn run_notification(&mut self, id: u64) {
        let Some(item) = self.notifications.items.iter().find(|item| item.id == id) else {
            return;
        };
        let (root, open, review) = (item.root.clone(), item.open.clone(), item.review);
        if let Some(root) = &root {
            match self
                .projects
                .iter()
                .position(|project| &project.root == root)
            {
                Some(index) if index != self.active_project => {
                    self.finish_insert_before_switch();
                    self.switch_project(index);
                }
                Some(_) => {}
                None if open.is_none() => {
                    self.status(
                        format!("{} is no longer open", crate::paths::display(root)),
                        true,
                    );
                    return;
                }
                None => {}
            }
        }
        if let Some(review) = review {
            let source = match review {
                NotificationReview::Working => git::ReviewSource::WorkingTree,
                NotificationReview::Branch => git::ReviewSource::Branch { base: None },
            };
            if let Err(error) = self.open_change_review(self.active_project, source) {
                self.status(error, true);
                return;
            }
        } else if let Some(open) = open {
            let mut file = OpenFile::source(open.path);
            file.location = open.location;
            let result = self.control_open(vec![file], None, false);
            if let Some(error) = result["items"]
                .as_array()
                .and_then(|items| items.first())
                .and_then(|item| item["error"]["message"].as_str())
            {
                self.status(error.to_owned(), true);
                return;
            }
        }
        self.set_notification_read(id, true);
    }
}

fn notification_action(item: &Notification) -> Option<String> {
    if let Some(review) = item.review {
        return Some(match review {
            NotificationReview::Working => "Review working changes".into(),
            NotificationReview::Branch => "Review branch changes".into(),
        });
    }
    if let Some(open) = &item.open {
        let name = file_title(&open.path);
        return Some(match &open.location {
            Some(location) => format!("Open {name}:{}", location.line),
            None => format!("Open {name}"),
        });
    }
    item.root.as_ref().map(|_| "Show project".into())
}

fn short_line(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() <= limit {
        line.to_owned()
    } else {
        let mut short: String = line.chars().take(limit - 1).collect();
        short.push('…');
        short
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn note(message: &str) -> Notification {
        Notification {
            id: 0,
            root: Some(PathBuf::from("C:/project")),
            level: NotificationLevel::Info,
            title: None,
            message: message.into(),
            open: None,
            review: None,
            time: 1,
            count: 1,
            read: false,
        }
    }

    #[test]
    fn identical_unread_messages_coalesce_and_read_ones_start_over() {
        let mut inbox = Inbox::default();
        assert_eq!(inbox.push(note("Tests pass")), (1, false, 1));
        assert_eq!(inbox.push(note("Build failed")), (2, false, 1));
        assert_eq!(inbox.push(note("Tests pass")), (1, true, 2));
        assert_eq!(
            inbox.items.last().unwrap().id,
            1,
            "a repeat moves to the top"
        );
        assert_eq!(inbox.unread_for(Path::new("C:/project")), 2);
        inbox.set_read(1, true);
        inbox.set_read(2, true);
        assert_eq!(inbox.push(note("Tests pass")), (3, false, 1));
        assert_eq!(inbox.unread(), 1);
    }

    #[test]
    fn the_inbox_retains_unread_entries_and_bounds_only_read_history() {
        let mut inbox = Inbox::default();
        for index in 0..MAX_READ_ENTRIES + 5 {
            inbox.push(note(&index.to_string()));
        }
        assert_eq!(inbox.items.len(), MAX_READ_ENTRIES + 5);
        // Read newest first; the oldest unread message still stays.
        for id in (2..=inbox.next_id).rev() {
            inbox.set_read(id, true);
        }
        assert_eq!(inbox.items.len(), MAX_READ_ENTRIES + 1);
        assert_eq!(inbox.items[0].message, "0");
        assert_eq!(inbox.unread(), 1);
        inbox.set_read(1, true);
        assert_eq!(inbox.items.len(), MAX_READ_ENTRIES);
        assert_eq!(inbox.items[0].message, "0", "keep the message just opened");
        assert_eq!(inbox.items[1].message, "6");
    }

    #[test]
    fn the_taskbar_flashes_once_per_interval() {
        let mut inbox = Inbox::default();
        assert!(inbox.take_flash());
        assert!(!inbox.take_flash());
    }

    #[test]
    fn the_user_reads_individual_messages_and_can_mark_them_unread_again() {
        let mut app = VEditApp::empty(egui::Context::default());
        visual_tests::workspace(&mut app);
        let root = app.projects[0].root.clone();
        for index in 0..20 {
            let mut item = note(&format!("Result {index}: all checks passed."));
            item.title = Some(format!("Task {index} finished"));
            item.root = Some(root.clone());
            app.notifications.push(item);
        }
        // Render only this owner's surfaces so no unrelated session/recovery
        // housekeeping clears the persistence flag under test.
        let mut screen = egui_kittest::Harness::builder()
            .with_size([1100.0, 760.0])
            .build_ui_state(
                |ui, app: &mut VEditApp| {
                    app.ctx = ui.ctx().clone();
                    app.notifications_toolbar(ui);
                    app.notifications_dialog();
                },
                app,
            );
        set_style(&screen.ctx);
        screen.ctx.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        screen.run_steps(3);
        screen.get_by_label("20 unread agent notifications").click();
        screen.run_steps(3);
        assert_eq!(screen.state().notifications.unread_for(&root), 20);
        assert!(
            !screen.state().session_dirty,
            "overview alone reads nothing, including offscreen entries"
        );
        screen.get_by_label("Task 19 finished").click();
        screen.run_steps(3);
        assert!(
            screen
                .query_by_label("Result 19: all checks passed.")
                .is_some()
        );
        assert_eq!(screen.state().notifications.unread_for(&root), 19);
        assert!(screen.state().session_dirty);
        screen.state_mut().session_dirty = false;
        screen.get_by_label("Mark unread").click();
        screen.run_steps(3);
        assert_eq!(screen.state().notifications.unread_for(&root), 20);
        assert!(screen.state().notifications.expanded.is_none());
        assert!(screen.state().session_dirty);
        screen.get_by_label("Close notifications").click();
        screen.run_steps(3);
        assert!(!screen.state().notifications.is_open());
        screen.state_mut().open_notifications();
        screen.run_steps(3);
        assert_eq!(screen.state().notifications.unread(), 20);
        screen.state_mut().session_dirty = false;
        screen.get_by_label("Clear all").click();
        screen.run_steps(3);
        assert!(screen.state().notifications.items_empty());
        assert!(screen.state().session_dirty);
        let session = serde_json::to_vec(&screen.state().session()).unwrap();
        let restored: Session = serde_json::from_slice(&session).unwrap();
        assert!(restored.notifications.items_empty());
        assert!(restored.notifications.last_flash.is_none());
        assert!(restored.notifications.expanded.is_none());
        let mut inbox = restored.notifications;
        assert_eq!(inbox.push(note("After clearing")), (21, false, 1));
    }
}
