//! `path:line[:column]` locations in command and application output.
//!
//! Detection is a cheap scan of one output line; it never touches the disk.
//! `resolve` checks candidates on disk and must run off the UI thread.

use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Byte range of the clickable text within the scanned line.
    pub range: Range<usize>,
    pub path: String,
    /// One-based line.
    pub line: usize,
    /// One-based character column, when the tool reports one.
    pub column: Option<usize>,
}

/// Every location in one output line, left to right.
pub fn find(line: &str) -> Vec<Link> {
    let mut links = python_traceback(line).into_iter().collect::<Vec<_>>();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let found = match bytes[index] {
            b':' => colon_location(line, index),
            b'(' => parenthesized_location(line, index),
            _ => None,
        };
        match found {
            Some(link)
                if !links
                    .iter()
                    .any(|known| overlaps(&known.range, &link.range)) =>
            {
                index = link.range.end;
                links.push(link);
            }
            _ => index += 1,
        }
    }
    links.sort_by_key(|link| link.range.start);
    links
}

fn overlaps(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

/// Python: `File "C:\project\x.py", line 12, in main`.
fn python_traceback(line: &str) -> Option<Link> {
    let start = line.find("File \"")? + "File \"".len();
    let length = line[start..].find('"')?;
    let path = &line[start..start + length];
    let rest = line[start + length..].strip_prefix("\", line ")?;
    let (number, digits) = number(rest)?;
    plausible_path(path).then(|| Link {
        range: start..start + length + "\", line ".len() + digits,
        path: path.into(),
        line: number,
        column: None,
    })
}

/// cargo/rustc/clippy `src/main.rs:10:5`, pytest `tests/test_x.py:12:`,
/// Dart `lib/x.dart:12:3` and the many tools that share the format.
fn colon_location(line: &str, colon: usize) -> Option<Link> {
    let (line_number, digits) = number(&line[colon + 1..])?;
    let mut end = colon + 1 + digits;
    let column = line[end..]
        .strip_prefix(':')
        .and_then(number)
        .map(|(column, digits)| {
            end += 1 + digits;
            column
        });
    let start = path_start(line, colon);
    let path = &line[start..colon];
    plausible_path(path).then(|| Link {
        range: start..end,
        path: path.into(),
        line: line_number,
        column,
    })
}

/// TypeScript and MSBuild: `src/x.ts(12,3)`.
fn parenthesized_location(line: &str, open: usize) -> Option<Link> {
    let rest = &line[open + 1..];
    let (line_number, digits) = number(rest)?;
    let rest = rest[digits..].strip_prefix(',')?;
    let (column, column_digits) = number(rest)?;
    rest[column_digits..].strip_prefix(')')?;
    let start = path_start(line, open);
    let path = &line[start..open];
    plausible_path(path).then(|| Link {
        range: start..open + 1 + digits + 1 + column_digits + 1,
        path: path.into(),
        line: line_number,
        column: Some(column),
    })
}

/// A positive decimal number at the start of `text` and its length.
fn number(text: &str) -> Option<(usize, usize)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let value: usize = text[..digits].parse().ok()?;
    (value > 0).then_some((value, digits))
}

fn delimiter(character: char) -> bool {
    character.is_whitespace() || "\"'`<>()[]{}|,;=*".contains(character)
}

/// Walk back from `end` over the path; a drive prefix such as `C:` belongs
/// to it, any other colon ends it.
fn path_start(line: &str, end: usize) -> usize {
    let mut start = end;
    for (index, character) in line[..end].char_indices().rev() {
        if delimiter(character) {
            break;
        }
        if character == ':' {
            let drive = index >= 1
                && line.as_bytes()[index - 1].is_ascii_alphabetic()
                && (index == 1 || line[..index - 1].ends_with(delimiter))
                && line[index + 1..end].starts_with(['\\', '/']);
            if drive {
                start = index - 1;
            }
            break;
        }
        start = index;
    }
    start
}

