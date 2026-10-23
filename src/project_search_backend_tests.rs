//! Regression coverage for user-configurable project search exclusions.

use super::*;
use std::{collections::BTreeSet, sync::atomic::AtomicU64, time::SystemTime};

const NEEDLE: &str = "vedit_search_needle";

struct Fixture {
    root: PathBuf,
    temporary_root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary_root = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = temporary_root.join(format!(
            "vedit-project-search-test-{label}-{}-{}-{}-ü 日本",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        assert!(root.starts_with(&temporary_root));
        Self {
            root,
            temporary_root,
        }
    }

    fn write(&self, name: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(name);
        assert!(path.starts_with(&self.root));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn searchable(&self, name: &str) {
        self.write(name, NEEDLE);
    }

    fn search(&self, excludes: &[&str]) -> SearchResults {
        search_with_excludes(
            &self.root,
            NEEDLE,
            SearchOptions::default(),
            &excludes
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn paths(&self, results: &SearchResults) -> BTreeSet<String> {
        results
            .matches
            .iter()
            .map(|item| {
                item.path
                    .strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // The only recursive removal in these tests must remain in the unique
        // fixture under the canonical system temp directory, never a workspace.
        assert!(self.root.starts_with(&self.temporary_root));
        assert_ne!(self.root, self.temporary_root);
        assert!(
            self.root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-project-search-test-")
        );
        if let Ok(resolved) = fs::canonicalize(&self.root) {
            assert_eq!(resolved, self.root);
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn expected(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|path| (*path).to_owned()).collect()
}

#[test]
fn defaults_exclude_generated_directories_at_every_depth() {
    let fixture = Fixture::new("defaults");
    let folders = [
        "target",
        "node_modules",
        "bower_components",
        "vendor",
        "dist",
        "build",
        ".venv",
        "venv",
        "__pycache__",
        ".next",
        ".nuxt",
        "coverage",
        ".cache",
        ".vs",
    ];
    for folder in folders {
        fixture.searchable(&format!("{folder}/generated.txt"));
        fixture.searchable(&format!("packages/sample/{folder}/nested.txt"));
    }
    fixture.searchable("src/main.rs");
    fixture.searchable("src/build.rs");
    fixture.searchable("build-tools/source.rs");
    fixture.searchable("targeting/source.rs");
    let defaults = default_search_excludes();
    assert!(validate_search_excludes(&defaults).is_ok());
    let results =
        search_with_excludes(&fixture.root, NEEDLE, SearchOptions::default(), &defaults).unwrap();
    assert!(!results.truncated);
    assert_eq!(
        fixture.paths(&results),
        expected(&[
            "src/main.rs",
            "src/build.rs",
            "build-tools/source.rs",
            "targeting/source.rs"
        ])
    );
}

#[test]
fn clearing_defaults_searches_source_inside_build_folders() {
    let fixture = Fixture::new("clear");
    fixture.searchable("build/source.rs");
    fixture.searchable("packages/library/target/source.rs");
    fixture.searchable(".cache/source.rs");
    fixture.searchable("node_modules/authored/source.ts");
    assert_eq!(
        fixture.paths(&fixture.search(&[])),
        expected(&[
            "build/source.rs",
            "packages/library/target/source.rs",
            ".cache/source.rs",
            "node_modules/authored/source.ts"
        ])
    );
}

#[test]
fn vcs_directories_stay_excluded_when_custom_excludes_are_empty() {
    let fixture = Fixture::new("vcs");
    for directory in [".git", ".hg", ".svn"] {
        fixture.searchable(&format!("{directory}/metadata"));
        fixture.searchable(&format!("nested/repository/{directory}/metadata"));
    }
    fixture.searchable("src/code.rs");
    fixture.searchable(".github/workflow.yml");
    assert_eq!(
        fixture.paths(&fixture.search(&[])),
        expected(&["src/code.rs", ".github/workflow.yml"])
    );
}

#[test]
fn custom_excludes_preserve_root_and_nested_gitignore_rules() {
    let fixture = Fixture::new("gitignore");
    // A minimal repository marker makes ignore crate apply .gitignore rules.
    fixture.write(".git/HEAD", "ref: refs/heads/main\n");
    fixture.write(".gitignore", "ignored/\n*.log\n");
    fixture.write("src/.gitignore", "generated.rs\n");
    fixture.searchable("ignored/secret.rs");
    fixture.searchable("trace.log");
    fixture.searchable("src/generated.rs");
    fixture.searchable("src/main.rs");
    fixture.searchable("build/authored.rs");
    let results = fixture.search(&[]);
    assert_eq!(
        fixture.paths(&results),
        expected(&["src/main.rs", "build/authored.rs"])
    );
}

#[test]
fn bare_folder_names_match_every_depth_but_slash_paths_are_root_relative() {
    let fixture = Fixture::new("anchoring");
    for path in [
        "generated/root.rs",
        "src/generated/nested.rs",
        "src/generatedish/keep.rs",
        "src/cache/root.rs",
        "packages/lib/src/cache/nested.rs",
        "src/main.rs",
    ] {
        fixture.searchable(path);
    }
    assert_eq!(
        fixture.paths(&fixture.search(&["generated/"])),
        expected(&[
            "src/generatedish/keep.rs",
            "src/cache/root.rs",
            "packages/lib/src/cache/nested.rs",
            "src/main.rs"
        ])
    );
    assert_eq!(
        fixture.paths(&fixture.search(&["src/cache/"])),
        expected(&[
            "generated/root.rs",
            "src/generated/nested.rs",
            "src/generatedish/keep.rs",
            "packages/lib/src/cache/nested.rs",
            "src/main.rs"
        ])
    );
}

#[test]
fn leading_slash_anchors_folder_to_the_project_root() {
    let fixture = Fixture::new("leading-slash");
    fixture.searchable("build/root.rs");
    fixture.searchable("packages/lib/build/nested.rs");
    fixture.searchable("src/main.rs");
    assert_eq!(
        fixture.paths(&fixture.search(&["/build/"])),
        expected(&["packages/lib/build/nested.rs", "src/main.rs"])
    );
}

#[test]
fn unicode_paths_backslashes_whitespace_and_blank_entries_are_supported() {
    let fixture = Fixture::new("unicode");
    fixture.searchable("源代码/生成/隐藏.rs");
    fixture.searchable("源代码/保留.rs");
    fixture.searchable("nested/源代码/生成/保留.rs");
    fixture.searchable("缓存/隐藏.rs");
    fixture.searchable("nested/缓存/隐藏.rs");
    assert_eq!(
        fixture.paths(&fixture.search(&["  源代码\\生成\\  ", " 缓存/ ", "", "   "])),
        expected(&["源代码/保留.rs", "nested/源代码/生成/保留.rs"])
    );
}

#[test]
fn star_question_character_class_and_double_star_globs_filter_paths() {
    let fixture = Fixture::new("globs");
    for path in [
        "src/unit1.generated.rs",
        "packages/unit2.generated.rs",
        "src/temp-a.rs",
        "src/temp-ab.rs",
        "src/variant-a.rs",
        "src/variant-b.rs",
        "src/variant-c.rs",
        "deep/vendor/cache/data.txt",
        "vendor/cache/root.txt",
        "vendor/source.txt",
        "src/keep.rs",
    ] {
        fixture.searchable(path);
    }
    assert_eq!(
        fixture.paths(&fixture.search(&[
            "*.generated.rs",
            "src/temp-?.rs",
            "src/variant-[ab].rs",
            "**/vendor/cache/**"
        ])),
        expected(&[
            "src/temp-ab.rs",
            "src/variant-c.rs",
            "vendor/source.txt",
            "src/keep.rs"
        ])
    );
}

#[test]
fn single_star_stays_in_one_directory_and_double_star_matches_zero_or_more() {
    let fixture = Fixture::new("glob-depth");
    fixture.searchable("src/root.rs");
    fixture.searchable("src/deep/nested.rs");
    fixture.searchable("src/readme.md");
    fixture.searchable("other/root.rs");
    assert_eq!(
        fixture.paths(&fixture.search(&["src/*.rs"])),
        expected(&["src/deep/nested.rs", "src/readme.md", "other/root.rs"])
    );
    assert_eq!(
        fixture.paths(&fixture.search(&["src/**/*.rs"])),
        expected(&["src/readme.md", "other/root.rs"])
    );
}

#[test]
fn directory_patterns_retain_regular_files_with_the_same_name() {
    let fixture = Fixture::new("folder-only");
    fixture.searchable("cache/generated.rs");
    fixture.searchable("nested/cache/generated.rs");
    fixture.searchable("src/cache");
    fixture.searchable("src/main.rs");
    assert_eq!(
        fixture.paths(&fixture.search(&["cache/"])),
        expected(&["src/cache", "src/main.rs"])
    );
    assert_eq!(
        fixture.paths(&fixture.search(&["cache"])),
        expected(&["src/main.rs"])
    );
}

#[test]
fn leading_hash_is_a_literal_folder_and_not_a_comment() {
    let fixture = Fixture::new("hash-folder");
    fixture.searchable("#cache/generated.rs");
    fixture.searchable("nested/#cache/generated.rs");
    fixture.searchable("cache/keep.rs");
    fixture.searchable("src/main.rs");
    assert_eq!(
        fixture.paths(&fixture.search(&["#cache/"])),
        expected(&["cache/keep.rs", "src/main.rs"])
    );
}

#[test]
fn changing_excludes_between_searches_does_not_mutate_other_searches() {
    let fixture = Fixture::new("different-lists");
    fixture.searchable("build/source.rs");
    fixture.searchable("src/main.rs");
    assert_eq!(
        fixture.paths(&fixture.search(&["build/"])),
        expected(&["src/main.rs"])
    );
    assert_eq!(
        fixture.paths(&fixture.search(&[])),
        expected(&["build/source.rs", "src/main.rs"])
    );
    assert_eq!(
        fixture.paths(&fixture.search(&["src/"])),
        expected(&["build/source.rs"])
    );
}

#[test]
fn excluded_files_are_pruned_before_reading_or_skip_accounting() {
    let fixture = Fixture::new("prune-first");
    fixture.write("generated/binary.bin", [0, 1, 2, 3]);
    fixture.write("generated/large.txt", "x".repeat(4096));
    fixture.searchable("src/main.rs");
    let results = search_with_excludes(
        &fixture.root,
        NEEDLE,
        SearchOptions {
            max_file_bytes: 1024,
            ..Default::default()
        },
        &["generated/".into()],
    )
    .unwrap();
    assert_eq!(results.files_searched, 1);
    assert_eq!(results.files_skipped, 0);
    assert!(results.warnings.is_empty());
}

#[test]
fn search_options_and_hard_match_limits_are_preserved_with_custom_excludes() {
    let fixture = Fixture::new("options");
    fixture.write("src/visible.txt", format!("{NEEDLE}\n{NEEDLE}\n{NEEDLE}\n"));
    fixture.write("excluded/hidden.txt", format!("{NEEDLE}\n{NEEDLE}\n"));
    let excludes = vec!["excluded/".to_owned()];
    let results = search_with_excludes(
        &fixture.root,
        NEEDLE,
        SearchOptions {
            max_matches: 2,
            ..Default::default()
        },
        &excludes,
    )
    .unwrap();
    assert_eq!(results.matches.len(), 2);
    assert!(results.truncated);
    assert_eq!(fixture.paths(&results), expected(&["src/visible.txt"]));
    fixture.write("src/words.txt", "Token token tokenizer\n");
    let results = search_with_excludes(
        &fixture.root,
        "token",
        SearchOptions {
            case_sensitive: true,
            whole_word: true,
            ..Default::default()
        },
        &excludes,
    )
    .unwrap();
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].column, 7);
}

#[test]
fn cancellation_and_zero_budgets_bound_custom_search() {
    let fixture = Fixture::new("cancel");
    for index in 0..20 {
        fixture.searchable(&format!("src/file-{index}.rs"));
    }
    let cancel = Arc::new(AtomicBool::new(true));
    let cancelled = search_with_excludes(
        &fixture.root,
        NEEDLE,
        SearchOptions {
            cancel: Some(cancel),
            ..Default::default()
        },
        &[],
    )
    .unwrap();
    assert!(cancelled.truncated);
    assert!(cancelled.matches.is_empty());
    assert_eq!(cancelled.files_searched, 0);
    let time_limited = search_with_excludes(
        &fixture.root,
        NEEDLE,
        SearchOptions {
            max_duration: Duration::ZERO,
            ..Default::default()
        },
        &[],
    )
    .unwrap();
    assert!(time_limited.truncated);
    assert!(time_limited.matches.is_empty());
    let bytes_limited = search_with_excludes(
        &fixture.root,
        NEEDLE,
        SearchOptions {
            max_total_bytes: 0,
            ..Default::default()
        },
        &[],
    )
    .unwrap();
    assert!(bytes_limited.truncated);
    assert!(bytes_limited.matches.is_empty());
}

#[test]
fn validation_rejects_negation_invalid_globs_and_oversized_lists() {
    for pattern in ["!target/", "  !src/  ", "src/[unterminated", "["] {
        assert!(
            validate_search_excludes(&[pattern.to_owned()]).is_err(),
            "accepted invalid exclude {pattern:?}"
        );
    }
    assert!(validate_search_excludes(&vec!["folder".to_owned(); 129]).is_err());
    assert!(validate_search_excludes(&["a".repeat(1025)]).is_err());
    // Enforce bytes, not Unicode scalar count.
    assert!(validate_search_excludes(&["ü".repeat(513)]).is_err());
    assert!(validate_search_excludes(&[" ".repeat(1025)]).is_err());
    assert!(validate_search_excludes(&vec!["a".repeat(1024); 17]).is_err());
    assert!(validate_search_excludes(&vec!["folder".to_owned(); 128]).is_ok());
    assert!(validate_search_excludes(&["a".repeat(1024)]).is_ok());
    assert!(validate_search_excludes(&["ü".repeat(512)]).is_ok());
    assert!(validate_search_excludes(&vec!["a".repeat(1024); 16]).is_ok());
    assert!(validate_search_excludes(&["".into(), "   ".into(), "**/build/**".into()]).is_ok());
    for pattern in ["foo\nbar", "foo\rbar", "foo\0bar", "foo\u{0001}bar"] {
        assert!(
            validate_search_excludes(&[pattern.to_owned()]).is_err(),
            "accepted control characters in {pattern:?}"
        );
    }
}

#[test]
fn invalid_excludes_fail_even_when_query_has_no_matches() {
    let fixture = Fixture::new("invalid-search");
    fixture.searchable("src/main.rs");
    assert!(
        search_with_excludes(
            &fixture.root,
            "absent",
            SearchOptions::default(),
            &["!src/".into()]
        )
        .is_err()
    );
    assert!(
        search_with_excludes(
            &fixture.root,
            "",
            SearchOptions::default(),
            &["!src/".into()]
        )
        .is_err()
    );
}
