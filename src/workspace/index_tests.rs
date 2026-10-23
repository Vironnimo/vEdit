use super::*;
use crate::workspace::test_support::Fixture;

#[test]
fn index_respects_nested_gitignore_and_generated_directories() {
    let fixture = Fixture::new("ignore");
    fixture.write(".gitignore", "ignored/\n*.log\n");
    fixture.write("src/.gitignore", "generated.rs\n");
    let source = fixture.write("src/main.rs", "fn main() {}\n");
    fixture.write("src/generated.rs", "generated");
    fixture.write("ignored/secret.txt", "secret");
    fixture.write("target/debug/cache", "cache");
    fixture.write("node_modules/package/index.js", "package");
    fixture.write("output.log", "log");
    fixture.write(".github/workflow.yml", "name: test\n");
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    assert!(index.files.contains(&source));
    assert!(
        index
            .files
            .contains(&fixture.0.join(".github/workflow.yml"))
    );
    assert!(!index.files.iter().any(|path| {
        path.ends_with("generated.rs")
            || path.ends_with("secret.txt")
            || path.ends_with("cache")
            || path.ends_with("index.js")
            || path.ends_with("output.log")
    }));
    let root_entries: Vec<_> = index.children(&fixture.0).collect();
    assert!(root_entries.first().unwrap().is_dir);
    assert!(!index.truncated);
}

#[test]
fn generated_directory_names_do_not_hide_extensionless_source_files() {
    let fixture = Fixture::new("generated-name-files");
    let files: Vec<_> = ["build", "target", "dist", "venv", "node_modules"]
        .into_iter()
        .map(|name| fixture.write(&format!("scripts/{name}"), "#!/bin/sh\necho hello\n"))
        .collect();
    fixture.write("build/generated.txt", "generated");
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    for path in files {
        assert!(index.files.contains(&path), "{path:?}");
        assert!(
            index
                .quick_open(&path.file_name().unwrap().to_string_lossy(), 10)
                .contains(&path)
        );
    }
    assert!(
        !index
            .files
            .iter()
            .any(|path| path.ends_with("generated.txt"))
    );
}

#[test]
fn showing_ignored_files_includes_nested_rules_and_generated_files_in_quick_open() {
    let fixture = Fixture::new("show-ignored");
    fixture.write(".gitignore", "ignored/\n*.log\n");
    fixture.write(".ignore", "local-only.txt\n");
    fixture.write(".git/info/exclude", "from-exclude.txt\n");
    fixture.write("src/.gitignore", "generated.rs\n");
    let ignored = [
        fixture.write("ignored/secret.txt", "ignored"),
        fixture.write("src/generated.rs", "generated"),
        fixture.write("target/debug/cache", "cache"),
        fixture.write("node_modules/package/index.js", "dependency"),
        fixture.write("dist/bundle.js", "bundle"),
        fixture.write("local-only.txt", "local"),
        fixture.write("from-exclude.txt", "local repository exclude"),
        fixture.write("output.log", "log"),
    ];
    let hidden = build_index(&fixture.0, IndexOptions::default()).unwrap();
    let shown = build_index(
        &fixture.0,
        IndexOptions {
            show_ignored_files: true,
            ..Default::default()
        },
    )
    .unwrap();
    for path in ignored {
        let query = path.file_name().unwrap().to_string_lossy();
        assert!(!hidden.files.contains(&path), "{path:?}");
        assert!(hidden.quick_open(&query, 40).is_empty(), "{path:?}");
        assert!(shown.files.contains(&path), "{path:?}");
        assert!(shown.quick_open(&query, 40).contains(&path), "{path:?}");
        assert!(
            shown
                .children(path.parent().unwrap())
                .any(|entry| entry.path == path)
        );
    }
    assert!(!shown.truncated);
}

#[test]
fn vcs_metadata_stays_hidden_when_showing_ignored_files() {
    let fixture = Fixture::new("metadata");
    for path in [
        ".git/config",
        ".hg/store/data",
        ".svn/entries",
        "nested/.GIT/HEAD",
        "worktree/.git",
    ] {
        fixture.write(path, "metadata");
    }
    let workflow = fixture.write(".github/workflow.yml", "workflow");
    for show_ignored_files in [false, true] {
        let index = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(index.files, vec![workflow.clone()]);
        assert!(index.entries.iter().all(|entry| {
            ![".git", ".hg", ".svn"]
                .iter()
                .any(|metadata| entry.name.eq_ignore_ascii_case(metadata))
        }));
    }
}

#[test]
fn rust_detection_ignores_dependency_and_ignored_manifests_but_keeps_nested_projects() {
    let fixture = Fixture::new("rust-detection");
    fixture.write("package.json", "{}");
    fixture.write(".gitignore", "ignored/\n");
    fixture.write(".ignore", "local/\n");
    fixture.write(".git/info/exclude", "private/\n");
    for manifest in [
        "node_modules/native/Cargo.toml",
        "target/nested/Cargo.toml",
        "ignored/Cargo.toml",
        "local/Cargo.toml",
        "private/Cargo.toml",
    ] {
        fixture.write(manifest, "[package]\nname = 'dependency'\n");
    }
    for show_ignored_files in [false, true] {
        let index = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            index.quick_open("Cargo.toml", 40).len(),
            if show_ignored_files { 5 } else { 0 }
        );
        assert!(!index.has_rust_manifest);
    }
    // Monorepos without a manifest at their root were already supported.
    let manifest = fixture.write(
        "crates/application/Cargo.toml",
        "[package]\nname = 'application'\n",
    );
    for show_ignored_files in [false, true] {
        let index = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(index.has_rust_manifest);
        assert!(index.files.contains(&manifest));
    }
}

