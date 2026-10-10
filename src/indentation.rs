//! Document-aware indentation. Detection is bounded and cached by revision;
//! preferences only affect later edits and never rewrite existing text.
use super::*;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Indent {
    pub width: usize,
    pub tabs: bool,
    pub detected: bool,
}

impl Indent {
    pub fn unit(self) -> String {
        if self.tabs {
            "\t".into()
        } else {
            " ".repeat(self.width)
        }
    }

    pub fn label(self) -> String {
        format!(
            "{}: {}{}",
            if self.tabs { "Tabs" } else { "Spaces" },
            self.width,
            if self.detected { " · detected" } else { "" }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Detected {
    width: Option<usize>,
    tabs: bool,
}

#[derive(Clone, Copy)]
struct Cached {
    revision: u64,
    byte_len: usize,
    detected: Option<Detected>,
}

#[derive(Default)]
pub(super) struct Cache(Cell<Option<Cached>>);

impl Cache {
    pub(super) fn invalidate(&self) {
        self.0.set(None);
    }

    fn get(&self, text: &str, revision: u64) -> Option<Detected> {
        if let Some(cached) = self.0.get()
            && cached.revision == revision
            && cached.byte_len == text.len()
        {
            return cached.detected;
        }
        let result = detect(text);
        self.0.set(Some(Cached {
            revision,
            byte_len: text.len(),
            detected: result,
        }));
        result
    }
}

pub(super) fn default_widths() -> HashMap<String, usize> {
    [
        ("python", 4),
        ("dart", 2),
        ("javascript", 2),
        ("typescript", 2),
        ("json", 2),
        ("yaml", 2),
        ("html", 2),
        ("css", 2),
        ("go", 4),
        ("makefile", 4),
    ]
    .into_iter()
    .map(|(language, width)| (language.into(), width))
    .collect()
}

pub(super) fn default_tab_styles() -> HashMap<String, bool> {
    default_widths()
        .into_keys()
        .map(|language| {
            let tabs = matches!(language.as_str(), "go" | "makefile");
            (language, tabs)
        })
        .collect()
}

impl EditorTab {
    pub(super) fn indentation(&self, settings: &Settings) -> Indent {
        let language = crate::file_kind::language_id(self.path.as_deref());
        let mut indent = Indent {
            width: settings
                .language_indent_widths
                .get(language)
                .copied()
                .unwrap_or(settings.tab_width)
                .clamp(1, 8),
            tabs: settings
                .language_indent_with_tabs
                .get(language)
                .copied()
                .unwrap_or(settings.indent_with_tabs),
            detected: false,
        };
        if settings.detect_indentation
            && let Some(detected) = self.indentation_cache.get(&self.text, self.revision)
        {
            indent.width = detected.width.unwrap_or(indent.width);
            indent.tabs = detected.tabs;
            indent.detected = true;
        }
        indent
    }
}

fn detect(text: &str) -> Option<Detected> {
    let mut end = text.len().min(65_536);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut space_lines = 0usize;
    let mut tab_lines = 0usize;
    let mut widths = [0usize; 9];
    let mut previous = 0usize;
    for line in text[..end].lines().take(400) {
        if line.trim().is_empty() {
            continue;
        }
        let leading = line.bytes().take_while(|byte| matches!(byte, b' ' | b'\t'));
        let mut spaces = 0usize;
        let mut tabs = 0usize;
        for byte in leading {
            if byte == b'\t' {
                tabs += 1;
            } else {
                spaces += 1;
            }
        }
        if tabs > 0 {
            tab_lines += 1;
            previous = 0;
        } else {
            if spaces > 0 {
                space_lines += 1;
            }
            let delta = spaces.abs_diff(previous);
            // One-space alignment is too ambiguous to override user settings.
            if (2..=8).contains(&delta) {
                widths[delta] += 1;
            }
            previous = spaces;
        }
    }
    if tab_lines > 0 && tab_lines >= space_lines * 2 {
        return Some(Detected {
            width: None,
            tabs: true,
        });
    }
    if space_lines == 0 || tab_lines > space_lines {
        return None;
    }
    let width = (2..=8).max_by_key(|&width| (widths[width], std::cmp::Reverse(width)))?;
    (widths[width] > 0).then_some(Detected {
        width: Some(width),
        tabs: false,
    })
}

impl VEditApp {
    pub(super) fn indentation_settings_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        editor_preferences::preference_heading(ui, "INDENTATION");
        changed |= ui
            .checkbox(
                &mut self.settings.detect_indentation,
                "Detect indentation from existing files",
            )
            .changed();
        ui.label(RichText::new("Detected indentation takes priority. Disable detection to always use your settings. Existing text is never reformatted.")
            .size(10.5).color(MUTED));
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::Slider::new(&mut self.settings.tab_width, 1..=8).text("Default width"))
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.indent_with_tabs, "Use tabs")
                .changed();
        });
        let id = ui.id().with("indentation_language");
        let initial = self
            .tab()
            .map(|tab| crate::file_kind::language_id(tab.path.as_deref()))
            .filter(|language| *language != "plaintext")
            .unwrap_or("python");
        let mut language = ui
            .ctx()
            .data_mut(|data| data.get_temp::<String>(id))
            .unwrap_or_else(|| initial.into());
        let choices = [
            ("python", "Python"),
            ("dart", "Dart"),
            ("rust", "Rust"),
            ("javascript", "JavaScript"),
            ("typescript", "TypeScript"),
            ("json", "JSON"),
            ("yaml", "YAML"),
            ("html", "HTML"),
            ("css", "CSS"),
            ("go", "Go"),
            ("java", "Java"),
            ("c", "C"),
            ("cpp", "C++"),
            ("csharp", "C#"),
            ("shellscript", "Shell"),
            ("lua", "Lua"),
            ("sql", "SQL"),
            ("toml", "TOML"),
            ("xml", "XML"),
            ("markdown", "Markdown"),
            ("kotlin", "Kotlin"),
            ("swift", "Swift"),
            ("scss", "SCSS"),
            ("less", "Less"),
            ("powershell", "PowerShell"),
            ("bat", "Batch"),
            ("ruby", "Ruby"),
            ("php", "PHP"),
            ("r", "R"),
            ("julia", "Julia"),
            ("ini", "INI"),
            ("dockerfile", "Dockerfile"),
            ("makefile", "Makefile"),
            ("cmake", "CMake"),
            ("latex", "LaTeX"),
            ("ignore", "Ignore patterns"),
            ("plaintext", "Plain text"),
        ];
        let label = choices
            .iter()
            .find(|(key, _)| *key == language)
            .map_or(language.as_str(), |(_, label)| label)
            .to_owned();
        egui::ComboBox::from_label("Language indentation")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (value, label) in choices {
                    ui.selectable_value(&mut language, value.to_owned(), label);
                }
            });
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, language.clone()));
        let mut custom = self.settings.language_indent_widths.contains_key(&language)
            || self
                .settings
                .language_indent_with_tabs
                .contains_key(&language);
        if ui
            .checkbox(&mut custom, "Use a separate setting for this language")
            .changed()
        {
            if custom {
                self.settings
                    .language_indent_widths
                    .insert(language.clone(), self.settings.tab_width);
                self.settings
                    .language_indent_with_tabs
                    .insert(language.clone(), self.settings.indent_with_tabs);
            } else {
                self.settings.language_indent_widths.remove(&language);
                self.settings.language_indent_with_tabs.remove(&language);
            }
            changed = true;
        }
        if custom {
            ui.horizontal(|ui| {
                let width = self
                    .settings
                    .language_indent_widths
                    .entry(language.clone())
                    .or_insert(self.settings.tab_width);
                changed |= ui
                    .add(egui::Slider::new(width, 1..=8).text("Language width"))
                    .changed();
                let tabs = self
                    .settings
                    .language_indent_with_tabs
                    .entry(language)
                    .or_insert(self.settings.indent_with_tabs);
                changed |= ui.checkbox(tabs, "Use tabs for this language").changed();
            });
        }
        if let Some(tab) = self.tab() {
            ui.label(
                RichText::new(format!(
                    "Current document: {}",
                    tab.indentation(&self.settings).label()
                ))
                .size(10.5)
                .color(ACCENT),
            );
        }
        changed
    }

    pub(super) fn indentation_status_ui(&mut self, ui: &mut egui::Ui) {
        let Some(tab) = self.tab() else {
            return;
        };
        let indent = tab.indentation(&self.settings);
        if design::ghost_button(ui, &indent.label(),
            "Indentation for this document. Click to configure detection, language widths and tabs. Preferences affect new edits only.").clicked() {
            self.open_settings_section("Editing");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(path: &str, text: &str) -> EditorTab {
        let mut tab = EditorTab::new(Some(PathBuf::from(path)), path.into());
        tab.text = text.into();
        tab
    }

    #[test]
    fn language_defaults_and_explicit_overrides_are_distinct() {
        let mut settings = Settings::default();
        assert_eq!(tab("a.py", "").indentation(&settings).unit(), "    ");
        assert_eq!(tab("a.dart", "").indentation(&settings).unit(), "  ");
        assert_eq!(tab("a.go", "").indentation(&settings).unit(), "\t");
        assert_eq!(tab("Makefile", "").indentation(&settings).unit(), "\t");
        settings.language_indent_widths.insert("python".into(), 3);
        assert_eq!(tab("a.py", "").indentation(&settings).unit(), "   ");
        settings.language_indent_widths.remove("python");
        settings.tab_width = 8;
        assert_eq!(tab("a.py", "").indentation(&settings).width, 8);
    }

    #[test]
    fn detection_tracks_revisions_and_can_be_disabled_without_reformatting() {
        let mut settings = Settings::default();
        let mut document = tab("a.py", "if ready:\n  run()\n");
        assert_eq!(document.indentation(&settings).width, 2);
        assert!(document.indentation(&settings).detected);
        document.text = "if ready:\n\trun()\n".into();
        document.changed();
        assert_eq!(document.indentation(&settings).unit(), "\t");
        settings.detect_indentation = false;
        assert_eq!(document.indentation(&settings).unit(), "    ");
        assert_eq!(document.text, "if ready:\n\trun()\n");
    }

    #[test]
    fn blank_lines_and_alignment_do_not_override_consistent_width() {
        let document = tab(
            "a.dart",
            "void main() {\n    first();\n\n    if (ok) {\n        next();\n    }\n}\n",
        );
        assert_eq!(document.indentation(&Settings::default()).width, 4);
        assert!(detect("zero\n next\n").is_none());
        assert!(detect("\n \n\t\n").is_none());
    }

    #[test]
    fn detection_bounds_large_and_unicode_documents() {
        let document = format!("{}é\n  value", "x".repeat(65_535));
        assert!(detect(&document).is_none());
        let document = format!("{}    later\n", "unindented\n".repeat(400));
        assert!(detect(&document).is_none());
    }

    #[test]
    fn reload_invalidates_same_length_same_revision_detection() {
        let cache = Cache::default();
        assert_eq!(cache.get("x\n  y\n", 1).unwrap().width, Some(2));
        cache.invalidate();
        assert!(cache.get("x\n\ty \n", 1).unwrap().tabs);
    }
}
