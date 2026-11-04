//! Vim input integration and command-line presentation.
use super::*;

impl VEditApp {
    pub(super) fn handle_vim(&mut self) {
        self.resume_editor_input();
        self.validate_vim_search();
        if !self.settings.vim
            || self.change_review_visible()
            || self.tab().is_some_and(|tab| tab.vim.multi.active())
            || egui::Popup::is_any_open(&self.ctx)
            || self
                .tab()
                .is_some_and(|t| t.agent_view == crate::cli::DocumentView::Preview)
            || self.palette.is_some()
            || self.picker.is_some()
            || self.settings_open
            || (self.status_details.is_some() || self.vim_reference_open())
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
        self.checkpoint_vim_macro_document();
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
            if tab.vim.mode == VimMode::VisualBlock {
                if let Some(yank) = tab.vim.block_yank(&tab.text) {
                    *system_clipboard = Some(yank.text);
                    tab.flash_yank(tab.vim.block_selection(&tab.text).unwrap());
                } else {
                    tab.vim_message =
                        Some("Block selection exceeds the clipboard size limit".into());
                }
                tab.vim.cancel_pending_command();
                return;
            }
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
            if matches!(
                tab.vim.mode,
                VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
            ) {
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
            tab.vim.notify_external_text_change(&tab.text);
        }
        fn paste_system_text(tab: &mut EditorTab, text: &str) {
            if tab.loading || text.is_empty() {
                return;
            }
            if tab.vim.is_replacing() {
                let action = tab.vim.insert_text(&mut tab.text, text);
                apply_action(tab, action, &mut None);
                return;
            }
            if tab.vim.mode == VimMode::VisualBlock {
                let action = tab.vim.paste_block_clipboard(&mut tab.text, text);
                apply_action(tab, action, &mut None);
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
            tab.vim.notify_external_text_change(&tab.text);
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
        let mut macros = std::mem::take(&mut self.vim_macros);
        let mut retained = Vec::with_capacity(events.len());
        let mut native_insert = Vec::new();
        let mut prompt: Option<char> = None;
        let mut prompt_text = String::new();
        let mut vim_searched = false;
        let mut file_mark_request = None;
        let mut definition = false;
        let mut references = false;
        let mut hover = false;
        // `[d` / `]d` and Ctrl+O / Ctrl+I: true for previous / back.
        let mut problem = None;
        let mut history = None;
        let mut completion = None;
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
        let mut register_bank = self.vim_registers.clone();
        let keybindings = self.settings.keybindings.clone();
        let vim_keys = self.settings.vim_keys.clone();
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
            tab.vim.set_register_bank(register_bank.clone());
            let mut events = events.into_iter().peekable();
            'vim_events: while let Some(event) = events.next() {
                // The backend preserves the chord before its clipboard event.
                // A bare semantic event belongs to native clipboard handling,
                // including menus and Ctrl+Insert / Shift+Insert / Shift+Delete.
                let control_key = keybindings::vim_control_key(&event);
                let forwarded = control_key.filter(|key| {
                    vim_keys.target(
                        *key,
                        tab.vim.mode,
                        keybindings::custom_source_binding(&keybindings, &event),
                    ) == Some(keybindings::vim_keys::Target::Vim)
                });
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
                    && (forwarded.is_some() || editor_input::vim_barrier(&event, &keybindings))
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
                if matches!(
                    &event,
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        ..
                    }
                ) && macros.cancel_prefix()
                {
                    continue;
                }
                if find.prompt.is_some() || find.waiting() {
                    macros.record_event(&event);
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
                    if !matches!(
                        &event,
                        egui::Event::Key {
                            key: Key::Enter | Key::Escape,
                            pressed: true,
                            ..
                        }
                    ) {
                        macros.record_event(&event);
                    }
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
                if let Some(key) = forwarded {
                    macros.record_event(&event);
                    if matches!(&event, egui::Event::Key { .. })
                        && events.peek().is_some_and(|next| {
                            matches!(
                                (key, next),
                                (Key::C, egui::Event::Copy)
                                    | (Key::X, egui::Event::Cut)
                                    | (Key::V, egui::Event::Paste(_))
                            )
                        })
                    {
                        events.next();
                    }
                    let translated = match key {
                        Key::C => Some(VimKey::Escape),
                        Key::A => Some(VimKey::Increment),
                        Key::X => Some(VimKey::Decrement),
                        Key::V | Key::Q => Some(VimKey::VisualBlock),
                        Key::R if tab.vim.mode == VimMode::Insert => {
                            let action = tab.vim.start_insert_register(&mut tab.text);
                            apply_action(tab, action, &mut vim_register);
                            None
                        }
                        Key::R => Some(VimKey::Redo),
                        Key::O | Key::I => {
                            tab.vim.cancel_pending_command();
                            history = Some(key == Key::O);
                            editor_input::defer(&ctx, tab.id, events.collect());
                            break;
                        }
                        Key::N | Key::P => {
                            completion = Some(key == Key::P);
                            editor_input::defer(&ctx, tab.id, events.collect());
                            break;
                        }
                        Key::F | Key::B | Key::D | Key::U => {
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
                        }
                        _ => None,
                    };
                    if let Some(key) = translated {
                        let action = tab.vim.handle_key(&mut tab.text, key);
                        apply_action(tab, action, &mut vim_register);
                    }
                    continue;
                }
                if tab.vim.insert_register_pending() {
                    macros.record_event(&event);
                    match event {
                        egui::Event::Text(text)
                        | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            let mut characters = text.chars();
                            if let Some(name) = characters.next() {
                                let action = tab.vim.handle_key(&mut tab.text, VimKey::Char(name));
                                apply_action(tab, action, &mut vim_register);
                                let remaining: String = characters.collect();
                                if !remaining.is_empty() {
                                    if tab.vim.is_replacing() {
                                        let action = tab.vim.insert_text(&mut tab.text, &remaining);
                                        apply_action(tab, action, &mut vim_register);
                                    } else {
                                        native_insert.push(egui::Event::Text(remaining));
                                    }
                                }
                            }
                        }
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } if !modifiers.ctrl
                            && !modifiers.command
                            && !modifiers.alt
                            && matches!(key, Key::Escape | Key::Backspace | Key::Enter) =>
                        {
                            let translated = match key {
                                Key::Escape => VimKey::Escape,
                                Key::Backspace => VimKey::Backspace,
                                _ => VimKey::Enter,
                            };
                            let action = tab.vim.handle_key(&mut tab.text, translated);
                            apply_action(tab, action, &mut vim_register);
                        }
                        _ => {}
                    }
                    continue;
                }
                // A mode change earlier in this same batch can hand a chord
                // back to the editor. Resume it through the normal dispatcher
                // with the resulting mode instead of dropping it in TextEdit.
                if keybindings::Action::ALL.iter().any(|&action| {
                    action.editor_command().is_none()
                        && keybindings::source_matches(
                            &keybindings,
                            action,
                            &event,
                            tab.vim.mode != VimMode::Insert,
                            false,
                            false,
                        )
                }) {
                    editor_input::defer(
                        &ctx,
                        tab.id,
                        std::iter::once(event).chain(events).collect(),
                    );
                    break;
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
                        macros.record_event(&event);
                        if events
                            .peek()
                            .is_some_and(|next| keybindings::shortcut_text_event(next, &event))
                        {
                            events.next();
                        }
                        let selection = if command == editor_input::Command::Newline {
                            tab.vim
                                .selection(&tab.text)
                                .unwrap_or_else(|| tab.selection.clone())
                        } else {
                            tab.vim_editor_selection()
                        };
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
                        } else if edit.text != tab.text
                            && matches!(
                                command,
                                editor_input::Command::Comment | editor_input::Command::Indent(_)
                            )
                        {
                            tab.apply_line_edit(edit);
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
                            tab.vim.notify_external_text_change(&tab.text);
                        }
                        editor_input::defer(&ctx, tab.id, events.collect());
                        break;
                    }
                }
                if !matches!(&event, egui::Event::Text(_)) || tab.vim.mode == VimMode::Insert {
                    let duplicate_clipboard = matches!(&event,
                        egui::Event::Key { key: Key::C, modifiers, .. } if (modifiers.ctrl || modifiers.command) && native_copy)
                        || matches!(&event, egui::Event::Key { key: Key::X, modifiers, .. } if (modifiers.ctrl || modifiers.command) && native_cut)
                        || matches!(&event, egui::Event::Key { key: Key::V, modifiers, .. } if (modifiers.ctrl || modifiers.command) && native_paste);
                    if !duplicate_clipboard {
                        macros.record_event(&event);
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
                    // Replace applies text itself and has no native composition.
                    // Insert hands every IME event to the source widget, so egui
                    // draws the composition and its candidate window.
                    egui::Event::Ime(egui::ImeEvent::Commit(text))
                        if tab.vim.mode == VimMode::Insert && tab.vim.is_replacing() =>
                    {
                        let action = tab.vim.insert_text(&mut tab.text, text);
                        apply_action(tab, action, &mut vim_register);
                        continue;
                    }
                    egui::Event::Ime(_)
                        if tab.vim.mode == VimMode::Insert && !tab.vim.is_replacing() => {}
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
                    if tab.vim.is_replacing() {
                        match event {
                            egui::Event::Text(text) => {
                                let action = tab.vim.insert_text(&mut tab.text, &text);
                                apply_action(tab, action, &mut vim_register);
                            }
                            egui::Event::Key {
                                key,
                                pressed: true,
                                modifiers,
                                ..
                            } if !modifiers.ctrl && !modifiers.command && !modifiers.alt => {
                                let key = match key {
                                    Key::Escape => Some(VimKey::Escape),
                                    Key::Enter => Some(VimKey::Enter),
                                    Key::Tab => Some(VimKey::Tab),
                                    Key::Backspace => Some(VimKey::Backspace),
                                    Key::ArrowLeft => Some(VimKey::Left),
                                    Key::ArrowRight => Some(VimKey::Right),
                                    Key::ArrowUp => Some(VimKey::Up),
                                    Key::ArrowDown => Some(VimKey::Down),
                                    Key::Home => Some(VimKey::Home),
                                    Key::End => Some(VimKey::End),
                                    Key::Delete => Some(VimKey::Delete),
                                    _ => None,
                                };
                                if let Some(key) = key {
                                    let action = tab.vim.handle_key(&mut tab.text, key);
                                    apply_action(tab, action, &mut vim_register);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
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
                                macros.record_event(&egui::Event::Text(remaining.clone()));
                                find.text(tab, &remaining);
                                break;
                            }
                            if tab.vim.mode == VimMode::Insert {
                                let remaining: String =
                                    std::iter::once(ch).chain(characters).collect();
                                macros.record_event(&egui::Event::Text(remaining.clone()));
                                if tab.vim.is_replacing() {
                                    let action = tab.vim.insert_text(&mut tab.text, &remaining);
                                    apply_action(tab, action, &mut vim_register);
                                } else {
                                    native_insert.push(egui::Event::Text(remaining));
                                }
                                break;
                            }
                            match macros.character(&mut tab.vim, ch) {
                                vim_macros::Input::Consumed => continue,
                                vim_macros::Input::Play(register, count) => {
                                    let remaining: String = characters.collect();
                                    let mut suffix = Vec::new();
                                    if !remaining.is_empty() {
                                        suffix.push(egui::Event::Text(remaining));
                                    }
                                    suffix.extend(events);
                                    macros.play_register(register, count, &tab.vim, suffix);
                                    break 'vim_events;
                                }
                                vim_macros::Input::Pass => {}
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
                                    let remaining: String = characters.collect();
                                    macros.record_event(&egui::Event::Text(remaining.clone()));
                                    prompt_text.push_str(&remaining);
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
                                    if macros.active() {
                                        let remaining: String = characters.collect();
                                        let mut suffix = Vec::new();
                                        if !remaining.is_empty() {
                                            suffix.push(egui::Event::Text(remaining));
                                        }
                                        suffix.extend(events);
                                        macros.defer_playback_events(suffix);
                                        break 'vim_events;
                                    }
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
                            if (tab.vim.take_command_failure() || tab.vim_message.is_some())
                                && macros.active()
                            {
                                macros.failed_motion(tab.vim_message.is_some());
                                break 'vim_events;
                            }
                            if let Some(request) = tab.vim.take_global_mark_request() {
                                let remaining: String = characters.collect();
                                let mut suffix = Vec::new();
                                if !remaining.is_empty() {
                                    suffix.push(egui::Event::Text(remaining));
                                }
                                suffix.extend(events);
                                file_mark_request = Some((request, suffix));
                                break 'vim_events;
                            }
                        }
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let translated = if !modifiers.command && !modifiers.ctrl && !modifiers.alt
                        {
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
                            if (tab.vim.take_command_failure() || tab.vim_message.is_some())
                                && macros.active()
                            {
                                macros.failed_motion(tab.vim_message.is_some());
                                break 'vim_events;
                            }
                        }
                    }
                    egui::Event::Paste(_) => {}
                    other => retained.push(other),
                }
            }
            yank_register = tab.vim.yank_register().cloned();
            register_bank = tab.vim.register_bank().clone();
        }
        self.vim_macros = macros;
        self.find = find;
        self.record_vim_jumps();
        if vim_searched {
            self.find_visible = false;
        }
        self.find_message();
        if let Some(message) = self.tab_mut().and_then(|tab| tab.vim_message.take()) {
            self.vim_input_status(message);
        }
        retained.extend(native_insert);
        ctx.input_mut(|input| input.events = retained);
        if let Some(yank) = vim_register {
            self.register = Some(yank);
        }
        self.yank_register = yank_register;
        self.vim_registers = register_bank;
        if let Some((request, events)) = file_mark_request {
            self.handle_vim_file_mark(request, events);
        }
        if let Some(text) = system_clipboard {
            ctx.copy_text(text);
        }
        if let Some(':') = prompt {
            self.begin_ex_command(prompt_text);
        }
        if definition {
            self.request_definition();
            self.wait_vim_macro_definition();
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
        if let Some(backwards) = completion {
            if let Some(popup) = self
                .completions
                .as_mut()
                .filter(|popup| !popup.items.is_empty())
            {
                popup.selected = if backwards {
                    popup.selected.saturating_sub(1)
                } else {
                    (popup.selected + 1).min(popup.items.len() - 1)
                };
                ctx.data_mut(|data| {
                    data.remove::<(u64, i32, usize, usize)>(egui::Id::new(
                        "completion_selected_row",
                    ));
                });
            } else {
                self.request_completion();
            }
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
