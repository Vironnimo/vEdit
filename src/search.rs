//! Search matches use Unicode scalar indices, matching the editor's cursor API.
//! Navigation is independent of highlighting so line anchors remain useful.

use std::{
    ops::Range,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[path = "search_cache.rs"]
mod cache;
pub(crate) use cache::{MatchCache, MatchResult};

const REGEX_SIZE_LIMIT: usize = 256 * 1024;
const REGEX_DFA_SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// Find non-overlapping literal or regular-expression matches in document order.
///
/// Literal matching keeps complete Unicode lowercase expansions, as the editor's
/// Find command does. Regex searches support Rust regex syntax, multiline `^`/`$`
/// anchors, and Vim's `\c` / `\C` case overrides. The last unescaped override wins.
/// Empty patterns produce no matches. Zero-width regex matches can be navigated
/// to; callers should omit them when painting match backgrounds.
pub fn search(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
) -> Result<Vec<Range<usize>>, String> {
    search_inner(text, query, case_sensitive, whole_word, regex, None)
}

pub(crate) fn search_cancellable(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Range<usize>>, String> {
    search_inner(text, query, case_sensitive, whole_word, regex, Some(cancel))
}

fn search_inner(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if !regex {
        return Ok(literal_matches(
            text,
            query,
            case_sensitive,
            whole_word,
            cancel,
        ));
    }

    let Some(pattern) = build_regex(query, case_sensitive)? else {
        return Ok(Vec::new());
    };

    let mut matches = Vec::new();
    let ascii = text.is_ascii();
    let mut previous_byte = 0;
    let mut previous_char = 0;
    for (index, found) in pattern.find_iter(text).enumerate() {
        if index & 4095 == 0 && cancelled(cancel) {
            return Err("Search cancelled".into());
        }
        if whole_word
            && (text[..found.start()]
                .chars()
                .next_back()
                .is_some_and(word_char)
                || text[found.end()..].chars().next().is_some_and(word_char))
        {
            continue;
        }
        let start = if ascii {
            found.start()
        } else {
            previous_char + text[previous_byte..found.start()].chars().count()
        };
        let end = if ascii {
            found.end()
        } else {
            start + found.as_str().chars().count()
        };
        matches.push(start..end);
        previous_byte = found.end();
        previous_char = end;
    }
    Ok(matches)
}

fn build_regex(query: &str, case_sensitive: bool) -> Result<Option<regex::Regex>, String> {
    let (pattern, case_sensitive) = case_overrides(query, case_sensitive);
    if pattern.is_empty() {
        return Ok(None);
    }
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .multi_line(true)
        .crlf(true)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .map(Some)
        .map_err(|error| error.to_string())
}

/// The regex matches of `search` with `replacement` expanded for each, in
/// Rust regex syntax: `$1`, `${name}`, `$0` for the whole match, `$$` for a
/// dollar sign. Ranges are character positions.
pub fn regex_replacements(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    replacement: &str,
) -> Result<Vec<(Range<usize>, String)>, String> {
    let Some(pattern) = build_regex(query, case_sensitive)? else {
        return Ok(Vec::new());
    };
    let mut replacements = Vec::new();
    let mut previous_byte = 0;
    let mut previous_char = 0;
    for captures in pattern.captures_iter(text) {
        let found = captures.get(0).unwrap();
        if whole_word
            && (text[..found.start()]
                .chars()
                .next_back()
                .is_some_and(word_char)
                || text[found.end()..].chars().next().is_some_and(word_char))
        {
            continue;
        }
        let start = previous_char + text[previous_byte..found.start()].chars().count();
        let end = start + found.as_str().chars().count();
        let mut expanded = String::new();
        captures.expand(replacement, &mut expanded);
        replacements.push((start..end, expanded));
        previous_byte = found.end();
        previous_char = end;
    }
    Ok(replacements)
}

/// Pick a counted result relative to the cursor, reporting whether a document
/// boundary was crossed. `matches` must be sorted by start position.
///
/// With `include_current`, a result beginning exactly at the cursor is eligible.
/// Otherwise movement is strict, allowing repeated searches to leave that result.
/// A zero count is treated as one. Large counts use modular arithmetic rather
/// than repeatedly stepping through results.
pub fn next_match(
    matches: &[Range<usize>],
    cursor: usize,
    backwards: bool,
    count: usize,
    include_current: bool,
) -> Option<(Range<usize>, bool)> {
    let len = matches.len();
    if len == 0 {
        return None;
    }
    let steps = count.max(1) - 1;
    let cycles = steps / len;
    let remainder = steps % len;

    let (index, wrapped) = if backwards {
        let boundary = matches.partition_point(|found| {
            if include_current {
                found.start <= cursor
            } else {
                found.start < cursor
            }
        });
        let base = boundary.checked_sub(1).unwrap_or(len - 1);
        let wrapped = boundary == 0 || cycles > 0 || remainder > base;
        let index = if remainder <= base {
            base - remainder
        } else {
            len - (remainder - base)
        };
        (index, wrapped)
    } else {
        let boundary = matches.partition_point(|found| {
            if include_current {
                found.start < cursor
            } else {
                found.start <= cursor
            }
        });
        let base = if boundary == len { 0 } else { boundary };
        let distance_to_end = len - base;
        let wrapped = boundary == len || cycles > 0 || remainder >= distance_to_end;
        let index = if remainder < distance_to_end {
            base + remainder
        } else {
            remainder - distance_to_end
        };
        (index, wrapped)
    };
    Some((matches[index].clone(), wrapped))
}

/// Lowercase one scalar for case-insensitive literal search. Query and text
/// use this same per-character mapping: `str::to_lowercase` would turn a
/// final `Σ` into `ς`, which a per-character `Σ` never becomes. Final and
/// medial sigma therefore fold to the same `σ`.
fn fold_case(ch: char) -> impl Iterator<Item = char> {
    ch.to_lowercase()
        .map(|lower| if lower == 'ς' { 'σ' } else { lower })
}

fn literal_matches(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    cancel: Option<&AtomicBool>,
) -> Vec<Range<usize>> {
    if case_sensitive {
        return direct_literal_matches(text, text, query, whole_word, cancel);
    }
    if text.is_ascii() {
        let searchable = text.to_ascii_lowercase();
        return direct_literal_matches(
            text,
            &searchable,
            &query.chars().flat_map(fold_case).collect::<String>(),
            whole_word,
            cancel,
        );
    }
    let chars: Vec<char> = text.chars().collect();
    let mut searchable = String::with_capacity(text.len());
    let mut boundaries = Vec::with_capacity(chars.len() + 1);
    for (index, &ch) in chars.iter().enumerate() {
        if index & 4095 == 0 && cancelled(cancel) {
            return Vec::new();
        }
        boundaries.push(searchable.len());
        searchable.extend(fold_case(ch));
    }
    boundaries.push(searchable.len());
    // Fold the query exactly like the text, one scalar at a time.
    let query: String = query.chars().flat_map(fold_case).collect();
    let mut matches = Vec::new();
    let mut offset = 0;
    'scan: loop {
        for (index, (relative_start, matched)) in
            searchable[offset..].match_indices(&query).enumerate()
        {
            if index & 4095 == 0 && cancelled(cancel) {
                return matches;
            }
            let byte_start = offset + relative_start;
            let start = match boundaries.binary_search(&byte_start) {
                Ok(start) => start,
                Err(next) => {
                    offset = boundaries[next];
                    continue 'scan;
                }
            };
            let end = match boundaries.binary_search(&(byte_start + matched.len())) {
                Ok(end) => end,
                Err(_) => {
                    // A partial lowercase expansion is not a source match.
                    // Resume at the next source character, so this rejected
                    // candidate cannot consume a later complete match.
                    offset = boundaries[start + 1];
                    continue 'scan;
                }
            };
            if whole_word
                && (start > 0 && word_char(chars[start - 1])
                    || end < chars.len() && word_char(chars[end]))
            {
                continue;
            }
            matches.push(start..end);
        }
        break;
    }
    matches
}

/// `searchable` shares the original byte boundaries: either the original text
/// or ASCII lowercase. Convert offsets while advancing once through the source.
fn direct_literal_matches(
    text: &str,
    searchable: &str,
    query: &str,
    whole_word: bool,
    cancel: Option<&AtomicBool>,
) -> Vec<Range<usize>> {
    let ascii = text.is_ascii();
    let mut previous_byte = 0;
    let mut previous_char = 0;
    searchable
        .match_indices(query)
        .enumerate()
        .take_while(|(index, _)| index & 4095 != 0 || !cancelled(cancel))
        .filter_map(|(_, (byte_start, matched))| {
            let byte_end = byte_start + matched.len();
            if whole_word
                && (text[..byte_start]
                    .chars()
                    .next_back()
                    .is_some_and(word_char)
                    || text[byte_end..].chars().next().is_some_and(word_char))
            {
                return None;
            }
            let start = if ascii {
                byte_start
            } else {
                previous_char + text[previous_byte..byte_start].chars().count()
            };
            let end = if ascii {
                byte_end
            } else {
                start + matched.chars().count()
            };
            previous_byte = byte_end;
            previous_char = end;
            Some(start..end)
        })
        .collect()
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

pub(crate) fn word_char(ch: char) -> bool {
    if ch.is_ascii() {
        return ch.is_ascii_alphanumeric() || ch == '_';
    }
    // Rust regex's Unicode word definition includes combining marks, connector
    // punctuation and join controls, keeping accented identifiers indivisible.
    static WORD: OnceLock<regex::Regex> = OnceLock::new();
    let mut buffer = [0; 4];
    WORD.get_or_init(|| regex::Regex::new(r"^\w$").expect("valid fixed word pattern"))
        .is_match(ch.encode_utf8(&mut buffer))
}

fn case_overrides(query: &str, mut case_sensitive: bool) -> (String, bool) {
    let mut pattern = String::with_capacity(query.len());
    let mut chars = query.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            pattern.push(ch);
            continue;
        }
        match chars.next() {
            Some('c') => case_sensitive = false,
            Some('C') => case_sensitive = true,
            Some(escaped) => {
                pattern.push('\\');
                pattern.push(escaped);
            }
            None => pattern.push('\\'),
        }
    }
    (pattern, case_sensitive)
}

