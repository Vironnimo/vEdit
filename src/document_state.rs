//! Shared document state and document-local editing and display helpers.
use super::*;

/// A cached bracket pair: (revision, cursor, between-characters caret) and the
/// pair found for it.
type BracketMatch = ((u64, usize, bool), Option<(usize, usize)>);

pub(super) struct EditorTab {
    pub(super) id: u64,
    pub(super) path: Option<PathBuf>,
    pub(super) title: String,
    pub(super) text: String,
    pub(super) saved_text: String,
    pub(super) dirty: bool,
    /// Explorer browsing may replace this buffer until it is edited or kept.
    pub(super) preview_tab: bool,
    pub(super) agent_unsaved: bool,
    pub(super) recovery_path: PathBuf,
    pub(super) recovery_revision: u64,
    pub(super) recovery_pending: bool,
    pub(super) recovery_cleanup_pending: Option<u64>,
    pub(super) recovery_cleanup_retry_at: Option<Instant>,
    pub(super) recovery_generation: u64,
    /// A recovery file may still exist although the buffer matches the disk,
    /// for example after a save or when recovered text equals the file.
    pub(super) recovery_stale: bool,
    pub(super) revision: u64,
    pub(super) fingerprint: Option<workspace::FileFingerprint>,
    pub(super) line_ending: workspace::LineEnding,
    pub(super) bom: bool,
    pub(super) loading: bool,
    pub(super) saving: bool,
    pub(super) error: Option<String>,
    /// A newer disk version while this buffer has unsaved edits. Only saving
    /// to the original path waits for the user's choice.
    pub(super) disk_conflict: Option<disk_sync::DiskConflict>,
    /// The last clean reload from disk, shown until the next edit.
    pub(super) disk_reload: Option<disk_sync::DiskReload>,
    pub(super) last_edit: Instant,
    pub(super) vim: VimState,
    pub(super) selection: Range<usize>,
    pub(super) pending_selection: Option<Range<usize>>,
    pub(super) focus: bool,
    pub(super) scroll_to_cursor: bool,
    pub(super) center_on_reveal: bool,
    /// Vim `zt` / `zz` / `zb`: place the cursor line at this edge on the next reveal.
    pub(super) scroll_align: Option<crate::vim::ScrollAlign>,
    /// A Vim status message waiting for the status bar, such as an unsupported command.
    pub(super) vim_message: Option<String>,
    /// The revision and text that other panes' positions refer to, kept only
    /// while another pane holds one; see `shift_shared_document_positions`.
    pub(super) pane_baseline: Option<(u64, String)>,
    /// The bracket pair at the caret, computed once per (revision, cursor, caret kind).
    pub(super) bracket_match: Option<BracketMatch>,
    pub(super) pending_navigation_feedback: bool,
    pub(super) feedback: Option<editor_feedback::Highlight>,
    pub(super) lines: crate::render::LineIndex,
    pub(super) indentation_cache: indentation::Cache,
    pub(super) agent_view: crate::cli::DocumentView,
    pub(super) annotations: Vec<crate::cli::Annotation>,
    pub(super) annotations_revision: Option<u64>,
    pub(super) pending_agent_location: Option<crate::cli::Location>,
    pub(super) agent_location_error: Option<String>,
}

