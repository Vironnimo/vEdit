//! Text editing commands shared by the normal and modal editor.
//!
//! Selections and search results use Unicode character indices, as egui does.
//! Selection ends are exclusive. Line and column numbers shown to users are
//! one-based; all editing commands clamp stale selections to the document.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditResult {
    pub text: String,
    pub selection: Range<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// The query is a Rust regular expression and replacements expand `$1`.
    pub regex: bool,
}

#[derive(Clone, Copy)]
struct Line {
    start: usize,
    end: usize,
    break_end: usize,
}

struct Edit {
    range: Range<usize>,
    replacement: String,
}

fn clamp_selection(selection: Range<usize>, len: usize) -> Range<usize> {
    let a = selection.start.min(len);
    let b = selection.end.min(len);
    a.min(b)..a.max(b)
}

/// Resolve scalar offsets once, stopping at the selection end. Local edits can
/// then copy the untouched UTF-8 slices without expanding the whole document.
fn selection_offsets(text: &str, selection: Range<usize>) -> (Range<usize>, Range<usize>) {
    let start = selection.start.min(selection.end);
    let end = selection.start.max(selection.end);
    let mut chars = 0..0;
    let mut bytes = 0..0;
    for (position, byte) in text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .enumerate()
    {
        if position <= start {
            chars.start = position;
            bytes.start = byte;
        }
        chars.end = position;
        bytes.end = byte;
        if position == end {
            break;
        }
    }
    (chars, bytes)
}

fn replace_bytes(text: &str, range: Range<usize>, replacement: &str, cursor: usize) -> EditResult {
    let mut output = String::with_capacity(text.len() - range.len() + replacement.len());
    output.push_str(&text[..range.start]);
    output.push_str(replacement);
    output.push_str(&text[range.end..]);
    EditResult {
        text: output,
        selection: cursor..cursor,
    }
}

fn lines(chars: &[char]) -> Vec<Line> {
    let mut result = Vec::new();
    let mut start = 0;
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '\n' {
            result.push(Line {
                start,
                end: if i > start && chars[i - 1] == '\r' {
                    i - 1
                } else {
                    i
                },
                break_end: i + 1,
            });
            start = i + 1;
        }
    }
    result.push(Line {
        start,
        end: chars.len(),
        break_end: chars.len(),
    });
    result
}

fn row_at(lines: &[Line], position: usize) -> usize {
    lines
        .partition_point(|line| line.start <= position)
        .saturating_sub(1)
}

fn selected_rows(lines: &[Line], selection: &Range<usize>) -> Range<usize> {
    let first = row_at(lines, selection.start);
    // A selection ending at the next line's start does not include that line.
    let last_position = if selection.is_empty() {
        selection.start
    } else {
        selection.end - 1
    };
    first..row_at(lines, last_position) + 1
}

fn apply_edits(chars: &[char], selection: Range<usize>, edits: Vec<Edit>) -> EditResult {
    let mut text = String::new();
    let mut previous_end = 0;
    for edit in &edits {
        text.extend(chars[previous_end..edit.range.start].iter());
        text.push_str(&edit.replacement);
        previous_end = edit.range.end;
    }
    text.extend(chars[previous_end..].iter());
    let map = |position: usize| {
        let mut delta = 0isize;
        for edit in &edits {
            if position < edit.range.start {
                break;
            }
            let inserted = edit.replacement.chars().count();
            if position <= edit.range.end {
                return (edit.range.start as isize + delta) as usize + inserted;
            }
            delta += inserted as isize - edit.range.len() as isize;
        }
        (position as isize + delta) as usize
    };
    EditResult {
        text,
        selection: map(selection.start)..map(selection.end),
    }
}

fn horizontal_space(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

fn indentation_end(chars: &[char], line: Line) -> usize {
    line.start
        + chars[line.start..line.end]
            .iter()
            .take_while(|&&ch| horizontal_space(ch))
            .count()
}

fn newline(text: &str) -> &'static str {
    if text
        .find('\n')
        .is_some_and(|at| at > 0 && text.as_bytes()[at - 1] == b'\r')
    {
        "\r\n"
    } else {
        "\n"
    }
}

