use super::*;

#[test]
fn diff_preview_never_runs_external_diff_or_textconv_helpers() {
    let repository = TestRepository::new();
    repository.write(".gitattributes", "code.txt diff=custom\n");
    repository.write("code.txt", "original\n");
    repository.commit_all("Initial");
    repository.git(&["config", "diff.external", "vedit-nonexistent-diff-program"]);
    repository.git(&[
        "config",
        "diff.custom.textconv",
        "vedit-nonexistent-textconv-program",
    ]);
    repository.write("code.txt", "changed\n");
    let patch = diff(&repository.0, Path::new("code.txt"), false).unwrap();
    assert!(patch.contains("+changed\n"));
    let files = status(&repository.0).unwrap().files;
    assert_eq!(
        working_tree_stats(&repository.0, &files).unwrap().unstaged["code.txt"].added,
        1
    );
    repository.commit_all("Changed without conversion helpers");
    let hash = repository.head_hash();
    // A working-tree path with the same spelling must not make Git treat a
    // full immutable revision as an ambiguous path argument.
    repository.write(&hash, "untracked path named after the commit\n");
    let details = commit_details(&repository.0, &hash).unwrap();
    let file = details
        .files
        .iter()
        .find(|file| file.path == "code.txt")
        .unwrap();
    assert_eq!(file.stats.added, 1);
    assert!(
        commit_file_diff(&repository.0, &details, file)
            .unwrap()
            .contains("+changed\n")
    );
}

fn review_paths(set: &ChangeSet) -> Vec<(&str, FileChange)> {
    set.files
        .iter()
        .map(|file| (file.path.as_str(), file.status))
        .collect()
}

fn loaded(file: &ReviewFile) -> &str {
    match &file.patch {
        ReviewPatch::Loaded(text) => text,
        other => panic!("{} has no loaded patch: {other:?}", file.path),
    }
}

#[test]
fn working_tree_review_combines_staged_unstaged_and_untracked_changes() {
    let repository = TestRepository::new();
    repository.write("kept.txt", "one\ntwo\n");
    repository.write("removed.txt", "gone\n");
    fs::create_dir(repository.0.join("src")).unwrap();
    repository.write("src/lib.rs", "fn a() {}\n");
    repository.commit_all("Initial");
    repository.write("kept.txt", "one\nchanged\n");
    repository.write("src/lib.rs", "fn b() {}\n");
    repository.git(&["add", "src/lib.rs"]);
    repository.write("src/lib.rs", "fn c() {}\n");
    fs::remove_file(repository.0.join("removed.txt")).unwrap();
    repository.write("src/new file.rs", "new\n");
    repository.write("staged.txt", "staged\n");
    repository.git(&["add", "staged.txt"]);

    let set = change_set(&repository.0, &ReviewSource::WorkingTree).unwrap();
    assert_eq!(set.comparison.title, "Working tree");
    assert_eq!(set.comparison.new, None);
    assert_eq!(set.comparison.old, repository.head_hash());
    assert_eq!(
        review_paths(&set),
        [
            ("kept.txt", FileChange::Modified),
            ("removed.txt", FileChange::Deleted),
            ("staged.txt", FileChange::Added),
            ("src/lib.rs", FileChange::Modified),
            ("src/new file.rs", FileChange::Untracked),
        ]
    );
    // Staged and unstaged edits are one comparison with HEAD.
    let library = loaded(&set.files[3]);
    assert!(library.contains("-fn a() {}\n+fn c() {}\n"), "{library}");
    assert!(!library.contains("fn b()"));
    assert!(loaded(&set.files[1]).contains("-gone\n"));
    assert!(loaded(&set.files[4]).contains("+new\n"));
    assert_eq!(
        set.files[4].stats,
        Some(LineStats {
            added: 1,
            removed: 0,
            binary: false
        })
    );
    assert_eq!(
        set.files[0].stats.map(|stats| (stats.added, stats.removed)),
        Some((1, 1))
    );

    let keys =
        |set: &ChangeSet| -> Vec<u64> { set.files.iter().map(|file| file.content_key).collect() };
    let again = change_set(&repository.0, &ReviewSource::WorkingTree).unwrap();
    assert_eq!(keys(&set), keys(&again));
    repository.write("kept.txt", "one\nchanged again\n");
    let changed = change_set(&repository.0, &ReviewSource::WorkingTree).unwrap();
    let (before, after) = (keys(&set), keys(&changed));
    assert_ne!(before[0], after[0]);
    assert_eq!(before[1..], after[1..]);
}

