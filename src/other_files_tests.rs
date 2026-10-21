use super::*;
use crate::cli::{DocumentView, LayoutAction, Request, SplitOrientation};
use egui_kittest::{Harness, kittest::Queryable};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "vedit-other-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
    fn app(&self) -> VEditApp {
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.recovery_session_path = self.0.join("recovery/session.json");
        app
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(fs::canonicalize(std::env::temp_dir()).unwrap())
        );
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-other-files-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn settle(app: &mut VEditApp) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll();
        if app
            .projects
            .iter()
            .all(|p| !p.indexing && p.tabs.iter().all(|t| !t.loading && !t.saving))
        {
            return;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Debug)]
struct FileDrop(PathBuf);
impl egui::DroppedFile for FileDrop {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        panic!("File reads belong to the document worker")
    }
}

fn screen(app: VEditApp) -> Harness<'static, VEditApp> {
    Harness::builder()
        .with_size([1000.0, 700.0])
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.last_session_save = Instant::now();
                app.last_disk_check = Instant::now();
                app.draw_ui(ui);
            },
            app,
        )
}

#[test]
fn loose_files_share_one_workspace_and_project_services_stay_idle() {
    let f = Fixture::new();
    let mut app = f.app();
    for name in ["one/note.py", "two/note.rs"] {
        app.open_argument(f.write(name, "hello 🦀"));
    }
    settle(&mut app);
    assert_eq!(app.projects.len(), 1);
    assert!(app.project().unwrap().is_other_files());
    assert_eq!(app.project().unwrap().tabs.len(), 2);
    app.index_project(0);
    app.refresh_git(0, true);
    app.sync_languages(true);
    app.file_watching = true;
    app.ensure_file_watcher(0);
    app.project_query = "hello".into();
    app.search_project();
    app.poll_launches();
    let p = app.project().unwrap();
    assert!(p.index.is_none() && p.explorer.is_none() && p.watcher.is_none());
    assert!(!p.indexing && !p.git_loading && !p.watcher_starting && !app.search_running);
    assert!(p.languages.is_empty() && p.launch.config_error.is_none());
    assert!(app.control_project_index(None).is_err());
    let response = app.control_request(Request::Tabs {
        limit: 100,
        offset: 0,
    });
    assert!(response.ok);
    assert!(response.result["items"][0]["workspace"].is_null());
    assert_eq!(response.result["items"][0]["workspace_kind"], "other_files");
}

#[test]
fn loose_files_close_independently_and_last_file_can_be_reopened() {
    let f = Fixture::new();
    let path = f.write("loose/a.txt", "kept");
    let project = f.write("project/main.rs", "main");
    let mut app = f.app();
    app.open_argument(path.clone());
    settle(&mut app);
    let id = app.tab().unwrap().id;
    app.add_project(project.parent().unwrap().to_owned());
    app.open_argument(project);
    settle(&mut app);
    app.perform_close(CloseAction::Project(1));
    assert_eq!(app.tab().unwrap().id, id);
    app.request_close(CloseAction::Tab(id));
    assert!(app.projects.is_empty());
    assert!(app.can_reopen_closed_tab());
    app.reopen_closed_tab();
    settle(&mut app);
    assert_eq!(app.tab().unwrap().path.as_ref(), Some(&path));
    let tab = app.tab_mut().unwrap();
    tab.apply(editor::replace_selection(&tab.text, 0..0, "unsaved "));
    let id = tab.id;
    app.request_close(CloseAction::Tab(id));
    assert!(app.close_action.is_some());
    assert!(app.tab().unwrap().dirty);
    assert!(app.tab().unwrap().text.starts_with("unsaved"));
}

#[test]
fn loose_session_restores_drafts_splits_and_selection_without_a_folder() {
    let f = Fixture::new();
    let mut app = f.app();
    app.open_argument(f.write("loose/a.md", "original\n"));
    settle(&mut app);
    let named = app.tab().unwrap().id;
    app.new_file();
    let untitled = app.tab().unwrap().id;
    for (id, text) in [(named, "changed 🦀\n"), (untitled, "draft café\n")] {
        let tab = app.tab_by_id_mut(id).unwrap();
        tab.apply(editor::replace_selection(
            &tab.text,
            0..tab.text.chars().count(),
            text,
        ));
        tab.recovery_path = f.0.join(format!("{id}.txt"));
        tab.set_cursor(3..3);
        fs::write(&tab.recovery_path, text).unwrap();
    }
    app.capture_pane_navigation();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.control_layout(LayoutAction::Assign {
        pane_id: right,
        tab_id: named,
        view: Some(DocumentView::Preview),
    })
    .unwrap();
    let saved = serde_json::to_vec(&app.session()).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains(OTHER_FILES_KEY));
    let session: Session = serde_json::from_slice(&saved).unwrap();
    let mut restored = f.app();
    restored.restore_session_workspace(session.projects, session.active_project, session.layout);
    settle(&mut restored);
    assert_eq!(restored.projects.len(), 1);
    assert!(restored.project().unwrap().is_other_files());
    assert_eq!(restored.tab_drag_pane_ids().len(), 2);
    assert!(restored.project().unwrap().tabs.iter().all(|t| t.dirty));
    assert_eq!(restored.project().unwrap().tabs[0].saved_text, "original\n");
    assert_eq!(restored.project().unwrap().tabs[1].text, "draft café\n");
    assert_eq!(restored.tab().unwrap().agent_view, DocumentView::Preview);
}

