//! Window-wide marks stay available when a Document's Vim state is replaced.
use super::*;
use crate::vim::{GlobalMarkRequest, VimKey, VimState};

fn fixture(document: &str) -> VEditApp {
    let mut app = VEditApp::empty(egui::Context::default());
    app.settings.vim = true;
    app.settings.autosave = false;
    let mut project = Project::new(std::env::temp_dir().join("vedit-vim-file-marks"));
    let mut tab = EditorTab::new(None, "marks.txt".into());
    tab.text = document.into();
    tab.saved_text = document.into();
    project.tabs.push(tab);
    app.projects.push(project);
    app.bind_current_pane();
    app
}

#[test]
fn window_mark_survives_a_replaced_document_state() {
    let mut app = fixture("one\ntwo\nthree");
    let tab = app.tab_mut().unwrap();
    tab.vim.set_cursor(&tab.text, 8);
    app.handle_vim_file_mark(GlobalMarkRequest::Set('A'), Vec::new());
    // A reload builds a new VimState for the Document; undo restores an older
    // snapshot. Both replace the engine's copy of the mark.
    let tab = app.tab_mut().unwrap();
    tab.vim = VimState::new();
    tab.vim.set_cursor(&tab.text, 0);
    app.refresh_vim_file_marks();
    let request = {
        let tab = app.tab_mut().unwrap();
        tab.vim.handle_key(&mut tab.text, VimKey::Char('\''));
        tab.vim.handle_key(&mut tab.text, VimKey::Char('A'));
        tab.vim.take_global_mark_request()
    };
    let Some(request) = request else {
        panic!("the mark motion did not request a window mark");
    };
    app.handle_vim_file_mark(request, Vec::new());
    assert_eq!(
        app.tab().unwrap().vim.cursor,
        8,
        "the window table still resolves the mark"
    );
}
