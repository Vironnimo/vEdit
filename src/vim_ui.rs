//! Discoverable Vim commands and delayed, non-interactive continuation hints.
use super::*;

const HINT_DELAY: f64 = 0.3;

struct Reference {
    query: String,
    focus: bool,
    restore_focus: Option<egui::Id>,
    return_to_settings: bool,
}

struct PendingHint {
    pane: u64,
    document: u64,
    command: String,
    since: f64,
}

#[derive(Default)]
pub(super) struct State {
    reference: Option<Reference>,
    pending: Option<PendingHint>,
}

struct Command {
    keys: &'static str,
    description: &'static str,
    group: &'static str,
    prefix: &'static str,
    next: &'static str,
}

const fn command(keys: &'static str, description: &'static str, group: &'static str) -> Command {
    Command {
        keys,
        description,
        group,
        prefix: "",
        next: "",
    }
}

impl Command {
    const fn hint(mut self, prefix: &'static str, next: &'static str) -> Self {
        self.prefix = prefix;
        self.next = next;
        self
    }
}

const COMMANDS: &[Command] = &[
    command("h j k l", "Move left, down, up or right", "Motions"),
    command(
        "w W b B e E",
        "Move by word or whitespace-separated WORD",
        "Motions",
    ),
    command("ge gE", "End of the previous word or WORD", "Motions").hint("g", "e / E"),
    command(
        "0 ^ $",
        "Line start, first nonblank, or line end",
        "Motions",
    ),
    command("g_", "Last nonblank character of the line", "Motions").hint("g", "_"),
    command(
        "gg / [count]G",
        "First line, last line, or a numbered line",
        "Motions",
    )
    .hint("g", "g"),
    command(
        "+ - _",
        "First nonblank on a following, previous or counted line",
        "Motions",
    ),
    command("[count]|", "Move to a character column", "Motions"),
    command("H M L", "Top, middle or bottom visible line", "Motions"),
    command("{ }", "Previous or next paragraph boundary", "Motions"),
    command("%", "Matching bracket", "Motions"),
    command(
        "f{char} F{char}",
        "Find a literal character forward or backward",
        "Motions",
    )
    .hint("find", "{character}"),
    command(
        "t{char} T{char}",
        "Move just before or after a found character",
        "Motions",
    ),
    command(
        "; ,",
        "Repeat the character find, or reverse it once",
        "Motions",
    ),
    command("Ctrl+F/B/D/U", "Full-page or half-page movement", "Motions"),
    command(
        "zt zz zb",
        "Show the cursor line at the top, center or bottom",
        "View",
    )
    .hint("z", "t / z / b"),
    command(
        "zEnter z. z-",
        "Align the view and move to the first nonblank",
        "View",
    )
    .hint("z", "Enter / . / -"),
    command(
        "i a I A",
        "Insert before, after, first nonblank or line end",
        "Editing",
    ),
    command("o O", "Open an indented line below or above", "Editing"),
    command(
        "gi",
        "Resume Insert at its last position in this Document",
        "Editing",
    )
    .hint("g", "i"),
    command("gI", "Insert in column one", "Editing").hint("g", "I"),
    command(
        "R",
        "Replace characters until Esc; Backspace restores overwritten text",
        "Editing",
    ),
    command(
        "r{char}",
        "Replace the current or counted characters",
        "Editing",
    )
    .hint("replace", "{character}"),
    command("x X", "Delete a character forward or backward", "Editing"),
    command("s S", "Change characters or whole lines", "Editing"),
    command(
        "d{motion} c{motion} y{motion}",
        "Delete, change or yank through a motion or text object",
        "Editing",
    ),
    command(
        "dd cc yy",
        "Delete, change or yank counted whole lines",
        "Editing",
    ),
    command(
        "D C Y",
        "Delete/change to line end, or yank whole lines",
        "Editing",
    ),
    command(
        "p P",
        "Paste after or before; Visual P preserves the source register",
        "Editing",
    ),
    command(
        ">{motion} <{motion} / >> <<",
        "Indent or outdent lines",
        "Editing",
    ),
    command(
        "gu gU g~",
        "Lowercase, uppercase or toggle case through a motion",
        "Editing",
    )
    .hint("g", "u / U / ~"),
    command("~", "Toggle case of counted characters", "Editing"),
    command(
        "J gJ",
        "Join lines with or without added spacing",
        "Editing",
    )
    .hint("g", "J"),
    command(
        "Alt+A / Alt+X",
        "Add to or subtract from a decimal number",
        "Editing",
    ),
    command(
        "u / Ctrl+R",
        "Undo or redo in Normal; Insert/Replace typing remains one step",
        "History",
    ),
    command(
        ".",
        "Repeat the last text change; a count replaces its count",
        "History",
    ),
    command(
        "v V / Ctrl+Q",
        "Character, line or rectangular Visual selection",
        "Visual",
    ),
    command(
        "o",
        "Exchange the active end of a Visual selection",
        "Visual",
    ),
    command("gv", "Reselect the last Visual area", "Visual").hint("g", "v"),
    command(
        "Visual I / A",
        "Insert at the left or right edge of a Visual Block",
        "Visual",
    ),
    command("iw aw / iW aW", "Inner/around word or WORD", "Text objects").hint("object", "w / W"),
    command("ip ap", "Inner/around paragraph", "Text objects").hint("object", "p"),
    command("i( a( / ib ab", "Inner/around parentheses", "Text objects")
        .hint("object", "( / ) / b"),
    command("i{ a{ / iB aB", "Inner/around braces", "Text objects").hint("object", "{ / } / B"),
    command(
        "i[ a[ / i< a<",
        "Inner/around square or angle brackets",
        "Text objects",
    )
    .hint("object", "[ / ] / < / >"),
    command(
        "i\" a\" / i' a' / i` a`",
        "Inner/around a quoted string",
        "Text objects",
    )
    .hint("object", "\" / ' / `"),
    command(
        "ys{motion}{pair}",
        "Add a surrounding pair to a motion or text object",
        "Surround",
    )
    .hint("ys", "{motion}, then pair"),
    command("ds{pair}", "Delete a surrounding pair", "Surround").hint("ds", "{pair}"),
    command("cs{old}{new}", "Change a surrounding pair", "Surround")
        .hint("cs", "{old pair}, then new pair"),
    command(
        "Visual S{pair}",
        "Surround the Visual selection",
        "Surround",
    )
    .hint("S", "{pair}"),
    command(
        "\"a{command} … \"z{command}",
        "Use a named register for yank, delete, change or paste",
        "Registers",
    )
    .hint("\"", "a … z"),
    command(
        "\"A{command} … \"Z{command}",
        "Append to the corresponding lowercase register",
        "Registers",
    )
    .hint("\"", "A … Z"),
    command(
        "\"0p / \"1p … \"9p / \"-p",
        "Paste the last yank, a numbered deletion or the small deletion",
        "Registers",
    )
    .hint("\"", "0 … 9 / -"),
    command(
        "\"_d{motion}",
        "Delete to the black-hole register without changing registers",
        "Registers",
    )
    .hint("\"", "_"),
    command(
        "Insert/Replace Ctrl+R{register}",
        "Insert register text without leaving the current typing transaction",
        "Registers",
    ),
    command(
        "q{register} … q",
        "Record commands in a macro register; uppercase appends",
        "Macros",
    )
    .hint("q", "a … z / A … Z"),
    command(
        "[count]@{register}",
        "Play a recorded macro or commands stored in a register",
        "Macros",
    )
    .hint("@", "a … z / 0 … 9"),
    command("@@", "Repeat the last played macro", "Macros").hint("@", "@"),
    command("m{a-z}", "Set a local mark in this Document", "Marks").hint("m", "a … z"),
    command(
        "m{A-Z}",
        "Set a window-wide mark that can jump between files",
        "Marks",
    )
    .hint("m", "A … Z"),
    command(
        "m' / m`",
        "Set the previous-jump context explicitly",
        "Marks",
    )
    .hint("m", "' / `"),
    command(
        "'{mark} / `{mark}",
        "Jump to a mark's first nonblank or exact character",
        "Marks",
    )
    .hint("mark", "a … z / A … Z"),
    command(
        "g'{mark} / g`{mark}",
        "Jump to a mark without a same-Document history entry",
        "Marks",
    )
    .hint("g", "' / `"),
    command("'. / `.", "Last change location", "Marks").hint("mark", "."),
    command("'^ / `^", "Last Insert position", "Marks").hint("mark", "^"),
    command(
        "'' / ``",
        "Position before the last jump; repeat to toggle",
        "Marks",
    )
    .hint("mark", "' / `"),
    command(
        "'[ '] / `[ `]",
        "Start or end of the last changed or yanked area",
        "Marks",
    )
    .hint("mark", "[ / ]"),
    command(
        "'< '> / `< `>",
        "Start or end of the last Visual selection",
        "Marks",
    )
    .hint("mark", "< / >"),
    command(
        "g; g,",
        "Older or newer change location; counts are accepted",
        "History",
    )
    .hint("g", "; / ,"),
    command(
        "Ctrl+O / Ctrl+I",
        "Older or newer location in this Pane's jump history",
        "History",
    ),
    command(
        "/pattern / ?pattern",
        "Incremental regex search forward or backward",
        "Search",
    ),
    command(
        "n N / * #",
        "Repeat search, or search the word under the cursor",
        "Search",
    ),
    command(
        "Search ↑/↓ / Ctrl+P/N",
        "Recall confirmed search patterns",
        "Search",
    ),
    command(
        "Search Ctrl+W/U",
        "Delete the preceding word or the input prefix",
        "Search",
    ),
    command("Esc Esc / :noh", "Hide search highlights", "Search"),
    command(
        "gd gr K",
        "Definition, references or hover information",
        "Language",
    )
    .hint("g", "d / r"),
    command("]d [d", "Next or previous error or warning", "Language").hint("bracket", "d"),
    command(
        ":w :wa :q :q! :wq :x",
        "Write, write all, close, discard or save and close",
        "Command line",
    ),
    command(
        ":e [path] :new :bn :bp",
        "Reload/open a file, create a draft or cycle tabs",
        "Command line",
    ),
    command(
        ":split [path] :vsplit [path]",
        "Split the focused Pane horizontally or vertically",
        "Command line",
    ),
    command(
        ":42 / :2,8 / :'<,'>",
        "Go to a line or a line-range endpoint",
        "Command line",
    ),
    command(
        ":s/old/new/ / :%s/old/new/g",
        "Regex substitution in a line, range or whole Document",
        "Command line",
    ),
    command(
        ":set number / :set nonumber",
        "Enable or disable line numbers",
        "Command line",
    ),
    command(
        "Esc",
        "Cancel a pending command or return to Normal",
        "Modes",
    ),
];

