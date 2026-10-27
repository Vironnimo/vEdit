//! Project-search filters through actual text fields and result buttons.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn fixture() -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.autosave = false;
    let root = std::env::temp_dir().join("vedit-search-ui-render-only");
    let mut project = Project::new(root.clone());
    let mut tab = EditorTab::new(Some(root.join("source.rs")), "source.rs".into());
    tab.text = "first line\n🙂 needle here\nlast line\n".into();
    tab.saved_text = tab.text.clone();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app.project_query = "needle".into();
    app
}

fn harness(app: VEditApp) -> Harness<'static, VEditApp> {
    let mut screen = Harness::builder().with_size([420.0, 850.0]).build_ui_state(
        |ui, app: &mut VEditApp| {
            app.ctx = ui.ctx().clone();
            egui::CentralPanel::default().show(ui, |ui| app.project_search_ui(ui));
        },
        app,
    );
    screen
        .ctx
        .all_styles_mut(|style| style.animation_time = 0.0);
    screen.run_steps(3);
    screen
}

fn result(count: usize) -> workspace::SearchResults {
    workspace::SearchResults {
        files_searched: count,
        ..Default::default()
    }
}

fn publish(app: &mut VEditApp, generation: u64, query: &str, excludes: Vec<String>, count: usize) {
    app.sender
        .send(WorkerResult::Search {
            owner: app.project().unwrap().search_owner(),
            root: app.project().unwrap().root.clone(),
            generation,
            query: query.into(),
            excludes,
            result: Ok(result(count)),
        })
        .unwrap();
}