/// Return a one-based line and column. Columns count Unicode characters.
#[cfg(test)]
pub fn line_col(text: &str, position: usize) -> (usize, usize) {
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut column = 1;
    for _ in 0..position {
        let Some(ch) = chars.next() else {
            break;
        };
        match ch {
            '\n' => {
                line += 1;
                column = 1;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            _ => column += 1,
        }
    }
    (line, column)
}

/// Return the character index at the start of a one-based line, clamped to the
/// document's first or last line when the requested line is out of range.
pub fn go_to_line(text: &str, line_number: usize) -> usize {
    if line_number <= 1 {
        return 0;
    }
    let mut line = 1;
    let mut start = 0;
    for (position, ch) in text.chars().enumerate() {
        if ch == '\n' {
            line += 1;
            start = position + 1;
            if line == line_number {
                break;
            }
        }
    }
    start
}

/// Replace the selection and place the caret after the replacement.
pub fn replace_selection(text: &str, selection: Range<usize>, replacement: &str) -> EditResult {
    let (selection, bytes) = selection_offsets(text, selection);
    let position = selection.start + replacement.chars().count();
    replace_bytes(text, bytes, replacement, position)
}

/// Indent or outdent the selected lines. A collapsed selection edits its line.
/// Outdent removes one tab or up to `indent`'s width in leading spaces.
#[cfg(test)]
pub fn indent_lines(
    text: &str,
    selection: Range<usize>,
    indent: &str,
    outdent: bool,
) -> EditResult {
    indent_lines_with_width(
        text,
        selection,
        indent,
        indent.chars().count().max(1),
        outdent,
    )
}

/// `width` is the visual indentation width, also when `indent` is a tab.
pub fn indent_lines_with_width(
    text: &str,
    selection: Range<usize>,
    indent: &str,
    width: usize,
    outdent: bool,
) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let width = width.clamp(1, 16);
    let edits = selected_rows(&rows, &selection)
        .filter_map(|row| {
            let line = rows[row];
            if outdent {
                let count = if chars.get(line.start) == Some(&'\t') {
                    1
                } else {
                    chars[line.start..line.end]
                        .iter()
                        .take(width)
                        .take_while(|&&ch| ch == ' ')
                        .count()
                };
                (count > 0).then_some(Edit {
                    range: line.start..line.start + count,
                    replacement: String::new(),
                })
            } else {
                (!indent.is_empty()).then_some(Edit {
                    range: line.start..line.start,
                    replacement: indent.to_owned(),
                })
            }
        })
        .collect();
    apply_edits(&chars, selection, edits)
}

fn comment_markers(language: &str) -> (&'static str, &'static str) {
    let language = language.trim_start_matches('.').to_ascii_lowercase();
    match language.as_str() {
        "py" | "python" | "pyi" | "pyw" | "rb" | "ruby" | "sh" | "shell" | "shellscript"
        | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "powershell" | "toml" | "yaml" | "yml"
        | "r" | "julia" | "jl" | "conf" | "cfg" | "dockerfile" | "makefile" | "make" | "cmake"
        | "ignore" | "ignore patterns" => ("#", ""),
        "sql" | "lua" | "hs" | "haskell" | "elm" => ("--", ""),
        "html" | "htm" | "xml" | "svg" | "markdown" | "md" => ("<!--", "-->"),
        "css" => ("/*", "*/"),
        "ini" | "lisp" | "clojure" | "clj" | "el" | "asm" => (";", ""),
        "tex" | "latex" | "matlab" | "erl" | "erlang" => ("%", ""),
        "vb" | "vbs" | "vbnet" => ("'", ""),
        "f" | "f90" | "fortran" => ("!", ""),
        "bat" | "cmd" | "batch" => ("REM", ""),
        _ => ("//", ""),
    }
}

