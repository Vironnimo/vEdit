//! Git status decorations for Explorer rows, derived once per status refresh.
use super::*;

/// One Explorer row's Git state, strongest first when folders aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ExplorerGitMark {
    Added,
    Untracked,
    Renamed,
    Modified,
    Deleted,
    Conflicted,
}

impl ExplorerGitMark {
    fn for_file(file: &git::FileStatus) -> Option<Self> {
        if file.is_conflicted() {
            return Some(Self::Conflicted);
        }
        // The working tree is what the user sees on disk; the index only
        // decides when the file is otherwise unchanged.
        let change = match file.worktree {
            git::FileChange::None => file.index,
            change => change,
        };
        Some(match change {
            git::FileChange::Untracked => Self::Untracked,
            git::FileChange::Added => Self::Added,
            git::FileChange::Deleted => Self::Deleted,
            git::FileChange::Renamed | git::FileChange::Copied => Self::Renamed,
            git::FileChange::Unmerged => Self::Conflicted,
            git::FileChange::Modified | git::FileChange::Other(_) => Self::Modified,
            git::FileChange::None => return None,
        })
    }

    pub(super) fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Untracked => "U",
            Self::Renamed => "R",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Conflicted => "!",
        }
    }

    pub(super) fn color(self) -> Color32 {
        match self {
            Self::Added | Self::Untracked => GREEN,
            Self::Renamed => BLUE,
            Self::Modified => ORANGE,
            Self::Deleted | Self::Conflicted => RED,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Untracked => "Untracked",
            Self::Renamed => "Renamed",
            Self::Modified => "Modified",
            Self::Deleted => "Deleted",
            Self::Conflicted => "Merge conflict",
        }
    }
}

/// Repository-relative paths of changed files and of every folder above them.
#[derive(Debug, Default)]
pub(super) struct ExplorerGitMarks {
    files: HashMap<PathBuf, ExplorerGitMark>,
    folders: HashMap<PathBuf, ExplorerGitMark>,
}

impl ExplorerGitMarks {
    pub(super) fn new(changes: &[git::FileStatus]) -> Self {
        let mut marks = Self::default();
        for file in changes {
            let Some(mark) = ExplorerGitMark::for_file(file) else {
                continue;
            };
            let path: PathBuf = Path::new(&file.path).components().collect();
            // Red folders mean conflicts only; a deletion inside is a change.
            let folder_mark = match mark {
                ExplorerGitMark::Deleted => ExplorerGitMark::Modified,
                mark => mark,
            };
            let mut folder = path.parent();
            while let Some(parent) = folder.filter(|parent| !parent.as_os_str().is_empty()) {
                let strongest = marks
                    .folders
                    .entry(parent.to_path_buf())
                    .or_insert(folder_mark);
                *strongest = (*strongest).max(folder_mark);
                folder = parent.parent();
            }
            marks.files.insert(path, mark);
        }
        marks
    }

    pub(super) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub(super) fn get(&self, relative: &Path, is_dir: bool) -> Option<ExplorerGitMark> {
        if is_dir {
            // Submodules and collapsed untracked directories have a status
            // entry of their own instead of individual child-file entries.
            self.folders
                .get(relative)
                .copied()
                .max(self.files.get(relative).copied())
        } else {
            self.files.get(relative).copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(path: &str, index: git::FileChange, worktree: git::FileChange) -> git::FileStatus {
        git::FileStatus {
            path: path.to_owned(),
            original_path: None,
            index,
            worktree,
        }
    }

    #[test]
    fn folders_carry_their_strongest_change_and_files_prefer_the_working_tree() {
        let marks = ExplorerGitMarks::new(&[
            status(
                "src/new.rs",
                git::FileChange::None,
                git::FileChange::Untracked,
            ),
            status(
                "src/app/ui.rs",
                git::FileChange::Added,
                git::FileChange::Modified,
            ),
            status(
                "docs/a.md",
                git::FileChange::Unmerged,
                git::FileChange::Unmerged,
            ),
            status(
                "README.md",
                git::FileChange::Modified,
                git::FileChange::None,
            ),
            status(
                "assets/old.png",
                git::FileChange::None,
                git::FileChange::Deleted,
            ),
        ]);
        assert_eq!(
            marks.get(Path::new("assets"), true),
            Some(ExplorerGitMark::Modified)
        );
        assert_eq!(
            marks.get(Path::new("src/new.rs"), false),
            Some(ExplorerGitMark::Untracked)
        );
        assert_eq!(
            marks.get(&Path::new("src").join("app").join("ui.rs"), false),
            Some(ExplorerGitMark::Modified)
        );
        assert_eq!(
            marks.get(Path::new("src"), true),
            Some(ExplorerGitMark::Modified)
        );
        assert_eq!(
            marks.get(Path::new("src/app"), true),
            Some(ExplorerGitMark::Modified)
        );
        assert_eq!(
            marks.get(Path::new("docs"), true),
            Some(ExplorerGitMark::Conflicted)
        );
        assert_eq!(
            marks.get(Path::new("README.md"), false),
            Some(ExplorerGitMark::Modified)
        );
        assert_eq!(marks.get(Path::new("src"), false), None);
        assert_eq!(marks.get(Path::new("unchanged"), true), None);
    }

    #[test]
    fn directory_statuses_mark_collapsed_untracked_folders_and_submodules() {
        let marks = ExplorerGitMarks::new(&[
            status(
                "generated output/",
                git::FileChange::None,
                git::FileChange::Untracked,
            ),
            status(
                "vendor/module",
                git::FileChange::None,
                git::FileChange::Modified,
            ),
        ]);
        assert_eq!(
            marks.get(Path::new("generated output"), true),
            Some(ExplorerGitMark::Untracked)
        );
        assert_eq!(
            marks.get(&Path::new("vendor").join("module"), true),
            Some(ExplorerGitMark::Modified)
        );
        assert_eq!(
            marks.get(Path::new("vendor"), true),
            Some(ExplorerGitMark::Modified)
        );
    }
}
