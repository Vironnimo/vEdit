//! Short labels that distinguish files or projects without repeating full paths.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Preserve existing titles unless distinct paths share a title. In that case,
/// append the shortest parent suffix that distinguishes each path. Pathless
/// drafts keep their titles; repeated views alone do not trigger longer labels.
///
/// Supply each path's file or project basename as the title. Identity is lexical:
/// this does not access the filesystem or resolve links.
pub(crate) fn disambiguate_labels(items: &[(&str, Option<&Path>)]) -> Vec<String> {
    let identities: Vec<_> = items
        .iter()
        .map(|(_, path)| path.map(Identity::new))
        .collect();
    let mut groups: HashMap<&str, Vec<&Identity>> = HashMap::new();
    for ((title, _), identity) in items.iter().zip(&identities) {
        if let Some(identity) = identity {
            groups.entry(*title).or_default().push(identity);
        }
    }
    let original_titles: HashSet<_> = items.iter().map(|(title, _)| *title).collect();

    items
        .iter()
        .enumerate()
        .map(|(index, (title, _))| {
            let Some(identity) = &identities[index] else {
                return (*title).to_owned();
            };
            let peers = &groups[*title];

            if peers.iter().all(|peer| peer.key == identity.key) {
                return (*title).to_owned();
            }

            for depth in 1..=identity.parent_len().max(1) {
                let suffix = identity.parent_suffix(depth);
                let suffix_key = identity.normalized(&suffix);
                if peers.iter().all(|peer| {
                    peer.key == identity.key
                        || peer.normalized(&peer.parent_suffix(depth)) != suffix_key
                }) {
                    let label = format!("{title} — {suffix}");
                    if !original_titles.contains(label.as_str()) {
                        return label;
                    }
                }
            }

            // Roots or unusual path forms may not have a distinguishing parent.
            format!("{title} — {}", identity.readable)
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Root {
    Relative,
    Drive,
    Network,
    Absolute,
}

struct Identity {
    readable: String,
    parts: Vec<String>,
    root: Root,
    windows: bool,
    key: String,
}

impl Identity {
    fn new(path: &Path) -> Self {
        let readable = crate::paths::display(path);
        let bytes = readable.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        let network = readable.starts_with(r"\\") || readable.starts_with("//");
        let windows = cfg!(windows) || drive || network || readable.contains('\\');
        let root = if drive {
            Root::Drive
        } else if network {
            Root::Network
        } else if readable.starts_with(['/', '\\']) {
            Root::Absolute
        } else {
            Root::Relative
        };
        let parts = readable
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        let mut identity = Self {
            readable,
            parts,
            root,
            windows,
            key: String::new(),
        };
        identity.key = identity.normalized(&identity.readable);
        identity
    }

    fn parent_len(&self) -> usize {
        self.parts.len().saturating_sub(1)
    }

    fn parent_suffix(&self, depth: usize) -> String {
        let separator = if self.windows { "\\" } else { "/" };
        let parent_len = self.parent_len();
        if parent_len == 0 {
            return match self.root {
                Root::Absolute => separator.to_owned(),
                _ => ".".to_owned(),
            };
        }
        let start = parent_len.saturating_sub(depth);
        let suffix = self.parts[start..parent_len].join(separator);
        if start != 0 {
            return suffix;
        }
        match self.root {
            Root::Network => format!("{separator}{separator}{suffix}"),
            Root::Absolute => format!("{separator}{suffix}"),
            Root::Drive if parent_len == 1 => format!("{suffix}{separator}"),
            _ => suffix,
        }
    }

    fn normalized(&self, text: &str) -> String {
        let text = text.replace('\\', "/");
        let bytes = text.as_bytes();
        let drive_root = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2..].iter().all(|byte| *byte == b'/');
        let prefix = if text.starts_with("//") {
            "//"
        } else if text.starts_with('/') {
            "/"
        } else {
            ""
        };
        let parts: Vec<_> = text.split('/').filter(|part| !part.is_empty()).collect();
        let mut text = format!("{prefix}{}", parts.join("/"));
        if drive_root {
            text.push('/');
        }
        if self.windows {
            text.to_lowercase()
        } else {
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_noncolliding_titles_stay_short() {
        assert_eq!(
            disambiguate_labels(&[
                ("main.rs", Some(Path::new(r"C:\work\demo\src\main.rs"))),
                ("README.md", Some(Path::new(r"C:\work\demo\README.md"))),
            ]),
            ["main.rs", "README.md"]
        );
    }

    #[test]
    fn files_in_one_project_use_only_the_distinguishing_folders() {
        assert_eq!(
            disambiguate_labels(&[
                ("lib.rs", Some(Path::new(r"C:\work\demo\src\lib.rs"))),
                ("lib.rs", Some(Path::new(r"C:\work\demo\tests\lib.rs"))),
            ]),
            [r"lib.rs — src", r"lib.rs — tests"]
        );
    }

    #[test]
    fn matching_parent_names_include_the_distinguishing_ancestor() {
        assert_eq!(
            disambiguate_labels(&[
                ("lib.rs", Some(Path::new(r"C:\work\client\src\lib.rs")),),
                ("lib.rs", Some(Path::new(r"C:\work\server\src\lib.rs")),),
                ("lib.rs", Some(Path::new(r"C:\work\demo\tests\lib.rs"))),
            ]),
            [
                r"lib.rs — client\src",
                r"lib.rs — server\src",
                r"lib.rs — tests"
            ]
        );
    }

    #[test]
    fn extended_drive_prefixes_are_clean_when_drives_distinguish_files() {
        assert_eq!(
            disambiguate_labels(&[
                ("lib.rs", Some(Path::new(r"\\?\C:\work\demo\src\lib.rs")),),
                ("lib.rs", Some(Path::new(r"\\?\D:\work\demo\src\lib.rs")),),
            ]),
            [r"lib.rs — C:\work\demo\src", r"lib.rs — D:\work\demo\src"]
        );
    }

    #[test]
    fn extended_network_prefixes_retain_readable_server_names() {
        assert_eq!(
            disambiguate_labels(&[
                ("lib.rs", Some(Path::new(r"\\?\UNC\alpha\share\src\lib.rs")),),
                ("lib.rs", Some(Path::new(r"\\?\UNC\beta\share\src\lib.rs")),),
            ]),
            [r"lib.rs — \\alpha\share\src", r"lib.rs — \\beta\share\src"]
        );
    }

    #[test]
    fn pathless_untitled_titles_are_preserved() {
        assert_eq!(
            disambiguate_labels(&[
                ("Untitled 1", None),
                ("Untitled 1", None),
                ("Untitled 1", Some(Path::new(r"C:\work\Untitled 1"))),
            ]),
            ["Untitled 1", "Untitled 1", "Untitled 1"]
        );
    }

    #[test]
    fn repeated_windows_path_views_do_not_gain_parent_suffixes() {
        assert_eq!(
            disambiguate_labels(&[
                ("lib.rs", Some(Path::new(r"\\?\C:\Work\demo\src\lib.rs")),),
                ("lib.rs", Some(Path::new("c:/work/demo/src/lib.rs")),),
                ("lib.rs", Some(Path::new(r"C:\Work\demo\\src\lib.rs")),),
            ]),
            ["lib.rs", "lib.rs", "lib.rs"]
        );
    }

    #[test]
    fn repeated_views_share_a_suffix_when_another_file_collides() {
        let path = Path::new(r"C:\work\demo\src\lib.rs");
        assert_eq!(
            disambiguate_labels(&[
                ("lib.rs", Some(path)),
                ("lib.rs", Some(path)),
                ("lib.rs", Some(Path::new(r"C:\work\demo\tests\lib.rs"))),
            ]),
            ["lib.rs — src", "lib.rs — src", "lib.rs — tests"]
        );
    }

    #[test]
    fn equally_named_projects_need_only_their_parent_folder() {
        assert_eq!(
            disambiguate_labels(&[
                ("demo", Some(Path::new(r"C:\work\client\demo"))),
                ("demo", Some(Path::new(r"C:\work\server\demo"))),
            ]),
            ["demo — client", "demo — server"]
        );
    }

    #[test]
    fn drive_root_and_drive_relative_path_remain_distinct() {
        assert_eq!(
            disambiguate_labels(&[
                ("root", Some(Path::new(r"C:\"))),
                ("root", Some(Path::new("C:"))),
            ]),
            [r"root — C:\", "root — C:"]
        );
    }

    #[test]
    fn generated_labels_do_not_replace_a_distinct_natural_title() {
        assert_eq!(
            disambiguate_labels(&[
                ("notes.md", Some(Path::new(r"C:\work\src\notes.md"))),
                ("notes.md", Some(Path::new(r"C:\work\tests\notes.md"))),
                ("notes.md — src", Some(Path::new(r"C:\work\notes.md — src")),),
            ]),
            [r"notes.md — work\src", "notes.md — tests", "notes.md — src"]
        );
    }
}