#[cfg(unix)]
#[test]
fn showing_ignored_files_never_follows_or_lists_symbolic_links() {
    let fixture = Fixture::new("links");
    let outside = Fixture::new("links-outside");
    let secret = outside.write("secret.txt", "outside");
    std::os::unix::fs::symlink(&outside.0, fixture.0.join("external")).unwrap();
    std::os::unix::fs::symlink(&secret, fixture.0.join("linked.txt")).unwrap();
    for show_ignored_files in [false, true] {
        let index = build_index(
            &fixture.0,
            IndexOptions {
                show_ignored_files,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(index.entries.is_empty());
        assert!(index.quick_open("secret", 40).is_empty());
    }
}

#[test]
fn index_is_bounded_and_can_be_cancelled() {
    let fixture = Fixture::new("bounded");
    for index in 0..30 {
        fixture.write(&format!("file-{index}.txt"), "text");
    }
    let index = build_index(
        &fixture.0,
        IndexOptions {
            show_ignored_files: true,
            max_entries: 5,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(index.entries.len(), 5);
    assert!(index.truncated);
    let cancelled = build_index(
        &fixture.0,
        IndexOptions {
            show_ignored_files: true,
            cancel: Some(Arc::new(AtomicBool::new(true))),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(cancelled.files.is_empty());
    assert!(cancelled.truncated);
}

#[test]
fn quick_open_ranks_filename_before_path_and_supports_subsequences() {
    let fixture = Fixture::new("quick-open");
    let best = fixture.write("src/main.rs", "");
    fixture.write("main.rs/other.txt", "");
    fixture.write("src/menu_items.rs", "");
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    assert_eq!(index.quick_open("main.rs", 1), vec![best]);
    assert_eq!(index.quick_open("mirs", 10).len(), 3);
    assert!(index.quick_open("does-not-exist", 10).is_empty());
    assert!(index.quick_open("main", 0).is_empty());
}

#[test]
fn quick_open_cache_preserves_limits_unicode_and_index_refreshes() {
    let fixture = Fixture::new("quick-open-cache");
    let unicode = fixture.write("src/grüße.rs", "");
    fixture.write("src/main.rs", "");
    fixture.write("src/main_test.rs", "");
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    assert_eq!(index.quick_open("SRC\\GRÜ", 40), vec![unicode.clone()]);
    assert_eq!(index.quick_open(" src/grü ", 40), vec![unicode]);
    assert_eq!(index.quick_open("main", 1).len(), 1);
    assert_eq!(index.quick_open("main", 40).len(), 2);
    assert!(index.quick_open("missing", 40).is_empty());
    assert!(index.quick_open("main", 0).is_empty());
    let clone = index.clone();
    assert_eq!(clone.quick_open("main", 40), index.quick_open("main", 40));
    for query in ["src", "rs", "mai", "a", "m", "test", "g", "grü", "n"] {
        index.quick_open(query, 40);
    }
    assert_eq!(index.quick_open_cache.lock().unwrap().len(), 8);

    let added = fixture.write("src/missing.rs", "");
    let refreshed = build_index(&fixture.0, IndexOptions::default()).unwrap();
    assert!(refreshed.quick_open_cache.lock().unwrap().is_empty());
    assert_eq!(refreshed.quick_open("missing", 40), vec![added]);
    assert!(index.quick_open("missing", 40).is_empty());
}

#[test]
#[ignore = "Manual release-mode workspace latency benchmark"]
fn benchmark_quick_open_and_directory_lookup() {
    use std::hint::black_box;

    let mut index = WorkspaceIndex::empty(PathBuf::from("benchmark"));
    for file in 0..100_000 {
        let relative = format!("crate_{:04}/src/module_{file:06}.rs", file / 20);
        let path = index.root.join(&relative);
        let position = index.entries.len();
        index
            .children
            .entry(path.parent().unwrap().to_path_buf())
            .or_default()
            .push(position);
        index.entries.push(WorkspaceEntry {
            name: format!("module_{file:06}.rs"),
            is_dir: false,
            ignored: false,
            path: path.clone(),
        });
        index.files.push(path);
        index.searchable_files.push(relative);
    }
    let rounds = 100;
    let started = Instant::now();
    for _ in 0..rounds {
        index.quick_open_cache.lock().unwrap().clear();
        black_box(index.quick_open("mod123", 40));
    }
    let cold = started.elapsed() / rounds;
    let started = Instant::now();
    for _ in 0..10_000 {
        black_box(index.quick_open("mod123", 40));
    }
    let cached = started.elapsed() / 10_000;
    let directory = index.root.join("crate_1000/src");
    let started = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(black_box(index.children(&directory).count()), 20);
    }
    let directory_lookup = started.elapsed() / 10_000;
    println!(
        "100,000 files: cold quick-open {cold:?}, cached quick-open {cached:?}, 20-child directory lookup {directory_lookup:?}"
    );
}

#[test]
fn depth_limit_is_reported_without_following_deeper_folders() {
    let fixture = Fixture::new("depth");
    fixture.write("one/two/three/source.rs", "source");
    let index = build_index(
        &fixture.0,
        IndexOptions {
            max_depth: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(index.truncated);
    assert!(index.files.is_empty());
    assert!(index.entries.iter().all(|entry| {
        entry
            .path
            .strip_prefix(&fixture.0)
            .unwrap()
            .components()
            .count()
            <= 2
    }));
}
