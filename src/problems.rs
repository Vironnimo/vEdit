//! Problems: language diagnostics ordered by severity, file and position, with
//! Unicode columns, and next/previous problem navigation across the project.
use super::*;

/// The Problems list shows at most this many rows; it says when it stops.
pub(super) const MAX_PROBLEM_ROWS: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub(super) fn from_lsp(severity: u8) -> Self {
        match severity {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            _ => Self::Hint,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Information => "Information",
            Self::Hint => "Hint",
        }
    }

    pub(super) fn color(self) -> Color32 {
        match self {
            Self::Error => RED,
            Self::Warning => ORANGE,
            Self::Information => BLUE,
            Self::Hint => MUTED,
        }
    }

    /// F8 visits errors and warnings, like VS Code's problem navigation.
    fn navigable(self) -> bool {
        matches!(self, Self::Error | Self::Warning)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Column {
    /// One-based Unicode scalar column, converted with the open document.
    Unicode(usize),
    /// One-based UTF-16 column as the language server reported it.
    Utf16(usize),
}

#[derive(Clone, Debug)]
pub(super) struct ProblemRow {
    pub(super) path: PathBuf,
    pub(super) severity: Severity,
    pub(super) position: crate::lsp::Position,
    pub(super) column: Column,
    pub(super) message: String,
    pub(super) source: Option<String>,
}

#[derive(Default)]
pub(super) struct ProblemList {
    /// Every diagnostic, by severity, then path, then position.
    pub(super) rows: Vec<ProblemRow>,
    /// Counts by severity: error, warning, information, hint.
    pub(super) counts: [usize; 4],
}

impl ProblemList {
    /// The badge number: everything but hints.
    pub(super) fn counted(&self) -> usize {
        self.counts[..3].iter().sum()
    }

    pub(super) fn badge_color(&self) -> Color32 {
        if self.counts[0] > 0 {
            RED
        } else if self.counts[1] > 0 {
            ORANGE
        } else {
            MUTED
        }
    }

    pub(super) fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (count, singular, plural) in [
            (self.counts[0], "error", "errors"),
            (self.counts[1], "warning", "warnings"),
            (
                self.counts[2],
                "information message",
                "information messages",
            ),
            (self.counts[3], "hint", "hints"),
        ] {
            if count > 0 {
                parts.push(format!(
                    "{count} {}",
                    if count == 1 { singular } else { plural }
                ));
            }
        }
        if parts.is_empty() {
            "No problems".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// Identifies the diagnostics and open-document revisions a list was built
/// from. Replacing a file's diagnostics allocates a new vector, and the
/// previous one is still alive at that moment, so consecutive states differ.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ProblemKey {
    root: PathBuf,
    files: Vec<(PathBuf, usize, usize, Option<u64>)>,
}

#[derive(Default)]
pub(super) struct ProblemCache {
    entry: Option<(ProblemKey, Arc<ProblemList>)>,
}

fn problem_key(project: &Project) -> ProblemKey {
    let mut files: Vec<_> = project
        .diagnostics
        .iter()
        .map(|(path, diagnostics)| {
            (
                path.clone(),
                diagnostics.as_ptr() as usize,
                diagnostics.len(),
                project
                    .tabs
                    .iter()
                    .find(|tab| tab.path.as_ref() == Some(path) && !tab.loading)
                    .map(|tab| tab.revision),
            )
        })
        .collect();
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    ProblemKey {
        root: project.root.clone(),
        files,
    }
}

/// One-based Unicode scalar columns for LSP positions (UTF-16 code units),
/// in one pass over the text. Lines split at '\n'; a trailing '\r' ends the
/// line content like in `lsp::position_to_char`.
pub(super) fn unicode_columns(text: &str, positions: &[crate::lsp::Position]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..positions.len()).collect();
    order.sort_unstable_by_key(|&index| positions[index].line);
    let mut columns = vec![1; positions.len()];
    let mut lines = text.split('\n').enumerate();
    let mut current: Option<(usize, &str)> = None;
    for index in order {
        let position = positions[index];
        let target = position.line as usize;
        while current.is_none_or(|(line, _)| line < target) {
            match lines.next() {
                Some(line) => current = Some(line),
                None => break,
            }
        }
        let Some((line, content)) = current.filter(|(line, _)| *line == target) else {
            continue;
        };
        debug_assert_eq!(line, target);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let mut units = 0;
        let mut column = 0;
        for ch in content.chars() {
            let width = ch.len_utf16() as u32;
            if units + width > position.character {
                break;
            }
            units += width;
            column += 1;
        }
        columns[index] = column + 1;
    }
    columns
}

pub(super) fn build_problem_list(project: &Project) -> ProblemList {
    let mut list = ProblemList::default();
    for (path, diagnostics) in &project.diagnostics {
        let text = project
            .tabs
            .iter()
            .find(|tab| tab.path.as_ref() == Some(path) && !tab.loading)
            .map(|tab| tab.text.as_str());
        let columns = text.map(|text| {
            let positions: Vec<_> = diagnostics
                .iter()
                .map(|diagnostic| diagnostic.range.start)
                .collect();
            unicode_columns(text, &positions)
        });
        for (index, diagnostic) in diagnostics.iter().enumerate() {
            let severity = Severity::from_lsp(diagnostic.severity);
            list.counts[severity as usize] += 1;
            list.rows.push(ProblemRow {
                path: path.clone(),
                severity,
                position: diagnostic.range.start,
                column: match &columns {
                    Some(columns) => Column::Unicode(columns[index]),
                    None => Column::Utf16(diagnostic.range.start.character as usize + 1),
                },
                message: diagnostic.message.clone(),
                source: diagnostic.source.clone(),
            });
        }
    }
    list.rows.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.position.line.cmp(&b.position.line))
            .then_with(|| a.position.character.cmp(&b.position.character))
    });
    list
}

