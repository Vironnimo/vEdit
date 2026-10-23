use super::*;
use crate::workspace::test_support::Fixture;
use crate::workspace::{IndexOptions, build_index};

#[test]
fn explorer_marks_gitignored_entries_without_hiding_them() {
    let fixture = Fixture::new("explorer-ignored-status");
    fixture.write(".gitignore", "ignored/\n*.log\n!keep.log\n");
    fixture.write("ignored/nested/.gitignore", "!secret.txt\n");
    fixture.write("ignored/nested/secret.txt", "ignored parent wins");
    fixture.write(
        "target/generated.txt",
        "not ignored just because it is generated",
    );
    fixture.write("output.log", "ignored");
    fixture.write("keep.log", "exception");
    fixture.write(".hidden", "hidden is not ignored");
    fixture.write(".ignore", "search-only.txt\n");
    fixture.write("search-only.txt", "not a Git ignore rule");
    fixture.write("src/.gitignore", "generated.rs\n!nested.log\n");
    fixture.write("src/generated.rs", "ignored nested rule");
    fixture.write("src/nested.log", "nested exception");
    fixture.write("src/main.rs", "normal");
    for (relative, ignored) in [
        ("ignored", true),
        ("ignored/nested", true),
        ("ignored/nested/secret.txt", true),
        ("output.log", true),
        ("keep.log", false),
        ("target", false),
        ("target/generated.txt", false),
        (".hidden", false),
        ("search-only.txt", false),
        ("src/generated.rs", true),
        ("src/nested.log", false),
        ("src/main.rs", false),
    ] {
        let path = fixture.0.join(relative);
        let listing = read_explorer_directory(&fixture.0, path.parent().unwrap()).unwrap();
        let entry = listing
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("ignored entries must remain visible: {relative}"));
        assert_eq!(entry.ignored, ignored, "{relative}");
    }
}

#[test]
fn explorer_lists_ignored_and_generated_entries_without_recursing() {
    let fixture = Fixture::new("explorer-complete-directory");
    fixture.write(".gitignore", "ignored/\n*.log\n");
    fixture.write("ignored/nested/secret.txt", "ignored");
    fixture.write("target/debug/generated.txt", "generated");
    fixture.write("node_modules/package/index.js", "dependency");
    fixture.write("output.log", "ignored log");
    fixture.write(".git/config", "metadata");
    fixture.write(".Hg/store/data", "metadata");
    fixture.write(".SVN/entries", "metadata");
    let listing = read_explorer_directory(&fixture.0, &fixture.0).unwrap();
    let names: Vec<_> = listing
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "ignored",
            "node_modules",
            "target",
            ".gitignore",
            "output.log"
        ]
    );
    assert!(listing.entries.iter().all(|entry| {
        entry
            .path
            .strip_prefix(&fixture.0)
            .unwrap()
            .components()
            .count()
            == 1
    }));
    assert!(listing.warnings.is_empty());
    let ignored = read_explorer_directory(&fixture.0, &fixture.0.join("ignored")).unwrap();
    assert_eq!(ignored.entries.len(), 1);
    assert_eq!(ignored.entries[0].name, "nested");
    assert!(ignored.entries[0].is_dir);
    assert_eq!(
        ignored.entries[0]
            .path
            .strip_prefix(&fixture.0)
            .unwrap()
            .components()
            .count(),
        2
    );
}

