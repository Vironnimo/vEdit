use super::*;

#[test]
fn clean_worktree_statistics_do_not_launch_git() {
    let repository = TestRepository::new();
    repository.commit_all("Initial");
    let files = status(&repository.0).unwrap().files;
    assert!(files.is_empty());
    GIT_COMMAND_COUNT.with(|count| count.set(0));
    assert_eq!(
        working_tree_stats(&repository.0, &files).unwrap(),
        WorkingTreeStats::default()
    );
    assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 0);
}

#[test]
fn worktree_statistics_only_run_comparisons_with_changed_files() {
    let repository = TestRepository::new();
    repository.write("file.txt", "initial\n");
    repository.commit_all("Initial");
    repository.write("untracked.txt", "first\nsecond\n");
    let files = status(&repository.0).unwrap().files;
    GIT_COMMAND_COUNT.with(|count| count.set(0));
    let statistics = working_tree_stats(&repository.0, &files).unwrap();
    assert_eq!(statistics.unstaged["untracked.txt"].added, 2);
    assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 0);

    repository.write("file.txt", "modified\n");
    let files = status(&repository.0).unwrap().files;
    GIT_COMMAND_COUNT.with(|count| count.set(0));
    let statistics = working_tree_stats(&repository.0, &files).unwrap();
    assert_eq!(statistics.unstaged["file.txt"].added, 1);
    assert!(statistics.staged.is_empty());
    assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 1);

    stage_all(&repository.0).unwrap();
    let files = status(&repository.0).unwrap().files;
    GIT_COMMAND_COUNT.with(|count| count.set(0));
    let statistics = working_tree_stats(&repository.0, &files).unwrap();
    assert_eq!(statistics.staged["file.txt"].added, 1);
    assert_eq!(statistics.staged["untracked.txt"].added, 2);
    assert!(statistics.unstaged.is_empty());
    assert_eq!(GIT_COMMAND_COUNT.with(|count| count.get()), 1);
}

#[test]
fn staged_and_worktree_diffs_show_the_correct_versions() {
    let repository = TestRepository::new();
    repository.write("code.rs", "original\n");
    repository.commit_all("First commit");
    repository.write("code.rs", "staged version\n");
    stage_paths(&repository.0, &[PathBuf::from("code.rs")]).unwrap();
    repository.write("code.rs", "working version\n");
    let entry = status(&repository.0)
        .unwrap()
        .files
        .into_iter()
        .find(|entry| entry.path == "code.rs")
        .unwrap();
    assert!(entry.is_staged() && entry.is_unstaged());
    let staged = diff(&repository.0, Path::new("code.rs"), true).unwrap();
    assert!(staged.contains("-original\n") && staged.contains("+staged version\n"));
    let worktree = diff(&repository.0, Path::new("code.rs"), false).unwrap();
    assert!(worktree.contains("-staged version\n") && worktree.contains("+working version\n"));
    unstage_all(&repository.0).unwrap();
    assert_eq!(
        fs::read_to_string(repository.0.join("code.rs")).unwrap(),
        "working version\n"
    );
}

#[test]
fn line_statistics_separate_index_worktree_binary_and_untracked_content() {
    let repository = TestRepository::new();
    repository.write("code ü.rs", "one\ntwo\nthree\n");
    fs::write(repository.0.join("asset.bin"), b"original\0asset").unwrap();
    repository.commit_all("Initial");
    repository.write("code ü.rs", "one\nchanged\nthree\n");
    stage_paths(&repository.0, &[PathBuf::from("code ü.rs")]).unwrap();
    repository.write("code ü.rs", "one\nchanged\nthree\nfour\n");
    fs::write(repository.0.join("asset.bin"), b"modified\0asset").unwrap();
    repository.write("untracked ü.txt", "first\nsecond");
    repository.write("empty.txt", "");
    fs::write(repository.0.join("new.bin"), b"new\0asset").unwrap();
    fs::write(
        repository.0.join("large.txt"),
        vec![b'x'; UNTRACKED_FILE_LIMIT + 1],
    )
    .unwrap();
    let index_before = fs::read(repository.0.join(".git/index")).unwrap();
    let files = status(&repository.0).unwrap().files;
    let stats = working_tree_stats(&repository.0, &files).unwrap();
    assert_eq!(
        stats.staged["code ü.rs"],
        LineStats {
            added: 1,
            removed: 1,
            binary: false
        }
    );
    assert_eq!(
        stats.unstaged["code ü.rs"],
        LineStats {
            added: 1,
            removed: 0,
            binary: false
        }
    );
    assert!(stats.unstaged["asset.bin"].binary);
    assert!(stats.unstaged["new.bin"].binary);
    assert_eq!(stats.unstaged["untracked ü.txt"].added, 2);
    assert_eq!(stats.unstaged["empty.txt"], LineStats::default());
    assert!(!stats.unstaged.contains_key("large.txt"));
    assert_eq!(
        fs::read(repository.0.join(".git/index")).unwrap(),
        index_before
    );
    let patch = untracked_diff(&repository.0, Path::new("untracked ü.txt")).unwrap();
    assert!(patch.contains("@@ -0,0 +1,2 @@\n+first\n+second\n\\ No newline at end of file\n"));
    assert!(
        untracked_diff(&repository.0, Path::new("new.bin"))
            .unwrap()
            .contains("Binary files")
    );
    assert!(untracked_diff(&repository.0, Path::new("large.txt")).is_err());
    assert!(untracked_diff(&repository.0, Path::new("../outside.txt")).is_err());
}

