//! Messages agents send with `vedit-cli notify`: the Messages sidebar view keeps
//! the full text, and a brief arrival card appears in the window. A message
//! counts as read once it is visible in the view or its file or review is opened
//! from the notification.
use super::*;
use crate::cli::{NotificationLevel, NotificationOpen, NotificationReview, OpenFile, Response};
use serde_json::{Value, json};

/// Bound the read history without discarding messages the user has not seen.
const MAX_READ_ENTRIES: usize = 100;
/// Read messages leave the inbox after this long.
const READ_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
/// The taskbar flashes at most this often, however many messages arrive.
const FLASH_INTERVAL: Duration = Duration::from_secs(5);
/// An arrival card stays this long; hovering holds it longer.
const TOAST_LIFETIME: Duration = Duration::from_secs(8);
/// Newer arrival cards push older ones out.
const TOAST_LIMIT: usize = 3;
const TOAST_WIDTH: f32 = 330.0;

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
    /// Unix seconds when the user first saw the message. Saved sessions from
    /// before the Messages view keep `read` without this stamp.
    #[serde(default)]
    read_at: Option<i64>,
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

/// One arrival card; transient, so it is never part of the saved session.
#[derive(Clone, Copy)]
struct Toast {
    id: u64,
    expires: Instant,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Inbox {
    /// Oldest first.
    items: Vec<Notification>,
    next_id: u64,
    #[serde(skip)]
    toast: Vec<Toast>,
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

    /// The most severe unread level of the whole inbox, for the Messages icon.
    pub(super) fn unread_severity(&self) -> Option<NotificationLevel> {
        self.items
            .iter()
            .filter(|item| !item.read)
            .map(|item| item.level)
            .max_by_key(|level| severity(*level))
    }

    pub(super) fn items_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Arrival cards currently waiting in the window.
    #[cfg(test)]
    pub(super) fn toast_count(&self) -> usize {
        self.toast.len()
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
        let now = unix_now();
        self.prune_history(now, None);
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
        let now = unix_now();
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if item.read == read {
            return false;
        }
        item.read = read;
        item.read_at = read.then_some(now);
        if read {
            // Keep the message just seen so its text cannot disappear from under
            // the reader while older read entries leave.
            self.prune_history(now, Some(id));
        }
        true
    }

    /// Drop read messages older than the retention window and bound the read
    /// history. Unread messages always stay.
    fn prune_history(&mut self, now: i64, keep: Option<u64>) {
        self.items.retain(|item| {
            !item.read
                || item
                    .read_at
                    .is_none_or(|at| now.saturating_sub(at) < READ_RETENTION_SECONDS)
        });
        let mut excess = self
            .items
            .iter()
            .filter(|item| item.read)
            .count()
            .saturating_sub(MAX_READ_ENTRIES);
        self.items.retain(|item| {
            if excess > 0 && item.read && Some(item.id) != keep {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Give old saved read entries a start for the retention window and drop
    /// messages whose retention has passed. Returns whether the inbox changed.
    pub(super) fn normalize(&mut self) -> bool {
        let now = unix_now();
        let before = self.items.len();
        for item in &mut self.items {
            if item.read && item.read_at.is_none() {
                item.read_at = Some(now);
            }
        }
        self.prune_history(now, None);
        self.items.len() != before
    }

    fn dismiss(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
        self.toast.retain(|toast| toast.id != id);
    }

    fn clear_read(&mut self) {
        self.items.retain(|item| !item.read);
        let remaining: Vec<u64> = self.items.iter().map(|item| item.id).collect();
        self.toast.retain(|toast| remaining.contains(&toast.id));
    }

    fn queue_toast(&mut self, id: u64) {
        self.toast.retain(|toast| toast.id != id);
        self.toast.push(Toast {
            id,
            expires: Instant::now() + TOAST_LIFETIME,
        });
        while self.toast.len() > TOAST_LIMIT {
            self.toast.remove(0);
        }
    }

    fn touch_toast(&mut self, id: u64) {
        if let Some(toast) = self.toast.iter_mut().find(|toast| toast.id == id) {
            toast.expires = Instant::now() + Duration::from_secs(2);
        }
    }

    fn dismiss_toast(&mut self, id: u64) {
        self.toast.retain(|toast| toast.id != id);
    }

    fn expire_toasts(&mut self, now: Instant) {
        self.toast.retain(|toast| toast.expires > now);
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
            read_at: None,
        });
        // A visible Messages view shows the message right away, so no card.
        if !self.sidebar_view_visible(SidebarView::Messages) {
            self.notifications.queue_toast(id);
        }
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

    /// Open the Messages view without taking the user away from their documents.
    pub(super) fn open_notifications(&mut self) {
        self.cancel_navigation();
        self.palette = None;
        self.picker = None;
        self.completions = None;
        self.show_sidebar(SidebarView::Messages);
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

    /// Window-level entry point, also available without documents or in a review.
    pub(super) fn notifications_toolbar(&mut self, ui: &mut egui::Ui) {
        if self.notifications.items.is_empty() {
            return;
        }
        let unread = self.notifications.unread();
        let width = ui.available_width().min(124.0);
        let compact = width < 108.0;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), egui::Sense::click());
        let label = if unread == 1 {
            "1 unread agent notification".to_owned()
        } else {
            format!("{unread} unread agent notifications")
        };
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label)
        });
        if unread > 0
            || response.hovered()
            || response.has_focus()
            || self.sidebar_view_visible(SidebarView::Messages)
        {
            ui.painter().rect_filled(rect, 4.0, HOVER);
        }
        let color = self
            .notifications
            .unread_severity()
            .map_or(MUTED, level_color);
        let center = rect.left_center() + Vec2::new(12.0, 0.0);
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
        if !compact {
            ui.painter().text(
                rect.left_center() + Vec2::new(26.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "Messages",
                FontId::proportional(12.0),
                if unread > 0 { TEXT } else { MUTED },
            );
        }
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
            self.toggle_sidebar_view(SidebarView::Messages);
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "Messages from agents · {label}\nOpen what a message points at, or copy its text."
            ));
    }

    /// The Messages sidebar view: full text, one card per message, newest first.
    /// A card is read as soon as it is visible while the window has focus.
    pub(super) fn messages_panel(&mut self, ui: &mut egui::Ui) {
        self.notifications.prune_history(unix_now(), None);
        let unread = self.notifications.unread();
        ui.horizontal(|ui| {
            let label = match unread {
                0 => "No new messages".to_owned(),
                1 => "1 new message".to_owned(),
                count => format!("{count} new messages"),
            };
            ui.label(
                RichText::new(label)
                    .size(11.0)
                    .color(if unread > 0 { BLUE } else { MUTED }),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let has_read = self.notifications.items.iter().any(|item| item.read);
                let clear = ui
                    .add_enabled(
                        has_read,
                        egui::Button::new(RichText::new("Clear read").size(11.0))
                            .frame_when_inactive(false)
                            .min_size(Vec2::new(0.0, 22.0)),
                    )
                    .on_hover_text(
                        "Remove messages you have already seen. Unread messages stay; read ones leave after seven days.",
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if clear.clicked() {
                    self.notifications.clear_read();
                    self.notification_state_changed();
                }
            });
        });
        ui.add_space(6.0);
        if self.notifications.items.is_empty() {
            ui.label(
                RichText::new(
                    "No messages yet. Agents notify you when work is finished, blocked or needs a decision.",
                )
                .size(11.0)
                .color(MUTED),
            );
            return;
        }
        // Only an on-screen message counts as seen; a background window or a
        // message below the scroll viewport stays unread.
        let window_focused = self.ctx.input(|input| input.focused);
        let now = unix_now();
        let items: Vec<Notification> = self.notifications.items.clone();
        let mut seen = Vec::new();
        let mut run = None;
        let mut dismiss = None;
        egui::ScrollArea::vertical()
            .id_salt("agent_messages")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in items.iter().rev() {
                    ui.push_id(item.id, |ui| {
                        let card = self.message_card(ui, item, now, &mut run, &mut dismiss);
                        if window_focused && !item.read && ui.is_rect_visible(card) {
                            seen.push(item.id);
                        }
                        ui.add_space(8.0);
                    });
                }
            });
        for id in seen {
            self.set_notification_read(id, true);
        }
        if let Some(id) = dismiss {
            self.notifications.dismiss(id);
            self.notification_state_changed();
        }
        if let Some(id) = run {
            self.run_notification(id);
        }
        if self.sidebar_view_has_focus(SidebarView::Messages)
            && ui.input(|input| input.key_pressed(egui::Key::Escape))
        {
            self.focus_editor();
        }
    }

    /// One message card; returns its rectangle for the seen check.
    fn message_card(
        &self,
        ui: &mut egui::Ui,
        item: &Notification,
        now: i64,
        run: &mut Option<u64>,
        dismiss: &mut Option<u64>,
    ) -> egui::Rect {
        let card = egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(
                1.0,
                if item.read {
                    BORDER
                } else {
                    level_color(item.level).linear_multiply(0.55)
                },
            ))
            .corner_radius(6)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if !item.read {
                        ui.label(RichText::new("●").size(9.0).color(level_color(item.level)));
                    }
                    // Bound the project name so time, repeat count and the
                    // dismiss button keep their space in a narrow sidebar.
                    let reserve = 60.0 + if item.count > 1 { 28.0 } else { 0.0 } + 26.0;
                    let max_project = (ui.available_width() - reserve).max(40.0);
                    let project = item
                        .root
                        .as_deref()
                        .map(file_title)
                        .unwrap_or_else(|| "No project".into());
                    ui.add_sized(
                        Vec2::new(max_project, 18.0),
                        egui::Label::new(RichText::new(project).size(10.5).color(MUTED)).truncate(),
                    )
                    .on_hover_text(
                        item.root
                            .as_deref()
                            .map(crate::paths::display)
                            .unwrap_or_else(|| "No project".into()),
                    );
                    ui.label(
                        RichText::new(git_panel::relative_time(item.time, now))
                            .size(10.5)
                            .color(MUTED),
                    );
                    if item.count > 1 {
                        ui.label(
                            RichText::new(format!("×{}", item.count))
                                .size(10.5)
                                .color(MUTED),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if design::ghost_button(ui, "×", "Dismiss this message").clicked() {
                            *dismiss = Some(item.id);
                        }
                    });
                });
                let (heading, body) = message_parts(item);
                if let Some(heading) = heading {
                    ui.add(egui::Label::new(RichText::new(heading).strong().color(TEXT)).wrap());
                }
                if !body.trim().is_empty() {
                    ui.add(
                        egui::Label::new(RichText::new(body).size(12.0).color(TEXT))
                            .wrap()
                            .selectable(true),
                    );
                }
                if let Some(target) = notification_target(item) {
                    ui.add_space(2.0);
                    ui.label(RichText::new(target).size(10.5).color(MUTED));
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if let Some(action) = notification_action(item)
                        && design::ghost_button(ui, &action, "Open this file or change review")
                            .clicked()
                    {
                        *run = Some(item.id);
                    }
                    if design::ghost_button(ui, "Copy", "Copy the full message text").clicked() {
                        ui.ctx().copy_text(item.message.clone());
                    }
                });
            });
        card.response.rect
    }

    /// Brief arrival card in the window corner; it never takes focus and opens
    /// the message's target or the Messages view.
    pub(super) fn notifications_toast(&mut self) {
        if self.notifications.toast.is_empty() || !self.toast_allowed() {
            return;
        }
        self.notifications.expire_toasts(Instant::now());
        let items: Vec<Notification> = self.notifications.items.clone();
        let ids: Vec<u64> = self
            .notifications
            .toast
            .iter()
            .map(|toast| toast.id)
            .collect();
        let mut open = None;
        let mut held = Vec::new();
        let ctx = self.ctx.clone();
        egui::Area::new(egui::Id::new("agent_message_toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -42.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(&ctx, |ui| {
                ui.set_width(TOAST_WIDTH);
                for id in ids {
                    let Some(item) = items.iter().find(|item| item.id == id) else {
                        continue;
                    };
                    let (heading, body) = message_parts(item);
                    let heading = heading.unwrap_or_else(|| "Agent message".into());
                    let action =
                        notification_action(item).unwrap_or_else(|| "Show in Messages".into());
                    let card = egui::Frame::new()
                        .fill(SURFACE)
                        .stroke(Stroke::new(
                            1.0,
                            level_color(item.level).linear_multiply(0.65),
                        ))
                        .corner_radius(6)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(TOAST_WIDTH - 20.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("●").size(9.0).color(level_color(item.level)),
                                );
                                let reserve = if item.count > 1 { 28.0 } else { 0.0 };
                                let max_project = (ui.available_width() - reserve).max(60.0);
                                let project = item
                                    .root
                                    .as_deref()
                                    .map(file_title)
                                    .unwrap_or_else(|| "No project".into());
                                ui.add_sized(
                                    Vec2::new(max_project, 18.0),
                                    egui::Label::new(
                                        RichText::new(project).size(10.5).color(MUTED),
                                    )
                                    .truncate(),
                                );
                                if item.count > 1 {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("×{}", item.count))
                                                .size(10.5)
                                                .color(MUTED),
                                        );
                                    });
                                }
                            });
                            ui.add(
                                egui::Label::new(
                                    RichText::new(heading.clone()).strong().color(TEXT),
                                )
                                .truncate(),
                            );
                            if !body.trim().is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(short_line(&body, 120))
                                            .size(11.0)
                                            .color(MUTED),
                                    )
                                    .truncate(),
                                );
                            }
                            ui.add_space(2.0);
                            ui.label(RichText::new(action.clone()).size(11.0).color(BLUE));
                        })
                        .response;
                    let response = ui.interact(
                        card.rect,
                        egui::Id::new(("agent_message_toast_card", id)),
                        egui::Sense::click(),
                    );
                    let accessible = format!("Agent message: {heading}");
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &accessible)
                    });
                    if response.hovered() {
                        ui.painter().rect_stroke(
                            card.rect,
                            6.0,
                            Stroke::new(1.0, level_color(item.level).linear_multiply(0.8)),
                            egui::StrokeKind::Inside,
                        );
                        held.push(id);
                    }
                    if response.clicked() {
                        open = Some(id);
                    }
                    ui.add_space(6.0);
                }
            });
        for id in held {
            self.notifications.touch_toast(id);
        }
        let Some(id) = open else {
            self.ctx.request_repaint_after(Duration::from_millis(250));
            return;
        };
        let has_target = self
            .notifications
            .items
            .iter()
            .find(|item| item.id == id)
            .is_some_and(|item| item.open.is_some() || item.review.is_some());
        self.notifications.dismiss_toast(id);
        if has_target {
            self.run_notification(id);
        } else {
            self.open_notifications();
        }
        self.ctx.request_repaint_after(Duration::from_millis(250));
    }

    /// Cards wait while another surface owns input and do not count down in a
    /// background window, so a return to vEdit still shows what arrived.
    fn toast_allowed(&self) -> bool {
        self.ctx.input(|input| input.focused) && !self.chrome_blocked()
    }

    /// Switch to the notification's project and open what it points at.
    pub(super) fn run_notification(&mut self, id: u64) {
        let Some(item) = self.notifications.items.iter().find(|item| item.id == id) else {
            return;
        };
        let (root, open, review) = (item.root.clone(), item.open.clone(), item.review);
        if open.is_none() && review.is_none() {
            return;
        }
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
    None
}

