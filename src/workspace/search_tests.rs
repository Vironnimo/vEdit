use super::*;
use crate::workspace::test_support::Fixture;
use crate::workspace::{IndexOptions, build_index};

#[test]
fn default_search_excludes_keep_source_files_named_after_generated_folders() {
    let fixture = Fixture::new("search-generated-name-files");
    let source = fixture.write("scripts/build", "needle");
    fixture.write("build/generated.txt", "needle");
    let results = search_with_excludes(
        &fixture.0,
        "needle",
        SearchOptions::default(),
        &default_search_excludes(),
    )
    .unwrap();
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].path, fs::canonicalize(source).unwrap());
}

#[test]
fn search_handles_unicode_offsets_literal_queries_and_whole_words() {
    let fixture = Fixture::new("search");
    let path = fixture.write(
        "src/main.rs",
        "ü Grüße\nHELLO hello_world hello.\n[hello]\n",
    );
    fixture.write("binary.bin", [0, 1, 2, 3]);
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    let results = search_project(
        &index,
        "hello",
        SearchOptions {
            whole_word: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(results.matches.len(), 3);
    let first = &results.matches[0];
    assert_eq!(first.path, path);
    assert_eq!(
        (first.line, first.column, first.start_char, first.end_char),
        (2, 1, 8, 13)
    );
    assert_eq!(results.matches[1].column, 19);
    assert_eq!(results.files_skipped, 1);
    let literal = search_project(&index, "[hello]", SearchOptions::default()).unwrap();
    assert_eq!(literal.matches.len(), 1);
    assert_eq!(literal.matches[0].line, 3);
    let sensitive = search_project(
        &index,
        "HELLO",
        SearchOptions {
            case_sensitive: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(sensitive.matches.len(), 1);
}

#[test]
fn disk_whole_word_search_keeps_unicode_identifiers_intact() {
    let fixture = Fixture::new("unicode-word-search");
    let path = fixture.write("words.txt", "git\u{301} git\u{200d} git‿ git");
    let results = search_with_excludes(
        &fixture.0,
        "git",
        SearchOptions {
            whole_word: true,
            ..Default::default()
        },
        &[],
    )
    .unwrap();
    assert_eq!(results.matches.len(), 1);
    let found = &results.matches[0];
    assert_eq!(found.path, fs::canonicalize(path).unwrap());
    assert_eq!(
        (found.line, found.column, found.start_char, found.end_char),
        (1, 16, 15, 18)
    );
}

#[test]
fn search_counts_multiline_matches_and_limits_results() {
    let fixture = Fixture::new("search-limits");
    fixture.write("notes.txt", "aa\nbb\naa\nbb\n");
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    let multiline = search_project(&index, "aa\nbb", SearchOptions::default()).unwrap();
    assert_eq!(multiline.matches.len(), 2);
    assert_eq!(
        (multiline.matches[1].line, multiline.matches[1].start_char),
        (3, 6)
    );
    let limited = search_project(
        &index,
        "a",
        SearchOptions {
            max_matches: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(limited.matches.len(), 1);
    assert!(limited.truncated);
}

#[test]
#[ignore = "Manual release-mode project search latency benchmark"]
fn benchmark_search_many_matches_in_long_unicode_line() {
    let fixture = Fixture::new("search-long-line-benchmark");
    let text = ("ö".repeat(1024) + "needle ").repeat(900);
    fixture.write("minified.txt", &text);
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    let started = Instant::now();
    for _ in 0..10 {
        let result = search_project(&index, "needle", SearchOptions::default()).unwrap();
        assert_eq!(result.matches.len(), 900);
        assert_eq!(result.matches[899].column, 899 * 1031 + 1025);
        std::hint::black_box(result);
    }
    println!(
        "{}-byte Unicode line, 900 matches: {:?}/search",
        text.len(),
        started.elapsed() / 10
    );
}

#[test]
fn search_previews_and_columns_keep_unicode_boundaries_on_long_lines() {
    let fixture = Fixture::new("search-long-line");
    let prefix = "ö🦀x".repeat(200);
    let line = format!("{prefix}needle {}needle", "ß".repeat(400));
    fixture.write("long.txt", format!("{line}\r\n{line}\r\n"));
    let index = build_index(&fixture.0, IndexOptions::default()).unwrap();
    let results = search_project(&index, "needle", SearchOptions::default()).unwrap();
    assert_eq!(results.matches.len(), 4);
    for (number, found) in results.matches.iter().enumerate() {
        let column = if number % 2 == 0 { 601 } else { 1008 };
        assert_eq!(found.line, number / 2 + 1);
        assert_eq!(found.column, column);
        assert_eq!(found.start_char, (number / 2) * 1014 + column - 1);
        assert_eq!(found.end_char, found.start_char + 6);
        assert!(found.preview.starts_with('…'));
        assert_eq!(found.preview.chars().nth(81), Some('n'));
    }
}

#[test]
fn search_preview_matches_character_window_at_every_unicode_boundary() {
    let line = "ö🦀e\u{301} ab".repeat(70);
    for (match_char, (match_byte, _)) in line.char_indices().enumerate() {
        let start = match_char.saturating_sub(80);
        let mut expected: String = line.chars().skip(start).take(240).collect();
        if start > 0 {
            expected.insert(0, '…');
        }
        if line.chars().count() > start + 240 {
            expected.push('…');
        }
        assert_eq!(line_preview(&line, match_byte), expected);
    }
    assert_eq!(line_preview("", 0), "");
    assert_eq!(line_preview("ö🦀", "ö🦀".len()), "ö🦀");
}

#[test]
fn binary_and_invalid_utf8_snapshots_consume_the_search_byte_budget() {
    let fixture = Fixture::new("search-invalid-byte-budget");
    let following = fixture.write("following.txt", "needle");
    for (name, bytes) in [
        ("binary.dat", vec![0, 1, 2, 3, 4, 5, 6, 7]),
        ("invalid.txt", vec![0xff; 8]),
    ] {
        let rejected = fixture.write(name, bytes);
        let results = search_paths(
            [rejected, following.clone()].into_iter().map(Ok),
            "needle",
            SearchOptions {
                max_total_bytes: 8,
                ..Default::default()
            },
            Instant::now(),
        )
        .unwrap();
        assert_eq!(results.files_skipped, 1);
        assert_eq!(results.files_searched, 0);
        assert!(results.matches.is_empty());
        assert!(results.truncated);
        assert!(results.warnings.is_empty());
    }
}

#[test]
fn search_never_lists_vcs_metadata_files_or_case_variants() {
    let fixture = Fixture::new("search-vcs-metadata");
    for path in [
        "nested/.GIT/config",
        "nested/.Hg/store/data",
        "nested/.SVN/entries",
        "worktree/.git",
    ] {
        fixture.write(path, "needle");
    }
    let source = fixture.write(".github/workflow.yml", "needle");
    let results =
        search_with_excludes(&fixture.0, "needle", SearchOptions::default(), &[]).unwrap();
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].path, fs::canonicalize(source).unwrap());
}