/// Toggle comments for selected lines using a language name or file extension.
/// Existing indentation is preserved; empty lines within a block are skipped.
pub fn toggle_line_comment(text: &str, selection: Range<usize>, language: &str) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let chosen = selected_rows(&rows, &selection);
    let (prefix, suffix) = comment_markers(language);
    let prefix_chars: Vec<char> = prefix.chars().collect();
    let suffix_chars: Vec<char> = suffix.chars().collect();
    let nonempty: Vec<(Line, usize)> = chosen
        .clone()
        .map(|row| (rows[row], indentation_end(&chars, rows[row])))
        .filter(|(line, at)| *at < line.end)
        .collect();
    let uncomment = !nonempty.is_empty()
        && nonempty.iter().all(|(line, at)| {
            let end = line.end
                - chars[*at..line.end]
                    .iter()
                    .rev()
                    .take_while(|&&ch| horizontal_space(ch))
                    .count();
            chars[*at..line.end].starts_with(&prefix_chars)
                && (suffix.is_empty()
                    || (end - at >= prefix_chars.len() + suffix_chars.len()
                        && chars[*at..end].ends_with(&suffix_chars)))
        });
    let mut edits = Vec::new();
    for row in chosen {
        let line = rows[row];
        let at = indentation_end(&chars, line);
        if at == line.end && !nonempty.is_empty() {
            continue;
        }
        if uncomment {
            let prefix_end = at + prefix_chars.len();
            let remove_end = prefix_end + usize::from(chars.get(prefix_end) == Some(&' '));
            edits.push(Edit {
                range: at..remove_end,
                replacement: String::new(),
            });
            if !suffix.is_empty() {
                let content_end = line.end
                    - chars[at..line.end]
                        .iter()
                        .rev()
                        .take_while(|&&ch| horizontal_space(ch))
                        .count();
                let suffix_start = content_end - suffix_chars.len();
                // Removal ranges must not overlap for an empty block comment.
                if suffix_start >= remove_end {
                    let remove_start = suffix_start
                        - usize::from(suffix_start > remove_end && chars[suffix_start - 1] == ' ');
                    edits.push(Edit {
                        range: remove_start..content_end,
                        replacement: String::new(),
                    });
                }
            }
        } else {
            edits.push(Edit {
                range: at..at,
                replacement: format!("{prefix} "),
            });
            if !suffix.is_empty() {
                edits.push(Edit {
                    range: line.end..line.end,
                    replacement: format!(" {suffix}"),
                });
            }
        }
    }
    apply_edits(&chars, selection, edits)
}

/// Delete complete selected lines, preserving the caret column where possible.
pub fn delete_lines(text: &str, selection: Range<usize>) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let chosen = selected_rows(&rows, &selection);
    let first = rows[chosen.start];
    let last = rows[chosen.end - 1];
    let column = selection.start.saturating_sub(first.start);
    // Removing the final unterminated line includes its preceding separator,
    // otherwise deleting it would leave an extra empty line behind.
    let start = if last.end == chars.len() && chosen.start > 0 {
        rows[chosen.start - 1].end
    } else {
        first.start
    };
    let mut result = replace_selection(text, start..last.break_end, "");
    let remaining: Vec<char> = result.text.chars().collect();
    let remaining_rows = lines(&remaining);
    let target = remaining_rows[chosen.start.min(remaining_rows.len() - 1)];
    let cursor = target.start + column.min(target.end - target.start);
    result.selection = cursor..cursor;
    result
}

/// Insert a complete line outside the selection, leaving its contents intact.
/// Below uses the normal newline indentation; above copies the first row's indent.
pub fn insert_line(text: &str, selection: Range<usize>, indent: &str, above: bool) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let chosen = selected_rows(&rows, &selection);
    if !above {
        let end = rows[chosen.end - 1].end;
        return smart_newline(text, end..end, indent);
    }
    let row = rows[chosen.start];
    let base: String = chars[row.start..indentation_end(&chars, row)]
        .iter()
        .collect();
    let cursor = row.start + base.chars().count();
    let mut result = replace_selection(
        text,
        row.start..row.start,
        &format!("{base}{}", newline(text)),
    );
    result.selection = cursor..cursor;
    result
}

/// Duplicate the selected complete lines and move the selection to the copy.
pub fn duplicate_lines(text: &str, selection: Range<usize>) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let chosen = selected_rows(&rows, &selection);
    let start = rows[chosen.start].start;
    let last = rows[chosen.end - 1];
    let block: String = chars[start..last.break_end].iter().collect();
    let replacement = if last.end == last.break_end {
        format!("{}{block}", newline(text))
    } else {
        block
    };
    let shift = replacement.chars().count();
    let mut result = apply_edits(
        &chars,
        selection.clone(),
        vec![Edit {
            range: last.break_end..last.break_end,
            replacement,
        }],
    );
    result.selection = selection.start + shift..selection.end + shift;
    result
}