fn hint_entries(command: &str) -> Vec<(&'static str, &'static str)> {
    if command == "^R" {
        return vec![
            ("a … z / 0 … 9", "Insert text from the selected register"),
            ("\" / - / _", "Unnamed, small-delete or black-hole register"),
            ("Esc", "Cancel register insertion and keep typing"),
        ];
    }
    let raw = command.trim_start_matches(|ch: char| ch.is_ascii_digit());
    let mut prefix: String = if raw.starts_with('"') && raw.chars().count() > 1 {
        raw.chars()
            .take(2)
            .chain(raw.chars().skip(2).filter(|ch| !ch.is_ascii_digit()))
            .collect()
    } else {
        raw.chars().filter(|ch| !ch.is_ascii_digit()).collect()
    };
    if prefix.starts_with('"') && prefix.chars().count() > 1 {
        prefix = prefix.chars().skip(2).collect();
        if prefix.is_empty() {
            return vec![
                (
                    "d / c / y",
                    "Delete, change or yank into the selected register",
                ),
                ("p / P", "Paste from the selected register"),
            ];
        }
    }
    let category = if matches!(prefix.as_str(), "'" | "`" | "g'" | "g`") {
        "mark"
    } else if matches!(prefix.as_str(), "[" | "]") {
        "bracket"
    } else if prefix.ends_with(['i', 'a']) {
        "object"
    } else if prefix.ends_with(['f', 'F', 't', 'T']) {
        "find"
    } else if prefix == "r" {
        "replace"
    } else if prefix.starts_with("ys") && prefix.len() > 2
        || prefix.starts_with("cs") && prefix.len() > 2
    {
        return vec![("{pair}", "Choose the new surrounding pair")];
    } else {
        prefix.as_str()
    };
    if matches!(category, "d" | "c" | "y" | ">" | "<" | "gu" | "gU" | "g~") {
        let repeated = match category {
            "d" => "d",
            "c" => "c",
            "y" => "y",
            ">" => ">",
            "<" => "<",
            "gu" => "u",
            "gU" => "U",
            _ => "~",
        };
        return vec![
            (repeated, "Apply to the current or counted whole lines"),
            ("w / b / e", "Apply to a word motion"),
            ("$ / 0 / ^", "Apply to a line boundary"),
            ("j / k / G", "Apply to whole lines"),
            ("i / a", "Choose an inner or around text object"),
            ("f / F / t / T", "Apply through a literal character find"),
            ("' / `", "Apply through a mark in this Document"),
            ("/ / ?", "Apply through a search"),
        ];
    }
    if prefix.is_empty() && command.chars().all(|ch| ch.is_ascii_digit()) {
        return vec![
            ("motion", "Repeat a cursor motion with this count"),
            ("operator", "Apply a counted edit"),
            ("G", "Jump to this line number"),
        ];
    }
    COMMANDS
        .iter()
        .filter(|entry| !entry.prefix.is_empty() && entry.prefix == category)
        .map(|entry| (entry.next, entry.description))
        .collect()
}