#[test]
fn exclusions_default_for_old_settings_and_an_explicit_empty_list_stays_empty() {
    let old: Settings = serde_json::from_str(r#"{"vim":true}"#).unwrap();
    assert_eq!(old.search_excludes, workspace::default_search_excludes());
    let mut settings = old;
    settings.search_excludes.clear();
    let saved = serde_json::to_string(&settings).unwrap();
    let restored: Settings = serde_json::from_str(&saved).unwrap();
    assert!(restored.search_excludes.is_empty());
}

#[test]
fn changed_query_or_excludes_reject_a_worker_snapshot_even_with_the_same_generation() {
    let mut app = fixture();
    app.search_generation = 31;
    let defaults = app.settings.search_excludes.clone();
    app.settings.search_excludes = vec!["generated/".into()];
    app.search_running = true;
    publish(&mut app, 31, "needle", defaults, 99);
    publish(&mut app, 31, "older query", vec!["generated/".into()], 98);
    app.poll();
    assert!(app.project_search.is_none());
    assert!(
        app.search_running,
        "old snapshots cannot stop the current search"
    );
    publish(&mut app, 31, "needle", vec!["generated/".into()], 2);
    app.poll();
    assert_eq!(app.project_search.as_ref().unwrap().files_searched, 2);
    assert!(!app.search_running);
}

#[test]
fn an_old_worker_finishing_after_the_latest_result_cannot_replace_it() {
    let mut app = fixture();
    app.search_generation = 42;
    app.search_running = true;
    let excludes = app.settings.search_excludes.clone();
    publish(&mut app, 42, "needle", excludes.clone(), 3);
    publish(&mut app, 41, "needle", excludes, 100);
    app.poll();
    assert_eq!(app.project_search.as_ref().unwrap().files_searched, 3);
    assert!(!app.search_running);
}

#[test]
fn document_changes_reject_pending_searches_and_clear_published_locations() {
    let mut app = fixture();
    app.search_generation = 42;
    app.search_running = true;
    let owner = app.project().unwrap().search_owner();
    app.search_owner = Some(owner.clone());
    app.projects[0].tabs[0].text.insert_str(0, "new line\n");
    app.projects[0].tabs[0].changed();
    app.finish_project_search(
        owner,
        app.projects[0].root.clone(),
        42,
        "needle".into(),
        app.settings.search_excludes.clone(),
        Ok(result(1)),
    );
    assert!(app.project_search.is_none());
    assert!(app.search_dirty);
    assert!(!app.search_running);

    let generation = app.search_generation;
    let excludes = app.settings.search_excludes.clone();
    publish(&mut app, generation, "needle", excludes, 2);
    app.poll();
    assert_eq!(app.project_search.as_ref().unwrap().files_searched, 2);
    // Saving, renaming and closing change result ownership even without typing.
    for change in 0..6 {
        let mut app = fixture();
        app.search_owner = Some(app.projects[0].search_owner());
        app.project_search = Some(result(1));
        match change {
            0 => app.projects[0].tabs[0].dirty = true,
            1 => app.projects[0].tabs[0].path = None,
            2 => app.projects[0].tabs.clear(),
            3 => app.projects[0].tabs[0].loading = true,
            4 => app.projects[0].tabs[0].agent_unsaved = true,
            _ => app.projects[0] = Project::new(app.projects[0].root.clone()),
        }
        app.track_project_search_documents();
        assert!(app.project_search.is_none());
        assert!(app.search_dirty);
    }
}

#[test]
fn unsaved_untitled_search_results_reveal_the_existing_document() {
    let mut app = fixture();
    app.projects[0].tabs[0].path = None;
    app.projects[0].tabs[0].dirty = true;
    let document = app.projects[0]
        .search_buffers(&workspace::SearchOptions::default())
        .remove(0)
        .document;
    let id = document.id;
    app.project_search = Some(workspace::SearchResults {
        matches: vec![workspace::SearchMatch {
            path: None,
            document: Some(document),
            line: 2,
            column: 3,
            start_char: 13,
            end_char: 19,
            preview: "🙂 needle here".into(),
        }],
        files_searched: 1,
        ..Default::default()
    });
    let mut screen = harness(app);
    assert!(screen.query_by_label("Unsaved").is_some());
    screen.get_by_label("🙂 needle here").click();
    screen.run_steps(2);
    assert_eq!(screen.state().tab().unwrap().id, id);
    assert_eq!(screen.state().tab().unwrap().vim.cursor, 13);
    assert!(screen.state().tab().unwrap().dirty);
    assert!(screen.state().tab().unwrap().path.is_none());
    assert_eq!(screen.state().projects[0].tabs.len(), 1);
}

#[test]
fn an_exclusion_error_is_shown_near_the_filters_without_retaining_old_matches() {
    let mut app = fixture();
    app.project_search = Some(result(2));
    app.search_generation = 17;
    app.search_running = true;
    app.sender
        .send(WorkerResult::Search {
            owner: app.project().unwrap().search_owner(),
            root: app.project().unwrap().root.clone(),
            generation: 17,
            query: "needle".into(),
            excludes: app.settings.search_excludes.clone(),
            result: Err("Invalid exclude rule 1: unclosed character class".into()),
        })
        .unwrap();
    app.poll();
    assert!(app.project_search.is_none());
    assert!(!app.search_running);
    let screen = harness(app);
    assert!(
        screen
            .query_by_label_contains("Invalid exclude rule 1")
            .is_some()
    );
}

#[test]
fn search_warnings_are_visible_alongside_the_completed_result_count() {
    let mut app = fixture();
    app.project_search = Some(workspace::SearchResults {
        files_searched: 3,
        warnings: vec![
            "private/config.rs: Access denied".into(),
            "archive/: Could not read this folder".into(),
        ],
        ..Default::default()
    });
    let screen = harness(app);
    assert!(
        screen
            .query_by_label("2 files or folders could not be searched")
            .is_some()
    );
    assert!(
        screen
            .query_by_label("0 matches · 3 files searched")
            .is_some()
    );
}

#[test]
fn collapsed_filters_have_a_count_and_clear_reset_cancel_search_and_persist() {
    let mut app = fixture();
    app.search_running = true;
    app.project_search = Some(result(2));
    let old_cancel = app.search_cancel.clone();
    let generation = app.search_generation;
    let count = workspace::default_search_excludes().len();
    let mut screen = harness(app);
    assert!(screen.query_by_label("Clear exclusions").is_none());
    screen
        .get_by_label(&format!("Exclusions · {count}"))
        .click();
    screen.run_steps(2);
    screen.get_by_label("Clear exclusions").click();
    screen.run_steps(2);
    let app = screen.state();
    assert!(app.settings.search_excludes.is_empty());
    assert!(app.session_dirty);
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(app.search_generation > generation);
    assert!(app.search_dirty);
    assert!(!app.search_running);
    assert!(app.project_search.is_none());
    assert!(screen.query_by_label("Exclusions · 0").is_some());
    screen.get_by_label("Reset exclusions").click();
    screen.run_steps(2);
    assert_eq!(
        screen.state().settings.search_excludes,
        workspace::default_search_excludes()
    );
    assert!(
        screen
            .query_by_label(&format!("Exclusions · {count}"))
            .is_some()
    );
}

#[test]
fn the_multiline_exclusion_field_accepts_unicode_and_path_globs_and_invalidates_results() {
    let mut app = fixture();
    app.settings.search_excludes.clear();
    app.search_running = true;
    let old_cancel = app.search_cancel.clone();
    let mut screen = harness(app);
    screen.get_by_label("Exclusions · 0").click();
    screen.run_steps(2);
    screen
        .get_by_role(egui::accesskit::Role::MultilineTextInput)
        .click();
    screen.run_steps(1);
    screen.event(egui::Event::Paste(
        "缓存/\npackages\\generated/**\n**/*.min.js".into(),
    ));
    screen.run_steps(2);
    assert_eq!(
        screen.state().settings.search_excludes,
        vec!["缓存/", "packages\\generated/**", "**/*.min.js"]
    );
    assert!(screen.state().session_dirty);
    assert!(screen.state().search_dirty);
    assert!(old_cancel.load(Ordering::Relaxed));
    let saved = serde_json::to_string(&screen.state().settings).unwrap();
    let restored: Settings = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        restored.search_excludes,
        screen.state().settings.search_excludes
    );
    assert!(screen.query_by_label("Exclusions · 3").is_some());
}