#[test]
fn explorer_can_access_all_rows_when_quick_open_index_is_truncated() {
    let fixture = Fixture::new("explorer-independent-index");
    for index in 0..240 {
        fixture.write(&format!("file-{index:03}.txt"), "source");
    }
    let index = build_index(
        &fixture.0,
        IndexOptions {
            max_entries: 5,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(index.truncated);
    assert_eq!(index.files.len(), 5);
    let mut tree = ExplorerTree::new(fixture.0.clone());
    let generation = tree.begin(&fixture.0).unwrap();
    assert!(tree.complete(
        &fixture.0,
        generation,
        read_explorer_directory(&fixture.0, &fixture.0)
    ));
    let snapshot = tree.snapshot();
    assert_eq!(snapshot.children(&fixture.0).count(), 240);
    assert!(snapshot.has_entry(&fixture.0.join("file-239.txt")));
    assert!(!snapshot.truncated);
}

#[test]
fn explorer_snapshots_share_directory_arrays_and_load_only_requested_folders() {
    let fixture = Fixture::new("explorer-lazy-snapshot");
    let source = fixture.write("nested/source.txt", "source");
    let mut tree = ExplorerTree::new(fixture.0.clone());
    assert!(tree.snapshot().is_explorer_snapshot());
    assert!(!tree.snapshot().directory_loaded(&fixture.0));
    let generation = tree.begin(&fixture.0).unwrap();
    assert!(tree.begin(&fixture.0).is_none());
    assert!(tree.is_loading(&fixture.0));
    tree.complete(
        &fixture.0,
        generation,
        read_explorer_directory(&fixture.0, &fixture.0),
    );
    let before = tree.snapshot();
    assert!(before.directory_loaded(&fixture.0));
    assert_eq!(before.children(&fixture.0).count(), 1);
    assert!(!before.has_entry(&source));
    let parent = source.parent().unwrap();
    assert!(!before.directory_loaded(parent));
    let generation = tree.begin(parent).unwrap();
    tree.complete(
        parent,
        generation,
        read_explorer_directory(&fixture.0, parent),
    );
    let after = tree.snapshot();
    assert!(after.has_entry(&source));
    assert!(after.directory_loaded(parent));
    assert!(
        before.children(parent).next().is_none(),
        "older snapshots stay immutable"
    );
    assert_eq!(
        before.children(&fixture.0).next().unwrap() as *const WorkspaceEntry,
        after.children(&fixture.0).next().unwrap() as *const WorkspaceEntry,
        "loading a child must not copy a large parent directory's entries"
    );
    assert_eq!(after.all_entries().count(), 2);
    assert!(after.entries.is_empty());
    assert!(tree.begin(parent).is_none());
}

#[test]
fn explorer_publication_changes_identity_without_external_snapshot_readers() {
    let fixture = Fixture::new("explorer-publication-identity");
    fixture.write("nested/source.txt", "source");
    let mut tree = ExplorerTree::new(fixture.0.clone());
    let generation = tree.begin(&fixture.0).unwrap();
    tree.complete(
        &fixture.0,
        generation,
        read_explorer_directory(&fixture.0, &fixture.0),
    );
    // Keep only an address, as navigation does. The temporary Arc is gone
    // before complete(), so the cache owns the sole strong reference.
    let previous_identity = Arc::as_ptr(&tree.snapshot()) as usize;
    assert_eq!(Arc::strong_count(&tree.snapshot), 1);
    let nested = fixture.0.join("nested");
    let generation = tree.begin(&nested).unwrap();
    tree.complete(
        &nested,
        generation,
        read_explorer_directory(&fixture.0, &nested),
    );
    assert_ne!(Arc::as_ptr(&tree.snapshot()) as usize, previous_identity);
    assert!(tree.snapshot().has_entry(&nested.join("source.txt")));
}

#[test]
fn explorer_refresh_retains_rows_and_rejects_late_directory_workers() {
    let fixture = Fixture::new("explorer-stale-generation");
    let old = fixture.write("old.txt", "old");
    let mut tree = ExplorerTree::new(fixture.0.clone());
    let original = tree.begin(&fixture.0).unwrap();
    tree.complete(
        &fixture.0,
        original,
        read_explorer_directory(&fixture.0, &fixture.0),
    );
    let stale = read_explorer_directory(&fixture.0, &fixture.0);
    tree.invalidate();
    assert!(tree.has_listing(&fixture.0));
    assert!(!tree.is_loaded(&fixture.0));
    let next = tree.begin(&fixture.0).unwrap();
    fs::remove_file(&old).unwrap();
    let new = fixture.write("new.txt", "new");
    assert!(!tree.complete(&fixture.0, original, stale));
    assert!(
        tree.is_loading(&fixture.0),
        "stale completion must not cancel the fresh request"
    );
    assert!(tree.snapshot().has_entry(&old));
    assert!(tree.complete(
        &fixture.0,
        next,
        read_explorer_directory(&fixture.0, &fixture.0)
    ));
    assert!(!tree.snapshot().has_entry(&old));
    assert!(tree.snapshot().has_entry(&new));
}

#[test]
fn explorer_folder_refresh_discards_a_read_that_predates_the_change() {
    let fixture = Fixture::new("explorer-targeted-refresh");
    fixture.write("src/old.txt", "old");
    let src = fixture.0.join("src");
    let mut tree = ExplorerTree::new(fixture.0.clone());
    for directory in [&fixture.0, &src] {
        let token = tree.begin(directory).unwrap();
        tree.complete(
            directory,
            token,
            read_explorer_directory(&fixture.0, directory),
        );
    }
    let root_identity = Arc::as_ptr(&tree.snapshot()) as usize;
    // Only the changed folder is read again; its rows stay meanwhile.
    tree.refresh(&BTreeSet::from([src.clone()]));
    assert!(tree.is_loaded(&fixture.0));
    assert!(!tree.is_loaded(&src));
    assert!(tree.snapshot().has_entry(&src.join("old.txt")));
    assert_eq!(Arc::as_ptr(&tree.snapshot()) as usize, root_identity);
    let early = tree.begin(&src).unwrap();
    let stale = read_explorer_directory(&fixture.0, &src);
    let new = fixture.write("src/new.txt", "new");
    // Another change arrives while the first read is in flight.
    tree.refresh(&BTreeSet::from([src.clone()]));
    let fresh = tree.begin(&src).unwrap();
    assert!(tree.complete(&src, fresh, read_explorer_directory(&fixture.0, &src)));
    assert!(!tree.complete(&src, early, stale));
    assert!(tree.snapshot().has_entry(&new));
    assert!(tree.is_loaded(&src));
    assert!(tree.listed().any(|path| path == src));
}

#[test]
fn explorer_forgets_a_removed_listed_folder_and_rereads_its_parent() {
    let fixture = Fixture::new("explorer-forget-folder");
    fixture.write("gone/inner/file.txt", "x");
    fixture.write("kept.txt", "x");
    let gone = fixture.0.join("gone");
    let inner = gone.join("inner");
    let mut tree = ExplorerTree::new(fixture.0.clone());
    for directory in [&fixture.0, &gone, &inner] {
        let token = tree.begin(directory).unwrap();
        tree.complete(
            directory,
            token,
            read_explorer_directory(&fixture.0, directory),
        );
    }
    tree.forget(&BTreeSet::from([gone.clone()]));
    let snapshot = tree.snapshot();
    assert!(!snapshot.has_entry(&gone));
    assert!(snapshot.has_entry(&fixture.0.join("kept.txt")));
    assert!(!tree.has_listing(&gone) && !tree.has_listing(&inner));
    assert!(!tree.is_loaded(&fixture.0), "the parent is read again");
    assert!(tree.begin(&fixture.0).is_some());
}

#[test]
fn explorer_reopened_project_rejects_workers_from_the_previous_cache() {
    let fixture = Fixture::new("explorer-reopened-generation");
    fixture.write("source.txt", "source");
    let mut previous = ExplorerTree::new(fixture.0.clone());
    let old_generation = previous.begin(&fixture.0).unwrap();
    let old_listing = read_explorer_directory(&fixture.0, &fixture.0);
    let mut reopened = ExplorerTree::new(fixture.0.clone());
    let new_generation = reopened.begin(&fixture.0).unwrap();
    assert_ne!(old_generation, new_generation);
    assert!(!reopened.complete(&fixture.0, old_generation, old_listing));
    assert!(reopened.is_loading(&fixture.0));
    assert!(reopened.complete(
        &fixture.0,
        new_generation,
        read_explorer_directory(&fixture.0, &fixture.0)
    ));
    assert!(reopened.snapshot().has_entry(&fixture.0.join("source.txt")));
}

#[cfg(unix)]
#[test]
fn explorer_lists_and_expands_user_selected_external_linked_folders() {
    let fixture = Fixture::new("explorer-linked-directory");
    let outside = Fixture::new("explorer-linked-target");
    outside.write("package/source.txt", "external dependency");
    let link = fixture.0.join("node_modules");
    std::os::unix::fs::symlink(&outside.0, &link).unwrap();
    let root = read_explorer_directory(&fixture.0, &fixture.0).unwrap();
    assert_eq!(root.entries.len(), 1);
    assert!(root.entries[0].is_dir);
    let linked = read_explorer_directory(&fixture.0, &link).unwrap();
    assert_eq!(linked.entries.len(), 1);
    assert_eq!(linked.entries[0].name, "package");
    let package = read_explorer_directory(&fixture.0, &link.join("package")).unwrap();
    assert_eq!(package.entries.len(), 1);
    assert_eq!(package.entries[0].path, link.join("package/source.txt"));
}

#[test]
fn explorer_read_errors_wait_for_explicit_refresh_and_cannot_escape_the_project() {
    let fixture = Fixture::new("explorer-retry");
    let missing = fixture.0.join("missing");
    let mut tree = ExplorerTree::new(fixture.0.clone());
    let generation = tree.begin(&missing).unwrap();
    assert!(tree.complete(
        &missing,
        generation,
        read_explorer_directory(&fixture.0, &missing)
    ));
    assert!(tree.error(&missing).is_some());
    assert!(tree.begin(&missing).is_none());
    fs::create_dir(&missing).unwrap();
    tree.invalidate();
    let generation = tree.begin(&missing).unwrap();
    assert!(tree.complete(
        &missing,
        generation,
        read_explorer_directory(&fixture.0, &missing)
    ));
    assert!(tree.error(&missing).is_none());
    assert!(tree.is_loaded(&missing));
    assert!(read_explorer_directory(&fixture.0, &fixture.0.join("..")).is_err());
}
