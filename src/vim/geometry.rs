//! Unicode scalar geometry, CRLF lines, words and text-object ranges.
use super::*;

pub(super) fn char_to_byte(text: &str, cursor: usize) -> usize {
    text.char_indices()
        .nth(cursor)
        .map_or(text.len(), |(index, _)| index)
}

pub(super) fn char_boundary(text: &str, cursor: usize) -> (usize, usize) {
    let mut count = 0;
    for (position, (byte, _)) in text.char_indices().enumerate() {
        if position == cursor {
            return (position, byte);
        }
        count = position + 1;
    }
    (count, text.len())
}

fn word_class(ch: char, big: bool) -> u8 {
    if ch.is_whitespace() {
        0
    } else if big || ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

/// Whether an empty line, LF or CRLF, starts at this index.
fn empty_line_at(chars: &[char], index: usize) -> bool {
    (index == 0 || chars[index - 1] == '\n')
        && match chars.get(index) {
            Some('\n') => true,
            Some('\r') => chars.get(index + 1) == Some(&'\n'),
            _ => false,
        }
}

pub(super) fn word_forward(chars: &[char], cursor: usize, big: bool) -> usize {
    let mut at = cursor.min(chars.len());
    if at == chars.len() {
        return at;
    }
    let empty_line_at = |index: usize| empty_line_at(chars, index);
    if empty_line_at(at) {
        at += 1;
    } else {
        let class = word_class(chars[at], big);
        while at < chars.len() && word_class(chars[at], big) == class && !empty_line_at(at) {
            at += 1;
        }
    }
    while at < chars.len() && chars[at].is_whitespace() {
        if empty_line_at(at) {
            break;
        }
        at += 1;
    }
    at
}

pub(super) fn word_backward(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() || cursor == 0 {
        return 0;
    }
    let mut at = cursor.min(chars.len()).saturating_sub(1);
    let empty_line_at = |index: usize| empty_line_at(chars, index);
    while at > 0 && chars[at].is_whitespace() {
        if empty_line_at(at) {
            return at;
        }
        at -= 1;
    }
    let class = word_class(chars[at], big);
    while at > 0 && word_class(chars[at - 1], big) == class {
        at -= 1;
    }
    at
}

pub(super) fn word_end(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() {
        return 0;
    }
    let mut at = cursor.min(chars.len() - 1);
    if at + 1 >= chars.len() {
        return at;
    }
    let class = word_class(chars[at], big);
    at += 1;
    if class == 0 || word_class(chars[at], big) != class {
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
    }
    if at == chars.len() {
        let line = last_line(chars);
        return line.end.saturating_sub(1).max(line.start);
    }
    let class = word_class(chars[at], big);
    while at + 1 < chars.len() && word_class(chars[at + 1], big) == class {
        at += 1;
    }
    at
}

pub(super) fn word_end_backward(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() || cursor == 0 {
        return 0;
    }
    let mut at = cursor.min(chars.len()).saturating_sub(1);
    // From inside a word, ge skips the current word before looking for the
    // preceding end. At its start or on whitespace, the previous end is next.
    if cursor < chars.len() && word_class(chars[cursor], big) != 0 {
        let class = word_class(chars[cursor], big);
        while word_class(chars[at], big) == class {
            if at == 0 {
                return 0;
            }
            at -= 1;
        }
    }
    while at > 0 && chars[at].is_whitespace() {
        at -= 1;
    }
    at
}

pub(super) fn change_word_end(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() {
        return 0;
    }
    let mut at = cursor.min(chars.len() - 1);
    let class = word_class(chars[at], big);
    while at + 1 < chars.len() && word_class(chars[at + 1], big) == class {
        at += 1;
    }
    at
}

pub(super) fn repeat_motion(
    chars: &[char],
    cursor: usize,
    count: usize,
    big: bool,
    motion: fn(&[char], usize, bool) -> usize,
) -> usize {
    let mut at = cursor;
    for _ in 0..count {
        let next = motion(chars, at, big);
        if next == at {
            break;
        }
        at = next;
    }
    at
}

pub(super) fn text_object(
    chars: &[char],
    cursor: usize,
    object: char,
    count: usize,
    around: bool,
) -> Option<Range<usize>> {
    if chars.is_empty() {
        return None;
    }
    let cursor = cursor.min(chars.len() - 1);
    if matches!(object, 'w' | 'W') {
        // Words, blanks and their counts stay on the cursor's line. An empty
        // line has no word to select.
        let line = line_at_cursor(chars, cursor);
        if line.start == line.end {
            return None;
        }
        let cursor = cursor.clamp(line.start, line.end - 1);
        let big = object == 'W';
        let class = |index: usize| word_class(chars[index], big);
        let run_end = |from: usize| {
            let kind = class(from);
            let mut end = from;
            while end < line.end && class(end) == kind {
                end += 1;
            }
            end
        };
        let mut start = cursor;
        while start > line.start && class(start - 1) == class(cursor) {
            start -= 1;
        }
        let mut end = run_end(cursor);
        if around {
            // On whitespace, "a word" includes the following word. Otherwise
            // each count adds another word and the whitespace between them.
            let words = if class(cursor) == 0 {
                count
            } else {
                count.saturating_sub(1)
            };
            for _ in 0..words {
                while end < line.end && class(end) == 0 {
                    end += 1;
                }
                if end == line.end {
                    break;
                }
                end = run_end(end);
            }
            if class(cursor) != 0 {
                let content_end = end;
                while end < line.end && class(end) == 0 {
                    end += 1;
                }
                // Without trailing blanks, take the leading ones instead.
                if end == content_end {
                    while start > line.start && class(start - 1) == 0 {
                        start -= 1;
                    }
                }
            }
        } else {
            // Inner-word counts include whitespace as its own text object.
            for _ in 1..count {
                if end == line.end {
                    break;
                }
                end = run_end(end);
            }
        }
        return Some(start..end);
    }
    if matches!(object, '\'' | '"' | '`') {
        let rows = lines(chars);
        let line = rows[row_for_cursor(&rows, cursor)];
        let quotes: Vec<usize> = (line.start..line.end)
            .filter(|index| {
                if chars[*index] != object {
                    return false;
                }
                let backslashes = chars[line.start..*index]
                    .iter()
                    .rev()
                    .take_while(|ch| **ch == '\\')
                    .count();
                backslashes % 2 == 0
            })
            .collect();
        let preceding = quotes.iter().rposition(|index| *index <= cursor);
        let opening = match preceding {
            Some(index) if quotes[index] == cursor && index % 2 == 1 => index - 1,
            Some(index) => index,
            None => 0,
        };
        let pair = (*quotes.get(opening)?, *quotes.get(opening + 1)?);
        let mut range = if around || count > 1 {
            pair.0..pair.1 + 1
        } else {
            pair.0 + 1..pair.1
        };
        if around {
            let content_end = range.end;
            while range.end < line.end && matches!(chars[range.end], ' ' | '\t') {
                range.end += 1;
            }
            if range.end == content_end {
                while range.start > line.start && matches!(chars[range.start - 1], ' ' | '\t') {
                    range.start -= 1;
                }
            }
        }
        return Some(range);
    }
    let (open, close) = match object {
        '(' | ')' | 'b' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' | 'B' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        _ => return None,
    };
    let mut pairs = Vec::new();
    let mut stack = Vec::new();
    for (index, ch) in chars.iter().enumerate() {
        if *ch == open {
            stack.push(index);
        } else if *ch == close
            && let Some(start) = stack.pop()
        {
            pairs.push((start, index));
        }
    }
    // Closing delimiters encounter enclosing pairs from the inside out. From
    // outside a block, Vim searches forward by opening delimiter instead.
    let enclosing: Vec<_> = pairs
        .iter()
        .copied()
        .filter(|pair| pair.0 <= cursor && cursor <= pair.1)
        .collect();
    let pair = if enclosing.is_empty() {
        pairs.sort_unstable_by_key(|pair| pair.0);
        let mut pair = *pairs.iter().find(|pair| pair.0 > cursor)?;
        for _ in 1..count {
            pair = *pairs
                .iter()
                .find(|nested| pair.0 < nested.0 && nested.1 < pair.1)?;
        }
        pair
    } else {
        *enclosing.get(count.saturating_sub(1))?
    };
    Some(if around {
        pair.0..pair.1 + 1
    } else {
        inner_block(chars, pair)
    })
}

/// The inside of a delimiter pair. As in Vim, an opening delimiter that ends
/// its line starts the inner text on the next line, and a closing delimiter
/// preceded only by indentation leaves its own line out, so a block on its own
/// lines yields whole lines.
fn inner_block(chars: &[char], (open, close): (usize, usize)) -> Range<usize> {
    let mut start = open + 1;
    if chars.get(start) == Some(&'\n') {
        start += 1;
    } else if chars.get(start) == Some(&'\r') && chars.get(start + 1) == Some(&'\n') {
        start += 2;
    }
    let line_start = chars[..close]
        .iter()
        .rposition(|ch| *ch == '\n')
        .map_or(0, |at| at + 1);
    let end = if line_start > open
        && chars[line_start..close]
            .iter()
            .all(|ch| matches!(ch, ' ' | '\t'))
    {
        line_start
    } else {
        close
    };
    start.min(end)..end
}

/// `ip` / `ap` as whole rows. Blank lines, also those containing only
/// whitespace, form blocks of their own; `ap` adds the following blank lines,
/// or the preceding ones when none follow.
pub(super) fn paragraph_object(
    chars: &[char],
    cursor: usize,
    count: usize,
    around: bool,
) -> (usize, usize) {
    let rows = lines(chars);
    let last = last_line_row(&rows);
    let blank = |row: usize| {
        chars[rows[row].start..rows[row].end]
            .iter()
            .all(|ch| ch.is_whitespace())
    };
    let run_end = |from: usize| {
        let kind = blank(from);
        let mut end = from;
        while end < last && blank(end + 1) == kind {
            end += 1;
        }
        end
    };
    let row = row_for_cursor(&rows, cursor).min(last);
    let mut first = row;
    while first > 0 && blank(first - 1) == blank(row) {
        first -= 1;
    }
    let mut end = run_end(row);
    let runs = if around {
        count.saturating_mul(2)
    } else {
        count
    };
    let mut taken = 1;
    while taken < runs && end < last {
        end = run_end(end + 1);
        taken += 1;
    }
    if around && !blank(row) && !blank(end) {
        while first > 0 && blank(first - 1) {
            first -= 1;
        }
    }
    (first, end)
}

pub(super) fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

pub(super) fn remove_last_line_ending(text: &mut String) {
    if text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if text.ends_with('\n') {
        text.pop();
    }
}

pub(super) fn lines(chars: &[char]) -> Vec<Line> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, ch) in chars.iter().enumerate() {
        if *ch == '\n' {
            result.push(Line {
                start,
                end: index - usize::from(index > start && chars[index - 1] == '\r'),
                newline: Some(index),
            });
            start = index + 1;
        }
    }
    result.push(Line {
        start,
        end: chars.len(),
        newline: None,
    });
    result
}