impl VEditApp {
    pub(super) fn vim_reference_open(&self) -> bool {
        self.vim_ui.reference.is_some()
    }

    pub(super) fn open_vim_reference(&mut self) {
        let restore_focus = if !self.settings_open && !self.terminal_keyboard_focused() {
            self.tab()
                .filter(|tab| tab.agent_view == crate::cli::DocumentView::Source)
                .map(|tab| self.document_widget_id(tab.id))
                .or_else(|| self.ctx.memory(|memory| memory.focused()))
        } else {
            self.ctx.memory(|memory| memory.focused())
        };
        if let Some(id) = restore_focus {
            self.ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = false;
        }
        self.completions = None;
        let return_to_settings = self.settings_open;
        self.settings_open = false;
        self.vim_ui.reference = Some(Reference {
            query: String::new(),
            focus: true,
            restore_focus,
            return_to_settings,
        });
    }

    pub(super) fn vim_reference_ui(&mut self) {
        if self.close_action.is_some() || self.save_as.is_some() || self.status_details.is_some() {
            return;
        }
        let Some(reference) = self.vim_ui.reference.as_mut() else {
            return;
        };
        let ctx = self.ctx.clone();
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("vim_command_reference")).show(&ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 48.0).clamp(240.0, 760.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Vim commands").size(20.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| { close |= ui.button("Close").clicked(); });
            });
            ui.label(RichText::new("Implemented in vEdit · counts precede motions and edits · Ctrl-key ownership follows Settings → Vim").small().color(MUTED));
            let search = ui.add(egui::TextEdit::singleline(&mut reference.query).id_salt("vim_reference_search").hint_text("Search commands or descriptions").desired_width(f32::INFINITY));
            if std::mem::take(&mut reference.focus) { search.request_focus(); }
            let query = reference.query.to_lowercase();
            ui.separator();
            let matches: Vec<_> = COMMANDS.iter().filter(|entry| query.split_whitespace().all(|word| entry.keys.to_lowercase().contains(word) || entry.description.to_lowercase().contains(word) || entry.group.to_lowercase().contains(word))).collect();
            ui.label(RichText::new(format!("{} commands", matches.len())).small().color(MUTED));
            egui::ScrollArea::vertical().id_salt("vim_reference_scroll").max_height((ctx.content_rect().height() - 220.0).clamp(70.0, 520.0)).show(ui, |ui| {
                let mut group = "";
                for entry in matches {
                    if group != entry.group { group = entry.group; ui.add_space(8.0); ui.label(RichText::new(group).strong().color(BLUE)); }
                    ui.horizontal_wrapped(|ui| { ui.label(RichText::new(entry.keys).monospace().color(ORANGE)); ui.label(entry.description); });
                }
            });
        });
        if close || modal.should_close() {
            let reference = self.vim_ui.reference.take().unwrap();
            self.settings_open = reference.return_to_settings;
            if !reference.return_to_settings && !self.terminal_keyboard_focused() {
                self.restore_document_focus();
            }
            if let Some(id) = reference.restore_focus {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
        }
    }

    pub(super) fn vim_command_hints_ui(&mut self) {
        if !self.settings.vim
            || !self.settings.vim_command_hints
            || self.chrome_blocked()
            || self.navigation_pending()
            || self.vim_search_active()
            || self.vim_command.is_some()
            || self.terminal_keyboard_focused()
        {
            self.vim_ui.pending = None;
            return;
        }
        let Some(tab) = self
            .tab()
            .filter(|tab| !tab.loading && tab.agent_view == crate::cli::DocumentView::Source)
        else {
            self.vim_ui.pending = None;
            return;
        };
        let command = self
            .vim_macros
            .command_prefix()
            .unwrap_or_else(|| tab.vim.pending_command());
        if self.find_focus
            || !self
                .ctx
                .memory(|memory| memory.has_focus(self.document_widget_id(tab.id)))
        {
            self.vim_ui.pending = None;
            return;
        }
        if command.is_empty() {
            self.vim_ui.pending = None;
            return;
        }
        let pane = self.current_pane_id();
        let document = tab.id;
        let now = self.ctx.input(|input| input.time);
        if self.vim_ui.pending.as_ref().is_none_or(|pending| {
            pending.pane != pane || pending.document != document || pending.command != command
        }) {
            self.vim_ui.pending = Some(PendingHint {
                pane,
                document,
                command: command.clone(),
                since: now,
            });
        }
        let elapsed = now - self.vim_ui.pending.as_ref().unwrap().since;
        if elapsed < HINT_DELAY {
            self.ctx
                .request_repaint_after(Duration::from_secs_f64(HINT_DELAY - elapsed));
            return;
        }
        let entries = hint_entries(&command);
        if entries.is_empty() {
            return;
        }
        let Some(editor) = self
            .editor_rect
            .filter(|rect| rect.width() >= 180.0 && rect.height() >= 150.0)
        else {
            return;
        };
        let ctx = self.ctx.clone();
        egui::Area::new(egui::Id::new("vim_pending_hints"))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(egui::pos2(editor.min.x + 10.0, editor.max.y - 10.0))
            .show(&ctx, |ui| {
                ui.set_max_width((editor.width() - 24.0).min(620.0));
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.spacing_mut().interact_size.y = ui.text_style_height(&egui::TextStyle::Body);
                    ui.label(
                        RichText::new(format!("{command} …"))
                            .monospace()
                            .strong()
                            .color(ORANGE),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt(("vim_hint_rows", &command))
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .max_height((editor.height() - 90.0).clamp(35.0, 240.0))
                        .show(ui, |ui| {
                            for (keys, description) in entries.iter().take(10) {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(*keys).monospace().color(BLUE));
                                    ui.label(*description);
                                });
                            }
                        });
                    ui.label(
                        RichText::new("Esc cancels · ? beside the mode opens all Vim commands")
                            .small()
                            .color(MUTED),
                    );
                });
            });
    }
}
