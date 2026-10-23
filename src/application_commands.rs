//! Typed application command execution, shared by shortcuts, menus and palette.
use super::*;

impl VEditApp {
    pub(super) fn run_shortcut_action(&mut self, action: keybindings::Action) {
        use keybindings::Action;
        match action {
            Action::CommandPalette => self.open_palette(Palette::Commands),
            Action::QuickOpen => self.open_palette(Palette::Files),
            Action::SaveAs => {
                if let Some(id) = self.tab().map(|tab| tab.id) {
                    self.request_save_as(id);
                }
            }
            Action::CloseTab => {
                if let Some(tab) = self.tab() {
                    self.request_close(CloseAction::PaneTab {
                        pane_id: self.current_pane_id(),
                        id: tab.id,
                    });
                }
            }
            Action::ReopenTab => self.reopen_closed_tab(),
            Action::NextTab | Action::PreviousTab => {
                self.bind_current_pane();
                let ids = self.current_pane_tab_ids();
                if ids.is_empty() {
                    return;
                }
                let active = self
                    .tab()
                    .and_then(|tab| ids.iter().position(|id| *id == tab.id))
                    .unwrap_or(0);
                let next = if action == Action::PreviousTab {
                    (active + ids.len() - 1) % ids.len()
                } else {
                    (active + 1) % ids.len()
                };
                if let Some((project, tab)) =
                    self.projects.iter().enumerate().find_map(|(pi, project)| {
                        project
                            .tabs
                            .iter()
                            .position(|tab| tab.id == ids[next])
                            .map(|ti| (pi, ti))
                    })
                {
                    self.activate_tab(project, tab);
                }
            }
            Action::FindNext | Action::FindPrevious => {
                self.find_next(action == Action::FindPrevious)
            }
            Action::KeyboardShortcuts => {
                self.open_settings_section("Keyboard");
            }
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                self.interface_zoom_draft = None;
                self.settings.ui_scale = match action {
                    Action::ZoomIn => (self.settings.ui_scale + 0.1).min(2.0),
                    Action::ZoomOut => (self.settings.ui_scale - 0.1).max(0.75),
                    _ => 1.0,
                };
                self.settings.ui_scale = (self.settings.ui_scale * 100.0).round() / 100.0;
                self.session_dirty = true;
                self.ctx.set_zoom_factor(self.settings.ui_scale);
                self.ctx.request_repaint();
            }
            Action::SwitchProject => self.open_workspace_picker(),
            Action::NextProject | Action::PreviousProject => {
                if !self.projects.is_empty() {
                    let count = self.projects.len();
                    let next = if action == Action::NextProject {
                        (self.active_project + 1) % count
                    } else {
                        (self.active_project + count - 1) % count
                    };
                    self.switch_project(next);
                }
            }
            Action::OpenProject => self.pick_project(),
            Action::OpenFile => self.pick_file(),
            Action::NewFile => self.new_file(),
            Action::Save => self.save_current(),
            Action::SaveAll => self.save_all(),
            Action::Find => {
                self.find_visible = true;
                self.find_focus = true;
            }
            Action::Replace => {
                self.find_visible = true;
                self.replace_visible = true;
                self.find_focus = true;
            }
            Action::SearchProject => {
                self.open_project_search();
            }
            Action::GoToLine => self.open_palette(Palette::Line),
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::Bookmarks => self.open_bookmarks(),
            Action::AgentMark => {
                self.capture_agent_mark();
            }
            Action::AgentMarks => self.open_agent_marks(),
            Action::AgentNotifications => self.open_notifications(),
            Action::ScreenJump => self.start_screen_jump(),
            Action::Definition => self.request_definition(),
            Action::Completion => self.request_completion(),
            Action::Hover => self.show_hover(),
            Action::FindReferences => self.find_references(),
            Action::DocumentSymbol => {
                self.open_symbol_palette(language_intel::SymbolScope::Document)
            }
            Action::WorkspaceSymbol => {
                self.open_symbol_palette(language_intel::SymbolScope::Workspace)
            }
            Action::RestartLanguage => self.restart_active_language(),
            Action::LanguageSupport => self.open_settings_section("Languages"),
            Action::Problems => {
                self.output_visible = true;
                self.problems_visible = true;
            }
            Action::NextProblem => self.go_to_problem(false),
            Action::PreviousProblem => self.go_to_problem(true),
            Action::NavigateBack => self.navigate_history(true),
            Action::NavigateForward => self.navigate_history(false),
            Action::MarkdownPreview => {
                if self
                    .tab()
                    .is_some_and(|tab| is_markdown(tab.path.as_deref()))
                {
                    let view = if self
                        .tab()
                        .is_some_and(|tab| tab.agent_view == crate::cli::DocumentView::Preview)
                    {
                        crate::cli::DocumentView::Source
                    } else {
                        crate::cli::DocumentView::Preview
                    };
                    self.set_document_view(view);
                } else {
                    self.status("Open a Markdown document to use preview", false);
                }
            }
            Action::SplitRight => self.split_command("right"),
            Action::SplitDown => self.split_command("down"),
            Action::FocusExplorer => {
                self.focus_explorer();
                self.defer_region_focus_owner();
            }
            Action::FocusEditor => {
                self.focus_editor();
                self.defer_region_focus_owner();
            }
            Action::FocusGit => {
                self.show_sidebar(SidebarView::SourceControl);
                self.focus_sidebar_region(self.sidebar_side(SidebarView::SourceControl));
                self.defer_region_focus_owner();
            }
            Action::FocusOutput => {
                self.output_visible = true;
                self.focus_output_region();
                self.defer_region_focus_owner();
            }
            Action::FocusNextPane => self.split_command("next"),
            Action::FocusPreviousPane => self.split_command("previous"),
            Action::FocusNextRegion => self.split_command("region_next"),
            Action::FocusPreviousRegion => self.split_command("region_previous"),
            Action::FocusLeft => self.split_command("focus_left"),
            Action::FocusRight => self.split_command("focus_right"),
            Action::FocusUp => self.split_command("focus_up"),
            Action::FocusDown => self.split_command("focus_down"),
            Action::MoveTabLeft => self.split_command("move_left"),
            Action::MoveTabRight => self.split_command("move_right"),
            Action::MoveTabUp => self.split_command("move_up"),
            Action::MoveTabDown => self.split_command("move_down"),
            Action::ClosePane => self.split_command("close"),
            Action::Review => {
                self.show_sidebar(SidebarView::Review);
            }
            Action::SourceControl => {
                self.show_sidebar(SidebarView::SourceControl);
            }
            Action::ReviewWorkingTree | Action::ReviewBranch => {
                let source = if action == Action::ReviewWorkingTree {
                    git::ReviewSource::WorkingTree
                } else {
                    git::ReviewSource::Branch { base: None }
                };
                self.show_change_review(self.active_project, source);
            }
            Action::Explorer => {
                self.show_sidebar(SidebarView::Explorer);
            }
            Action::ToggleLeftSidebar => self.toggle_sidebar(sidebar_layout::Side::Left),
            Action::ToggleRightSidebar => self.toggle_sidebar(sidebar_layout::Side::Right),
            Action::ExplorerGitLayout => self.explorer_git_layout(),
            Action::ToggleOutput => {
                self.output_visible = !self.output_visible;
                if !self.output_visible {
                    self.clear_hidden_output_focus();
                }
            }
            Action::CheckProject => self.run_project_task(project_tasks::ProjectTaskAction::Check),
            Action::TestProject => self.run_project_task(project_tasks::ProjectTaskAction::Test),
            Action::BuildProject => self.run_project_task(project_tasks::ProjectTaskAction::Build),
            Action::RunFile => self.run_project_task(project_tasks::ProjectTaskAction::RunFile),
            Action::RunApplication => self.run_selected_application(),
            Action::StopApplication => self.stop_application(),
            Action::ConfigureTasks => self.open_project_task_settings(),
            Action::Terminal => self.open_terminal(),
            Action::ToggleWrap => {
                self.settings.word_wrap = !self.settings.word_wrap;
                self.session_dirty = true;
            }
            Action::ToggleVim => {
                self.settings.vim = !self.settings.vim;
                self.session_dirty = true;
            }
            Action::Reload => self.reload_current(),
            Action::CompareWithSaved => self.compare_current_with_saved(),
            Action::Settings => self.settings_open = true,
            Action::OpenRecent => self.open_palette(Palette::Recent),
            Action::Undo
            | Action::Redo
            | Action::ToggleComment
            | Action::DuplicateLine
            | Action::DeleteLine
            | Action::InsertLineAbove
            | Action::InsertLineBelow
            | Action::MoveLineUp
            | Action::MoveLineDown
            | Action::InsertNewline
            | Action::Indent
            | Action::Outdent => {
                self.apply_editor_command(action.editor_command().expect("editor action"));
            }
            Action::AddCursorAbove
            | Action::AddCursorBelow
            | Action::MarkCursor
            | Action::SelectNextOccurrence
            | Action::SelectAllOccurrences
            | Action::SkipOccurrence
            | Action::CursorUndo
            | Action::CursorsAtLineEnds
            | Action::ExitMulticursor => self.multicursor_action(action),
            Action::FinishCursorPlacement => {
                self.finish_cursor_placement();
            }
            Action::ExplorerRename | Action::ExplorerDelete => self.explorer_command(action),
            Action::ContextMenu => self.status(
                "Focus an Explorer, Git or project tab item to open its context menu",
                false,
            ),
            Action::ReviewNextFile
            | Action::ReviewPreviousFile
            | Action::ReviewSeenNext
            | Action::ReviewOpenFile
            | Action::ReviewComment => self.review_command(action),
            Action::ReviewSubmitComment => self.submit_review_comment(),
        }
    }
}