pub(super) fn row_for_cursor(rows: &[Line], cursor: usize) -> usize {
    rows.partition_point(|line| line.start <= cursor)
        .saturating_sub(1)
}

pub(super) fn line_at_cursor(chars: &[char], cursor: usize) -> Line {
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|&ch| ch == '\n')
        .map_or(0, |at| at + 1);
    let newline = chars[cursor..]
        .iter()
        .position(|&ch| ch == '\n')
        .map(|at| cursor + at);
    let end = newline.map_or(chars.len(), |at| {
        at - usize::from(at > start && chars[at - 1] == '\r')
    });
    Line {
        start,
        end,
        newline,
    }
}

pub(super) fn relative_line(chars: &[char], mut line: Line, count: usize, down: bool) -> Line {
    for _ in 0..count {
        let next = if down {
            line.newline.map(|at| at + 1)
        } else {
            line.start.checked_sub(1)
        };
        let Some(next) = next else { break };
        line = line_at_cursor(chars, next);
    }
    line
}

pub(super) fn first_nonblank(chars: &[char], line: Line) -> usize {
    let mut at = line.start;
    while at < line.end && (chars[at] == ' ' || chars[at] == '\t') {
        at += 1;
    }
    at.min(line.end.saturating_sub(1).max(line.start))
}