#[test]
fn working_tree_review_before_the_first_commit_uses_the_empty_tree() {
    let repository = TestRepository::new();
    repository.write("first.txt", "hello\n");
    repository.git(&["add", "first.txt"]);
    let set = change_set(&repository.0, &ReviewSource::WorkingTree).unwrap();
    assert_eq!(
        review_paths(&set),
        [
            (".gitignore", FileChange::Untracked),
            ("first.txt", FileChange::Added)
        ]
    );
    assert!(loaded(&set.files[1]).contains("+hello\n"));
    assert!(resolve_review(&repository.0, &ReviewSource::Branch { base: None }).is_err());
}

#[test]
fn commit_review_resolves_revisions_and_compares_with_the_first_parent() {
    let repository = TestRepository::new();
    repository.write("a.txt", "a\n");
    repository.commit_all("Root");
    let root = repository.head_hash();
    repository.write("a.txt", "a\nb\n");
    repository.commit_all("Second");

    let first = change_set(&repository.0, &ReviewSource::Commit("HEAD~1".into())).unwrap();
    assert_eq!(first.comparison.new.as_deref(), Some(root.as_str()));
    assert_eq!(first.comparison.title, format!("{} Root", &root[..7]));
    assert_eq!(first.comparison.key, format!("commit:{root}"));
    assert!(first.comparison.detail.starts_with("Root commit"));
    assert_eq!(
        review_paths(&first),
        [
            (".gitignore", FileChange::Added),
            ("a.txt", FileChange::Added)
        ]
    );
    let second = change_set(&repository.0, &ReviewSource::Commit("HEAD".into())).unwrap();
    assert_eq!(review_paths(&second), [("a.txt", FileChange::Modified)]);
    assert!(loaded(&second.files[0]).contains(" a\n+b\n"));
    for revision in ["--output=x", "-", "", "HEAD\n", "missing-branch"] {
        assert!(
            resolve_review(&repository.0, &ReviewSource::Commit(revision.into())).is_err(),
            "{revision:?}"
        );
    }

    repository.git(&["switch", "-c", "side"]);
    repository.write("side.txt", "side\n");
    repository.commit_all("Side");
    repository.git(&["switch", "main"]);
    repository.write("main.txt", "main\n");
    repository.commit_all("Main");
    repository.git(&["merge", "--no-ff", "--no-edit", "side"]);
    let merge = change_set(&repository.0, &ReviewSource::Commit("HEAD".into())).unwrap();
    assert_eq!(review_paths(&merge), [("side.txt", FileChange::Added)]);
    assert!(merge.comparison.detail.starts_with("Merge commit"));
}

#[test]
fn branch_review_uses_the_merge_base_with_main_or_master() {
    let repository = TestRepository::new();
    repository.write("base.txt", "base\n");
    repository.commit_all("Base");
    repository.git(&["switch", "-c", "feature/x"]);
    repository.write("feature.txt", "one\n");
    repository.commit_all("Feature one");
    repository.write("feature.txt", "one\ntwo\n");
    repository.commit_all("Feature two");
    repository.git(&["switch", "main"]);
    repository.write("base.txt", "main moved\n");
    repository.commit_all("Main moved");
    repository.git(&["switch", "feature/x"]);

    let source = ReviewSource::Branch { base: None };
    let set = change_set(&repository.0, &source).unwrap();
    assert_eq!(set.comparison.title, "feature/x vs main");
    // Commits that only main gained after the branch left it are excluded.
    assert_eq!(review_paths(&set), [("feature.txt", FileChange::Added)]);
    assert_eq!(set.files[0].stats.map(|stats| stats.added), Some(2));
    repository.write("feature.txt", "one\ntwo\nthree\n");
    repository.commit_all("Feature three");
    let next = change_set(&repository.0, &source).unwrap();
    assert_eq!(next.comparison.key, set.comparison.key);
    assert_ne!(next.files[0].content_key, set.files[0].content_key);

    let explicit = resolve_review(
        &repository.0,
        &ReviewSource::Branch {
            base: Some("HEAD~1".into()),
        },
    )
    .unwrap();
    assert_eq!(explicit.title, "feature/x vs HEAD~1");
    repository.git(&["branch", "-m", "main", "master"]);
    assert_eq!(
        resolve_review(&repository.0, &source).unwrap().title,
        "feature/x vs master"
    );
    repository.git(&["branch", "-m", "master", "trunk"]);
    assert!(resolve_review(&repository.0, &source).is_err());
}