#[test]
fn editing_the_query_immediately_clears_old_matches_and_cancels_the_worker() {
    let mut app = fixture();
    app.project_search = Some(result(8));
    app.search_running = true;
    app.project_search_focus = true;
    let cancel = app.search_cancel.clone();
    let mut screen = harness(app);
    screen.event(egui::Event::Text("x".into()));
    screen.run_steps(2);
    assert!(screen.state().project_query.contains('x'));
    assert!(cancel.load(Ordering::Relaxed));
    assert!(screen.state().project_search.is_none());
    assert!(screen.state().search_dirty);
    assert!(!screen.state().search_running);
}

#[test]
fn clicking_a_search_result_uses_its_unicode_location_and_requests_centered_line_feedback() {
    let mut app = fixture();
    let path = app.tab().unwrap().path.clone().unwrap();
    app.project_search = Some(workspace::SearchResults {
        files_searched: 1,
        matches: vec![workspace::SearchMatch {
            path: Some(path),
            document: None,
            line: 2,
            column: 3,
            start_char: 13,
            end_char: 19,
            preview: "🙂 needle here".into(),
        }],
        ..Default::default()
    });
    // Render the editor as well as the result list: navigation feedback is
    // applied after the source is ready, and reveal flags are consumed there.
    let mut screen = Harness::builder()
        .with_size([1200.0, 850.0])
        .build_ui_state(
            |ui, app: &mut VEditApp| {
                app.ctx = ui.ctx().clone();
                app.syntax.begin_frame(ui);
                egui::Panel::left("search_result_test")
                    .exact_size(320.0)
                    .show(ui, |ui| app.project_search_ui(ui));
                app.editor_ui(ui);
            },
            app,
        );
    set_style(&screen.ctx);
    screen.ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    });
    screen.run_steps(3);
    screen.get_by_label("🙂 needle here").click();
    screen.run_steps(3);
    let app = screen.state();
    let tab = app.tab().unwrap();
    assert_eq!(tab.vim.cursor, 13);
    assert_eq!(tab.selection, 13..13);
    assert!(!tab.center_on_reveal);
    assert!(!tab.scroll_to_cursor);
    assert!(tab.feedback.is_some());
    assert!(!tab.pending_navigation_feedback);
    assert!(tab.pending_agent_location.is_none());
    assert!(!tab.dirty);
    assert_eq!(tab.text, "first line\n🙂 needle here\nlast line\n");
    assert_eq!(tab.saved_text, tab.text);
    assert_eq!(app.projects[0].tabs.len(), 1);
    let viewport = app.editor_rect.unwrap();
    let caret = app.editor_cursor_rect.unwrap();
    assert!(
        viewport.contains(caret.center()),
        "the Unicode result caret must be visible: viewport={viewport:?}, caret={caret:?}"
    );
}