#[test]
fn root_commit_details_include_full_metadata_message_files_and_line_counts() {
    let repository = TestRepository::new();
    repository.git(&["config", "user.name", "vEdit Göß Test"]);
    repository.write("root ü.txt", "one\ntwo\n");
    fs::write(repository.0.join("asset.bin"), b"binary\0content").unwrap();
    repository.commit_all(
        "Root subject\n\nA complete multiline body.\nSecond paragraph with ü and tabs\ttoo.",
    );
    let hash = repository.head_hash();
    let details = commit_details(&repository.0, &hash).unwrap();
    assert_eq!(
        commit_stats(&repository.0, &hash).unwrap(),
        CommitStats {
            // The fixture's two-line .gitignore is part of the root commit.
            files_changed: 3,
            lines: LineStats {
                added: 4,
                removed: 0,
                binary: true,
            },
        }
    );
    assert_eq!(details.hash, hash);
    assert!(details.parents.is_empty());
    assert_eq!(details.author, "vEdit Göß Test");
    assert_eq!(details.author_email, "vedit-tests@example.invalid");
    assert!(details.date.contains('T') && details.date.len() >= 20);
    assert!(
        details
            .message
            .starts_with("Root subject\n\nA complete multiline body.\nSecond paragraph")
    );
    assert!(details.message.contains("ü and tabs\ttoo."));
    let root_file = details
        .files
        .iter()
        .find(|file| file.path == "root ü.txt")
        .unwrap();
    assert_eq!(root_file.status, FileChange::Added);
    assert_eq!(
        root_file.stats,
        LineStats {
            added: 2,
            removed: 0,
            binary: false
        }
    );
    assert!(
        details
            .files
            .iter()
            .find(|file| file.path == "asset.bin")
            .unwrap()
            .stats
            .binary
    );
    let patch = commit_diff(&repository.0, &hash, Path::new("root ü.txt")).unwrap();
    assert!(patch.contains("+one\n+two\n") && patch.contains("/dev/null"));
    assert!(commit_diff(&repository.0, &hash, Path::new("not-in-commit.txt")).is_err());
    for invalid in [
        "HEAD",
        "--help",
        "a12345",
        "../../HEAD",
        "fffffffffffffffffffffffffffffffffffffffz",
    ] {
        assert!(commit_details(&repository.0, invalid).is_err());
        assert!(commit_stats(&repository.0, invalid).is_err());
        assert!(commit_diff(&repository.0, invalid, Path::new("root ü.txt")).is_err());
    }
}

