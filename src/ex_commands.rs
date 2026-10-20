//! A bounded Ex command line and atomic, line-scoped regex substitutions.
use super::*;

const HISTORY_LIMIT: usize = 100;

#[derive(Clone, Default)]
struct CommandState {
    history: Vec<String>,
    history_index: Option<usize>,
    draft: String,
    visual_lines: Option<Range<usize>>,
    visual_document: Option<(u64, u64)>,
    substitute: Option<Substitute>,
    trailing_events: Vec<egui::Event>,
}

#[derive(Clone, Debug)]
struct Substitute {
    pattern: String,
    replacement: String,
    global: bool,
    insensitive: bool,
    count_only: bool,
    allow_empty: bool,
}

struct Substitution {
    edit: editor::EditResult,
    count: usize,
    lines: usize,
    spec: Substitute,
}

fn state_id() -> egui::Id {
    egui::Id::new("vim_ex_command_state")
}

fn address<'a>(
    input: &'a str,
    current: usize,
    total: usize,
    visual: Option<&Range<usize>>,
) -> Result<(usize, &'a str), String> {
    let (mut line, mut rest) = if let Some(rest) = input.strip_prefix('.') {
        (current, rest)
    } else if let Some(rest) = input.strip_prefix('$') {
        (total - 1, rest)
    } else if let Some(rest) = input.strip_prefix("'<") {
        (
            visual.ok_or("No visual selection for this range")?.start,
            rest,
        )
    } else if let Some(rest) = input.strip_prefix("'>") {
        (
            visual
                .ok_or("No visual selection for this range")?
                .end
                .saturating_sub(1),
            rest,
        )
    } else {
        let digits = input.bytes().take_while(u8::is_ascii_digit).count();
        let value = input[..digits]
            .parse::<usize>()
            .map_err(|_| "Expected a line number, . or $".to_owned())?;
        if value == 0 || value > total {
            return Err(format!("Line number must be between 1 and {total}"));
        }
        (value - 1, &input[digits..])
    };
    while let Some(sign) = rest.chars().next().filter(|ch| matches!(ch, '+' | '-')) {
        rest = &rest[1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let offset = if digits == 0 {
            1
        } else {
            rest[..digits]
                .parse::<usize>()
                .map_err(|_| "Line offset is too large")?
        };
        rest = &rest[digits..];
        line = if sign == '+' {
            line.checked_add(offset)
        } else {
            line.checked_sub(offset)
        }
        .filter(|line| *line < total)
        .ok_or_else(|| format!("Line number must be between 1 and {total}"))?;
    }
    if line >= total {
        return Err("Visual selection is no longer in this document".into());
    }
    Ok((line, rest.trim_start()))
}

fn field(input: &str, delimiter: char) -> (String, &str, bool) {
    let mut value = String::new();
    let mut escaped = false;
    for (at, ch) in input.char_indices() {
        if !escaped && ch == delimiter {
            return (value, &input[at + ch.len_utf8()..], true);
        }
        value.push(ch);
        escaped = ch == '\\' && !escaped;
    }
    (value, "", false)
}

fn parse_spec(
    input: &str,
    previous: Option<&Substitute>,
    search: Option<&str>,
) -> Result<Substitute, String> {
    if input.is_empty() {
        return previous
            .cloned()
            .ok_or_else(|| "No previous substitution".into());
    }
    let delimiter = input.chars().next().unwrap();
    if !delimiter.is_ascii_punctuation() || delimiter == '\\' {
        return Err("Use :s/pattern/replacement/flags (flags: g i I n e)".into());
    }
    let (pattern, rest, closed) = field(&input[delimiter.len_utf8()..], delimiter);
    if !closed {
        return Err("Missing delimiter after the substitution pattern".into());
    }
    let (replacement, flags, _) = field(rest, delimiter);
    let mut spec = Substitute {
        pattern,
        replacement,
        global: false,
        insensitive: false,
        count_only: false,
        allow_empty: false,
    };
    if spec.pattern.is_empty() {
        spec.pattern = search
            .map(str::to_owned)
            .or_else(|| previous.map(|previous| previous.pattern.clone()))
            .ok_or("No previous search or substitution pattern")?;
    }
    for flag in flags.trim().chars() {
        match flag {
            'g' => spec.global = true,
            'i' => spec.insensitive = true,
            'I' => spec.insensitive = false,
            'n' => spec.count_only = true,
            'e' => spec.allow_empty = true,
            'c' => {
                return Err(
                    "Interactive substitution confirmation is not supported; use Find and Replace"
                        .into(),
                );
            }
            _ => return Err(format!("Unsupported substitution flag: {flag}")),
        }
    }
    Ok(spec)
}

/// The number of lines in Vim's model: a final line break terminates the last
/// line rather than starting an empty one.
fn last_line_number(text: &str) -> usize {
    text.split('\n').count() - usize::from(text.ends_with('\n'))
}

#[derive(Clone, Copy)]
enum Case {
    Upper,
    Lower,
}

/// Expand a Vim replacement: `&` and `\0`-`\9` insert the match and its
/// groups, `\r` and `\n` a line break, `\t` a Tab. `\u` / `\l` change the
/// case of the next character, `\U` / `\L` that of the following text until
/// `\E` or `\e`. Other escaped characters are inserted literally.
fn replacement(captures: &regex::Captures<'_>, template: &str, ending: &str) -> String {
    struct Output {
        text: String,
        next_char: Option<Case>,
        following: Option<Case>,
    }
    impl Output {
        fn push(&mut self, text: &str) {
            for ch in text.chars() {
                match self.next_char.take().or(self.following) {
                    Some(Case::Upper) => self.text.extend(ch.to_uppercase()),
                    Some(Case::Lower) => self.text.extend(ch.to_lowercase()),
                    None => self.text.push(ch),
                }
            }
        }
    }
    let mut out = Output {
        text: String::new(),
        next_char: None,
        following: None,
    };
    let mut chars = template.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '&' => out.push(captures.get(0).unwrap().as_str()),
            '\\' => match chars.next() {
                Some('r' | 'n') => out.text.push_str(ending),
                Some('t') => out.push("\t"),
                Some('u') => out.next_char = Some(Case::Upper),
                Some('l') => out.next_char = Some(Case::Lower),
                Some('U') => out.following = Some(Case::Upper),
                Some('L') => out.following = Some(Case::Lower),
                Some('E' | 'e') => out.following = None,
                Some(digit @ '0'..='9') => {
                    if let Some(found) = captures.get(digit as usize - '0' as usize) {
                        out.push(found.as_str());
                    }
                }
                Some(literal) => out.push(literal.encode_utf8(&mut [0; 4])),
                None => out.text.push('\\'),
            },
            _ => out.push(ch.encode_utf8(&mut [0; 4])),
        }
    }
    out.text
}