/// Move the selected complete lines one row, keeping their character selection
/// and preserving an unterminated final line and the document's line endings.
pub fn move_lines(text: &str, selection: Range<usize>, down: bool) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let chosen = selected_rows(&rows, &selection);
    let first = rows[chosen.start];
    let last = rows[chosen.end - 1];
    let unchanged = || EditResult {
        text: text.to_owned(),
        selection: selection.clone(),
    };
    // A trailing newline terminates the last complete row; its synthetic empty
    // row should not turn a line move into removal of that terminator.
    let complete_rows =
        rows.len() - usize::from(rows.len() > 1 && rows.last().unwrap().start == chars.len());
    if chosen.end > complete_rows
        || (down && chosen.end >= complete_rows)
        || (!down && chosen.start == 0)
    {
        return unchanged();
    }
    let block: String = chars[first.start..last.end].iter().collect();
    let (range, replacement, moved_start) = if down {
        let next = rows[chosen.end];
        let neighbor: String = chars[next.start..next.end].iter().collect();
        let separator: String = chars[last.end..last.break_end].iter().collect();
        let ending: String = chars[next.end..next.break_end].iter().collect();
        let moved_start = first.start + neighbor.chars().count() + separator.chars().count();
        (
            first.start..next.break_end,
            format!("{neighbor}{separator}{block}{ending}"),
            moved_start,
        )
    } else {
        let previous = rows[chosen.start - 1];
        let neighbor: String = chars[previous.start..previous.end].iter().collect();
        let separator: String = chars[previous.end..previous.break_end].iter().collect();
        let ending: String = chars[last.end..last.break_end].iter().collect();
        (
            previous.start..last.break_end,
            format!("{block}{separator}{neighbor}{ending}"),
            previous.start,
        )
    };
    let mut result = replace_selection(text, range, &replacement);
    let offset = |position: usize| moved_start + position.saturating_sub(first.start);
    let len = result.text.chars().count();
    result.selection = offset(selection.start).min(len)..offset(selection.end).min(len);
    result
}

/// Insert a tab or advance to the next visual tab stop with spaces.
pub fn insert_tab(text: &str, selection: Range<usize>, width: usize, tabs: bool) -> EditResult {
    let width = width.clamp(1, 16);
    let (selection, bytes) = selection_offsets(text, selection);
    let start = text[..bytes.start].rfind('\n').map_or(0, |at| at + 1);
    let column = text[start..bytes.start].chars().fold(0, |column, ch| {
        column
            + if ch == '\t' {
                width - column % width
            } else {
                1
            }
    });
    let insertion = if tabs {
        "\t".into()
    } else {
        " ".repeat(width - column % width)
    };
    let cursor = selection.start + insertion.len();
    replace_bytes(text, bytes, &insertion, cursor)
}

/// Insert a newline with the current line's indent. Opening braces, brackets,
/// parentheses and a trailing colon add one level. Between matching delimiters
/// this creates an indented empty line and places the caret there.
pub fn smart_newline(text: &str, selection: Range<usize>, indent: &str) -> EditResult {
    let (selection, bytes) = selection_offsets(text, selection);
    let line_start = text[..bytes.start].rfind('\n').map_or(0, |at| at + 1);
    let prefix = &text[line_start..bytes.start];
    let base = &prefix[..prefix.len() - prefix.trim_start_matches([' ', '\t']).len()];
    let left = prefix.chars().rev().find(|&ch| !horizontal_space(ch));
    let deeper = matches!(left, Some('{' | '[' | '(' | ':'));
    let expected_close = match left {
        Some('{') => Some('}'),
        Some('[') => Some(']'),
        Some('(') => Some(')'),
        _ => None,
    };
    let suffix = &text[bytes.end..];
    let right = suffix.trim_start_matches([' ', '\t']);
    let right_at = bytes.end + suffix.len() - right.len();
    let between = expected_close.is_some() && right.chars().next() == expected_close;
    let ending = newline(text);
    let mut replacement = format!("{ending}{base}{}", if deeper { indent } else { "" });
    // Keep a separate caret offset when a closing delimiter needs its own line.
    let cursor = selection.start + replacement.chars().count();
    if between {
        replacement.push_str(ending);
        replacement.push_str(base);
    }
    let end = if between { right_at } else { bytes.end };
    replace_bytes(text, bytes.start..end, &replacement, cursor)
}

