//! Pane-local back/forward navigation history (Alt+Left / Alt+Right, the
//! mouse's back and forward buttons, and Ctrl+O / Ctrl+I in Vim Normal mode).
//!
//! Each editor pane observes its active document once per frame. Switching
//! to another document or moving the cursor more than `LARGE_MOVE_LINES`
//! lines without an edit records the previous location. Features whose
//! navigation can be short (go to definition, references, search results,
//! bookmarks, problems) call `record_jump` first. Equal neighbouring entries
//! are merged, so both mechanisms may see the same move.
use super::*;

const MAX_JUMPS: usize = 100;
const LARGE_MOVE_LINES: usize = 10;

/// A location in character (Unicode scalar) coordinates. `line`/`column` are
/// one-based and survive a reload of a closed document; `offset` is exact
/// while the document stays open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Jump {
    document: u64,
    path: Option<PathBuf>,
    offset: usize,
    line: usize,
    column: usize,
}

impl Jump {
    fn same_place(&self, other: &Self) -> bool {
        let same_document =
            self.document == other.document || (self.path.is_some() && self.path == other.path);
        same_document && self.line == other.line
    }
}

#[derive(Clone, Debug)]
struct Observed {
    jump: Jump,
    revision: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct JumpList {
    back: Vec<Jump>,
    forward: Vec<Jump>,
    observed: Option<Observed>,
}

impl JumpList {
    /// A new navigation starts a new branch: forward history is dropped.
    fn push(&mut self, jump: Jump) {
        if self.back.last().is_some_and(|last| last.same_place(&jump)) {
            return;
        }
        self.back.push(jump);
        if self.back.len() > MAX_JUMPS {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    #[cfg(test)]
    pub(super) fn depths(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }
}

/// The character offset of a one-based line and column, clamped to the text.
fn offset_at(text: &str, line: usize, column: usize) -> usize {
    let mut offset = 0;
    for (index, content) in text.split('\n').enumerate() {
        let length = content.chars().count();
        if index + 1 == line {
            let visible = content.strip_suffix('\r').map_or(length, |_| length - 1);
            return offset + column.saturating_sub(1).min(visible);
        }
        offset += length + 1;
    }
    text.chars().count()
}

impl VEditApp {
    fn current_observation(&self) -> Option<Observed> {
        let tab = self.tab()?;
        if tab.loading {
            return None;
        }
        let offset = tab.vim.cursor;
        let (line, column) = tab.lines.line_col(&tab.text, tab.revision, offset);
        Some(Observed {
            jump: Jump {
                document: tab.id,
                path: tab.path.clone(),
                offset,
                line,
                column,
            },
            revision: tab.revision,
        })
    }

    /// Record the focused pane's current location before navigating away.
    /// Other features call this single hook for jumps that may be short.
    pub(super) fn record_jump(&mut self) {
        let Some(current) = self.current_observation() else {
            return;
        };
        if let Some(list) = self.focused_jump_list() {
            list.push(current.jump);
        }
    }

    /// Show a one-based line and character column of `path` in the focused
    /// pane, as a new history entry.
    pub(super) fn reveal_line_column(&mut self, path: PathBuf, line: usize, column: usize) {
        self.record_jump();
        let target = Jump {
            // No open document has this id, so the line and column decide.
            document: u64::MAX,
            path: Some(path),
            offset: 0,
            line,
            column,
        };
        if !self.open_jump(&target) {
            self.status("The file could not be opened", true);
        }
    }

    /// Runs once per frame after input and rendering.
    pub(super) fn observe_navigation(&mut self) {
        if self.change_review_visible() {
            return;
        }
        let loading = self.tab().is_some_and(|tab| tab.loading);
        let applying = self.pending_location.is_some() || self.pending_jump.is_some();
        if loading || applying {
            // The destination is not in place yet; compare once it is.
            return;
        }
        let current = self.current_observation();
        let navigating = std::mem::take(&mut self.jump_navigating);
        let Some(list) = self.focused_jump_list() else {
            return;
        };
        let previous = list.observed.take();
        if !navigating && let (Some(previous), Some(current)) = (&previous, &current) {
            let moved_document = previous.jump.document != current.jump.document;
            let large_move = previous.revision == current.revision
                && previous.jump.line.abs_diff(current.jump.line) > LARGE_MOVE_LINES;
            if moved_document || large_move {
                list.push(previous.jump.clone());
            }
        }
        list.observed = current.or(previous);
    }

    /// The mouse's back and forward buttons, outside dialogs and menus.
    pub(super) fn history_pointer_buttons(&mut self) {
        let (back, forward) = self.ctx.input(|input| {
            (
                input.pointer.button_pressed(egui::PointerButton::Extra1),
                input.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        if (!back && !forward)
            || self.chrome_blocked()
            || self.change_review_visible()
            || self.screen_jump.is_some()
            || self.navigation_pending()
        {
            return;
        }
        self.navigate_history(back);
    }

    pub(super) fn navigate_history(&mut self, backwards: bool) {
        let current = self.current_observation().map(|observed| observed.jump);
        loop {
            let Some(list) = self.focused_jump_list() else {
                return;
            };
            let target = if backwards {
                list.back.pop()
            } else {
                list.forward.pop()
            };
            let Some(target) = target else {
                self.status(
                    if backwards {
                        "No earlier location in this pane"
                    } else {
                        "No later location in this pane"
                    },
                    false,
                );
                return;
            };
            if current.as_ref().is_some_and(|current| {
                current.same_place(&target) && current.offset == target.offset
            }) {
                continue;
            }
            if !self.open_jump(&target) {
                continue;
            }
            if let Some(current) = current
                && let Some(list) = self.focused_jump_list()
            {
                if backwards {
                    list.forward.push(current);
                } else {
                    list.back.push(current);
                }
            }
            self.jump_navigating = true;
            return;
        }
    }

    /// Show a recorded location in the focused pane. A closed document is
    /// reopened from its path; one without a path, or whose folder listing
    /// shows it no longer exists, is skipped.
    fn open_jump(&mut self, target: &Jump) -> bool {
        let Some(project) = self.project() else {
            return false;
        };
        let open = project
            .tabs
            .iter()
            .position(|tab| tab.id == target.document)
            .or_else(|| {
                target.path.as_ref().and_then(|path| {
                    project
                        .tabs
                        .iter()
                        .position(|tab| tab.path.as_ref() == Some(path))
                })
            });
        let id = if let Some(index) = open {
            let id = project.tabs[index].id;
            self.keep_tab_open(id);
            self.activate_tab(self.active_project, index);
            id
        } else {
            let Some(path) = target.path.clone() else {
                return false;
            };
            let missing = self.explorer_index().is_some_and(|index| {
                index.is_explorer_snapshot()
                    && path.parent().is_some_and(|parent| {
                        index.directory_loaded(parent) && !index.has_entry(&path)
                    })
            });
            if missing {
                return false;
            }
            self.open_file(path.clone());
            let Some(tab) = self.tab().filter(|tab| tab.path.as_ref() == Some(&path)) else {
                return false;
            };
            tab.id
        };
        self.set_document_view(crate::cli::DocumentView::Source);
        if let Some(tab) = self.tab_by_id_mut(id) {
            tab.pending_navigation_feedback = true;
            tab.focus = true;
        }
        self.pending_jump = Some((id, target.clone()));
        self.apply_pending_jump();
        true
    }

    pub(super) fn apply_pending_jump(&mut self) {
        let Some((id, target)) = self.pending_jump.clone() else {
            return;
        };
        let Some(tab) = self.tab_by_id_mut(id) else {
            self.pending_jump = None;
            return;
        };
        if tab.loading {
            return;
        }
        let length = tab.text.chars().count();
        // The open document's exact offset, unless it was reopened or its
        // text changed so much that the offset left the line.
        let exact = tab.id == target.document
            && target.offset <= length
            && tab.lines.line_col(&tab.text, tab.revision, target.offset).0 == target.line;
        let cursor = if exact {
            target.offset
        } else {
            offset_at(&tab.text, target.line, target.column)
        };
        tab.set_cursor(cursor..cursor);
        self.pending_jump = None;
        self.sync_external_document_navigation(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_clamp_to_lines_and_ignore_carriage_returns() {
        let text = "ab\r\ncdé\nlast";
        assert_eq!(offset_at(text, 1, 1), 0);
        assert_eq!(offset_at(text, 1, 9), 2);
        assert_eq!(offset_at(text, 2, 3), 6);
        assert_eq!(offset_at(text, 3, 99), 12);
        assert_eq!(offset_at(text, 9, 1), 12);
    }

    #[test]
    fn neighbouring_equal_locations_merge_and_new_jumps_drop_forward_history() {
        let jump = |document, line| Jump {
            document,
            path: None,
            offset: line * 10,
            line,
            column: 1,
        };
        let mut list = JumpList::default();
        list.push(jump(1, 5));
        list.push(jump(1, 5));
        assert_eq!(list.depths(), (1, 0));
        list.forward.push(jump(2, 1));
        list.push(jump(1, 40));
        assert_eq!(list.depths(), (2, 0));
        for line in 0..MAX_JUMPS + 5 {
            list.push(jump(3, line * 20));
        }
        assert_eq!(list.depths(), (MAX_JUMPS, 0));
    }
}