#[test]
fn review_patches_classify_large_and_binary_files_and_match_chunks() {
    let repository = TestRepository::new();
    fs::create_dir(repository.0.join("many")).unwrap();
    for index in 0..60 {
        repository.write(&format!("many/file {index:02}.txt"), "before\n");
    }
    let similar: String = (0..12)
        .map(|line| format!("shared line {line}\n"))
        .collect();
    repository.write("old name.txt", &similar);
    repository.write("große Datei.txt", "x\n");
    repository.write("binary.bin", "\0\u{1}binary");
    repository.commit_all("Initial");
    for index in 0..60 {
        repository.write(
            &format!("many/file {index:02}.txt"),
            &format!("after {index}\n"),
        );
    }
    repository.write("old name.txt", &format!("{similar}one more\n"));
    repository.git(&["mv", "old name.txt", "moved [x].txt"]);
    let large: String = (0..=REVIEW_LARGE_FILE_LINES)
        .map(|line| format!("line {line}\n"))
        .collect();
    repository.write("große Datei.txt", &large);
    repository.write("new large.txt", &large);
    repository.write("binary.bin", "\0\u{2}changed");

    let set = change_set(&repository.0, &ReviewSource::WorkingTree).unwrap();
    let find = |path: &str| set.files.iter().find(|file| file.path == path).unwrap();
    assert_eq!(find("binary.bin").patch, ReviewPatch::Binary);
    assert_eq!(find("große Datei.txt").patch, ReviewPatch::Large);
    assert_eq!(find("new large.txt").patch, ReviewPatch::Large);
    let moved = find("moved [x].txt");
    assert_eq!(moved.status, FileChange::Renamed);
    assert_eq!(moved.original_path.as_deref(), Some("old name.txt"));
    assert!(loaded(moved).contains("rename from old name.txt"));
    assert!(loaded(moved).contains("+one more\n"));
    for index in 0..60 {
        let file = find(&format!("many/file {index:02}.txt"));
        assert!(
            loaded(file).contains(&format!("-before\n+after {index}\n")),
            "{}",
            file.path
        );
    }
    let patches = review_patches(
        &repository.0,
        &set.comparison,
        &[find("große Datei.txt").clone()],
    );
    let ReviewPatch::Loaded(text) = &patches[0] else {
        panic!("{patches:?}");
    };
    assert!(text.starts_with("diff --git a/große Datei.txt b/große Datei.txt\n"));
    assert!(text.contains(&format!("+line {REVIEW_LARGE_FILE_LINES}\n")));

    // A deferred patch must not silently replace the listed file version.
    let replace_same_size = |path: &str| {
        let path_on_disk = repository.0.join(path);
        let modified = fs::metadata(&path_on_disk).unwrap().modified().unwrap();
        repository.write(path, &large.replacen("line 0\n", "LINE 0\n", 1));
        fs::OpenOptions::new()
            .write(true)
            .open(path_on_disk)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(modified + std::time::Duration::from_secs(2)),
            )
            .unwrap();
    };
    replace_same_size("große Datei.txt");
    let patches = review_patches(
        &repository.0,
        &set.comparison,
        &[find("große Datei.txt").clone()],
    );
    assert!(matches!(patches[0], ReviewPatch::Unavailable(_)));
    replace_same_size("new large.txt");
    let patches = review_patches(
        &repository.0,
        &set.comparison,
        &[find("new large.txt").clone()],
    );
    assert!(matches!(patches[0], ReviewPatch::Unavailable(_)));
}

