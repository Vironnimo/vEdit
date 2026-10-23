//! Application shortcut gates and frame input ordering.
use super::*;

impl VEditApp {
    pub(super) fn shortcuts(&mut self) {
        use keybindings::Action;
        self.resume_editor_input();
        self.resume_shortcut_input();
        keybindings::preserve_altgr_text(&self.ctx);
        let ctx = self.ctx.clone();
        if self.picker.is_some()
            || self.palette.is_some()
            || self.settings_open
            || self.status_details.is_some()
            || self.agent_marks_modal_open()
            || self.save_as.is_some()
            || self.close_action.is_some()
            || self.diff.is_some()
            || self.git_detail_open()
            || self.navigation_modal_open()
            || self.navigation_pending()
            || self.screen_jump.is_some()
            || self.vim_search_active()
            || self.context_file_dialog.is_some()
            || self.vim_command.is_some()
            || egui::Popup::is_any_open(&ctx)
        {
            return;
        }
        self.multicursor_keys();
        if self.completion_keys() || self.split_shortcuts(&ctx) {
            return;
        }
        let vim_modal = self.settings.vim
            && self.tab().is_some_and(|tab| {
                tab.vim.mode != VimMode::Insert
                    && !tab.vim.multi.active()
                    && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            });
        let vim_insert = self.settings.vim
            && self.tab().is_some_and(|tab| {
                tab.vim.mode == VimMode::Insert
                    && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            });
        let vim_normal = vim_modal
            && self
                .tab()
                .is_some_and(|tab| tab.vim.mode == VimMode::Normal);
        let review_visible = self.change_review_visible();
        let mut ready = Vec::new();
        for &action in Action::ALL {
            if action.multicursor() || action.editor_command().is_some() {
                continue;
            }
            if matches!(
                action,
                Action::SwitchProject
                    | Action::PreviousProject
                    | Action::NextProject
                    | Action::FocusExplorer
                    | Action::FocusEditor
                    | Action::FocusGit
                    | Action::FocusOutput
                    | Action::FocusLeft
                    | Action::FocusRight
                    | Action::FocusUp
                    | Action::FocusDown
                    | Action::FocusNextRegion
                    | Action::FocusPreviousRegion
                    | Action::FocusNextPane
                    | Action::FocusPreviousPane
                    | Action::SplitRight
                    | Action::SplitDown
                    | Action::ClosePane
                    | Action::MoveTabLeft
                    | Action::MoveTabRight
                    | Action::MoveTabUp
                    | Action::MoveTabDown
                    | Action::AgentMark
                    | Action::AgentMarks
                    | Action::ZoomIn
                    | Action::ZoomOut
                    | Action::ZoomReset
            ) {
                continue;
            }
            // A change review hides the documents; their commands wait for it to close.
            if review_visible
                && matches!(
                    action,
                    Action::Save
                        | Action::SaveAs
                        | Action::CloseTab
                        | Action::Find
                        | Action::Replace
                        | Action::FindNext
                        | Action::FindPrevious
                        | Action::GoToLine
                        | Action::Definition
                        | Action::Completion
                        | Action::Hover
                        | Action::FindReferences
                        | Action::DocumentSymbol
                        | Action::WorkspaceSymbol
                        | Action::MarkdownPreview
                        | Action::ToggleBookmark
                        | Action::ScreenJump
                        | Action::Reload
                        | Action::CompareWithSaved
                        | Action::NextProblem
                        | Action::PreviousProblem
                        | Action::NavigateBack
                        | Action::NavigateForward
                )
            {
                continue;
            }
            if action == Action::Find
                && vim_modal
                && !self.settings.keybindings.contains_key(action.id())
            {
                continue;
            }
            // Ctrl+O goes back in Vim Normal mode, like Vim's jump list.
            if action == Action::OpenFile
                && vim_normal
                && !self.settings.keybindings.contains_key(action.id())
            {
                continue;
            }
            // Ctrl+B keeps its Vim page-back meaning; explicit custom bindings
            // have no hidden Vim exception, and the safe Ctrl+Alt+E works everywhere.
            if action == Action::ToggleLeftSidebar
                && vim_modal
                && !self.settings.keybindings.contains_key(action.id())
            {
                let available = ctx.input(|input| {
                    input.events.iter().any(|event| {
                        self.shortcut_matches(action, event)
                            && matches!(event, egui::Event::Key { modifiers, .. } if modifiers.alt)
                    })
                });
                if !available {
                    continue;
                }
            }
            ready.push(action);
        }
        let shortcut_context = self.shortcut_input_context();
        let candidate = ctx.input(|input| {
            input.events.iter().enumerate().find_map(|(index, event)| {
                ready
                    .iter()
                    .copied()
                    .find(|action| self.shortcut_boundary(*action, event, shortcut_context))
                    .map(|action| (index, action))
            })
        });
        let Some((index, action)) = candidate else {
            return;
        };
        if self.earlier_shortcut_boundary(index) {
            return;
        }
        // Save and navigation must observe text typed before the shortcut.
        // Render that prefix in its current source, then dispatch next frame.
        if let Some(tab) = self.tab()
            && tab.agent_view == crate::cli::DocumentView::Source
            && ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
            && ctx.input(|input| input.events[..index].iter().any(editor_input::native_input))
        {
            let suffix = ctx.input_mut(|input| input.events.split_off(index));
            editor_input::defer(&ctx, tab.id, suffix);
            return;
        }
        if ctx.memory(|memory| memory.focused().is_some())
            && ctx.input(|input| input.events[..index].iter().any(editor_input::native_input))
        {
            let suffix = ctx.input_mut(|input| input.events.split_off(index));
            self.defer_widget_shortcut_input(suffix);
            return;
        }
        self.take_shortcut(action);
        let previous_document = self.tab().map(|tab| tab.id);
        let source_owned = self.tab().is_some_and(|tab| {
            ctx.memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
        });
        if action == Action::NewFile
            && vim_insert
            && !self.settings.keybindings.contains_key(action.id())
        {
            self.request_completion();
        } else {
            self.run_shortcut_action(action);
        }
        // Continue the suffix only after the resulting source has rendered.
        // This preserves two commands in one OS event batch in input order.
        if self.find_focus {
            // Find takes focus while its toolbar renders, after sidebars. Keep
            // the suffix for that field instead of letting the old field eat it.
            let suffix = ctx.input_mut(|input| std::mem::take(&mut input.events));
            self.defer_widget_shortcut_input(suffix);
        } else if self.palette.is_none()
            && !self.settings_open
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.picker.is_none()
            && !self.navigation_modal_open()
            && self.status_details.is_none()
            && !self.agent_marks_modal_open()
            && let Some(tab) = self.tab().filter(|tab| {
                tab.agent_view == crate::cli::DocumentView::Source
                    && (source_owned || Some(tab.id) != previous_document)
            })
        {
            let suffix = ctx.input_mut(|input| std::mem::take(&mut input.events));
            editor_input::defer(&ctx, tab.id, suffix);
        } else if self.palette.is_none()
            && !self.settings_open
            && !self.find_focus
            && self.save_as.is_none()
            && self.close_action.is_none()
            && self.picker.is_none()
            && !self.navigation_modal_open()
            && self.status_details.is_none()
            && !self.agent_marks_modal_open()
        {
            self.defer_widget_shortcut_tail();
        }
    }
}