#[test]
fn explorer_drop_opens_multiple_files_and_reuses_dirty_buffers() {
    let f = Fixture::new();
    let a = f.write("one/a.txt", "a");
    let b = f.write("two/b.txt", "b");
    let mut screen = screen(f.app());
    screen.input_mut().dropped_files =
        vec![Arc::new(FileDrop(a.clone())), Arc::new(FileDrop(b.clone()))];
    screen.run_steps(1);
    let deadline = Instant::now() + Duration::from_secs(10);
    while screen
        .state()
        .projects
        .iter()
        .map(|p| p.tabs.len())
        .sum::<usize>()
        != 2
    {
        assert!(Instant::now() < deadline);
        screen.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(screen.state_mut());
    let tab = screen.state_mut().tab_mut().unwrap();
    tab.apply(editor::replace_selection(&tab.text, 0..0, "unsaved "));
    let id = tab.id;
    screen.input_mut().dropped_files = vec![Arc::new(FileDrop(b))];
    screen.run_steps(1);
    // The worker completion is asynchronous; check both its queue and result.
    for _ in 0..20 {
        screen.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(screen.state().projects.len(), 1);
    assert_eq!(screen.state().project().unwrap().tabs.len(), 2);
    assert_eq!(screen.state().tab().unwrap().id, id);
    assert_eq!(screen.state().tab().unwrap().text, "unsaved b");
    assert!(screen.query_by_label("Project: Other files").is_some());
}

#[test]
fn mixed_drop_adds_folders_before_routing_and_reports_partial_errors() {
    let f = Fixture::new();
    let inside = f.write("project/a.txt", "a");
    let outside = f.write("elsewhere/b.txt", "b");
    let mut app = f.app();
    app.finish_dropped_paths(DroppedPaths {
        target: None,
        entries: vec![
            Ok((inside.clone(), false)),
            Err("missing.txt: not found".into()),
            Ok((inside.parent().unwrap().to_owned(), true)),
            Ok((outside, false)),
        ],
    });
    settle(&mut app);
    assert_eq!(app.projects.len(), 2);
    assert_eq!(app.projects[0].tabs[0].path.as_ref(), Some(&inside));
    assert!(app.projects[1].is_other_files());
    assert!(app.status.as_ref().unwrap().0.contains("missing.txt"));
}

#[test]
fn explorer_drop_uses_the_pane_under_the_pointer() {
    let f = Fixture::new();
    let first = f.write("project/first.txt", "first");
    let next = f.write("project/next.txt", "next");
    let mut app = f.app();
    app.add_project(first.parent().unwrap().to_owned());
    app.open_argument(first);
    settle(&mut app);
    let left = app.current_pane_id();
    let right = app.split_active(SplitOrientation::Vertical).unwrap();
    app.focus_editor_pane(left).unwrap();
    let mut screen = screen(app);
    screen.run_steps(2);
    let position = screen
        .state()
        .tab_drag_pane_rects()
        .into_iter()
        .find(|(id, _)| *id == right)
        .unwrap()
        .1
        .center();
    screen
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(position));
    screen
        .input_mut()
        .dropped_files
        .push(Arc::new(FileDrop(next.clone())));
    let deadline = Instant::now() + Duration::from_secs(10);
    while screen.state().project().unwrap().tabs.len() < 2 {
        assert!(Instant::now() < deadline);
        screen.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(screen.state_mut());
    let tab = screen.state().tab().unwrap();
    assert_eq!(tab.path.as_ref(), Some(&next));
    assert_eq!(screen.state().current_pane_id(), right);
    assert!(screen.state().pane_contains_tab(right, tab.id));
    assert!(!screen.state().pane_contains_tab(left, tab.id));
}

#[test]
fn drop_during_a_modal_preserves_documents_and_reports_the_block() {
    let f = Fixture::new();
    let path = f.write("notes.txt", "notes");
    let mut app = f.app();
    app.settings_open = true;
    let mut screen = screen(app);
    screen
        .input_mut()
        .dropped_files
        .push(Arc::new(FileDrop(path)));
    screen.run_steps(1);
    assert!(screen.state().projects.is_empty());
    assert!(screen.state().status.as_ref().unwrap().0.contains("dialog"));
}

#[test]
fn delayed_drop_preserves_a_subsequent_project_switch() {
    let f = Fixture::new();
    let a = f.write("A/a.txt", "a");
    let b = f.write("B/b.txt", "b");
    let loose = f.write("loose/c.txt", "c");
    let mut app = f.app();
    app.add_project(a.parent().unwrap().to_owned());
    app.open_argument(a);
    let target = Some((app.project().unwrap().instance_id, app.current_pane_id()));
    app.add_project(b.parent().unwrap().to_owned());
    app.open_argument(b);
    settle(&mut app);
    let current = app.tab().unwrap().id;
    app.finish_dropped_paths(DroppedPaths {
        target,
        entries: vec![Ok((loose, false))],
    });
    settle(&mut app);
    assert_eq!(app.tab().unwrap().id, current);
    assert!(app.projects.last().unwrap().is_other_files());
    assert_eq!(app.projects.last().unwrap().tabs.len(), 1);
}

#[test]
fn relative_external_file_is_routed_after_resolving_its_path() {
    let f = Fixture::new();
    let project = f.write("project/main.rs", "main");
    let loose = f.write("loose/note.txt", "notes");
    let root = project.parent().unwrap();
    let mut app = f.app();
    app.add_project(root.to_owned());
    app.open_routed_file(root.join("../loose/note.txt"));
    settle(&mut app);
    assert!(app.project().unwrap().is_other_files());
    assert_eq!(app.tab().unwrap().path.as_ref(), Some(&loose));
    assert!(app.projects[0].tabs.is_empty());
}

#[test]
fn opening_a_parent_folder_keeps_the_existing_dirty_document() {
    let f = Fixture::new();
    let path = f.write("project/notes.txt", "original");
    let mut app = f.app();
    app.open_argument(path.clone());
    settle(&mut app);
    let tab = app.tab_mut().unwrap();
    tab.apply(editor::replace_selection(&tab.text, 0..0, "draft "));
    let id = tab.id;
    app.add_project(path.parent().unwrap().to_owned());
    app.open_explorer_file(path, false);
    assert!(app.project().unwrap().is_other_files());
    assert_eq!(app.tab().unwrap().id, id);
    assert_eq!(app.tab().unwrap().text, "draft original");
    assert!(app.projects[1].tabs.is_empty());
}

#[test]
fn other_files_sidebar_keeps_the_projects_selected_panel() {
    let f = Fixture::new();
    let project = f.write("project/main.rs", "main");
    let mut app = f.app();
    app.add_project(project.parent().unwrap().to_owned());
    app.open_argument(project);
    app.show_sidebar(SidebarView::Search);
    assert!(app.sidebar_view_visible(SidebarView::Search));
    app.open_argument(f.write("loose/notes.txt", "notes"));
    assert!(app.sidebar_view_visible(SidebarView::Explorer));
    assert!(!app.sidebar_view_visible(SidebarView::Search));
    app.switch_project(0);
    assert!(app.sidebar_view_visible(SidebarView::Search));
}

#[test]
fn delayed_drop_does_not_reopen_a_closed_workspace() {
    let f = Fixture::new();
    let a = f.write("project/a.txt", "a");
    let b = f.write("project/b.txt", "b");
    let mut app = f.app();
    app.add_project(a.parent().unwrap().to_owned());
    app.open_argument(a);
    settle(&mut app);
    let target = Some((app.project().unwrap().instance_id, app.current_pane_id()));
    app.perform_close(CloseAction::Project(0));
    app.add_project(b.parent().unwrap().to_owned());
    app.finish_dropped_paths(DroppedPaths {
        target,
        entries: vec![Ok((b, false))],
    });
    assert_eq!(app.projects.len(), 1);
    assert!(app.project().unwrap().tabs.is_empty());
    assert!(app.status.as_ref().unwrap().0.contains("closed"));
}

#[test]
fn other_files_visual_layout() {
    let mut app = VEditApp::empty(egui::Context::default());
    app.projects
        .push(Project::new(PathBuf::from(r"C:\Code\vEdit")));
    let index = app.ensure_other_files();
    for (path, text) in [
        (
            r"C:\Downloads\notes.md",
            "# Notes\n\nA file opened alongside your projects.\n",
        ),
        (r"D:\Logs\server.log", "Server started\n"),
    ] {
        let mut tab = EditorTab::new(Some(PathBuf::from(path)), file_title(Path::new(path)));
        tab.text = text.into();
        tab.saved_text = text.into();
        app.projects[index].tabs.push(tab);
    }
    app.activate_tab(index, 0);
    let mut screen = super::super::visual_tests::harness(app);
    super::super::visual_tests::snapshot(&mut screen, "vedit_other_files");
}