fn notification_target(item: &Notification) -> Option<String> {
    if let Some(review) = item.review {
        return Some(match review {
            NotificationReview::Working => "Change review · Working changes".into(),
            NotificationReview::Branch => "Change review · Branch changes".into(),
        });
    }
    if let Some(open) = &item.open {
        let path = item
            .root
            .as_deref()
            .and_then(|root| open.path.strip_prefix(root).ok())
            .unwrap_or(&open.path);
        return Some(match &open.location {
            Some(location) => format!("{} · line {}", crate::paths::display(path), location.line),
            None => crate::paths::display(path),
        });
    }
    None
}

/// Split a message into a heading line and the remaining body, so a title or
/// first line is not repeated as both heading and text. A single-line message
/// stays whole in the body.
fn message_parts(item: &Notification) -> (Option<String>, String) {
    if let Some(title) = &item.title {
        return (Some(title.clone()), item.message.clone());
    }
    let mut lines = item.message.lines();
    let first = lines.next().unwrap_or_default();
    let rest = lines.collect::<Vec<_>>().join("\n");
    if rest.trim().is_empty() {
        (None, item.message.clone())
    } else {
        (Some(first.to_owned()), rest)
    }
}

fn short_line(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        line
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
            read_at: None,
        }
    }

    fn harness(app: VEditApp) -> egui_kittest::Harness<'static, VEditApp> {
        let mut screen = egui_kittest::Harness::builder()
            .with_size([1100.0, 760.0])
            .build_ui_state(
                |ui, app: &mut VEditApp| {
                    app.ctx = ui.ctx().clone();
                    app.header(ui);
                    app.activity(ui);
                    app.sidebar_ui(ui);
                    app.notifications_toast();
                },
                app,
            );
        set_style(&screen.ctx);
        screen.ctx.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        screen.input_mut().focused = true;
        screen.run_steps(3);
        screen
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
        assert_eq!(inbox.items[0].message, "0", "keep the message just seen");
        assert_eq!(inbox.items[1].message, "6");
    }

    #[test]
    fn read_messages_leave_after_seven_days_and_legacy_entries_restart_their_window() {
        let mut inbox = Inbox::default();
        let (stale, _, _) = inbox.push(note("stale"));
        inbox.set_read(stale, true);
        inbox.items[0].read_at = Some(unix_now() - READ_RETENTION_SECONDS - 1);
        let (fresh, _, _) = inbox.push(note("fresh"));
        inbox.set_read(fresh, true);
        let (unread, _, _) = inbox.push(note("unread"));
        inbox.normalize();
        assert_eq!(
            inbox.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![fresh, unread]
        );

        let mut legacy = Inbox::default();
        let (id, _, _) = legacy.push(note("legacy"));
        legacy.items[0].read = true;
        legacy.normalize();
        assert_eq!(legacy.items.len(), 1, "an old read entry stays usable");
        assert!(
            legacy.items[0].read_at.is_some(),
            "the retention window starts when the session is restored"
        );
        assert_eq!(id, legacy.items[0].id);
    }

    #[test]
    fn dismissing_removes_one_message_and_clearing_read_keeps_unread_ones() {
        let mut inbox = Inbox::default();
        let (read, _, _) = inbox.push(note("seen"));
        let (unread, _, _) = inbox.push(note("new"));
        inbox.set_read(read, true);
        inbox.queue_toast(unread);
        inbox.clear_read();
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(inbox.items[0].id, unread);
        assert_eq!(inbox.toast.len(), 1, "the unread card stays");
        inbox.dismiss(unread);
        assert!(inbox.items_empty());
        assert!(inbox.toast.is_empty());
    }

    #[test]
    fn arrival_cards_hold_the_newest_three_and_pause_while_hovered() {
        let mut inbox = Inbox::default();
        let mut ids = Vec::new();
        for index in 0..4 {
            let (id, _, _) = inbox.push(note(&index.to_string()));
            inbox.queue_toast(id);
            ids.push(id);
        }
        assert_eq!(
            inbox.toast.iter().map(|toast| toast.id).collect::<Vec<_>>(),
            ids[1..].to_vec(),
            "older cards leave before newer ones"
        );
        let first = inbox.toast[0].expires;
        // Hover holds a card that would otherwise leave.
        inbox.toast[0].expires = Instant::now() - Duration::from_secs(1);
        inbox.touch_toast(ids[2]);
        let now = Instant::now();
        inbox.expire_toasts(now);
        assert_eq!(
            inbox.toast.iter().map(|toast| toast.id).collect::<Vec<_>>(),
            vec![ids[2], ids[3]],
            "an expired card leaves, the held card and its neighbour stay"
        );
        assert!(inbox.toast[0].expires > now);
        assert!(first > now);
        inbox.expire_toasts(now + Duration::from_secs(3));
        assert_eq!(
            inbox.toast.iter().map(|toast| toast.id).collect::<Vec<_>>(),
            vec![ids[3]]
        );
        inbox.expire_toasts(now + TOAST_LIFETIME + Duration::from_secs(3));
        assert!(inbox.toast.is_empty());
    }

    #[test]
    fn the_taskbar_flashes_once_per_interval() {
        let mut inbox = Inbox::default();
        assert!(inbox.take_flash());
        assert!(!inbox.take_flash());
    }

    #[test]
    fn messages_open_in_their_own_view_with_full_text_and_are_read_once_visible() {
        let mut app = VEditApp::empty(egui::Context::default());
        visual_tests::workspace(&mut app);
        let root = app.projects[0].root.clone();
        let long = format!(
            "All checks passed. {}The parser change is ready for review.",
            "Detailed report line. ".repeat(14)
        );
        let mut report = note(&long);
        report.title = Some("Checks finished".into());
        report.root = Some(root.clone());
        app.notifications.push(report);
        let mut target = note("The parser now accepts empty input.");
        target.title = Some("Parser ready".into());
        target.level = NotificationLevel::Success;
        target.root = Some(root.clone());
        target.open = Some(NotificationOpen {
            path: root.join("src/main.rs"),
            location: Some(crate::cli::Location { line: 3, column: 1 }),
        });
        app.notifications.push(target);

        let mut screen = harness(app);
        screen.get_by_label("2 unread agent notifications").click();
        screen.run_steps(3);
        assert!(screen.state().sidebar_view_visible(SidebarView::Messages));
        assert!(
            screen.query_by_label(&long).is_some(),
            "the view shows the whole message without an extra click"
        );
        assert_eq!(
            screen.state().notifications.unread(),
            0,
            "visible messages count as read"
        );
        assert!(screen.state().session_dirty);
        screen.state_mut().session_dirty = false;

        screen.get_by_label("Open main.rs:3").click();
        screen.run_steps(3);
        assert_eq!(
            screen.state().tab().and_then(|tab| tab.path.clone()),
            Some(root.join("src/main.rs"))
        );

        screen.get_by_label("Clear read").click();
        screen.run_steps(3);
        assert!(screen.state().notifications.items_empty());
        assert!(screen.state().session_dirty);
        assert!(
            screen
                .query_by_label(
                    "No messages yet. Agents notify you when work is finished, blocked or needs a decision."
                )
                .is_some()
        );
    }

    #[test]
    fn an_arrival_card_opens_the_target_without_taking_focus() {
        let mut app = VEditApp::empty(egui::Context::default());
        visual_tests::workspace(&mut app);
        let root = app.projects[0].root.clone();
        let response = app.control_request(crate::cli::Request::Notify {
            message: "All tests pass; the parser change is ready.".into(),
            title: Some("Parser ready".into()),
            level: NotificationLevel::Success,
            workspace: None,
            open: Some(NotificationOpen {
                path: root.join("src/main.rs"),
                location: None,
            }),
            review: None,
        });
        assert!(response.ok);
        assert_eq!(app.notifications.toast.len(), 1);
        let mut screen = harness(app);
        let focus = screen.ctx.memory(|memory| memory.focused());
        assert!(
            screen
                .query_by_label("Agent message: Parser ready")
                .is_some()
        );
        screen.get_by_label("Agent message: Parser ready").click();
        screen.run_steps(3);
        assert_eq!(
            screen.state().tab().and_then(|tab| tab.path.clone()),
            Some(root.join("src/main.rs"))
        );
        assert!(screen.state().notifications.toast.is_empty());
        assert_eq!(screen.state().notifications.unread(), 0);
        assert_eq!(
            screen.ctx.memory(|memory| memory.focused()),
            focus,
            "the card never takes keyboard focus"
        );
    }
}