#[cfg(test)]
fn substitute(
    command: &str,
    text: &str,
    cursor: usize,
    visual: Option<&Range<usize>>,
    previous: Option<&Substitute>,
) -> Result<Option<Substitution>, String> {
    substitute_with_search(command, text, cursor, visual, previous, None)
}

fn substitute_with_search(
    command: &str,
    text: &str,
    cursor: usize,
    visual: Option<&Range<usize>>,
    previous: Option<&Substitute>,
    search: Option<&str>,
) -> Result<Option<Substitution>, String> {
    let rows: Vec<_> = text.split('\n').collect();
    // A final line break terminates the last line; the empty text after it is
    // not a line that ranges such as `%` or `$` address.
    let line_count = last_line_number(text).max(1);
    let current = text
        .chars()
        .take(cursor)
        .filter(|&ch| ch == '\n')
        .count()
        .min(line_count - 1);
    let mut input = command;
    let range = if let Some(rest) = input.strip_prefix('%') {
        input = rest.trim_start();
        0..line_count
    } else if input.starts_with(['.', '$', '\''])
        || input.starts_with(|ch: char| ch.is_ascii_digit())
    {
        let (first, rest) = address(input, current, line_count, visual)?;
        input = rest;
        let last = if let Some(rest) = input.strip_prefix(',') {
            let (last, rest) = address(rest.trim_start(), current, line_count, visual)?;
            input = rest;
            last
        } else {
            first
        };
        if first > last {
            return Err("The first range line must precede the last".into());
        }
        first..last + 1
    } else {
        current..current + 1
    };
    let Some(body) = input
        .strip_prefix("substitute")
        .or_else(|| input.strip_prefix('s'))
    else {
        return Ok(None);
    };
    let spec = parse_spec(body.trim_start(), previous, search)?;
    let (pattern, insensitive) = regex_case_overrides(&spec.pattern, spec.insensitive);
    let regex = regex::RegexBuilder::new(&pattern)
        .case_insensitive(insensitive)
        .size_limit(256 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|error| format!("Invalid substitution pattern: {error}"))?;
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut result = if spec.count_only {
        String::new()
    } else {
        String::with_capacity(text.len())
    };
    let mut count = 0;
    let mut changed_lines = 0;
    let mut caret = cursor;
    for (row, source) in rows.iter().enumerate() {
        let crlf = row + 1 < rows.len() && source.ends_with('\r');
        let line = if crlf {
            &source[..source.len() - 1]
        } else {
            source
        };
        let mut end = 0;
        let mut line_matches = 0;
        if range.contains(&row) {
            for capture in regex.captures_iter(line) {
                let found = capture.get(0).unwrap();
                if !spec.count_only {
                    result.push_str(&line[end..found.start()]);
                    if count == 0 {
                        caret = result.chars().count();
                    }
                    result.push_str(&replacement(&capture, &spec.replacement, ending));
                }
                end = found.end();
                count += 1;
                line_matches += 1;
                if result.len() > (MAX_DOCUMENT as usize).max(text.len()) {
                    return Err("Substitution would exceed the document size limit".into());
                }
                if !spec.global {
                    break;
                }
            }
        }
        changed_lines += usize::from(line_matches > 0);
        if !spec.count_only {
            result.push_str(&line[end..]);
            if crlf {
                result.push('\r');
            }
            if row + 1 < rows.len() {
                result.push('\n');
            }
        }
    }
    if count == 0 && !spec.allow_empty && !spec.count_only {
        return Err(format!("Pattern not found: {}", spec.pattern));
    }
    if result.len() > (MAX_DOCUMENT as usize).max(text.len()) {
        return Err("Substitution would exceed the document size limit".into());
    }
    if spec.count_only {
        result = text.to_owned();
        caret = cursor;
    }
    Ok(Some(Substitution {
        edit: editor::EditResult {
            text: result,
            selection: caret..caret,
        },
        count,
        lines: changed_lines,
        spec,
    }))
}

