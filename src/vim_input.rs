//! Vim input integration and command-line presentation.
use super::*;

impl VEditApp {
    pub(super) fn handle_vim(&mut self) {
        self.resume_editor_input();
        self.validate_vim_search();
        if !self.settings.vim
            || self.tab().is_some_and(|tab| tab.vim.multi.active())
            || egui::Popup::is_any_open(&self.ctx)
            || self
                .tab()
                .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || self.status_details.is_some()
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.vim_command.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.context_file_dialog.is_some()
        {
            return;
        }
        let ctx = self.ctx.clone();
        if self.vim_search_active() && ctx.input(|input| input.pointer.any_pressed()) {
            self.cancel_vim_search(false);
        }
        let Some(tab) = self.tab() else {
            return;
        };
        let id = self.document_widget_id(tab.id);
        if !ctx.memory(|memory| memory.has_focus(id)) {
            return;
        }
        if ctx.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.interact_pos().is_some_and(|position| {
                    !self.editor_rect.is_some_and(|rect| rect.contains(position))
                })
        }) {
            return;
        }

        fn apply_action(
            tab: &mut EditorTab,
            action: crate::vim::Action,
            vim_register: &mut Option<Yank>,
        ) {
            if action.text_changed {
                tab.changed();
            }
            if action.yanked.is_some() {
                if !action.text_changed {
                    if action.yanked.as_ref().is_some_and(|yank| yank.linewise) {
                        tab.flash_line_yank(action.yanked_ranges);
                    } else {
                        tab.flash_yank(action.yanked_ranges);
                    }
                }
                *vim_register = action.yanked;
            }
            if action.scroll.is_some() {
                tab.scroll_align = action.scroll;
            }
            if action.message.is_some() {
                tab.vim_message = action.message;
            }
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or(tab.vim.cursor..tab.vim.cursor);
            tab.selection = selection.clone();
            tab.pending_selection = Some(selection);
            tab.scroll_to_cursor = true;
        }
        fn copy_to_clipboard(tab: &mut EditorTab, system_clipboard: &mut Option<String>) {
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or_else(|| tab.selection.clone());
            let linewise = selection.is_empty() || tab.vim.mode == VimMode::VisualLine;
            let selection = crate::vim::clipboard_range(&tab.text, selection, tab.vim.cursor);
            if !selection.is_empty() {
                *system_clipboard = Some(
                    tab.text
                        .chars()
                        .skip(selection.start)
                        .take(selection.len())
                        .collect(),
                );
                if linewise {
                    tab.flash_line_yank(vec![selection]);
                } else {
                    tab.flash_yank(vec![selection]);
                }
            }
            // Native Copy keeps the mode and selection, but cannot leave an
            // unfinished Vim delete armed for the next navigation key.
            tab.vim.cancel_pending_command();
        }
        fn cut_selection(
            tab: &mut EditorTab,
            system_clipboard: &mut Option<String>,
            vim_register: &mut Option<Yank>,
        ) {
            if tab.loading {
                return;
            }
            if matches!(tab.vim.mode, VimMode::Visual | VimMode::VisualLine) {
                let (action, copied) = tab.vim.cut_visual_to_clipboard(&mut tab.text);
                *system_clipboard = copied;
                apply_action(tab, action, vim_register);
                return;
            }
            let selection =
                crate::vim::clipboard_range(&tab.text, tab.selection.clone(), tab.vim.cursor);
            if selection.is_empty() {
                tab.vim.cancel_pending_command();
                return;
            }
            *system_clipboard = Some(
                tab.text
                    .chars()
                    .skip(selection.start)
                    .take(selection.len())
                    .collect(),
            );
            let edit = editor::replace_selection(&tab.text, selection, "");
            tab.apply(edit);
            tab.set_cursor(tab.selection.start..tab.selection.start);
            tab.vim.notify_external_text_change();
        }
        fn paste_system_text(tab: &mut EditorTab, text: &str) {
            if tab.loading || text.is_empty() {
                return;
            }
            let selection = tab
                .vim
                .selection(&tab.text)
                .unwrap_or_else(|| tab.selection.clone());
            let edit = editor::replace_selection(&tab.text, selection, text);
            let cursor = edit.selection.start;
            tab.apply(edit);
            if matches!(tab.vim.mode, VimMode::Visual | VimMode::VisualLine) {
                tab.vim.set_selection(&tab.text, cursor..cursor);
            }
            tab.set_cursor(cursor..cursor);
            tab.vim.notify_external_text_change();
        }
        fn publish_search_yank(
            tab: &mut EditorTab,
            find: &mut find::FindState,
            vim_register: &mut Option<Yank>,
        ) {
            if let Some(yank) = find.take_yank() {
                tab.vim.set_register(yank.clone());
                *vim_register = Some(yank);
            }
        }

        let events = ctx.input(|input| input.events.clone());
        let mut retained = Vec::with_capacity(events.len());
        let mut native_insert = Vec::new();
        let mut prompt: Option<char> = None;
        let mut prompt_text = String::new();
        let mut vim_searched = false;
        let mut definition = false;
        let mut references = false;
        let mut hover = false;
        // `[d` / `]d` and Ctrl+O / Ctrl+I: true for previous / back.
        let mut problem = None;
        let mut history = None;
        let mut vim_register = None;
        let mut system_clipboard = None;
        let swap_jk = self.settings.swap_jk;
        let event_time = ctx.input(|input| input.time);
        let native_copy = events
            .iter()
            .any(|event| matches!(event, egui::Event::Copy));
        let native_cut = events.iter().any(|event| matches!(event, egui::Event::Cut));
        let native_paste = events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_)));
        let register = self.register.clone();
        let mut yank_register = self.yank_register.clone();
        let keybindings = self.settings.keybindings.clone();
        let indent = self.tab().map(|tab| tab.indentation(&self.settings));
        let page = self.editor_rect.map_or(24, |rect| {
            (rect.height() / (self.settings.font_size * self.settings.line_height))
                .floor()
                .max(1.0) as isize
        });
        let pane_id = self.current_pane_id();
        let mut find = std::mem::take(&mut self.find);
        if let Some(tab) = self.tab_mut() {
            if let Some(indent) = indent {
                tab.vim.set_indentation(indent.width, indent.tabs);
            }
            if let Some(register) = register {
                tab.vim.set_register(register);
            }
            tab.vim.set_yank_register(yank_register.clone());
            let mut events = events.into_iter().peekable();
            while let Some(event) = events.next() {
                if tab.vim.multi.active() {
                    tab.sync_multicursor();
                    editor_input::defer(
                        &ctx,
                        tab.id,
                        std::iter::once(event).chain(events).collect(),
                    );
                    break;
                }
                if tab.vim.mode == VimMode::Insert
                    && editor_input::vim_barrier(&event, &keybindings)
                    && native_insert.iter().any(editor_input::native_input)
                {
                    editor_input::defer(
                        &ctx,
                        tab.id,
                        std::iter::once(event).chain(events).collect(),
                    );
                    break;
                }
                find.observe_escape(&event, event_time);
                if find.prompt.is_some() || find.waiting() {
                    match event {
                        egui::Event::Text(text)
                        | egui::Event::Paste(text)
                        | egui::Event::Ime(egui::ImeEvent::Commit(text)) => find.text(tab, &text),
                        egui::Event::Copy => find.key(tab, Key::C, egui::Modifiers::CTRL),
                        egui::Event::Cut | egui::Event::Ime(_) => {}
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => find.key(tab, key, modifiers),
                        other => retained.push(other),
                    }
                    publish_search_yank(tab, &mut find, &mut vim_register);
                    continue;
                }
                if prompt.is_some() {
                    match event {
                        boundary @ egui::Event::Key {
                            key: Key::Enter | Key::Escape,
                            pressed: true,
                            modifiers,
                            ..
                        } if !modifiers.ctrl
                            && !modifiers.command
                            && !modifiers.alt
                            && !modifiers.shift =>
                        {
                            retained.push(boundary);
                            retained.extend(events);
                            break;
                        }
                        egui::Event::Text(text) => prompt_text.push_str(&text),
                        egui::Event::Paste(text) => prompt_text.push_str(&text),
                        egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            prompt_text.push_str(&text)
                        }
                        egui::Event::Copy => {
                            prompt = None;
                            prompt_text.clear();
                            copy_to_clipboard(tab, &mut system_clipboard);
                        }
                        egui::Event::Cut | egui::Event::Ime(_) => {}
                        egui::Event::Key {
                            key: Key::Backspace,
                            pressed: true,
                            modifiers,
                            ..
                        } if !modifiers.command && !modifiers.ctrl && !modifiers.alt => {
                            prompt_text.pop();
                        }
                        egui::Event::Key {
                            pressed: true, key, ..
                        } if !matches!(key, Key::Enter | Key::Escape) => {}
                        other => retained.push(other),
                    }
                    continue;
                }
                if tab.vim.mode != VimMode::Insert {
                    let command = keybindings::Action::ALL.iter().find_map(|&action| {
                        let command = action.editor_command()?;
                        (!matches!(
                            command,
                            editor_input::Command::Undo | editor_input::Command::Redo
                        ) && keybindings::source_matches(
                            &keybindings,
                            action,
                            &event,
                            true,
                            false,
                            false,
                        ))
                        .then_some(command)
                    });
                    if let Some(command) = command {
                        if events
                            .peek()
                            .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                        {
                            events.next();
                        }
                        let selection = tab
                            .vim
                            .selection(&tab.text)
                            .unwrap_or_else(|| tab.selection.clone());
                        let edit = editor_input::edit(
                            command,
                            &tab.text,
                            selection,
                            indent.unwrap(),
                            crate::file_kind::comment_language(tab.path.as_deref()),
                        )
                        .unwrap();
                        if matches!(
                            command,
                            editor_input::Command::MoveLine(_) | editor_input::Command::Duplicate
                        ) {
                            tab.apply_line_move(edit);
                        } else if edit.text != tab.text {
                            tab.apply(edit);
                            if matches!(
                                command,
                                editor_input::Command::DeleteLine
                                    | editor_input::Command::InsertLine(_)
                                    | editor_input::Command::Newline
                            ) {
                                tab.set_cursor(tab.selection.clone());
                            }
                            tab.vim.notify_external_text_change();
                        }
                        editor_input::defer(&ctx, tab.id, events.collect());
                        break;
                    }
                }
                match &event {
                    event
                        if keybindings::matches(&keybindings, keybindings::Action::Undo, event)
                            || keybindings::matches(
                                &keybindings,
                                keybindings::Action::Redo,
                                event,
                            ) =>
                    {
                        if events
                            .peek()
                            .is_some_and(|next| keybindings::shortcut_text_event(next, event))
                        {
                            events.next();
                        }
                        let redo =
                            keybindings::matches(&keybindings, keybindings::Action::Redo, event);
                        let action = tab.vim.history_edit(&mut tab.text, redo);
                        apply_action(tab, action, &mut vim_register);
                        editor_input::defer(&ctx, tab.id, events.collect());
                        break;
                    }
                    egui::Event::Copy => {
                        copy_to_clipboard(tab, &mut system_clipboard);
                        continue;
                    }
                    egui::Event::Cut => {
                        cut_selection(tab, &mut system_clipboard, &mut vim_register);
                        continue;
                    }
                    egui::Event::Paste(text) => {
                        paste_system_text(tab, text);
                        continue;
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(text))
                        if tab.vim.mode == VimMode::Insert =>
                    {
                        // Composition previews remain separate from source;
                        // committed text uses the same native event chronology.
                        native_insert.push(egui::Event::Text(text.clone()));
                        continue;
                    }
                    egui::Event::Ime(_) => continue,
                    egui::Event::Key {
                        key: Key::C,
                        pressed: true,
                        modifiers,
                        ..
                    } if (modifiers.ctrl || modifiers.command) && !modifiers.alt => {
                        if !native_copy {
                            copy_to_clipboard(tab, &mut system_clipboard);
                        }
                        continue;
                    }
                    egui::Event::Key {
                        key: Key::X,
                        pressed: true,
                        modifiers,
                        ..
                    } if (modifiers.ctrl || modifiers.command) && !modifiers.alt => {
                        if !native_cut {
                            cut_selection(tab, &mut system_clipboard, &mut vim_register);
                        }
                        continue;
                    }
                    egui::Event::Key {
                        key: Key::V,
                        pressed: true,
                        modifiers,
                        ..
                    } if (modifiers.ctrl || modifiers.command) && !modifiers.alt => {
                        if !native_paste {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                        continue;
                    }
                    _ => {}
                }
                if tab.vim.mode == VimMode::Insert {
                    if matches!(
                        &event,
                        egui::Event::Key {
                            key: Key::Escape,
                            pressed: true,
                            ..
                        }
                    ) {
                        let action = tab.vim.handle_key(&mut tab.text, VimKey::Escape);
                        apply_action(tab, action, &mut vim_register);
                    } else {
                        native_insert.push(event);
                    }
                    continue;
                }
                // Vim's Ctrl+A / Ctrl+X: Ctrl+A and Ctrl+X stay Select All and
                // Cut, so Alt+A and Alt+X add to or subtract from a number,
                // unless the user bound those keys to a command.
                let number_step = match &event {
                    egui::Event::Key {
                        key: key @ (Key::A | Key::X),
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt
                        && !modifiers.ctrl
                        && !modifiers.command
                        && !modifiers.shift
                        && tab.vim.mode == VimMode::Normal
                        && !keybindings::Action::ALL.iter().any(|action| {
                            keybindings::source_matches(
                                &keybindings,
                                *action,
                                &event,
                                true,
                                false,
                                false,
                            )
                        }) =>
                    {
                        Some(if *key == Key::A {
                            VimKey::Increment
                        } else {
                            VimKey::Decrement
                        })
                    }
                    _ => None,
                };
                if let Some(key) = number_step {
                    if events
                        .peek()
                        .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                    {
                        events.next();
                    }
                    let action = tab.vim.handle_key(&mut tab.text, key);
                    apply_action(tab, action, &mut vim_register);
                    continue;
                }
                match event {
                    egui::Event::Text(text) => {
                        let mut characters = text.chars().peekable();
                        while let Some(ch) = characters.next() {
                            if find.prompt.is_some() || find.waiting() {
                                let remaining: String =
                                    std::iter::once(ch).chain(characters).collect();
                                find.text(tab, &remaining);
                                break;
                            }
                            if tab.vim.mode == VimMode::Insert {
                                let remaining: String =
                                    std::iter::once(ch).chain(characters).collect();
                                native_insert.push(egui::Event::Text(remaining));
                                break;
                            }
                            if !tab.vim.expects_literal_character() {
                                if matches!(ch, '/' | '?') {
                                    vim_searched = true;
                                    find.start(tab, pane_id, ch == '?');
                                    continue;
                                }
                                if ch == ':' {
                                    tab.vim.cancel_pending_command();
                                    prompt = Some(ch);
                                    prompt_text.extend(characters);
                                    break;
                                }
                                if matches!(ch, 'n' | 'N') {
                                    vim_searched = true;
                                    find.repeat(tab, ch == 'N');
                                    publish_search_yank(tab, &mut find, &mut vim_register);
                                    continue;
                                }
                                if matches!(ch, '*' | '#') {
                                    vim_searched = true;
                                    find.word(tab, ch == '#');
                                    publish_search_yank(tab, &mut find, &mut vim_register);
                                    continue;
                                }
                                if ch == 'd'
                                    && tab.vim.mode == VimMode::Normal
                                    && tab.vim.pending_command() == "g"
                                {
                                    tab.vim.cancel_pending_command();
                                    definition = true;
                                    continue;
                                }
                                if ch == 'r'
                                    && tab.vim.mode == VimMode::Normal
                                    && tab.vim.pending_command() == "g"
                                {
                                    tab.vim.cancel_pending_command();
                                    references = true;
                                    continue;
                                }
                                if ch == 'K'
                                    && tab.vim.mode == VimMode::Normal
                                    && tab.vim.pending_command().is_empty()
                                {
                                    hover = true;
                                    continue;
                                }
                            }
                            if ch == 'd' && tab.vim.mode == VimMode::Normal {
                                let pending = tab.vim.pending_command();
                                if pending == "[" || pending == "]" {
                                    tab.vim.cancel_pending_command();
                                    problem = Some(pending == "[");
                                    continue;
                                }
                            }
                            let ch = if tab.vim.expects_literal_character() {
                                ch
                            } else {
                                crate::vim::map_jk(ch, swap_jk)
                            };
                            let action = tab.vim.handle_key(&mut tab.text, VimKey::Char(ch));
                            apply_action(tab, action, &mut vim_register);
                        }
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let translated = if modifiers.ctrl
                            && !modifiers.alt
                            && !modifiers.shift
                            && matches!(key, Key::O | Key::I)
                            && tab.vim.mode == VimMode::Normal
                        {
                            // Vim's jump list. Ctrl+O stays Open file in
                            // every other mode and when it is rebound.
                            tab.vim.cancel_pending_command();
                            history = Some(key == Key::O);
                            None
                        } else if modifiers.ctrl && !modifiers.alt && key == Key::R {
                            Some(VimKey::Redo)
                        } else if modifiers.ctrl
                            && !modifiers.alt
                            && matches!(key, Key::F | Key::B | Key::D | Key::U)
                        {
                            let amount = if matches!(key, Key::D | Key::U) {
                                (page / 2).max(1)
                            } else {
                                page
                            };
                            let action = tab.vim.scroll_lines(
                                &mut tab.text,
                                if matches!(key, Key::B | Key::U) {
                                    -amount
                                } else {
                                    amount
                                },
                            );
                            apply_action(tab, action, &mut vim_register);
                            None
                        } else if !modifiers.command && !modifiers.ctrl && !modifiers.alt {
                            match key {
                                Key::Escape => Some(VimKey::Escape),
                                Key::Enter => Some(VimKey::Enter),
                                Key::Tab if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Tab)
                                }
                                Key::Backspace if tab.vim.expects_literal_character() => {
                                    Some(VimKey::Backspace)
                                }
                                Key::ArrowLeft | Key::Backspace => Some(VimKey::Char('h')),
                                Key::ArrowDown => Some(VimKey::Char('j')),
                                Key::ArrowUp => Some(VimKey::Char('k')),
                                Key::ArrowRight => Some(VimKey::Char('l')),
                                Key::Home => Some(VimKey::Char('0')),
                                Key::End => Some(VimKey::Char('$')),
                                _ => None,
                            }
                        } else {
                            retained.push(egui::Event::Key {
                                key,
                                pressed: true,
                                repeat: false,
                                physical_key: None,
                                modifiers,
                            });
                            None
                        };
                        if let Some(key) = translated {
                            let action = tab.vim.handle_key(&mut tab.text, key);
                            apply_action(tab, action, &mut vim_register);
                        }
                    }
                    egui::Event::Paste(_) => {}
                    other => retained.push(other),
                }
            }
            yank_register = tab.vim.yank_register().cloned();
        }
        self.find = find;
        if vim_searched {
            self.find_visible = false;
        }
        self.find_message();
        if let Some(message) = self.tab_mut().and_then(|tab| tab.vim_message.take()) {
            self.status(message, true);
        }
        retained.extend(native_insert);
        ctx.input_mut(|input| input.events = retained);
        if let Some(yank) = vim_register {
            self.register = Some(yank);
        }
        self.yank_register = yank_register;
        if let Some(text) = system_clipboard {
            ctx.copy_text(text);
        }
        if let Some(':') = prompt {
            self.begin_ex_command(prompt_text);
        }
        if definition {
            self.request_definition();
        }
        if references {
            self.find_references();
        }
        if hover {
            self.show_hover();
        }
        if let Some(backwards) = problem {
            self.go_to_problem(backwards);
        }
        if let Some(backwards) = history {
            self.navigate_history(backwards);
        }
    }
    pub(super) fn ex_ui(&mut self, ui: &mut egui::Ui) {
        let mut run = None;
        let mut cancel = false;
        let mut changed = false;
        self.ex_input_prefix();
        self.ex_history_key(ui);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(":").monospace().color(ACCENT));
            if let Some(command) = &mut self.vim_command {
                let response = ui.add(
                    egui::TextEdit::singleline(command)
                        .id(egui::Id::new("ex-command"))
                        .font(FontId::monospace(13.0))
                        .hint_text("w · q · bn/bp · split · %s/old/new/g · ↑↓ history")
                        .desired_width(ui.available_width()),
                );
                response.request_focus();
                changed |= response.changed();
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    run = Some(command.clone());
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                    cancel = true;
                }
            }
        });
        if changed {
            self.ex_command_changed();
        }
        let finished = cancel || run.is_some();
        if cancel || run.is_some() {
            self.vim_command = None;
            if let Some(t) = self.tab_mut() {
                t.focus = true;
            }
        }
        if let Some(command) = run {
            self.execute_ex_command(&command);
        }
        if finished {
            self.finish_ex_command();
        }
    }
}