#[cfg(test)]
// Fixtures contain match ranges, not integer sequences.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn literal_search_uses_scalar_positions_and_complete_lowercase_expansions() {
        let text = "🦀 Ö ö İ i\u{307} Straße STRASSE";
        assert_eq!(
            search(text, "ö", false, false, false).unwrap(),
            vec![2..3, 4..5]
        );
        assert_eq!(search(text, "ö", true, false, false).unwrap(), vec![4..5]);
        assert!(search("İ", "i", false, false, false).unwrap().is_empty());
        assert_eq!(
            search("İ", "i\u{307}", false, false, false).unwrap(),
            vec![0..1]
        );
        assert_eq!(
            search("i\u{307}", "İ", false, false, false).unwrap(),
            vec![0..2]
        );
        assert_eq!(
            search(text, "strasse", false, false, false).unwrap(),
            vec![18..25]
        );
        assert!(search("🦀", "", false, false, false).unwrap().is_empty());
        assert_eq!(search("aaa", "aa", true, false, false).unwrap(), vec![0..2]);
    }

    #[test]
    fn regex_replacements_expand_groups_at_character_positions() {
        let text = "🦀 key=1, Key=22, keys=3";
        assert_eq!(
            regex_replacements(text, r"(?P<k>key)=(\d+)", false, true, "$2:${k}$$").unwrap(),
            vec![(2..7, "1:key$".into()), (9..15, "22:Key$".into())]
        );
        assert_eq!(
            regex_replacements(text, r"(?P<k>key)=(\d+)", false, true, "$2:${k}$$")
                .unwrap()
                .iter()
                .map(|(range, _)| range.clone())
                .collect::<Vec<_>>(),
            search(text, r"(?P<k>key)=(\d+)", false, true, true).unwrap()
        );
        assert!(regex_replacements(text, "(", true, false, "x").is_err());
        assert!(
            regex_replacements(text, "", true, false, "x")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn case_insensitive_search_lowercases_a_final_sigma_like_the_text() {
        // `"ΟΔΟΣ".to_lowercase()` ends in `ς`; the text is lowered per scalar.
        for text in ["ΟΔΟΣ", "οδος", "οδοσ", "ΟΔΟΣ ΣΤΗ"] {
            assert_eq!(
                search(text, "ΟΔΟΣ", false, false, false).unwrap(),
                vec![0..4],
                "{text}"
            );
        }
        assert_eq!(
            search("a ΟΔΟΣ b", "ΟΔΟΣ", false, true, false).unwrap(),
            vec![2..6]
        );
    }

    #[test]
    fn rejected_partial_case_expansions_do_not_hide_complete_matches() {
        assert_eq!(
            search("İİi", "İi", false, false, false).unwrap(),
            vec![1..3]
        );
        assert_eq!(
            search("İİi İİi", "İi", false, false, false).unwrap(),
            vec![1..3, 5..7]
        );
        assert_eq!(
            search("İİiİi", "İi", false, false, false).unwrap(),
            vec![1..3, 3..5]
        );
    }

    #[test]
    fn literal_search_does_not_interpret_regex_or_vim_case_overrides() {
        assert_eq!(
            search("a.b axb", "a.b", true, false, false).unwrap(),
            vec![0..3]
        );
        assert_eq!(
            search(r"a\c A", r"a\c", true, false, false).unwrap(),
            vec![0..3]
        );
    }

    #[test]
    fn whole_words_respect_unicode_identifiers_in_both_modes() {
        let text = "git git2 _git git_ λgit gitλ git\u{301} git git\u{200d} git‿";
        for regex in [false, true] {
            assert_eq!(
                search(text, "git", true, true, regex).unwrap(),
                vec![0..3, 34..37]
            );
        }
        assert_eq!(
            search("🦀git 🦀", "git", true, true, true).unwrap(),
            vec![1..4]
        );
    }

    #[test]
    fn regex_supports_unicode_multiline_and_invalid_patterns() {
        assert_eq!(
            search("🦀 αβ\r\nγδ\n", r"^\p{Greek}+", true, false, true).unwrap(),
            vec![6..8]
        );
        assert_eq!(
            search("one\ntwo\n", r"^", true, false, true).unwrap(),
            vec![0..0, 4..4, 8..8]
        );
        assert!(search("one", "[", true, false, true).is_err());
        assert!(search("one", r"(a)\1", true, false, true).is_err());
        assert!(search("one", r"a{1000000}", true, false, true).is_err());
    }

    #[test]
    fn regex_line_anchors_and_dot_respect_windows_line_endings() {
        let text = "α\r\nβ\n\r\n";
        assert_eq!(search(text, "α$", true, false, true).unwrap(), vec![0..1]);
        assert_eq!(
            search(text, ".", true, false, true).unwrap(),
            vec![0..1, 3..4]
        );
        assert_eq!(
            search(text, "$", true, false, true).unwrap(),
            vec![1..1, 4..4, 5..5, 7..7]
        );
        assert_eq!(
            search(text, "^$", true, false, true).unwrap(),
            vec![5..5, 7..7]
        );
    }

    #[test]
    fn vim_case_overrides_respect_escaped_backslashes_and_last_override() {
        assert_eq!(
            search("Git git GIT", r"git\c", true, false, true).unwrap(),
            vec![0..3, 4..7, 8..11]
        );
        assert_eq!(
            search("Git git GIT", r"git\c\C", false, false, true).unwrap(),
            vec![4..7]
        );
        assert_eq!(
            search("Git git GIT", r"\cGit", true, false, true).unwrap(),
            vec![0..3, 4..7, 8..11]
        );
        assert_eq!(
            search(r"a\c A\c", r"a\\c", true, false, true).unwrap(),
            vec![0..3]
        );
        assert_eq!(
            search(r"a\c A\c", r"a\\\cc", true, false, true).unwrap(),
            vec![0..3, 4..7]
        );
        assert!(
            search("anything", r"\c\C", false, false, true)
                .unwrap()
                .is_empty()
        );
        assert!(search("anything", "\\", false, false, true).is_err());
    }

    #[test]
    fn navigation_is_strict_unless_current_match_is_included() {
        let matches = [2..5, 9..12, 18..21];
        assert_eq!(
            next_match(&matches, 2, false, 1, false),
            Some((9..12, false))
        );
        assert_eq!(next_match(&matches, 2, false, 1, true), Some((2..5, false)));
        assert_eq!(next_match(&matches, 9, true, 1, false), Some((2..5, false)));
        assert_eq!(next_match(&matches, 9, true, 1, true), Some((9..12, false)));
        assert_eq!(
            next_match(&matches, 3, false, 1, true),
            Some((9..12, false))
        );
        assert_eq!(next_match(&matches, 3, true, 1, false), Some((2..5, false)));
    }

    #[test]
    fn counted_navigation_wraps_once_or_many_times_without_overflow() {
        let matches = [2..5, 9..12, 18..21];
        assert_eq!(
            next_match(&matches, 18, false, 1, false),
            Some((2..5, true))
        );
        assert_eq!(
            next_match(&matches, 2, true, 1, false),
            Some((18..21, true))
        );
        assert_eq!(
            next_match(&matches, 0, false, 3, false),
            Some((18..21, false))
        );
        assert_eq!(next_match(&matches, 0, false, 4, false), Some((2..5, true)));
        assert_eq!(
            next_match(&matches, 30, true, 3, false),
            Some((2..5, false))
        );
        assert_eq!(
            next_match(&matches, 30, true, 4, false),
            Some((18..21, true))
        );
        assert_eq!(next_match(&matches, 9, false, 3, true), Some((2..5, true)));
        assert_eq!(next_match(&matches, 9, true, 3, true), Some((18..21, true)));
        let expected = (usize::MAX - 1) % matches.len();
        assert_eq!(
            next_match(&matches, 0, false, usize::MAX, false),
            Some((matches[expected].clone(), true))
        );
        assert_eq!(
            next_match(&matches, 30, true, usize::MAX, false),
            Some((matches[2 - expected].clone(), true))
        );
    }

    #[test]
    fn navigation_handles_empty_single_and_zero_width_results() {
        assert_eq!(next_match(&[], 0, false, 1, false), None);
        assert_eq!(next_match(&[0..0], 0, false, 1, true), Some((0..0, false)));
        assert_eq!(next_match(&[0..0], 0, false, 1, false), Some((0..0, true)));
        assert_eq!(next_match(&[0..0], 0, true, 1, false), Some((0..0, true)));
        assert_eq!(next_match(&[4..7], 4, false, 2, true), Some((4..7, true)));
        assert_eq!(next_match(&[4..7], 0, false, 0, false), Some((4..7, false)));
    }
}