pub(super) fn last_nonblank(chars: &[char], line: Line) -> usize {
    chars[line.start..line.end]
        .iter()
        .rposition(|ch| !ch.is_whitespace())
        .map_or(line.start, |column| line.start + column)
}

pub(super) fn normal_cursor(text: &str, cursor: usize) -> usize {
    let (mut cursor, byte) = char_boundary(text, cursor);
    let mut prefix = &text[..byte];
    let suffix = &text[byte..];
    match suffix.chars().next() {
        Some('\n') => {
            if prefix.ends_with('\r') {
                prefix = &prefix[..prefix.len() - 1];
                cursor -= 1;
            }
        }
        Some('\r') if suffix.starts_with("\r\n") => {}
        None => {}
        _ => return cursor,
    }
    // Only a line ending or EOF needs moving back. Empty lines keep their
    // boundary, including CRLF and the empty row after a final newline.
    if !prefix.is_empty() && !prefix.ends_with('\n') {
        cursor -= 1;
    }
    cursor
}

pub(super) fn matching_bracket(chars: &[char], cursor: usize) -> Option<usize> {
    let rows = lines(chars);
    let line = rows[row_for_cursor(&rows, cursor)];
    let at = (cursor..line.end).find(|index| is_bracket(chars[*index]))?;
    bracket_partner(chars, at, chars.len())
}