/// File names with an extension; rejects URLs, times, versions and hosts.
fn plausible_path(path: &str) -> bool {
    // `//host/x.js` is what remains of a URL after its scheme's colon.
    if path.is_empty() || path.contains("://") || path.starts_with("//") || path.len() > 1024 {
        return false;
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !extension.is_empty()
        && extension
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && extension.chars().any(|c| c.is_ascii_alphabetic())
}

/// The file `path` names: absolute, or relative to the first base that has
/// it. Relative paths are joined component by component so `/` and `..`
/// also work under verbatim (`\\?\`) project roots. Paths under `root` keep
/// the root's spelling; others are canonicalized, as opening a file does.
pub fn resolve(path: &str, root: &Path, bases: &[PathBuf]) -> Option<PathBuf> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return std::fs::canonicalize(candidate)
            .ok()
            .filter(|path| path.is_file());
    }
    bases.iter().find_map(|base| {
        let mut joined = base.clone();
        for part in path.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    joined.pop();
                }
                part => joined.push(part),
            }
        }
        if !joined.is_file() {
            return None;
        }
        if joined.starts_with(root) {
            Some(joined)
        } else {
            std::fs::canonicalize(&joined).ok()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(line: &str) -> (String, &str, usize, Option<usize>) {
        let links = find(line);
        assert_eq!(links.len(), 1, "{line}: {links:?}");
        let link = &links[0];
        (
            link.path.clone(),
            &line[link.range.clone()],
            link.line,
            link.column,
        )
    }

    #[test]
    fn cargo_and_rustc_arrows_and_panics() {
        assert_eq!(
            one("  --> src/main.rs:10:5"),
            ("src/main.rs".into(), "src/main.rs:10:5", 10, Some(5))
        );
        assert_eq!(
            one("thread 'main' panicked at src\\lib.rs:7:13:"),
            ("src\\lib.rs".into(), "src\\lib.rs:7:13", 7, Some(13))
        );
        assert_eq!(
            one("warning: unused variable --> crates/a-b/src/x_y.rs:1:1"),
            (
                "crates/a-b/src/x_y.rs".into(),
                "crates/a-b/src/x_y.rs:1:1",
                1,
                Some(1)
            )
        );
    }

    #[test]
    fn clippy_with_windows_absolute_paths() {
        assert_eq!(
            one("error: C:\\work\\proj\\src\\app.rs:120:9: this could be simpler"),
            (
                "C:\\work\\proj\\src\\app.rs".into(),
                "C:\\work\\proj\\src\\app.rs:120:9",
                120,
                Some(9)
            )
        );
        assert_eq!(one("D:/x/y.rs:3").0, "D:/x/y.rs");
    }

    #[test]
    fn pytest_locations_without_columns() {
        assert_eq!(
            one("tests/test_x.py:12: AssertionError"),
            ("tests/test_x.py".into(), "tests/test_x.py:12", 12, None)
        );
    }

    #[test]
    fn python_tracebacks() {
        let line = r#"  File "C:\Users\me\project\x.py", line 12, in main"#;
        let (path, text, number, column) = one(line);
        assert_eq!(path, r"C:\Users\me\project\x.py");
        assert_eq!(text, r#"C:\Users\me\project\x.py", line 12"#);
        assert_eq!((number, column), (12, None));
        assert_eq!(one(r#"File "app/run.py", line 3"#).0, "app/run.py");
        assert!(find(r#"File "<string>", line 1, in <module>"#).is_empty());
    }

    #[test]
    fn dart_analyzer_and_flutter() {
        assert_eq!(
            one("  error - lib/x.dart:12:3 - Undefined name 'y'."),
            ("lib/x.dart".into(), "lib/x.dart:12:3", 12, Some(3))
        );
        assert_eq!(one("lib/main.dart:4:7: Error: Expected ';'").2, 4);
    }

    #[test]
    fn typescript_parentheses() {
        assert_eq!(
            one("src/app.ts(12,3): error TS2304: Cannot find name 'x'."),
            ("src/app.ts".into(), "src/app.ts(12,3)", 12, Some(3))
        );
    }

    #[test]
    fn several_locations_and_non_locations() {
        let links = find("a.rs:1:2 and b.py:3");
        assert_eq!(
            links
                .iter()
                .map(|link| link.path.as_str())
                .collect::<Vec<_>>(),
            ["a.rs", "b.py"]
        );
        for line in [
            "Listening on http://localhost:8080",
            "https://example.com/a.js:10:2",
            "started at 12:30:45",
            "version 1.2:3",
            "127.0.0.1:8080",
            "line:12",
            "src/main.rs:0:1",
            "Finished `dev` profile in 0.42s",
            "",
            "Zürich 😀 x.rs",
        ] {
            assert!(find(line).is_empty(), "{line}: {:?}", find(line));
        }
        // Non-ASCII text before and inside the path keeps byte ranges valid.
        let line = "😀 fehler: src/grüße.rs:2:1";
        assert_eq!(&line[find(line)[0].range.clone()], "src/grüße.rs:2:1");
    }

    #[test]
    fn resolve_prefers_the_project_root_then_the_working_directory() {
        let root = std::env::temp_dir().join(format!("vedit-output-links-{}", std::process::id()));
        let cwd = root.join("app");
        std::fs::create_dir_all(cwd.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(cwd.join("lib/x.dart"), "").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let cwd = root.join("app");
        let bases = [root.clone(), cwd.clone()];
        assert_eq!(
            resolve("src/main.rs", &root, &bases),
            Some(root.join("src").join("main.rs"))
        );
        assert_eq!(
            resolve("./lib\\x.dart", &root, &bases),
            Some(cwd.join("lib").join("x.dart"))
        );
        assert_eq!(
            resolve("../src/main.rs", &root, std::slice::from_ref(&cwd)),
            Some(root.join("src").join("main.rs"))
        );
        let absolute = root.join("src").join("main.rs");
        assert_eq!(
            resolve(&absolute.to_string_lossy(), &root, &bases),
            Some(std::fs::canonicalize(&absolute).unwrap())
        );
        assert_eq!(resolve("src/missing.rs", &root, &bases), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