#[test]
fn commit_statistics_count_text_binary_deletions_and_empty_commits() {
    let repository = TestRepository::new();
    repository.write("modified ü.txt", "one\ntwo\nthree\n");
    repository.write("deleted.txt", "old\ncontents\n");
    fs::write(repository.0.join("asset.bin"), b"original\0asset").unwrap();
    repository.commit_all("Initial");
    repository.write("modified ü.txt", "one\nchanged\nthree\n");
    fs::remove_file(repository.0.join("deleted.txt")).unwrap();
    repository.write("added.txt", "new\ncontents without a trailing newline");
    fs::write(repository.0.join("asset.bin"), b"modified\0asset").unwrap();
    repository.commit_all("Text additions, removals, and a binary change");
    let hash = repository.head_hash();
    // Working-tree edits must not alter immutable commit hover totals.
    repository.write("modified ü.txt", "uncommitted\n");
    assert_eq!(
        commit_stats(&repository.0, &hash).unwrap(),
        CommitStats {
            files_changed: 4,
            lines: LineStats {
                added: 3,
                removed: 3,
                binary: true,
            },
        }
    );
    let tree = String::from_utf8(repository.git(&["rev-parse", "HEAD^{tree}"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    assert!(commit_stats(&repository.0, &tree).is_err());
    repository.git(&["commit", "--allow-empty", "-m", "Empty commit"]);
    assert_eq!(
        commit_stats(&repository.0, &repository.head_hash()).unwrap(),
        CommitStats::default()
    );
}

#[test]
fn commit_rename_statistics_and_patch_preserve_both_paths() {
    let repository = TestRepository::new();
    // Status classification and counted comparisons must agree even when
    // the user's default diff settings disable rename detection.
    repository.git(&["config", "diff.renames", "false"]);
    let initial = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
    repository.write("old ü name.txt", initial);
    repository.commit_all("Initial");
    let parent = repository.head_hash();
    fs::rename(
        repository.0.join("old ü name.txt"),
        repository.0.join("new ü name.txt"),
    )
    .unwrap();
    repository.write("new ü name.txt", &format!("{initial}eleven\n"));
    stage_all(&repository.0).unwrap();
    let stats = working_tree_stats(&repository.0, &status(&repository.0).unwrap().files).unwrap();
    assert_eq!(stats.staged["new ü name.txt"].added, 1);
    assert_eq!(stats.staged["new ü name.txt"].removed, 0);
    let entry = status(&repository.0)
        .unwrap()
        .files
        .into_iter()
        .find(|file| file.path == "new ü name.txt")
        .unwrap();
    let staged_patch = status_diff(&repository.0, &entry, true).unwrap();
    assert!(staged_patch.contains("rename from ") && staged_patch.contains("rename to "));
    assert!(staged_patch.contains("+eleven\n"));
    commit(&repository.0, "Rename with one added line").unwrap();
    let hash = repository.head_hash();
    let details = commit_details(&repository.0, &hash).unwrap();
    assert_eq!(details.parents, vec![parent]);
    assert_eq!(
        commit_stats(&repository.0, &hash).unwrap(),
        CommitStats {
            files_changed: 1,
            lines: LineStats {
                added: 1,
                removed: 0,
                binary: false,
            },
        }
    );
    assert_eq!(details.files.len(), 1);
    let file = &details.files[0];
    assert_eq!(file.path, "new ü name.txt");
    assert_eq!(file.original_path.as_deref(), Some("old ü name.txt"));
    assert_eq!(file.status, FileChange::Renamed);
    assert_eq!(file.stats.added, 1);
    let patch = commit_diff(&repository.0, &hash, Path::new("new ü name.txt")).unwrap();
    assert!(patch.contains("rename from ") && patch.contains("rename to "));
    assert!(patch.contains("+eleven\n"));
    assert_eq!(
        commit_file_diff(&repository.0, &details, file).unwrap(),
        patch
    );
}

#[test]
fn copied_file_previews_do_not_include_changes_to_the_source() {
    let repository = TestRepository::new();
    repository.write("source.txt", "original contents\n");
    repository.commit_all("Initial");
    repository.write("source.txt", "changed source only\n");
    repository.write("copy.txt", "original contents\n");
    stage_all(&repository.0).unwrap();
    // Copy detection is configurable in Git. Preserve the status returned
    // by such a configuration without expanding the selected path to the
    // independently modified source file.
    let copied = FileStatus {
        path: "copy.txt".into(),
        original_path: Some("source.txt".into()),
        index: FileChange::Copied,
        worktree: FileChange::None,
    };
    let patch = status_diff(&repository.0, &copied, true).unwrap();
    assert!(patch.contains("b/copy.txt"));
    assert!(!patch.contains("changed source only"));
    assert!(!patch.contains("b/source.txt"));
    commit(&repository.0, "Copy and independently change its source").unwrap();
    let hash = repository.head_hash();
    let mut details = commit_details(&repository.0, &hash).unwrap();
    let copy = details
        .files
        .iter_mut()
        .find(|file| file.path == "copy.txt")
        .unwrap();
    copy.status = FileChange::Copied;
    copy.original_path = Some("source.txt".into());
    let file = details
        .files
        .iter()
        .find(|file| file.path == "copy.txt")
        .unwrap();
    let patch = commit_file_diff(&repository.0, &details, file).unwrap();
    assert!(patch.contains("b/copy.txt"));
    assert!(!patch.contains("changed source only"));
    assert!(!patch.contains("b/source.txt"));
}

#[test]
fn merge_commit_files_and_patch_compare_to_the_first_parent() {
    let repository = TestRepository::new();
    repository.write("common.txt", "common\n");
    repository.commit_all("Initial");
    repository.git(&["checkout", "-b", "feature"]);
    repository.write("feature.txt", "feature branch\n");
    repository.commit_all("Feature");
    repository.git(&["checkout", "main"]);
    repository.write("main.txt", "main branch\n");
    repository.commit_all("Main");
    // The graph includes all branch tips. Sibling tips may share dates;
    // the first row is not an alias for the checked-out branch's HEAD.
    let first_parent = repository.head_hash();
    repository.git(&["merge", "--no-ff", "--no-edit", "feature"]);
    let hash = repository.head_hash();
    let details = commit_details(&repository.0, &hash).unwrap();
    assert_eq!(details.parents.len(), 2);
    assert_eq!(details.parents[0], first_parent);
    assert_eq!(details.files.len(), 1);
    assert_eq!(details.files[0].path, "feature.txt");
    assert_eq!(details.files[0].stats.added, 1);
    assert_eq!(
        commit_stats(&repository.0, &hash).unwrap(),
        CommitStats {
            files_changed: 1,
            lines: LineStats {
                added: 1,
                removed: 0,
                binary: false,
            },
        }
    );
    assert!(
        commit_diff(&repository.0, &hash, Path::new("feature.txt"))
            .unwrap()
            .contains("+feature branch\n")
    );
}

#[test]
fn nul_delimited_statistics_and_commit_paths_keep_tabs_newlines_and_renames() {
    let stats = parse_numstat(b"2\t1\tdir/with\ttab\nline.txt\0-\t-\tasset.bin\x000\t0\t\0old\tname\n.txt\0new\tname\n.txt\0").unwrap();
    assert_eq!(stats[0].0, "dir/with\ttab\nline.txt");
    assert_eq!(
        stats[0].1,
        LineStats {
            added: 2,
            removed: 1,
            binary: false
        }
    );
    assert!(stats[1].1.binary);
    assert_eq!(stats[2], ("new\tname\n.txt".into(), LineStats::default()));
    let files =
        parse_commit_files(b"R100\0old\tname\n.txt\0new\tname\n.txt\0M\0dir/with\ttab\nline.txt\0")
            .unwrap();
    assert_eq!(files[0].original_path.as_deref(), Some("old\tname\n.txt"));
    assert_eq!(files[0].path, "new\tname\n.txt");
    assert_eq!(files[1].path, "dir/with\ttab\nline.txt");
    for invalid in [
        b"1\t2\tpath".as_slice(),
        b"1\t2\t\0old\0",
        b"x\t1\tfile\0",
        b"-\t2\tfile\0",
    ] {
        assert!(parse_numstat(invalid).is_err());
    }
    assert!(parse_commit_files(b"R100\0old\0").is_err());
    assert!(parse_commit_files(b"M\0truncated").is_err());
}

#[cfg(unix)]
#[test]
fn real_git_statistics_handle_control_characters_in_file_names() {
    let repository = TestRepository::new();
    let file = "tab\tand\nnewline ü.txt";
    repository.write(file, "before\n");
    repository.commit_all("Initial");
    repository.write(file, "after\nextra\n");
    stage_paths(&repository.0, &[PathBuf::from(file)]).unwrap();
    let stats = working_tree_stats(&repository.0, &status(&repository.0).unwrap().files).unwrap();
    assert_eq!(
        stats.staged[file],
        LineStats {
            added: 2,
            removed: 1,
            binary: false
        }
    );
    commit(&repository.0, "Control characters").unwrap();
    let hash = repository.head_hash();
    let details = commit_details(&repository.0, &hash).unwrap();
    assert_eq!(details.files[0].path, file);
    assert_eq!(details.files[0].stats, stats.staged[file]);
}
