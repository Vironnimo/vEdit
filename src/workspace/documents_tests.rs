use super::*;
use crate::workspace::test_support::Fixture;

fn save_document(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    expected: Option<&FileFingerprint>,
) -> Result<FileFingerprint, SaveError> {
    super::save_document(
        path,
        text,
        line_ending,
        bom,
        expected,
        expected.map(|baseline| (path, baseline)),
    )
    .map(|saved| saved.fingerprint)
}

#[test]
fn loading_rejects_large_binary_and_invalid_utf8_files() {
    let fixture = Fixture::new("validation");
    let too_big = fixture.write("large.txt", "12345");
    let binary = fixture.write("binary.dat", [0, 65]);
    let invalid = fixture.write("invalid.txt", [0xff, 0xfe]);
    assert!(
        read_document(&too_big, 4)
            .unwrap_err()
            .contains("too large")
    );
    assert!(read_document(&binary, 100).unwrap_err().contains("binary"));
    assert!(read_document(&invalid, 100).unwrap_err().contains("UTF-8"));
}

#[test]
fn atomic_save_preserves_crlf_and_bom_and_detects_external_edits() {
    let fixture = Fixture::new("save");
    let path = fixture.write("document.txt", b"\xef\xbb\xbfhello\r\nworld\r\n");
    let document = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
    assert_eq!(document.text, "hello\nworld\n");
    assert_eq!(document.line_ending, LineEnding::CrLf);
    assert!(document.bom);
    let saved = save_document(
        &path,
        "hello\nedited\n",
        document.line_ending,
        document.bom,
        Some(&document.fingerprint),
    )
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfhello\r\nedited\r\n");
    fs::write(&path, "external edit").unwrap();
    assert!(matches!(
        save_document(&path, "my edit", LineEnding::Lf, false, Some(&saved)),
        Err(SaveError::Conflict(_))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "external edit");
    assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vedit-save")
    }));
}

#[test]
fn only_save_temporaries_of_exited_processes_are_removed() {
    let fixture = Fixture::new("stale-save-temporaries");
    let exited = {
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    };
    let current = std::process::id();
    let stale = fixture.write(&format!(".vedit-save-{exited}-3.tmp"), "interrupted");
    let kept = [
        fixture.write(&format!(".vedit-save-{current}-9.tmp"), "saving now"),
        fixture.write(&format!(".vedit-save-{exited}-3.tmp.bak"), "other"),
        fixture.write(&format!(".vedit-save-{exited}-x.tmp"), "other"),
        fixture.write(&format!("vedit-save-{exited}-3.tmp"), "other"),
        fixture.write("notes.tmp", "other"),
    ];
    let directory = fixture.0.join(format!(".vedit-save-{exited}-4.tmp"));
    fs::create_dir(&directory).unwrap();
    let document = fixture.write("document.txt", "before");
    let (_, fingerprint) = read_bytes(&document, 1024).unwrap();
    // The editor's save worker cleans the folder before each save.
    remove_stale_save_temporaries(&fixture.0);
    save_document(
        &document,
        "after",
        LineEnding::Lf,
        false,
        Some(&fingerprint),
    )
    .unwrap();
    assert!(!stale.exists());
    assert!(kept.iter().all(|path| path.is_file()));
    assert!(directory.is_dir());
    assert_eq!(fs::read_to_string(&document).unwrap(), "after");
    // A folder is checked once; files left later wait for the next session.
    let later = fixture.write(&format!(".vedit-save-{exited}-5.tmp"), "later");
    remove_stale_save_temporaries(&fixture.0);
    assert!(later.exists());
}

#[test]
fn save_as_never_overwrites_existing_files_and_deleted_files_conflict() {
    let fixture = Fixture::new("save-as");
    let path = fixture.0.join("new.txt");
    let fingerprint = save_document(&path, "draft", LineEnding::Lf, false, None).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "draft");
    assert!(matches!(
        save_document(&path, "replace", LineEnding::Lf, false, None),
        Err(SaveError::Conflict(_))
    ));
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        save_document(&path, "replace", LineEnding::Lf, false, Some(&fingerprint)),
        Err(SaveError::Conflict(_))
    ));
    assert!(!path.exists());
}

#[test]
fn mixed_endings_survive_edits_and_insertions() {
    let fixture = Fixture::new("mixed-endings");
    let path = fixture.write("mixed.txt", "one\r\ntwo\nthree\r\nfour\r");
    let original = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
    assert_eq!(original.line_ending, LineEnding::Mixed);
    assert!(original.mixed_line_endings);
    let saved = save_document(
        &path,
        "one\ntwo edited\nthree\nfour\n",
        LineEnding::Mixed,
        false,
        Some(&original.fingerprint),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "one\r\ntwo edited\nthree\r\nfour\r"
    );
    save_document(
        &path,
        "one\ninserted\ntwo edited\nthree\nfour\n",
        LineEnding::Mixed,
        false,
        Some(&saved),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "one\r\ninserted\r\ntwo edited\nthree\r\nfour\r"
    );
}

#[test]
fn same_size_external_edits_are_not_overwritten() {
    let fixture = Fixture::new("same-size-conflict");
    let path = fixture.write("source.rs", "original");
    let original = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
    fs::write(&path, "external").unwrap();
    assert!(matches!(
        save_document(
            &path,
            "my edits",
            LineEnding::Lf,
            false,
            Some(&original.fingerprint)
        ),
        Err(SaveError::Conflict(_))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
}

#[test]
fn read_only_file_is_left_intact() {
    let fixture = Fixture::new("read-only");
    let path = fixture.write("locked.txt", "original");
    let original = read_document(&path, MAX_DOCUMENT_BYTES).unwrap();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut locked_permissions = original_permissions.clone();
    locked_permissions.set_readonly(true);
    fs::set_permissions(&path, locked_permissions).unwrap();
    let result = save_document(
        &path,
        "modified",
        LineEnding::Lf,
        false,
        Some(&original.fingerprint),
    );
    let content = fs::read_to_string(&path).unwrap();
    fs::set_permissions(&path, original_permissions).unwrap();
    assert!(result.unwrap_err().to_string().contains("read-only"));
    assert_eq!(content, "original");
}
