//! Explicit, opt-in checks against the actual Python and Dart language servers.
use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vedit-polyglot-{}-{stamp} spaces ü",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-polyglot-")
        );
        for _ in 0..20 {
            if fs::remove_dir_all(&self.0).is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn exercise(extension: &str, original: &str, updated: &str, symbol: &str) {
    exercise_with_environment(extension, original, updated, symbol, false);
}

fn exercise_with_environment(
    extension: &str,
    original: &str,
    updated: &str,
    symbol: &str,
    virtual_environment: bool,
) {
    let fixture = Fixture::new();
    let module = if virtual_environment {
        let environment = fixture.0.join(".venv");
        let created = Command::new("python")
            .args(["-m", "venv", "--without-pip"])
            .arg(&environment)
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "Could not create Python fixture: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        let interpreter = environment.join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        let output = Command::new(&interpreter)
            .args([
                "-X",
                "utf8",
                "-c",
                "import sysconfig; print(sysconfig.get_paths()['purelib'])",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let site_packages = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        assert!(site_packages.starts_with(&fixture.0));
        let module = site_packages.join("vedit_venv_only/__init__.py");
        fs::create_dir_all(module.parent().unwrap()).unwrap();
        fs::write(
            &module,
            "def double(value: int) -> int:\n    return value * 2\n",
        )
        .unwrap();
        fs::write(module.parent().unwrap().join("py.typed"), "").unwrap();
        Some(module)
    } else {
        None
    };
    if extension == "dart" {
        fs::write(
            fixture.0.join("pubspec.yaml"),
            "name: vedit_language_smoke\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\n",
        )
        .unwrap();
    }
    let path = fixture.0.join(format!("sample.{extension}"));
    fs::write(&path, original).unwrap();
    let language = if extension == "py" {
        "python"
    } else {
        extension
    };
    let config = crate::app::builtin_language_smoke_config(language, &fixture.0);
    let server = LanguageServer::start_configured(fixture.0.clone(), config, None);
    assert!(server.open(&path, original));
    let completion_position = char_to_position(original, original.rfind("doubl").unwrap() + 5);
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut ready = false;
    let mut diagnosed = false;
    let mut completed = false;
    let mut next_request = Instant::now();
    while Instant::now() < deadline && !(ready && diagnosed && completed) {
        if ready && Instant::now() >= next_request {
            assert_ne!(server.completion(&path, completion_position, 1), 0);
            next_request = Instant::now() + Duration::from_millis(500);
        }
        match server.receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(LspEvent::Ready) => ready = true,
            Ok(LspEvent::Diagnostics {
                path: target,
                diagnostics,
                ..
            }) if normalize_path(&target) == normalize_path(&path) => {
                if virtual_environment {
                    assert!(
                        !diagnostics
                            .iter()
                            .any(|item| item.message.contains("vedit_venv_only")),
                        "The selected venv's module was not resolved: {diagnostics:?}"
                    );
                }
                diagnosed |= diagnostics
                    .iter()
                    .any(|item| item.severity == 1 && !item.message.is_empty());
            }
            Ok(LspEvent::Completions { items, .. }) => {
                completed |= items.iter().any(|item| item.label.starts_with(symbol));
            }
            Ok(LspEvent::Error(error)) => panic!("{extension} analysis failed: {error}"),
            Ok(LspEvent::RequestFailed {
                error: Some(error), ..
            }) => panic!("{extension} request failed: {error}"),
            _ => {}
        }
    }
    assert!(ready, "{extension} server never became ready");
    assert!(
        diagnosed,
        "{extension} server did not report the unresolved name"
    );
    assert!(completed, "{extension} server did not offer {symbol}");

    // Keep the old file on disk: definition must use the changed, unsaved buffer.
    assert!(server.change(&path, updated, 2));
    let position = char_to_position(updated, updated.rfind(symbol).unwrap() + 2);
    let mut request = server.definition(&path, position);
    assert_ne!(request, 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    next_request = Instant::now() + Duration::from_secs(1);
    let mut defined = false;
    while Instant::now() < deadline && !defined {
        match server.receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(LspEvent::Definition {
                request_id,
                path: target,
                line,
                column,
            }) if request_id == request => {
                assert_eq!(
                    normalize_path(&target),
                    normalize_path(module.as_deref().unwrap_or(&path))
                );
                assert_eq!(line, 0);
                assert_eq!(column, 4);
                defined = true;
            }
            Ok(LspEvent::Error(error)) => panic!("{extension} definition failed: {error}"),
            Ok(LspEvent::RequestFailed {
                error: Some(error), ..
            }) => panic!("{extension} definition request failed: {error}"),
            _ => {}
        }
        if !defined && Instant::now() >= next_request {
            request = server.definition(&path, position);
            next_request = Instant::now() + Duration::from_secs(1);
        }
    }
    assert!(
        defined,
        "{extension} server did not navigate to the local definition"
    );
    server.shutdown();
    drop(server);
    thread::sleep(Duration::from_millis(200));
}

#[test]
#[ignore = "requires the pinned local basedpyright package and Node; run explicitly"]
fn live_python_diagnostics_completion_and_definition() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools/basedpyright/node_modules/basedpyright/langserver.index.js");
    assert!(
        script.is_file(),
        "Run npm ci --prefix tools/basedpyright --ignore-scripts first"
    );
    exercise(
        "py",
        "def double(value: int) -> int:\n    return value * 2\n\nresult: int = doubl\n",
        "def double(value: int) -> int:\n    return value * 2\n\nresult: int = double(4)\n",
        "double",
    );
}

#[test]
#[ignore = "requires Dart SDK executable in VEDIT_TEST_DART_SERVER; run explicitly"]
fn live_dart_diagnostics_completion_and_definition() {
    std::env::var("VEDIT_TEST_DART_SERVER")
        .expect("Set VEDIT_TEST_DART_SERVER to the Dart SDK dart.exe");
    exercise(
        "dart",
        "int doubleValue(int value) => value * 2;\nvoid main() { final result = doubl; print(result); }\n",
        "int doubleValue(int value) => value * 2;\nvoid main() { final result = doubleValue(4); print(result); }\n",
        "doubleValue",
    );
}

#[test]
#[ignore = "requires Python, Node and pinned basedpyright; creates a disposable venv"]
fn live_python_project_venv_resolves_its_own_imports() {
    exercise_with_environment(
        "py",
        "from vedit_venv_only import double\nresult = doubl\n",
        "from vedit_venv_only import double\nresult = double(4)\n",
        "double",
        true,
    );
}
