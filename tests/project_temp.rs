use std::{env, fs, path::PathBuf, process::Command};

#[test]
fn cargo_and_child_processes_use_checkout_temp() {
    let expected = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp"))
        .expect("the checkout includes .tmp/.gitkeep");
    assert_eq!(fs::canonicalize(env::temp_dir()).unwrap(), expected);
    for name in ["TEMP", "TMP", "TMPDIR", "GIT_CEILING_DIRECTORIES"] {
        assert_eq!(
            fs::canonicalize(env::var_os(name).expect(name)).unwrap(),
            expected,
            "{name} must point into this checkout"
        );
    }

    // Git in an empty fixture must not discover the enclosing source repository.
    let fixture = env::temp_dir().join(format!("vedit-temp-check-{}", std::process::id()));
    fs::create_dir(&fixture).unwrap();
    let result = Command::new("git")
        .current_dir(&fixture)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    fs::remove_dir(&fixture).unwrap();
    let output = result.expect("Git is required for repository tests");
    assert!(
        !output.status.success(),
        "fixture escaped into a parent repo"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a git repository"));
}
