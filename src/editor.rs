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
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let position = selection.start + replacement.chars().count();
    let mut result = apply_edits(
        &chars,
        selection.clone(),
        vec![Edit {
            range: selection,
            replacement: replacement.to_owned(),
        }],
    );
    result.selection = position..position;
    result
}

/// Indent or outdent the selected lines. A collapsed selection edits its line.
/// Outdent removes one tab or up to `indent`'s width in leading spaces.
pub fn indent_lines(
    text: &str,
    selection: Range<usize>,
    indent: &str,
    outdent: bool,
) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let width = indent.chars().count().max(1);
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
        "py" | "python" | "pyw" | "rb" | "ruby" | "sh" | "bash" | "zsh" | "fish" | "ps1"
        | "psm1" | "powershell" | "toml" | "yaml" | "yml" | "r" | "julia" | "jl" | "conf"
        | "dockerfile" | "makefile" | "make" => ("#", ""),
        "sql" | "lua" | "hs" | "haskell" | "elm" => ("--", ""),
        "html" | "htm" | "xml" | "svg" | "markdown" | "md" => ("<!--", "-->"),
        "css" => ("/*", "*/"),
        "ini" | "lisp" | "clojure" | "clj" | "el" | "asm" => (";", ""),
        "tex" | "latex" | "matlab" | "erl" | "erlang" => ("%", ""),
        "vb" | "vbs" | "vbnet" => ("'", ""),
        "f" | "f90" | "fortran" => ("!", ""),
        "bat" | "cmd" => ("REM", ""),
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

/// Insert a newline with the current line's indent. Opening braces, brackets,
/// parentheses and a trailing colon add one level. Between matching delimiters
/// this creates an indented empty line and places the caret there.
pub fn smart_newline(text: &str, selection: Range<usize>, indent: &str) -> EditResult {
    let chars: Vec<char> = text.chars().collect();
    let selection = clamp_selection(selection, chars.len());
    let rows = lines(&chars);
    let line = rows[row_at(&rows, selection.start)];
    let indent_end = indentation_end(&chars, line).min(selection.start);
    let base: String = chars[line.start..indent_end].iter().collect();
    let left = chars[line.start..selection.start]
        .iter()
        .rev()
        .find(|&&ch| !horizontal_space(ch))
        .copied();
    let deeper = matches!(left, Some('{' | '[' | '(' | ':'));
    let expected_close = match left {
        Some('{') => Some('}'),
        Some('[') => Some(']'),
        Some('(') => Some(')'),
        _ => None,
    };
    let right_at = selection.end
        + chars[selection.end..]
            .iter()
            .take_while(|&&ch| horizontal_space(ch))
            .count();
    let between = expected_close.is_some() && chars.get(right_at).copied() == expected_close;
    let ending = newline(text);
    let mut replacement = format!("{ending}{base}{}", if deeper { indent } else { "" });
    // Keep a separate caret offset when a closing delimiter needs its own line.
    let cursor = selection.start + replacement.chars().count();
    if between {
        replacement.push_str(ending);
        replacement.push_str(&base);
    }
    let end = if between { right_at } else { selection.end };
    let mut result = apply_edits(
        &chars,
        selection.start..end,
        vec![Edit {
            range: selection.start..end,
            replacement,
        }],
    );
    result.selection = cursor..cursor;
    result
}

fn word_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

/// Find non-overlapping literal matches. Empty queries produce no matches.
/// Case-insensitive matching uses Unicode lowercase while retaining the original
/// character boundaries; a match never selects half of a lowercase expansion.
pub fn find_matches(text: &str, query: &str, options: SearchOptions) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut searchable = String::new();
    let mut boundaries = Vec::with_capacity(chars.len() + 1);
    for &ch in &chars {
        boundaries.push(searchable.len());
        if options.case_sensitive {
            searchable.push(ch);
        } else {
            searchable.extend(ch.to_lowercase());
        }
    }
    boundaries.push(searchable.len());
    let query = if options.case_sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };
    searchable
        .match_indices(&query)
        .filter_map(|(start, matched)| {
            let start = boundaries.binary_search(&start).ok()?;
            let end = boundaries
                .binary_search(&(boundaries[start] + matched.len()))
                .ok()?;
            if options.whole_word
                && (start > 0 && word_char(chars[start - 1])
                    || end < chars.len() && word_char(chars[end]))
            {
                return None;
            }
            Some(start..end)
        })
        .collect()
}

/// Replace all literal matches, returning the number replaced. The selection
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
    let matches = find_matches(text, query, options);
    let count = matches.len();
    let edits = matches
        .into_iter()
        .map(|range| Edit {
            range,
            replacement: replacement.to_owned(),
        })
        .collect();
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
    fn search_respects_unicode_word_boundaries_and_character_indices() {
        assert_eq!(
            find_matches(
                "🦀 Über über_ über.",
                "ÜBER",
                SearchOptions {
                    case_sensitive: false,
                    whole_word: true
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
                    whole_word: false
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
}