#[test]
fn review_patch_headers_follow_git_quoting() {
    assert_eq!(
        quoted_patch_path("a/", "plain ü [x].txt"),
        "a/plain ü [x].txt"
    );
    assert_eq!(
        quoted_patch_path("b/", "tab\there\"q\\"),
        "\"b/tab\\there\\\"q\\\\\""
    );
    assert_eq!(quoted_patch_path("a/", "bell\u{7}"), "\"a/bell\\a\"");
    assert_eq!(quoted_patch_path("a/", "escape\u{1b}"), "\"a/escape\\033\"");
    let blocks = split_patch_blocks("diff --git a/x b/x\n+diff --git inside\ndiff --git a/y b/y\n");
    assert_eq!(
        blocks,
        [
            "diff --git a/x b/x\n+diff --git inside\n",
            "diff --git a/y b/y\n"
        ]
    );
}

#[test]
fn base_versions_read_committed_staged_untracked_and_unsupported_files() {
    let repository = TestRepository::new();
    fs::create_dir(repository.0.join("src")).unwrap();
    repository.write("src/lib.rs", "\u{feff}one\r\ntwo\r\n");
    repository.write("binary.dat", "a\0b");
    repository.write("old name.txt", "kept\n");
    repository.commit_all("Initial");
    let head = repository.head_hash();
    let read = |path: &str| base_version(&repository.0, Some(&head), path).unwrap();
    // Raw blob bytes: line endings stay as committed, only the BOM goes.
    assert_eq!(
        read("src/lib.rs"),
        BaseVersion::Text("one\r\ntwo\r\n".into())
    );
    assert_eq!(read("binary.dat"), BaseVersion::Binary);
    assert_eq!(read("src"), BaseVersion::Unsupported);
    repository.write("new.txt", "staged\n");
    repository.git(&["add", "new.txt"]);
    repository.write("untracked.txt", "loose\n");
    assert_eq!(read("new.txt"), BaseVersion::Added);
    assert_eq!(read("untracked.txt"), BaseVersion::Untracked);
    assert_eq!(read("missing.txt"), BaseVersion::Untracked);
    // A staged rename reads its committed content under the old path.
    repository.git(&["mv", "old name.txt", "new name.txt"]);
    assert_eq!(read("old name.txt"), BaseVersion::Text("kept\n".into()));
    assert_eq!(
        base_version(&repository.0, None, "new.txt").unwrap(),
        BaseVersion::Added
    );
    assert!(base_version(&repository.0, Some(&head), "../outside.txt").is_err());
    repository.write("large.txt", &"x".repeat(BASE_VERSION_LIMIT as usize + 1));
    repository.write(
        "pointer.bin",
        "version https://git-lfs.github.com/spec/v1\noid sha256:0\n",
    );
    repository.commit_all("Large");
    let head = repository.head_hash();
    let read = |path: &str| base_version(&repository.0, Some(&head), path).unwrap();
    assert_eq!(read("large.txt"), BaseVersion::TooLarge);
    assert_eq!(read("pointer.bin"), BaseVersion::Unsupported);
}

#[test]
fn status_fingerprint_follows_head_status_and_further_edits_of_changed_files() {
    let repository = TestRepository::new();
    repository.write("a.txt", "one\n");
    repository.commit_all("Initial");
    let (_, clean) = working_tree_fingerprint(&repository.0).unwrap();
    assert_eq!(working_tree_fingerprint(&repository.0).unwrap().1, clean);
    repository.write("a.txt", "two\n");
    let (_, modified) = working_tree_fingerprint(&repository.0).unwrap();
    assert_ne!(modified, clean);
    // The status still reads " M"; the size tells this edit apart.
    repository.write("a.txt", "three\n");
    let (_, edited) = working_tree_fingerprint(&repository.0).unwrap();
    assert_ne!(edited, modified);
    repository.commit_all("Second");
    let (head, committed) = working_tree_fingerprint(&repository.0).unwrap();
    assert_ne!(committed, clean, "HEAD is part of the key");
    assert_eq!(head, Some(StatusHead::Commit(repository.head_hash())));
}
