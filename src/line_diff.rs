//! Line diffs between two in-memory texts.
//!
//! Gutter change markers, draft comparison and disk-conflict views share this
//! core. Lines are compared without a trailing `\r`, so raw CRLF from agent
//! edits or Git blobs does not turn a normalized buffer into a whole-file change.
//! Line numbers are zero-based; patches use Git's one-based unified format and
//! can be shown by `git_diff_view`.

use std::{ops::Range, time::Duration};

use similar::{Algorithm, DiffOp, DiffTag, TextDiff};

/// Diffs taking longer than this fall back to a coarser but still correct result.
const DEADLINE: Duration = Duration::from_millis(250);

/// One contiguous change: `old` lines were replaced by `new` lines.
///
/// An empty `old` range is a pure insertion before old line `old.start`; an
/// empty `new` range is a pure deletion before new line `new.start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub(crate) old: Range<usize>,
    pub(crate) new: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HunkKind {
    Added,
    Removed,
    Modified,
}

impl Hunk {
    pub(crate) fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (_, true) => HunkKind::Removed,
            _ => HunkKind::Modified,
        }
    }
}

/// Split text into lines without their terminators. A final line break does
/// not create an additional empty line; an empty text has no lines.
pub(crate) fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n')
        .map(|line| {
            let line = line.strip_suffix('\n').unwrap_or(line);
            line.strip_suffix('\r').unwrap_or(line)
        })
        .collect()
}

/// Changed regions between `old` and `new`, in document order.
pub(crate) fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let old_lines = lines(old);
    let new_lines = lines(new);
    hunks_between(&old_lines, &new_lines)
}

fn hunks_between(old: &[&str], new: &[&str]) -> Vec<Hunk> {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DEADLINE)
        .diff_slices(old, new);
    let mut hunks: Vec<Hunk> = Vec::new();
    for op in diff.ops() {
        if op.tag() == DiffTag::Equal {
            continue;
        }
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match hunks.last_mut() {
            Some(last) if last.old.end == old_range.start && last.new.end == new_range.start => {
                last.old.end = old_range.end;
                last.new.end = new_range.end;
            }
            _ => hunks.push(Hunk {
                old: old_range,
                new: new_range,
            }),
        }
    }
    hunks
}

/// A Git-style unified patch from `old` to `new`, or an empty string when the
/// texts have the same lines. `label` names the file in the header.
pub(crate) fn unified_patch(old: &str, new: &str, label: &str, context: usize) -> String {
    let old_lines = lines(old);
    let new_lines = lines(new);
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DEADLINE)
        .diff_slices(&old_lines, &new_lines);
    let groups = diff.grouped_ops(context);
    if groups.is_empty() {
        return String::new();
    }
    let label = label.replace('\\', "/");
    let mut patch = format!("diff --git a/{label} b/{label}\n--- a/{label}\n+++ b/{label}\n");
    for group in groups {
        push_group(&mut patch, &group, &old_lines, &new_lines);
    }
    patch
}

fn push_group(patch: &mut String, group: &[DiffOp], old: &[&str], new: &[&str]) {
    let (Some(first), Some(last)) = (group.first(), group.last()) else {
        return;
    };
    let old_range = first.old_range().start..last.old_range().end;
    let new_range = first.new_range().start..last.new_range().end;
    patch.push_str(&format!(
        "@@ -{} +{} @@\n",
        header_range(&old_range),
        header_range(&new_range)
    ));
    for op in group {
        match op.tag() {
            DiffTag::Equal => push_lines(patch, ' ', &old[op.old_range()]),
            DiffTag::Delete => push_lines(patch, '-', &old[op.old_range()]),
            DiffTag::Insert => push_lines(patch, '+', &new[op.new_range()]),
            DiffTag::Replace => {
                push_lines(patch, '-', &old[op.old_range()]);
                push_lines(patch, '+', &new[op.new_range()]);
            }
        }
    }
}

fn push_lines(patch: &mut String, prefix: char, lines: &[&str]) {
    for line in lines {
        patch.push(prefix);
        patch.push_str(line);
        patch.push('\n');
    }
}

/// Unified headers are one-based; an empty range names the line before it.
fn header_range(range: &Range<usize>) -> String {
    match range.len() {
        0 => format!("{},0", range.start),
        1 => format!("{}", range.start + 1),
        count => format!("{},{count}", range.start + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_ignore_terminators_and_a_final_break() {
        assert_eq!(lines(""), Vec::<&str>::new());
        assert_eq!(lines("a"), vec!["a"]);
        assert_eq!(lines("a\n"), vec!["a"]);
        assert_eq!(lines("a\r\n\r\nb"), vec!["a", "", "b"]);
        assert_eq!(lines("a\n\n"), vec!["a", ""]);
    }

    #[test]
    fn crlf_and_lf_texts_with_equal_lines_have_no_hunks() {
        assert!(hunks("a\r\nb\r\n", "a\nb\n").is_empty());
        assert_eq!(unified_patch("a\r\nb", "a\nb", "x.rs", 3), "");
    }

    #[test]
    fn hunks_classify_insertions_deletions_and_modifications() {
        let old = "one\ntwo\nthree\nfour\n";
        let new = "zero\none\nTWO\nthree\n";
        let found = hunks(old, new);
        assert_eq!(
            found,
            vec![
                Hunk {
                    old: 0..0,
                    new: 0..1
                },
                Hunk {
                    old: 1..2,
                    new: 2..3
                },
                Hunk {
                    old: 3..4,
                    new: 4..4
                },
            ]
        );
        let kinds: Vec<_> = found.iter().map(Hunk::kind).collect();
        assert_eq!(
            kinds,
            vec![HunkKind::Added, HunkKind::Modified, HunkKind::Removed]
        );
    }

    #[test]
    fn unified_patch_uses_git_headers_and_parses_in_the_diff_view() {
        let patch = unified_patch("a\nb\nc\n", "a\nB\nc\nd\n", r"src\lib.rs", 1);
        assert_eq!(
            patch,
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n\
             @@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d\n"
        );
        let parsed = crate::git_diff_view::ParsedPatch::parse(&patch);
        assert!(parsed.rows() > 0);
    }

    #[test]
    fn empty_ranges_in_headers_name_the_preceding_line() {
        let patch = unified_patch("", "new\n", "n.txt", 3);
        assert!(patch.contains("@@ -0,0 +1 @@\n+new\n"), "{patch}");
        let patch = unified_patch("gone\n", "", "n.txt", 3);
        assert!(patch.contains("@@ -1 +0,0 @@\n-gone\n"), "{patch}");
    }
}
