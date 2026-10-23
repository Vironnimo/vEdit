//! Persistent file bookmarks with one-based Unicode cursor positions.
//!
//! The application owns session I/O. This module only manages serializable
//! records, path identity, and conservative relocation after text changes.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

const ANCHOR_SEARCH_RADIUS: usize = 80;

fn first_position() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default = "first_position")]
    pub line: usize,
    #[serde(default = "first_position")]
    pub column: usize,
    #[serde(default)]
    pub label: String,
    /// The exact line without its line ending, captured when the bookmark was
    /// set. Empty or whitespace-only context never causes relocation.
    #[serde(default)]
    pub context_line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleResult {
    Added(u64),
    Removed(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkStore {
    /// Stable insertion order, independent of which projects or tabs are open.
    pub items: Vec<Bookmark>,
    next_id: u64,
}

impl Default for BookmarkStore {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
        }
    }
}

impl BookmarkStore {
    /// Restore session records, assigning IDs for legacy entries and keeping
    /// the first record when a legacy session repeats the same file and line.
    pub fn from_items(items: Vec<Bookmark>) -> Self {
        let mut store = Self {
            next_id: items
                .iter()
                .map(|item| item.id)
                .max()
                .unwrap_or(0)
                .wrapping_add(1)
                .max(1),
            ..Self::default()
        };
        let mut positions = HashSet::new();
        for mut item in items {
            if item.path.as_os_str().is_empty() {
                continue;
            }
            item.line = item.line.max(1);
            item.column = item.column.max(1);
            if !positions.insert((path_key(&item.path), item.line)) {
                continue;
            }
            if item.id == 0 || store.items.iter().any(|existing| existing.id == item.id) {
                item.id = store.allocate_id();
            }
            store.items.push(item);
        }
        store
    }

    pub fn get(&self, id: u64) -> Option<&Bookmark> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn for_path<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Bookmark> {
        let key = path_key(path);
        self.items
            .iter()
            .filter(move |item| path_key(&item.path) == key)
    }

    /// Toggle a single bookmark per file and logical line. Clicking again at a
    /// different column on that line removes the existing bookmark.
    pub fn toggle(
        &mut self,
        path: &Path,
        line: usize,
        column: usize,
        context_line: Option<&str>,
    ) -> ToggleResult {
        let key = path_key(path);
        let line = line.max(1);
        if let Some(index) = self
            .items
            .iter()
            .position(|item| path_key(&item.path) == key && item.line == line)
        {
            return ToggleResult::Removed(self.items.remove(index).id);
        }
        let id = self.allocate_id();
        self.items.push(Bookmark {
            id,
            path: path.to_path_buf(),
            line,
            column: column.max(1),
            label: String::new(),
            context_line: without_line_ending(context_line.unwrap_or_default()).to_owned(),
        });
        ToggleResult::Added(id)
    }