/// Where F8 / Shift+F8 goes from `cursor` (an LSP position in `current`):
/// errors and warnings of the current file in order, then the next file with
/// problems in path order, wrapping around the project.
pub(super) fn next_problem(
    diagnostics: &HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>,
    current: Option<(&Path, crate::lsp::Position)>,
    backwards: bool,
) -> Option<(PathBuf, crate::lsp::Position)> {
    let key = |position: &crate::lsp::Position| (position.line, position.character);
    let mut files: Vec<(&PathBuf, Vec<crate::lsp::Position>)> = diagnostics
        .iter()
        .filter_map(|(path, list)| {
            let mut positions: Vec<_> = list
                .iter()
                .filter(|diagnostic| Severity::from_lsp(diagnostic.severity).navigable())
                .map(|diagnostic| diagnostic.range.start)
                .collect();
            positions.sort_unstable_by_key(key);
            positions.dedup();
            (!positions.is_empty()).then_some((path, positions))
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let first = |(path, positions): &(&PathBuf, Vec<crate::lsp::Position>)| {
        let position = if backwards {
            *positions.last().unwrap()
        } else {
            positions[0]
        };
        ((*path).clone(), position)
    };
    let Some((path, cursor)) = current else {
        return Some(first(if backwards {
            files.last().unwrap()
        } else {
            &files[0]
        }));
    };
    if let Some(file) = files.iter().find(|(file, _)| file.as_path() == path) {
        let found = if backwards {
            file.1
                .iter()
                .rev()
                .find(|position| key(position) < key(&cursor))
        } else {
            file.1.iter().find(|position| key(position) > key(&cursor))
        };
        if let Some(position) = found {
            return Some((path.to_path_buf(), *position));
        }
    }
    // Continue with the next file in path order, wrapping to the current one.
    let next = if backwards {
        files
            .iter()
            .rev()
            .find(|(file, _)| file.as_path() < path)
            .or(files.last())
    } else {
        files
            .iter()
            .find(|(file, _)| file.as_path() > path)
            .or(files.first())
    };
    next.map(first)
}

impl VEditApp {
    pub(super) fn problem_list(&mut self) -> Arc<ProblemList> {
        let Some(project) = self.projects.get(self.active_project) else {
            return Arc::default();
        };
        let key = problem_key(project);
        if let Some((cached, list)) = &self.problem_cache.entry
            && *cached == key
        {
            return list.clone();
        }
        let list = Arc::new(build_problem_list(project));
        self.problem_cache.entry = Some((key, list.clone()));
        list
    }

    /// Open `path` and place the cursor at an LSP position once the document
    /// has loaded, with the usual navigation highlight.
    pub(super) fn reveal_lsp_position(&mut self, path: PathBuf, position: crate::lsp::Position) {
        self.record_jump();
        self.open_file(path);
        self.set_document_view(crate::cli::DocumentView::Source);
        let Some(tab) = self.tab_mut() else {
            return;
        };
        tab.pending_navigation_feedback = true;
        tab.focus = true;
        let id = tab.id;
        self.pending_location = Some((id, position));
        self.apply_pending_location();
    }

    pub(super) fn apply_pending_location(&mut self) {
        if let Some((id, position)) = self.pending_location
            && let Some(t) = self.tab_by_id_mut(id)
            && !t.loading
        {
            let cursor = crate::lsp::position_to_char(&t.text, position);
            t.set_cursor(cursor..cursor);
            self.pending_location = None;
            self.sync_external_document_navigation(id);
        }
    }

    /// F8 / Shift+F8: the next or previous error or warning in the project.
    pub(super) fn go_to_problem(&mut self, backwards: bool) {
        let Some(project) = self.project() else {
            return;
        };
        let current = self.tab().and_then(|tab| {
            let path = tab.path.as_deref()?;
            let cursor = tab.vim.cursor.min(tab.text.chars().count());
            Some((path, crate::lsp::char_to_position(&tab.text, cursor)))
        });
        match next_problem(&project.diagnostics, current, backwards) {
            Some((path, position)) => {
                let same =
                    current.is_some_and(|(current, cursor)| current == path && cursor == position);
                if same {
                    self.status("This is the only error or warning", false);
                }
                self.reveal_lsp_position(path, position);
            }
            None => self.status("No errors or warnings", false),
        }
    }

    pub(super) fn problems_ui(&mut self, ui: &mut egui::Ui) {
        self.language_controls_ui(ui);
        let Some(root) = self.project().map(|project| project.root.clone()) else {
            return;
        };
        let list = self.problem_list();
        let active = self
            .project()
            .is_some_and(|project| project.languages.values().any(|session| session.active()));
        if list.rows.is_empty() {
            ui.label(
                RichText::new(if active {
                    "No problems reported"
                } else {
                    "No language server is connected. Configure Languages in Settings to receive diagnostics."
                })
                .size(12.0)
                .color(MUTED),
            );
            return;
        }
        let shown = list.rows.len().min(MAX_PROBLEM_ROWS);
        let mut summary = list.summary();
        if shown < list.rows.len() {
            summary.push_str(&format!(
                " · showing the first {shown} of {} (most severe first)",
                list.rows.len()
            ));
        }
        ui.label(RichText::new(summary).size(11.0).color(MUTED))
            .on_hover_text(format!(
                "Next / previous error or warning: {} / {}",
                self.shortcut_label(keybindings::Action::NextProblem),
                self.shortcut_label(keybindings::Action::PreviousProblem)
            ));
        let mut jump = None;
        let row_height = 12.0 + ui.spacing().item_spacing.y + 6.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("problems")
            .show_rows(ui, row_height, shown, |ui, range| {
                for row in &list.rows[range] {
                    let relative = crate::paths::display(row.path.strip_prefix(&root).unwrap_or(&row.path));
                    let location = match row.column {
                        Column::Unicode(column) => {
                            format!("{relative}:{}:{column}", row.position.line + 1)
                        }
                        Column::Utf16(column) => format!(
                            "{relative}:{} (UTF-16 col {column})",
                            row.position.line + 1
                        ),
                    };
                    let label = format!(
                        "{}  {location}   {}",
                        row.severity.label(),
                        row.message.lines().next().unwrap_or("")
                    );
                    let response = ui.add_sized(
                        [ui.available_width(), row_height - ui.spacing().item_spacing.y],
                        egui::Label::new(RichText::new(label).size(12.0).color(row.severity.color()))
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    let column = match row.column {
                        Column::Unicode(column) => format!("column {column}"),
                        Column::Utf16(column) => format!(
                            "column {column} in UTF-16 code units, as reported by the language server (the file is not open)"
                        ),
                    };
                    let tooltip = format!(
                        "{}{}\n{}, line {}, {column}\n{}",
                        row.severity.label(),
                        row.source.as_deref().map(|source| format!(" · {source}")).unwrap_or_default(),
                        crate::paths::display(&row.path),
                        row.position.line + 1,
                        row.message
                    );
                    self.output_problem_focus_target(&response);
                    if design::keyboard_focus(&response) {
                        ui.painter().rect_stroke(
                            response.rect.expand(2.0),
                            2.0,
                            Stroke::new(1.0, ACCENT),
                            egui::StrokeKind::Outside,
                        );
                    }
                    if response.clicked() {
                        jump = Some((row.path.clone(), row.position));
                    }
                    response.on_hover_text(tooltip);
                }
            });
        if let Some((path, position)) = jump {
            self.reveal_lsp_position(path, position);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::{Diagnostic, Position, TextRange};

    fn diagnostic(line: u32, character: u32, severity: u8, message: &str) -> Diagnostic {
        Diagnostic {
            range: TextRange {
                start: Position { line, character },
                end: Position {
                    line,
                    character: character + 1,
                },
            },
            severity,
            message: message.into(),
            source: Some("test".into()),
        }
    }

    #[test]
    fn unicode_columns_convert_utf16_units_per_line() {
        let text = "a🦀b\r\nüx\n\tz";
        let positions = [
            Position {
                line: 0,
                character: 3,
            },
            Position {
                line: 1,
                character: 1,
            },
            Position {
                line: 0,
                character: 0,
            },
            Position {
                line: 2,
                character: 9,
            },
            Position {
                line: 7,
                character: 2,
            },
        ];
        // The crab is two UTF-16 units but one scalar; past the end clamps
        // to the line end; a missing line keeps column 1.
        assert_eq!(unicode_columns(text, &positions), vec![3, 2, 1, 3, 1]);
    }

    #[test]
    fn problems_sort_by_severity_then_path_then_position_and_count_without_hints() {
        let mut project = Project::new(PathBuf::from(r"C:\project"));
        let a = PathBuf::from(r"C:\project\a.rs");
        let b = PathBuf::from(r"C:\project\b.rs");
        project.diagnostics.insert(
            b.clone(),
            vec![
                diagnostic(4, 0, 2, "b warning"),
                diagnostic(1, 0, 1, "b error"),
                diagnostic(0, 0, 4, "b hint"),
            ],
        );
        project.diagnostics.insert(
            a.clone(),
            vec![
                diagnostic(9, 0, 3, "a info"),
                diagnostic(7, 2, 1, "a error late"),
                diagnostic(7, 1, 1, "a error early"),
            ],
        );
        let list = build_problem_list(&project);
        let messages: Vec<_> = list.rows.iter().map(|row| row.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "a error early",
                "a error late",
                "b error",
                "b warning",
                "a info",
                "b hint"
            ]
        );
        assert_eq!(list.counts, [3, 1, 1, 1]);
        assert_eq!(list.counted(), 5);
        assert_eq!(list.badge_color(), RED);
        assert_eq!(
            list.summary(),
            "3 errors · 1 warning · 1 information message · 1 hint"
        );
        assert!(matches!(list.rows[0].column, Column::Utf16(2)));
        assert_eq!(Severity::from_lsp(3).color(), BLUE);
        assert_eq!(Severity::from_lsp(4).color(), MUTED);
    }

    #[test]
    fn open_documents_give_unicode_columns() {
        let mut project = Project::new(PathBuf::from(r"C:\project"));
        let path = PathBuf::from(r"C:\project\emoji.rs");
        let mut tab = EditorTab::new(Some(path.clone()), "emoji.rs".into());
        tab.text = "let 🦀 = x;\n".into();
        project.tabs.push(tab);
        project
            .diagnostics
            .insert(path, vec![diagnostic(0, 7, 1, "unused")]);
        let list = build_problem_list(&project);
        assert!(matches!(list.rows[0].column, Column::Unicode(7)));
    }

    #[test]
    fn next_problem_finishes_the_current_file_then_continues_and_wraps() {
        let a = PathBuf::from(r"C:\p\a.rs");
        let b = PathBuf::from(r"C:\p\b.rs");
        let c = PathBuf::from(r"C:\p\c.rs");
        let diagnostics = HashMap::from([
            (
                a.clone(),
                vec![diagnostic(10, 0, 2, "w"), diagnostic(2, 4, 1, "e")],
            ),
            (b.clone(), vec![diagnostic(1, 0, 4, "hint only")]),
            (c.clone(), vec![diagnostic(5, 1, 1, "e")]),
        ]);
        let at = |line, character| Position { line, character };
        let go = |path: &Path, position, backwards| {
            next_problem(&diagnostics, Some((path, position)), backwards)
        };
        assert_eq!(go(&a, at(0, 0), false), Some((a.clone(), at(2, 4))));
        // A problem at the cursor is passed over.
        assert_eq!(go(&a, at(2, 4), false), Some((a.clone(), at(10, 0))));
        // Hints are skipped: b.rs has none to visit.
        assert_eq!(go(&a, at(10, 0), false), Some((c.clone(), at(5, 1))));
        assert_eq!(go(&c, at(5, 1), false), Some((a.clone(), at(2, 4))));
        assert_eq!(go(&b, at(0, 0), false), Some((c.clone(), at(5, 1))));
        assert_eq!(go(&a, at(2, 4), true), Some((c.clone(), at(5, 1))));
        assert_eq!(go(&c, at(5, 1), true), Some((a.clone(), at(10, 0))));
        assert_eq!(go(&a, at(5, 0), true), Some((a.clone(), at(2, 4))));
        assert_eq!(
            next_problem(&diagnostics, None, false),
            Some((a.clone(), at(2, 4)))
        );
        let only = HashMap::from([(a.clone(), vec![diagnostic(3, 0, 1, "e")])]);
        assert_eq!(
            next_problem(&only, Some((&a, at(3, 0))), false),
            Some((a.clone(), at(3, 0)))
        );
        assert_eq!(next_problem(&HashMap::new(), None, false), None);
    }
}