impl EditorTab {
    pub(super) fn new(path: Option<PathBuf>, title: String) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let id = (u64::from(std::process::id()) << 32) | sequence;
        let recovery_path = recovery_directory().join(format!(
            "{}-{sequence}.txt",
            crate::recovery::session_prefix()
        ));
        Self {
            id,
            path,
            title,
            text: String::new(),
            saved_text: String::new(),
            dirty: false,
            preview_tab: false,
            agent_unsaved: false,
            recovery_path,
            recovery_revision: 0,
            recovery_pending: false,
            recovery_cleanup_pending: None,
            recovery_cleanup_retry_at: None,
            recovery_generation: 0,
            recovery_stale: false,
            revision: 0,
            fingerprint: None,
            line_ending: workspace::LineEnding::Lf,
            bom: false,
            loading: false,
            saving: false,
            error: None,
            disk_conflict: None,
            disk_reload: None,
            last_edit: Instant::now(),
            vim: VimState::new(),
            selection: 0..0,
            pending_selection: Some(0..0),
            focus: true,
            scroll_to_cursor: false,
            center_on_reveal: false,
            scroll_align: None,
            vim_message: None,
            pane_baseline: None,
            bracket_match: None,
            pending_navigation_feedback: false,
            feedback: None,
            lines: crate::render::LineIndex::default(),
            indentation_cache: indentation::Cache::default(),
            agent_view: crate::cli::DocumentView::default(),
            annotations: Vec::new(),
            annotations_revision: None,
            pending_agent_location: None,
            agent_location_error: None,
        }
    }
    pub(super) fn changed(&mut self) {
        self.preview_tab = false;
        self.revision += 1;
        self.dirty = self.text != self.saved_text;
        self.clear_agent_draft_when_saved();
        self.last_edit = Instant::now();
    }
    /// An agent draft ends once the buffer matches its saved text again, e.g.
    /// after Undo. Later edits belong to whoever makes them.
    pub(super) fn clear_agent_draft_when_saved(&mut self) {
        if !self.dirty {
            self.agent_unsaved = false;
        }
    }
    pub(super) fn apply(&mut self, edit: editor::EditResult) {
        if edit.text != self.text {
            self.vim.begin_external_edit(&self.text);
        }
        self.text = edit.text;
        self.selection = edit.selection.clone();
        self.pending_selection = Some(edit.selection);
        self.changed();
        self.focus = true;
    }
    pub(super) fn apply_line_edit(&mut self, edit: editor::EditResult) {
        let navigation = self
            .vim
            .navigation()
            .remapped(|position| edit.map_position(position));
        self.apply(edit);
        self.vim.notify_external_text_change();
        self.vim.restore_navigation(&self.text, &navigation);
        if let Some(selection) = self.vim.selection(&self.text) {
            self.selection = selection.clone();
            self.pending_selection = Some(selection);
        }
        self.scroll_to_cursor = true;
    }

    /// The bracket at the caret and its partner. The search looks at most
    /// `BRACKET_SCAN` characters either way and skips very large documents.
    pub(super) fn bracket_pair(&mut self, vim: bool) -> Option<(usize, usize)> {
        const BRACKET_SCAN: usize = 20_000;
        const BRACKET_MAX_DOCUMENT: usize = 2 * 1024 * 1024;
        // A Vim block cursor covers a character; a text caret sits between two.
        let between = !vim || self.vim.mode == VimMode::Insert;
        let key = (self.revision, self.vim.cursor, between);
        if let Some((cached, pair)) = self.bracket_match
            && cached == key
        {
            return pair;
        }
        let pair = if self.loading || self.text.len() > BRACKET_MAX_DOCUMENT {
            None
        } else {
            let offset = self.vim.cursor.saturating_sub(BRACKET_SCAN);
            let window: Vec<char> = self
                .text
                .chars()
                .skip(offset)
                .take(2 * BRACKET_SCAN + 1)
                .collect();
            crate::vim::bracket_pair(&window, offset, self.vim.cursor, between)
        };
        self.bracket_match = Some((key, pair));
        pair
    }
    pub(super) fn set_cursor(&mut self, range: Range<usize>) {
        self.vim.set_cursor(&self.text, range.start);
        // Explicit navigation may target the boundary after the last character.
        self.vim.cursor = range.start;
        self.selection = range.clone();
        self.pending_selection = Some(range);
        self.focus = true;
        self.scroll_to_cursor = true;
    }
    pub(super) fn apply_line_move(&mut self, edit: editor::EditResult) {
        if edit.text == self.text {
            return;
        }
        let navigation = self.vim.navigation();
        let start = self.vim_editor_selection().start;
        let delta = edit.selection.start as isize - start as isize;
        self.apply(edit);
        self.vim.notify_external_text_change();
        self.vim
            .restore_navigation(&self.text, &navigation.shifted(delta));
        self.scroll_to_cursor = true;
    }

    /// A rectangular area has no contiguous text selection, but editor line
    /// commands still operate on every logical row that the rectangle covers.
    pub(super) fn vim_editor_selection(&self) -> Range<usize> {
        if let Some(block) = self.vim.block_selection(&self.text)
            && let Some((first, last)) = block.first().zip(block.last())
        {
            let mut start = 0;
            for (index, ch) in self.text.chars().enumerate() {
                if ch == '\n' {
                    if index < first.start {
                        start = index + 1;
                    } else if index >= last.start {
                        return start..index + 1;
                    }
                }
            }
            return start..self.text.chars().count();
        }
        self.vim
            .selection(&self.text)
            .unwrap_or_else(|| self.selection.clone())
    }
}

pub(super) fn file_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::paths::display(path))
}

pub(super) fn extension(path: Option<&Path>) -> &str {
    path.and_then(Path::extension)
        .and_then(|v| v.to_str())
        .unwrap_or("")
}

pub(super) fn file_badge(path: &Path) -> &'static str {
    crate::file_kind::from_path(Some(path)).badge()
}

pub(super) fn file_color(path: Option<&Path>) -> Color32 {
    use crate::file_kind::FileKind;
    match crate::file_kind::from_path(path) {
        FileKind::Rust => ORANGE,
        FileKind::Markdown | FileKind::TypeScript | FileKind::Dart | FileKind::Python => BLUE,
        FileKind::Json | FileKind::JavaScript => Color32::from_rgb(208, 195, 137),
        FileKind::Css | FileKind::Scss | FileKind::Less | FileKind::Html | FileKind::Xml => {
            Color32::from_rgb(171, 158, 216)
        }
        _ => MUTED,
    }
}

pub(super) fn language_name(path: Option<&Path>) -> &'static str {
    crate::file_kind::from_path(path).label()
}

pub(super) fn is_markdown(path: Option<&Path>) -> bool {
    path.is_some_and(crate::cli::is_markdown)
}

pub(super) fn syntax_language(path: Option<&Path>) -> &str {
    let kind = crate::file_kind::from_path(path);
    if kind == crate::file_kind::FileKind::PlainText && !extension(path).is_empty() {
        // Preserve syntax support for less common extensions in Syntect's set.
        extension(path)
    } else {
        kind.syntax()
    }
}