pub(super) fn is_bracket(ch: char) -> bool {
    matches!(ch, '(' | ')' | '[' | ']' | '{' | '}')
}

/// The bracket that pairs with the one at `at`, looking at most `limit`
/// characters away.
pub(super) fn bracket_partner(chars: &[char], at: usize, limit: usize) -> Option<usize> {
    let (open, close, forward) = match chars[at] {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        _ => return None,
    };
    let mut depth: usize = 1;
    if forward {
        for (index, ch) in chars.iter().enumerate().skip(at + 1).take(limit) {
            if *ch == open {
                depth += 1;
            } else if *ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
    } else {
        for index in (at.saturating_sub(limit)..at).rev() {
            if chars[index] == close {
                depth += 1;
            } else if chars[index] == open {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
    }
    None
}

/// `{` / `}`: the next empty line after at least one nonempty line, counting
/// the cursor's own line. Without one, the motion ends at the start or end of
/// the document.
pub(super) fn paragraph(chars: &[char], cursor: usize, count: usize, forward: bool) -> usize {
    let rows = lines(chars);
    let empty = |row: usize| rows[row].start == rows[row].end;
    let mut row = row_for_cursor(&rows, cursor);
    for _ in 0..count {
        let mut content = !empty(row);
        loop {
            let next = if forward {
                Some(row + 1).filter(|next| *next < rows.len())
            } else {
                row.checked_sub(1)
            };
            let Some(next) = next else {
                return if forward { chars.len() } else { 0 };
            };
            row = next;
            if content && empty(row) {
                break;
            }
            content |= !empty(row);
        }
    }
    rows[row].start
}

/// The last line in Vim's model, where a final line break terminates the
/// last line instead of starting an empty one.
pub(super) fn last_line_row(rows: &[Line]) -> usize {
    let last = rows.len() - 1;
    if last > 0 && rows[last].start == rows[last].end {
        last - 1
    } else {
        last
    }
}

pub(super) fn last_line(chars: &[char]) -> Line {
    line_at_cursor(
        chars,
        chars.len() - usize::from(chars.last() == Some(&'\n')),
    )
}