fn regex_case_overrides(query: &str, mut insensitive: bool) -> (String, bool) {
    let mut pattern = String::with_capacity(query.len());
    let mut chars = query.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            pattern.push(ch);
            continue;
        }
        match chars.next() {
            Some('c') => insensitive = true,
            Some('C') => insensitive = false,
            Some(escaped) => {
                pattern.push('\\');
                pattern.push(escaped);
            }
            None => pattern.push('\\'),
        }
    }
    (pattern, insensitive)
}

impl VEditApp {
    pub(super) fn begin_ex_command(&mut self, mut command: String) {
        let visual_lines = self.tab().and_then(|tab| {
            tab.vim.selection(&tab.text).map(|selection| {
                let start = tab
                    .text
                    .chars()
                    .take(selection.start)
                    .filter(|&ch| ch == '\n')
                    .count();
                let end = tab
                    .text
                    .chars()
                    .take(selection.end.saturating_sub(1))
                    .filter(|&ch| ch == '\n')
                    .count();
                start..end + 1
            })
        });
        if visual_lines.is_some() {
            command.insert_str(0, "'<,'>");
        }
        self.ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<CommandState>(state_id());
            state.visual_lines = visual_lines;
            state.visual_document = self.tab().map(|tab| (tab.id, tab.revision));
            state.history_index = None;
            state.draft.clear();
            state.trailing_events.clear();
        });
        let cursor = command.chars().count();
        self.vim_command = Some(command);
        // A batched Vim text event can open Ex before a later Space makes the
        // leader dispatcher hand its suffix to the native field. Focus and
        // position its caret before rendering so that suffix reaches Ex.
        let id = egui::Id::new("ex-command");
        let mut state = egui::TextEdit::load_state(&self.ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(cursor),
            )));
        state.store(&self.ctx, id);
        self.ctx.memory_mut(|memory| memory.request_focus(id));
    }

    pub(super) fn execute_ex_command(&mut self, command: &str) {
        let command = command.trim().trim_start_matches(':').trim_start();
        if command.is_empty() {
            return;
        }
        self.ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<CommandState>(state_id());
            if state
                .history
                .last()
                .is_none_or(|previous| previous != command)
            {
                state.history.push(command.to_owned());
                if state.history.len() > HISTORY_LIMIT {
                    state.history.remove(0);
                }
            }
            state.history_index = None;
        });
        let (name, argument) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let argument = argument.trim();
        match name {
            "w" | "write" if argument.is_empty() => self.save_current(),
            "w" | "write" => {
                if let (Some(tab), Some(project)) = (self.tab(), self.project()) {
                    self.save_tab(tab.id, Some(project.root.join(argument.trim_matches('"'))));
                }
            }
            "wa" | "wall" if argument.is_empty() => self.save_all(),
            "q" | "quit" if argument.is_empty() => {
                if let Some(tab) = self.tab() {
                    self.request_close(CloseAction::PaneTab {
                        pane_id: self.current_pane_id(),
                        id: tab.id,
                    });
                }
            }
            "q!" | "quit!" if argument.is_empty() => {
                if let Some(tab) = self.tab() {
                    self.perform_close(CloseAction::PaneTab {
                        pane_id: self.current_pane_id(),
                        id: tab.id,
                    });
                }
            }
            "wq" | "x" | "exit" if argument.is_empty() => {
                if let Some(tab) = self.tab() {
                    self.close_action = Some(CloseAction::PaneTab {
                        pane_id: self.current_pane_id(),
                        id: tab.id,
                    });
                    self.close_after_save = true;
                    self.save_current();
                }
            }
            "noh" | "nohlsearch" if argument.is_empty() => {
                self.find_visible = false;
                self.find.highlighted = false;
            }
            "e" | "edit" if argument.is_empty() => self.reload_current(),
            "e" | "edit" => {
                if let Some(project) = self.project() {
                    self.open_file(project.root.join(argument.trim_matches('"')));
                }
            }
            "bn" | "bnext" if argument.is_empty() => {
                self.run_shortcut_action(keybindings::Action::NextTab)
            }
            "bp" | "bprevious" if argument.is_empty() => {
                self.run_shortcut_action(keybindings::Action::PreviousTab)
            }
            "new" | "enew" if argument.is_empty() => self.new_file(),
            "sp" | "split" | "vs" | "vsp" | "vsplit" => {
                let orientation = if name.starts_with('v') {
                    crate::cli::SplitOrientation::Vertical
                } else {
                    crate::cli::SplitOrientation::Horizontal
                };
                match self.split_active(orientation) {
                    Ok(_) => {
                        if !argument.is_empty()
                            && let Some(project) = self.project()
                        {
                            self.open_file(project.root.join(argument.trim_matches('"')));
                        }
                    }
                    Err(error) => self.status(error, true),
                }
            }
            "set" => self.ex_set(argument),
            _ => {
                let line = command.parse::<usize>().ok().or_else(|| {
                    (command == "$")
                        .then(|| self.tab().map_or(1, |tab| last_line_number(&tab.text)))
                });
                if let Some(line) = line {
                    if let Some(tab) = self.tab_mut() {
                        let cursor = editor::go_to_line(&tab.text, line);
                        tab.set_cursor(cursor..cursor);
                    }
                    return;
                }
                let state = self.ctx.data_mut(|data| {
                    data.get_temp_mut_or_default::<CommandState>(state_id())
                        .clone()
                });
                let Some(tab) = self
                    .tab()
                    .filter(|tab| !tab.loading && !tab.saving && tab.error.is_none())
                else {
                    self.status("Wait for the file to become editable", true);
                    return;
                };
                let visual = state
                    .visual_lines
                    .as_ref()
                    .filter(|_| state.visual_document == Some((tab.id, tab.revision)));
                let search = self.find.last_substitute_pattern();
                match substitute_with_search(
                    command,
                    &tab.text,
                    tab.vim.cursor,
                    visual,
                    state.substitute.as_ref(),
                    search.as_deref(),
                ) {
                    Ok(Some(result)) => {
                        if !result.spec.count_only
                            && let Some(tab) = self.tab_mut()
                            && tab.text != result.edit.text
                        {
                            tab.apply(result.edit);
                            tab.vim.notify_external_text_change();
                            tab.set_cursor(tab.selection.clone());
                        }
                        self.status(
                            format!(
                                "{} {} on {} lines",
                                result.count,
                                if result.spec.count_only {
                                    "matches"
                                } else {
                                    "substitutions"
                                },
                                result.lines
                            ),
                            false,
                        );
                        self.find.remember_substitute_pattern(
                            result.spec.pattern.clone(),
                            !result.spec.insensitive,
                        );
                        self.ctx.data_mut(|data| {
                            data.get_temp_mut_or_default::<CommandState>(state_id())
                                .substitute = Some(result.spec)
                        });
                    }
                    Ok(None) => self.status(format!("Unknown command: {command}"), true),
                    Err(error) => self.status(error, true),
                }
            }
        }
    }

    fn ex_set(&mut self, argument: &str) {
        use crate::cli::LineNumbers;
        if argument.is_empty() {
            self.status(
                format!(
                    "{} · line numbers: {:?}",
                    if self.settings.word_wrap {
                        "wrap"
                    } else {
                        "nowrap"
                    },
                    self.settings.line_numbers
                ),
                false,
            );
            return;
        }
        let mut settings = self.settings.clone();
        for option in argument.split_whitespace() {
            match option {
                "wrap" => settings.word_wrap = true,
                "nowrap" => settings.word_wrap = false,
                "nu" | "number" => settings.line_numbers = LineNumbers::Absolute,
                "nonu" | "nonumber" => settings.line_numbers = LineNumbers::Off,
                "rnu" | "relativenumber" => settings.line_numbers = LineNumbers::Relative,
                "nornu" | "norelativenumber" => settings.line_numbers = LineNumbers::Absolute,
                _ => {
                    self.status(format!("Unsupported option: {option}"), true);
                    return;
                }
            }
        }
        self.settings = settings;
        self.session_dirty = true;
    }

    pub(super) fn ex_history_key(&mut self, ui: &mut egui::Ui) {
        let backwards =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
        let forwards =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
        if !backwards && !forwards {
            return;
        }
        let Some(command) = &mut self.vim_command else {
            return;
        };
        self.ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<CommandState>(state_id());
            if state.history.is_empty() {
                return;
            }
            if backwards {
                if state.history_index.is_none() {
                    state.draft = command.clone();
                }
                let index = state
                    .history_index
                    .unwrap_or(state.history.len())
                    .saturating_sub(1);
                state.history_index = Some(index);
                *command = state.history[index].clone();
            } else if let Some(index) = state.history_index {
                if index + 1 < state.history.len() {
                    state.history_index = Some(index + 1);
                    *command = state.history[index + 1].clone();
                } else {
                    state.history_index = None;
                    *command = state.draft.clone();
                }
            }
        });
        let id = egui::Id::new("ex-command");
        let mut state = egui::TextEdit::load_state(&self.ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(command.chars().count()),
            )));
        state.store(&self.ctx, id);
    }

    pub(super) fn ex_command_changed(&mut self) {
        self.ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<CommandState>(state_id());
            state.history_index = None;
            state.draft.clear();
        });
    }

    /// Native TextEdit must see only the command prefix. Input following its
    /// Enter/Escape resumes under the command's resulting source/modal owner.
    pub(super) fn ex_input_prefix(&mut self) {
        let trailing = self.ctx.input_mut(|input| {
            let boundary = input.events.iter().position(|event| {
                matches!(event,
                egui::Event::Key { key: Key::Enter | Key::Escape, pressed: true, modifiers, .. }
                    if *modifiers == egui::Modifiers::NONE)
            });
            boundary.map_or_else(Vec::new, |boundary| input.events.split_off(boundary + 1))
        });
        if !trailing.is_empty() {
            self.ctx.data_mut(|data| {
                data.get_temp_mut_or_default::<CommandState>(state_id())
                    .trailing_events
                    .extend(trailing)
            });
        }
    }

    pub(super) fn finish_ex_command(&mut self) {
        let trailing = self.ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<CommandState>(state_id());
            state.visual_lines = None;
            state.visual_document = None;
            state.history_index = None;
            std::mem::take(&mut state.trailing_events)
        });
        self.ctx
            .memory_mut(|memory| memory.surrender_focus(egui::Id::new("ex-command")));
        if self.save_as.is_none() && self.close_action.is_none() {
            let tab_id = self
                .tab_mut()
                .filter(|tab| tab.agent_view == crate::cli::DocumentView::Source)
                .map(|tab| {
                    tab.focus = true;
                    tab.id
                });
            if let Some(tab_id) = tab_id {
                let id = self.document_widget_id(tab_id);
                self.ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
        self.defer_widget_shortcut_input(trailing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(command: &str, text: &str, cursor: usize) -> Substitution {
        substitute(command, text, cursor, None, None)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn substitute_defaults_to_current_line_and_global_is_per_line() {
        assert_eq!(
            change("s/foo/x/", "foo foo\nfoo foo", 9).edit.text,
            "foo foo\nx foo"
        );
        let result = change("%s/foo/x/g", "foo foo\nfoo foo", 0);
        assert_eq!(result.edit.text, "x x\nx x");
        assert_eq!((result.count, result.lines), (4, 2));
    }

    #[test]
    fn substitute_ranges_capture_expansion_unicode_and_crlf() {
        let result = change(r"2,$s/(α)(β)/\2\1-&/g", "αβ\r\nαβ αβ\r\nαβ", 0);
        assert_eq!(result.edit.text, "αβ\r\nβα-αβ βα-αβ\r\nβα-αβ");
        assert_eq!(result.edit.selection, 4..4);
        assert_eq!(
            change(r"%s#α/β#\&-\\#", "α/β\r\nα/β\r\n", 0).edit.text,
            "&-\\\r\n&-\\\r\n"
        );
    }

    #[test]
    fn substitute_case_flags_count_only_and_zero_width_matches() {
        assert_eq!(change("%s/foo/x/gi", "Foo fOO\nfoo", 0).edit.text, "x x\nx");
        assert_eq!(change("%s/foo/x/giI", "Foo foo", 0).edit.text, "Foo x");
        let count = change("%s/foo/x/gn", "foo foo", 3);
        assert_eq!(count.edit.text, "foo foo");
        assert_eq!(count.edit.selection, 3..3);
        assert_eq!(count.count, 2);
        // A final line break terminates the last line; it starts no empty one.
        assert_eq!(
            change("%s/^/>/g", "α\r\nβ\r\n", 0).edit.text,
            ">α\r\n>β\r\n"
        );
    }

    #[test]
    fn ranges_use_vims_line_model_after_a_final_line_break() {
        assert_eq!(change("%s/^/# /", "a\nb\n", 0).edit.text, "# a\n# b\n");
        assert_eq!(change("%s/$/;/", "a\nb\n", 0).edit.text, "a;\nb;\n");
        assert_eq!(change("$s/x/y/", "x\nx\n", 0).edit.text, "x\ny\n");
        assert_eq!(change("s/x/y/", "x\nx\n", 4).edit.text, "x\ny\n");
        assert_eq!(change("%s/$/;/", "a\r\nb\r\n", 0).edit.text, "a;\r\nb;\r\n");
        assert_eq!(change("%s/^/>/", "", 0).edit.text, ">");
        assert_eq!(change("%s/^/>/", "\n", 0).edit.text, ">\n");
        assert_eq!(change("%s/$/;/", "a\nb", 0).edit.text, "a;\nb;");
        assert!(substitute("3s/x/y/", "x\nx\n", 0, None, None).is_err());
        assert_eq!(last_line_number("a\nb\n"), 2);
        assert_eq!(last_line_number("a\nb"), 2);
        assert_eq!(last_line_number(""), 1);
    }

    #[test]
    fn replacement_escapes_insert_tabs_line_breaks_and_change_case() {
        for (command, source, expected) in [
            (r"s/ /\t/g", "a b c", "a\tb\tc"),
            (r"s/,/\n/g", "a,b", "a\nb"),
            (r"s/,/\n/g", "a,b\r\nc", "a\r\nb\r\nc"),
            (r"s/\w+/\u&/g", "foo bar", "Foo Bar"),
            (r"s/\w+/\U&/", "foo bar", "FOO bar"),
            (r"s/(\w+) (\w+)/\U\1\E \2/", "foo bar", "FOO bar"),
            (r"s/(\w+) (\w+)/\U\1\e-\l\2/", "foo BAR", "FOO-bAR"),
            (r"s/\w+/\L&/", "FOO Bar", "foo Bar"),
            (r"s/(\w)(\w+)/\u\L&/", "hELLO", "Hello"),
            (r"s/x/\uü/", "x", "Ü"),
            (r"s/x/a\\tb/", "x", r"a\tb"),
            (r"s/x/~/", "x", "~"),
        ] {
            assert_eq!(change(command, source, 0).edit.text, expected, "{command}");
        }
    }

    #[test]
    fn substitute_validation_is_atomic_and_visual_range_is_respected() {
        for command in [
            "%s/[//g",
            "%s/x/y/c",
            "3,1s/x/y/",
            "0s/x/y/",
            "9s/x/y/",
            "s/x",
            "s/x/y/z",
        ] {
            assert!(
                substitute(command, "x\nx\nx", 0, None, None).is_err(),
                "{command}"
            );
        }
        let result = substitute("'<,'>s/x/y/", "x\nx\nx", 0, Some(&(1..2)), None)
            .unwrap()
            .unwrap();
        assert_eq!(result.edit.text, "x\ny\nx");
        assert!(substitute("s/missing/x/", "abc", 0, None, None).is_err());
        assert_eq!(change("s/missing/x/e", "abc", 0).count, 0);
    }

    #[test]
    fn substitution_reuses_pattern_and_repeats_command() {
        let previous = change("s/foo/x/g", "foo", 0).spec;
        let repeated = substitute("%s", "foo foo\nfoo", 0, None, Some(&previous))
            .unwrap()
            .unwrap();
        assert_eq!(repeated.edit.text, "x x\nx");
        let empty = substitute("%s//y/g", "foo foo", 0, None, Some(&previous))
            .unwrap()
            .unwrap();
        assert_eq!(empty.edit.text, "y y");
        assert_eq!(change(r"s/x/a\rb/", "x\r\ny", 0).edit.text, "a\r\nb\r\ny");
    }

    #[test]
    fn count_only_never_expands_a_replacement_or_hits_the_output_size_limit() {
        let command = format!("%s/x/{}/gn", "y".repeat(1024));
        let source = "x".repeat(8193);
        let result = change(&command, &source, 3);
        assert_eq!(result.count, 8193);
        assert_eq!(result.edit.text, source);
        assert_eq!(result.edit.selection, 3..3);
    }

    #[test]
    fn empty_substitution_pattern_uses_the_latest_search_and_case_overrides() {
        let previous = change("s/old/x/g", "old", 0).spec;
        let result = substitute_with_search(
            "%s//x/g",
            "old new NEW",
            0,
            None,
            Some(&previous),
            Some(r"new\c"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.edit.text, "old x x");
        assert_eq!(change(r"s/new\c\C/x/g", "new NEW", 0).edit.text, "x NEW");
        assert_eq!(change(r"s#path\#name#ok#", "path#name", 0).edit.text, "ok");
    }

    #[test]
    fn relative_line_offsets_are_checked_before_any_edit() {
        assert_eq!(
            change(".+1,$-1s/x/y/", "x\nx\nx\nx", 0).edit.text,
            "x\ny\ny\nx"
        );
        for command in [".-1s/x/y/", "$+1s/x/y/", ".+999999999999999999999s/x/y/"] {
            assert!(substitute(command, "x\nx", 0, None, None).is_err());
        }
        assert!(substitute("'<,'>s/x/y/", "x", 0, Some(&(3..5)), None).is_err());
    }

    fn fixture(document: &str) -> VEditApp {
        let mut app = VEditApp::empty(egui::Context::default());
        app.settings.autosave = false;
        app.settings.smooth_scroll = false;
        app.sidebar_visible = false;
        let root = std::env::temp_dir().join("vedit-ex-render-only");
        let mut project = Project::new(root.clone());
        project.last_git_refresh = Instant::now();
        let mut tab = EditorTab::new(Some(root.join("notes.txt")), "notes.txt".into());
        tab.text = document.into();
        tab.saved_text = document.into();
        project.tabs.push(tab);
        app.projects.push(project);
        app.bind_current_pane();
        app
    }

    fn harness(app: VEditApp) -> egui_kittest::Harness<'static, VEditApp> {
        let mut screen = egui_kittest::Harness::builder()
            .with_size([1100.0, 700.0])
            .with_os(egui::os::OperatingSystem::Windows)
            .build_ui_state(
                |ui, app: &mut VEditApp| {
                    app.ctx = ui.ctx().clone();
                    app.last_disk_check = Instant::now();
                    app.last_session_save = Instant::now();
                    for project in &mut app.projects {
                        project.last_git_refresh = Instant::now();
                    }
                    app.draw_ui(ui);
                },
                app,
            );
        set_style(&screen.ctx);
        screen.ctx.all_styles_mut(|style| {
            style.visuals.text_cursor.blink = false;
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        screen.run_steps(3);
        screen
    }

    fn text(screen: &mut egui_kittest::Harness<'_, VEditApp>, input: &str) {
        screen.event(egui::Event::Text(input.into()));
        screen.run_steps(2);
    }

    fn key(screen: &mut egui_kittest::Harness<'_, VEditApp>, key: Key) {
        screen
            .input_mut()
            .events
            .extend([true, false].map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }));
        screen.run_steps(2);
    }

    #[test]
    fn real_ex_substitution_keeps_unicode_captures_and_undo_is_one_change() {
        let document = "intro\nαβ αβ\nαβ end";
        let mut screen = harness(fixture(document));
        text(&mut screen, r":%s/(α)(β)/\2\1-&/g");
        assert!(screen.state().vim_command.is_some());
        assert_eq!(screen.state().tab().unwrap().text, document);
        key(&mut screen, Key::Enter);
        assert!(screen.state().vim_command.is_none());
        assert_eq!(
            screen.state().tab().unwrap().text,
            "intro\nβα-αβ βα-αβ\nβα-αβ end"
        );
        let tab = screen.state().tab().unwrap();
        assert!(
            screen
                .ctx
                .memory(|memory| memory.has_focus(screen.state().document_widget_id(tab.id)))
        );
        text(&mut screen, "u");
        assert_eq!(screen.state().tab().unwrap().text, document);
    }

    #[test]
    fn real_visual_ex_range_and_cancel_do_not_reuse_stale_selection() {
        let document = "x\nx\nx\nx";
        let mut screen = harness(fixture(document));
        text(&mut screen, "jVj:s/x/y/");
        assert_eq!(screen.state().vim_command.as_deref(), Some("'<,'>s/x/y/"));
        key(&mut screen, Key::Enter);
        assert_eq!(screen.state().tab().unwrap().text, "x\ny\ny\nx");
        text(&mut screen, "u");
        text(&mut screen, ":'<,'>s/x/z/");
        key(&mut screen, Key::Enter);
        assert_eq!(screen.state().tab().unwrap().text, document);
        assert!(screen.state().status.as_ref().unwrap().1);
        text(&mut screen, ":set nowrap");
        key(&mut screen, Key::Escape);
        assert_eq!(screen.state().tab().unwrap().text, document);
        assert!(screen.state().vim_command.is_none());
    }

    #[test]
    fn ex_history_edits_become_a_new_draft_and_down_restores_it() {
        let mut screen = harness(fixture("unchanged"));
        text(&mut screen, ":set nowrap");
        key(&mut screen, Key::Enter);
        text(&mut screen, ":set number");
        key(&mut screen, Key::Enter);
        text(&mut screen, ":draft");
        key(&mut screen, Key::ArrowUp);
        assert_eq!(screen.state().vim_command.as_deref(), Some("set number"));
        text(&mut screen, "x");
        key(&mut screen, Key::ArrowUp);
        assert_eq!(screen.state().vim_command.as_deref(), Some("set number"));
        key(&mut screen, Key::ArrowDown);
        assert_eq!(screen.state().vim_command.as_deref(), Some("set numberx"));
        key(&mut screen, Key::Escape);
        assert_eq!(screen.state().tab().unwrap().text, "unchanged");
        assert!(!screen.state().tab().unwrap().vim.can_undo());
    }

    #[test]
    fn dollar_jumps_to_the_last_line_not_past_the_final_line_break() {
        let mut app = fixture("one\ntwo\nthree\n");
        app.execute_ex_command("$");
        assert_eq!(app.tab().unwrap().selection, 8..8);
        app.execute_ex_command("1");
        assert_eq!(app.tab().unwrap().selection, 0..0);
    }

    #[test]
    fn set_options_are_atomic_and_split_commands_use_vim_axes() {
        let mut app = fixture("unchanged");
        let wrap = app.settings.word_wrap;
        app.execute_ex_command("set nowrap invalid-option");
        assert_eq!(app.settings.word_wrap, wrap);
        app.execute_ex_command("set nowrap rnu");
        assert!(!app.settings.word_wrap);
        assert_eq!(app.settings.line_numbers, crate::cli::LineNumbers::Relative);
        for (command, orientation) in [("split", "horizontal"), ("vsplit", "vertical")] {
            let mut app = fixture("unchanged");
            app.execute_ex_command(command);
            assert_eq!(app.layout_status()["tree"]["orientation"], orientation);
        }
    }

    #[test]
    fn ex_enter_and_source_suffix_in_one_frame_keep_their_original_order() {
        let mut screen = harness(fixture("foo foo"));
        screen.input_mut().events.extend([
            egui::Event::Text(":s/foo/bar/".into()),
            egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("x".into()),
        ]);
        screen.run_steps(4);
        assert_eq!(screen.state().tab().unwrap().text, "ar foo");
        text(&mut screen, "u");
        assert_eq!(screen.state().tab().unwrap().text, "bar foo");
        text(&mut screen, "u");
        assert_eq!(screen.state().tab().unwrap().text, "foo foo");
    }

    #[test]
    fn open_ex_field_enter_replays_source_suffix_and_escape_discards_only_the_command() {
        let mut screen = harness(fixture("foo foo"));
        text(&mut screen, ":s/foo/bar/");
        screen.input_mut().events.extend([
            egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("x".into()),
        ]);
        screen.run_steps(4);
        assert_eq!(screen.state().tab().unwrap().text, "ar foo");
        text(&mut screen, ":s/ar/wrong/");
        screen.input_mut().events.extend([
            egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("x".into()),
        ]);
        screen.run_steps(4);
        assert_eq!(screen.state().tab().unwrap().text, "r foo");
    }

    #[test]
    fn ex_buffer_navigation_and_new_return_focus_to_the_resulting_document() {
        let mut app = fixture("first");
        let first_id = app.tab().unwrap().id;
        let mut second = EditorTab::new(None, "second.txt".into());
        second.text = "second".into();
        second.saved_text = second.text.clone();
        let second_id = second.id;
        app.projects[0].tabs.push(second);
        app.adopt_orphan_documents();
        let mut screen = harness(app);
        text(&mut screen, ":bn");
        key(&mut screen, Key::Enter);
        assert_eq!(screen.state().tab().unwrap().id, second_id);
        text(&mut screen, ":bp");
        key(&mut screen, Key::Enter);
        assert_eq!(screen.state().tab().unwrap().id, first_id);
        text(&mut screen, ":new");
        key(&mut screen, Key::Enter);
        let tab = screen.state().tab().unwrap();
        assert_ne!(tab.id, first_id);
        assert_ne!(tab.id, second_id);
        assert!(tab.text.is_empty());
        assert!(
            screen
                .ctx
                .memory(|memory| memory.has_focus(screen.state().document_widget_id(tab.id)))
        );
    }
}