    pub fn set_label(&mut self, id: u64, label: &str) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        item.label = label.to_owned();
        true
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        before != self.items.len()
    }

    pub fn clear(&mut self) -> usize {
        let count = self.items.len();
        self.items.clear();
        count
    }

    /// Follow a file or directory rename without touching unrelated siblings.
    /// Components are compared using Windows path identity on Windows. IDs,
    /// labels, cursor positions, and insertion order survive the rename.
    pub fn rename_prefix(&mut self, old: &Path, new: &Path) -> usize {
        let old_components = normalized_components(old);
        if old_components.is_empty() || new.as_os_str().is_empty() {
            return 0;
        }
        let old_keys: Vec<_> = old_components.iter().map(component_key).collect();
        let mut changed = 0;
        for item in &mut self.items {
            let components = normalized_components(&item.path);
            if components.len() < old_keys.len()
                || !components
                    .iter()
                    .zip(&old_keys)
                    .all(|(part, old)| component_key(part) == *old)
            {
                continue;
            }
            let mut path = new.to_path_buf();
            for component in &components[old_keys.len()..] {
                path.push(component);
            }
            item.path = path;
            changed += 1;
        }
        // Replacing a destination can make two old records refer to the same
        // file and line. Keep the first stable record as during session load.
        let mut positions = HashSet::new();
        self.items
            .retain(|item| positions.insert((path_key(&item.path), item.line)));
        changed
    }

    fn allocate_id(&mut self) -> u64 {
        loop {
            let id = self.next_id.max(1);
            self.next_id = id.wrapping_add(1).max(1);
            if !self.items.iter().any(|item| item.id == id) {
                return id;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmarkLocation {
    pub line: usize,
    pub column: usize,
    /// Zero-based Unicode character index, compatible with editor selections.
    pub char_index: usize,
}

/// Locate a stored cursor without guessing a distant destination. The exact
/// anchor may move within 80 lines. Equal-distance repeated anchors and missing
/// anchors keep the old line; columns and stale line numbers are always clamped.
pub fn locate(document: &str, bookmark: &Bookmark) -> BookmarkLocation {
    let lines: Vec<_> = document.split('\n').collect();
    let preferred = bookmark.line.saturating_sub(1).min(lines.len() - 1);
    let text_line = |index: usize| {
        if index + 1 < lines.len() {
            lines[index].strip_suffix('\r').unwrap_or(lines[index])
        } else {
            lines[index]
        }
    };
    let context = without_line_ending(&bookmark.context_line);
    let mut row = preferred;
    if !context.trim().is_empty() && text_line(preferred) != context {
        let start = preferred.saturating_sub(ANCHOR_SEARCH_RADIUS);
        let end = preferred
            .saturating_add(ANCHOR_SEARCH_RADIUS)
            .min(lines.len() - 1);
        let mut nearest = None;
        let mut ambiguous = false;
        for candidate in start..=end {
            if text_line(candidate) != context {
                continue;
            }
            let distance = candidate.abs_diff(preferred);
            match nearest {
                Some((_, best_distance)) if distance == best_distance => ambiguous = true,
                Some((_, best_distance)) if distance > best_distance => {}
                _ => {
                    nearest = Some((candidate, distance));
                    ambiguous = false;
                }
            }
        }
        if !ambiguous && let Some((candidate, _)) = nearest {
            row = candidate;
        }
    }
    let offset = bookmark
        .column
        .saturating_sub(1)
        .min(text_line(row).chars().count());
    let char_index = lines[..row]
        .iter()
        .map(|line| line.chars().count() + 1)
        .sum::<usize>()
        + offset;
    BookmarkLocation {
        line: row + 1,
        column: offset + 1,
        char_index,
    }
}

fn without_line_ending(line: &str) -> &str {
    line.strip_suffix('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .unwrap_or(line)
}

fn path_key(path: &Path) -> Vec<OsString> {
    normalized_components(path)
        .iter()
        .map(component_key)
        .collect()
}

fn component_key(part: &OsString) -> OsString {
    #[cfg(windows)]
    {
        OsString::from(part.to_string_lossy().to_lowercase().replace('/', r"\"))
    }
    #[cfg(not(windows))]
    {
        part.clone()
    }
}

fn normalized_components(path: &Path) -> Vec<OsString> {
    #[cfg(windows)]
    let normal_path = {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if rest
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
            {
                PathBuf::from(format!(r"\\{}", &rest[4..]))
            } else {
                PathBuf::from(rest)
            }
        } else {
            path.to_path_buf()
        }
    };
    #[cfg(not(windows))]
    let normal_path = path.to_path_buf();
    let mut components = Vec::new();
    for component in normal_path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if matches!(components.last(), Some(Component::Normal(_))) => {
                components.pop();
            }
            Component::ParentDir if matches!(components.last(), Some(Component::RootDir)) => {}
            component => components.push(component),
        }
    }
    components
        .into_iter()
        .map(|part| part.as_os_str().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmark(path: &str, line: usize, column: usize, context: &str) -> Bookmark {
        Bookmark {
            id: 42,
            path: PathBuf::from(path),
            line,
            column,
            label: String::new(),
            context_line: context.to_owned(),
        }
    }

    #[test]
    fn toggling_uses_one_bookmark_per_file_line_and_stable_order() {
        let mut store = BookmarkStore::default();
        assert_eq!(
            store.toggle(Path::new("src/ä.rs"), 3, 7, Some("🦀 title")),
            ToggleResult::Added(1)
        );
        assert_eq!(
            store.toggle(Path::new("src/b.rs"), 9, 2, None),
            ToggleResult::Added(2)
        );
        assert_eq!(
            store.toggle(Path::new("src/ä.rs"), 3, 1, None),
            ToggleResult::Removed(1)
        );
        assert_eq!(
            store.toggle(Path::new("src/ä.rs"), 4, 2, None),
            ToggleResult::Added(3)
        );
        assert_eq!(
            store.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(store.for_path(Path::new("src/ä.rs")).count(), 1);
        assert!(store.set_label(3, "Next change"));
        assert_eq!(store.get(3).unwrap().label, "Next change");
        assert!(!store.set_label(99, "missing"));
        assert!(store.remove(2));
        assert!(!store.remove(2));
        assert_eq!(store.clear(), 1);
        assert_eq!(
            store.toggle(Path::new("new.rs"), 0, 0, None),
            ToggleResult::Added(4)
        );
        assert_eq!((store.items[0].line, store.items[0].column), (1, 1));
    }

    #[test]
    fn session_restore_keeps_ids_and_repairs_legacy_duplicates() {
        let mut first = bookmark("src/a.rs", 2, 3, "anchor");
        first.id = 8;
        let mut duplicate = first.clone();
        duplicate.id = 9;
        let mut legacy = bookmark("src/b.rs", 0, 0, "");
        legacy.id = 0;
        let mut repeated_id = bookmark("src/c.rs", 1, 1, "");
        repeated_id.id = 8;
        let store = BookmarkStore::from_items(vec![first, duplicate, legacy, repeated_id]);
        assert_eq!(store.items.len(), 3);
        assert_eq!(store.items[0].id, 8);
        assert_eq!(store.items[1].id, 10);
        assert_eq!(store.items[2].id, 11);
        assert_eq!((store.items[1].line, store.items[1].column), (1, 1));
        let restored = BookmarkStore::from_items(store.items.clone());
        assert_eq!(restored.items, store.items);
    }

    #[test]
    fn bookmarks_follow_file_and_directory_renames_without_matching_siblings() {
        let mut store = BookmarkStore::default();
        store.toggle(Path::new("project/src/main.rs"), 4, 9, Some("fn main()"));
        store.toggle(Path::new("project/src/nested/ä.rs"), 8, 1, None);
        store.toggle(Path::new("project/src-extra/keep.rs"), 3, 1, None);
        assert_eq!(
            store.rename_prefix(Path::new("project/src"), Path::new("project/source")),
            2
        );
        assert_eq!(store.items[0].path, PathBuf::from("project/source/main.rs"));
        assert_eq!(
            store.items[1].path,
            PathBuf::from("project/source/nested/ä.rs")
        );
        assert_eq!(
            store.items[2].path,
            PathBuf::from("project/src-extra/keep.rs")
        );
        assert_eq!(
            store.rename_prefix(
                Path::new("project/source/main.rs"),
                Path::new("project/source/app.rs")
            ),
            1
        );
        assert_eq!(store.items[0].id, 1);
        assert_eq!(store.items[0].context_line, "fn main()");
        assert_eq!(store.items[0].column, 9);
    }

    #[test]
    fn unicode_columns_and_crlf_are_character_based_and_clamped() {
        let item = bookmark("file.rs", 2, 3, "a🦀é");
        assert_eq!(
            locate("ü\r\na🦀é\r\n", &item),
            BookmarkLocation {
                line: 2,
                column: 3,
                char_index: 5
            }
        );
        let item = bookmark("file.rs", 2, 99, "a🦀é");
        assert_eq!(
            locate("ü\r\na🦀é\r\n", &item),
            BookmarkLocation {
                line: 2,
                column: 4,
                char_index: 6
            }
        );
        assert_eq!(
            locate("", &item),
            BookmarkLocation {
                line: 1,
                column: 1,
                char_index: 0
            }
        );
    }

    #[test]
    fn a_final_lone_carriage_return_is_part_of_the_bookmarked_line() {
        let item = bookmark("file.rs", 2, 4, "");
        assert_eq!(
            locate("first\r\na🦀\r", &item),
            BookmarkLocation {
                line: 2,
                column: 4,
                char_index: 10,
            }
        );
        let mut store = BookmarkStore::default();
        store.toggle(Path::new("file.rs"), 2, 4, Some("a🦀\r"));
        assert_eq!(store.items[0].context_line, "a🦀\r");
        assert_eq!(
            locate("first\r\na🦀\ninserted\na🦀\r", &store.items[0]).line,
            4
        );
    }

    #[test]
    fn nearby_exact_context_follows_reordered_lines() {
        let item = bookmark("file.rs", 2, 4, "target 🦀");
        assert_eq!(
            locate("before\ninserted\nother\ntarget 🦀\nafter", &item),
            BookmarkLocation {
                line: 4,
                column: 4,
                char_index: 25
            }
        );
        assert_eq!(locate("target 🦀\nbefore\nafter", &item).line, 1);
        assert_eq!(locate("before\ntarget 🦀\ntarget 🦀", &item).line, 2);
    }

    #[test]
    fn missing_deleted_or_ambiguous_context_does_not_invent_a_destination() {
        let item = bookmark("file.rs", 3, 99, "deleted anchor");
        assert_eq!(
            locate("first\nlast", &item),
            BookmarkLocation {
                line: 2,
                column: 5,
                char_index: 10
            }
        );
        let item = bookmark("file.rs", 3, 2, "repeat");
        assert_eq!(
            locate("before\nrepeat\nchanged\nrepeat\nafter", &item).line,
            3
        );
        let distant = format!("{}needle", "unrelated\n".repeat(100));
        assert_eq!(
            locate(&distant, &bookmark("file.rs", 1, 2, "needle")).line,
            1
        );
        assert_eq!(
            locate("before\nchanged\n \nafter", &bookmark("file.rs", 2, 2, " ")).line,
            2
        );
    }

    #[test]
    fn serde_legacy_defaults_and_roundtrip_preserve_positions() {
        let legacy: Bookmark = serde_json::from_str(r#"{"path":"plan.md"}"#).unwrap();
        assert_eq!((legacy.id, legacy.line, legacy.column), (0, 1, 1));
        assert!(legacy.label.is_empty() && legacy.context_line.is_empty());
        let original = bookmark("Änderung 🦀.md", 7, 4, "Unicode content");
        let restored: Bookmark =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn rename_collisions_and_id_wrap_keep_unique_stable_records() {
        let mut first = bookmark("old/main.rs", 3, 2, "saved anchor");
        first.id = u64::MAX;
        first.label = "Keep original bookmark".to_owned();
        let mut second = bookmark("new/main.rs", 3, 8, "other anchor");
        second.id = 1;
        let mut store = BookmarkStore::from_items(vec![first, second]);
        assert_eq!(
            store.toggle(Path::new("third.rs"), 1, 1, None),
            ToggleResult::Added(2)
        );
        assert_eq!(store.rename_prefix(Path::new("old"), Path::new("new")), 1);
        assert_eq!(store.items.len(), 2);
        assert_eq!(store.items[0].id, u64::MAX);
        assert_eq!(store.items[0].label, "Keep original bookmark");
        assert_eq!(store.items[0].path, PathBuf::from("new/main.rs"));
        assert_eq!(store.rename_prefix(Path::new(""), Path::new("other")), 0);
    }

    #[test]
    fn lexical_dot_segments_do_not_create_duplicate_bookmarks_or_wrong_renames() {
        let mut store = BookmarkStore::default();
        store.toggle(Path::new("project/src/../main.rs"), 2, 1, None);
        assert_eq!(store.for_path(Path::new("project/./main.rs")).count(), 1);
        assert_eq!(
            store.rename_prefix(Path::new("project/src"), Path::new("project/source")),
            0
        );
        assert_eq!(
            store.toggle(Path::new("project/main.rs"), 2, 8, None),
            ToggleResult::Removed(1)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_separators_and_canonical_prefix_share_identity() {
        let mut store = BookmarkStore::default();
        store.toggle(Path::new(r"\\?\C:\Projects\Ä\MAIN.rs"), 3, 2, None);
        assert_eq!(
            store.for_path(Path::new("c:/projects/ä/main.rs")).count(),
            1
        );
        assert_eq!(
            store.toggle(Path::new(r"c:\PROJECTS\ä\main.rs"), 3, 8, None),
            ToggleResult::Removed(1)
        );
        store.toggle(Path::new(r"\\?\UNC\SERVER\Share\src\main.rs"), 4, 3, None);
        assert_eq!(
            store
                .for_path(Path::new(r"\\server\share\src\MAIN.rs"))
                .count(),
            1
        );
        assert_eq!(
            store.rename_prefix(
                Path::new(r"\\server\share\SRC"),
                Path::new(r"\\server\share\source")
            ),
            1
        );
        assert_eq!(
            store.items[0].path,
            PathBuf::from(r"\\server\share\source\main.rs")
        );
    }
}