/// Find non-overlapping matches. Empty queries produce no matches.
/// Case-insensitive literal matching uses Unicode lowercase while retaining the
/// original character boundaries; a match never selects half of a lowercase
/// expansion. An invalid regular expression has no matches.
pub fn find_matches(text: &str, query: &str, options: SearchOptions) -> Vec<Range<usize>> {
    crate::search::search(
        text,
        query,
        options.case_sensitive,
        options.whole_word,
        options.regex,
    )
    .unwrap_or_default()
}

/// The matches with their replacement text: literal, or with `$1` / `${name}`
/// expanded for a regular expression.
pub fn replacements(
    text: &str,
    query: &str,
    replacement: &str,
    options: SearchOptions,
) -> Vec<(Range<usize>, String)> {
    if options.regex {
        crate::search::regex_replacements(
            text,
            query,
            options.case_sensitive,
            options.whole_word,
            replacement,
        )
        .unwrap_or_default()
    } else {
        find_matches(text, query, options)
            .into_iter()
            .map(|range| (range, replacement.to_owned()))
            .collect()
    }
}

/// Replace all matches, returning the number replaced. The selection
/// follows the same original characters after edits, or the end of a replacement
/// when its original endpoint was inside a match.
pub fn replace_all(
    text: &str,
    selection: Range<usize>,
    query: &str,
    replacement: &str,
    options: SearchOptions,
) -> (EditResult, usize) {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let edits: Vec<_> = replacements(text, query, replacement, options)
        .into_iter()
        .map(|(range, replacement)| Edit { range, replacement })
        .collect();
    let count = edits.len();
    (apply_edits(&chars, selection, edits), count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_positions_and_one_based_lines() {
        let text = "ö🦀\r\nλ\n";
        assert_eq!(line_col(text, 2), (1, 3));
        assert_eq!(line_col(text, 4), (2, 1));
        assert_eq!(line_col(text, usize::MAX), (3, 1));
        assert_eq!(go_to_line(text, 0), 0);
        assert_eq!(go_to_line(text, 2), 4);
        assert_eq!(go_to_line(text, usize::MAX), 6);
    }

    #[test]
    fn indent_excludes_selection_ending_at_next_line_start() {
        let result = indent_lines("α\r\nβ\r\nγ", 0..6, "  ", false);
        assert_eq!(result.text, "  α\r\n  β\r\nγ");
        assert_eq!(result.selection, 2..10);
        let result = indent_lines(&result.text, result.selection, "  ", true);
        assert_eq!(result.text, "α\r\nβ\r\nγ");
        assert_eq!(result.selection, 0..6);
    }

    #[test]
    fn comments_round_trip_unicode_and_crlf() {
        let text = "  let α = 1;\r\n\r\n    β();";
        let original = 0..text.chars().count();
        let commented = toggle_line_comment(text, original.clone(), "rs");
        assert_eq!(commented.text, "  // let α = 1;\r\n\r\n    // β();");
        let restored = toggle_line_comment(&commented.text, commented.selection, "rust");
        assert_eq!(restored.text, text);
        // Python files use their own valid line comment marker.
        assert_eq!(toggle_line_comment("λ = 1", 0..0, "python").text, "# λ = 1");
    }

    #[test]
    fn block_comments_handle_empty_and_existing_whitespace() {
        let commented = toggle_line_comment("  <p>ö</p>  ", 0..0, "html");
        assert_eq!(commented.text, "  <!-- <p>ö</p>   -->");
        assert_eq!(
            toggle_line_comment(&commented.text, 0..0, "html").text,
            "  <p>ö</p>  "
        );
        let empty = toggle_line_comment("", 0..0, "html");
        assert_eq!(toggle_line_comment(&empty.text, 0..0, "html").text, "");
    }

    #[test]
    fn standard_line_deletion_handles_unicode_crlf_and_document_boundaries() {
        for (text, selection, expected, cursor) in [
            ("a\r\nβ🦀\r\nlast", 3..7, "a\r\nlast", 3),
            ("a\r\nβ🦀", 4..4, "a", 1),
            ("a\r\nβ🦀\r\n", 3..3, "a\r\n", 3),
            ("a\r\n", 3..3, "a", 0),
            ("β🦀", 1..usize::MAX, "", 0),
            ("", 0..0, "", 0),
            ("one\ntwo\nthree", 1..8, "three", 1),
        ] {
            let result = delete_lines(text, selection.clone());
            assert_eq!(result.text, expected, "{text:?} {selection:?}");
            assert_eq!(result.selection, cursor..cursor, "{text:?} {selection:?}");
        }
    }

    #[test]
    fn standard_line_insertion_preserves_blocks_and_raw_endings() {
        for (text, selection, above, expected, cursor) in [
            (
                "  if x:\r\n  next",
                4..4,
                false,
                "  if x:\r\n    \r\n  next",
                13,
            ),
            (
                "  if x:\r\n  next",
                4..4,
                true,
                "  \r\n  if x:\r\n  next",
                2,
            ),
            ("β🦀", 1..2, false, "β🦀\n", 3),
            ("β🦀", 1..2, true, "\nβ🦀", 0),
            ("  a\n  b\nc", 2..8, false, "  a\n  b\n  \nc", 10),
            ("", 0..0, false, "\n", 1),
            ("", 0..0, true, "\n", 0),
        ] {
            let result = insert_line(text, selection.clone(), "  ", above);
            assert_eq!(
                result.text, expected,
                "{text:?} {selection:?} above={above}"
            );
            assert_eq!(
                result.selection,
                cursor..cursor,
                "{text:?} {selection:?} above={above}"
            );
        }
    }

    #[test]
    fn duplicate_final_line_preserves_endings() {
        let duplicate = duplicate_lines("α\r\nβ", 4..4);
        assert_eq!(duplicate.text, "α\r\nβ\r\nβ");
        assert_eq!(duplicate.selection, 7..7);
        assert_eq!(duplicate_lines("", 0..0).text, "\n");
        assert_eq!(duplicate_lines("α\r\n", 0..0).text, "α\r\nα\r\n");
        assert_eq!(duplicate_lines("α\r\n", 3..3).text, "α\r\n\r\n");
    }

    #[test]
    fn newline_indents_pair_and_preserves_crlf() {
        let result = smart_newline("fn x() {  }\r\n", 8..8, "    ");
        assert_eq!(result.text, "fn x() {\r\n    \r\n}\r\n");
        assert_eq!(result.selection, 14..14);
        let result = smart_newline("  café();", 9..9, "    ");
        assert_eq!(result.text, "  café();\n  ");
        assert_eq!(result.selection, 12..12);
    }

    #[test]
    fn local_edits_clamp_unicode_selections_and_preserve_raw_line_endings() {
        let replaced = replace_selection("α🦀\r\nβ", 1..usize::MAX, "中\r\n");
        assert_eq!(replaced.text, "α中\r\n");
        assert_eq!(replaced.selection, 4..4);
        let appended = replace_selection("α🦀", usize::MAX..usize::MAX, "β");
        assert_eq!(appended.text, "α🦀β");
        assert_eq!(appended.selection, 3..3);
        let tab = insert_tab("α\r\n\t🦀tail", Range { start: 8, end: 5 }, 4, false);
        assert_eq!(tab.text, "α\r\n\t🦀   l");
        assert_eq!(tab.selection, 8..8);
        let newline = smart_newline("α\r\n\t{🦀  }\r\nβ", Range { start: 6, end: 5 }, "\t");
        assert_eq!(newline.text, "α\r\n\t{\r\n\t\t\r\n\t}\r\nβ");
        assert_eq!(newline.selection, 9..9);
        let empty = smart_newline("", usize::MAX..usize::MAX, "    ");
        assert_eq!(empty.text, "\n");
        assert_eq!(empty.selection, 1..1);
    }

    #[test]
    fn search_respects_unicode_word_boundaries_and_character_indices() {
        assert_eq!(
            find_matches(
                "🦀 Über über_ über.",
                "ÜBER",
                SearchOptions {
                    case_sensitive: false,
                    whole_word: true,
                    ..SearchOptions::default()
                }
            ),
            vec![2..6, 13..17]
        );
        assert!(
            find_matches(
                "über",
                "ÜBER",
                SearchOptions {
                    case_sensitive: true,
                    whole_word: false,
                    ..SearchOptions::default()
                }
            )
            .is_empty()
        );
        assert!(find_matches("İ", "i", SearchOptions::default()).is_empty());
        assert_eq!(
            find_matches("İ", "i\u{307}", SearchOptions::default()),
            vec![0..1]
        );
        assert!(find_matches("a", "", SearchOptions::default()).is_empty());
        assert_eq!(
            find_matches("aaa", "aa", SearchOptions::default()),
            vec![0..2]
        );
    }

    #[test]
    fn replace_is_literal_and_keeps_character_selection() {
        let (result, count) =
            replace_all("α foo β foo", 6..7, "foo", "🦀", SearchOptions::default());
        assert_eq!(result.text, "α 🦀 β 🦀");
        assert_eq!(result.selection, 4..5);
        assert_eq!(count, 2);
        let replacement = replace_selection("α🦀β", Range { start: 2, end: 1 }, "λ");
        assert_eq!(replacement.text, "αλβ");
        assert_eq!(replacement.selection, 2..2);
        let (result, count) = replace_all("abc", 999..999, "", "x", SearchOptions::default());
        assert_eq!(result.text, "abc");
        assert_eq!(result.selection, 3..3);
        assert_eq!(count, 0);
    }

    #[test]
    fn whole_word_replace_changes_only_the_matches_shown_in_find() {
        let text = "git\u{301} git git\u{203f}other";
        let options = SearchOptions {
            case_sensitive: true,
            whole_word: true,
            ..SearchOptions::default()
        };
        let highlighted = crate::search::search(text, "git", true, true, false).unwrap();
        assert_eq!(highlighted, vec![5..8]);
        assert_eq!(find_matches(text, "git", options), highlighted);
        let (result, count) = replace_all(text, 0..0, "git", "svn", options);
        assert_eq!(count, highlighted.len());
        assert_eq!(result.text, "git\u{301} svn git\u{203f}other");
    }

    #[test]
    fn moving_lines_preserves_unicode_crlf_eof_and_selection_boundaries() {
        let moved = move_lines("α\r\nβ\r\nγ", 0..1, true);
        assert_eq!(moved.text, "β\r\nα\r\nγ");
        assert_eq!(moved.selection, 3..4);
        assert_eq!(
            move_lines(&moved.text, moved.selection, false).text,
            "α\r\nβ\r\nγ"
        );
        assert_eq!(move_lines("a\nb", 0..1, true).text, "b\na");
        assert_eq!(move_lines("a\nb\n", 0..1, true).text, "b\na\n");
        let moved = move_lines("a\nb\nc\nd", 2..6, true);
        assert_eq!(moved.text, "a\nd\nb\nc");
        assert_eq!(moved.selection, 4..7);
        assert_eq!(move_lines("a\nb", 0..0, false).text, "a\nb");
        assert_eq!(move_lines("a\nb\n", 2..2, true).text, "a\nb\n");
        assert_eq!(move_lines("", 99..99, true).selection, 0..0);
    }

    #[test]
    fn space_tabs_advance_to_visual_stops_after_unicode_and_existing_tabs() {
        assert_eq!(insert_tab("α", 1..1, 4, false).text, "α   ");
        assert_eq!(insert_tab("\tα", 2..2, 4, false).text, "\tα   ");
        assert_eq!(insert_tab("1234", 4..4, 4, false).text, "1234    ");
        assert_eq!(insert_tab("α\r\nβ", 4..4, 4, false).text, "α\r\nβ   ");
        assert_eq!(insert_tab("α", 1..1, 4, true).text, "α\t");
    }
}
