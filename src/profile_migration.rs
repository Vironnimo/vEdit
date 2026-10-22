//! One-time, resumable import of profiles from before the vEdit installation.
//! Normal profile resolution never falls back to the previous product name.
use super::Session;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const STAGE: &str = ".profile-migration-stage";
const PENDING: &str = "profile-migration-pending.json";
const COMPLETE: &str = "profile-migration-v1.json";

pub(crate) fn initialize() -> Result<(), String> {
    // Private profiles are already independent; never touch the real profile
    // from a test or an explicitly isolated editor.
    if std::env::var_os("VEDIT_PROFILE_DIR").is_some() {
        return Ok(());
    }
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if !needs_migration(&appdata) {
        return Ok(());
    }
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    appdata.to_string_lossy().to_lowercase().hash(&mut hash);
    let name = format!("Local\\vEdit.ProfileMigration.{:016x}", hash.finish());
    let deadline = Instant::now() + Duration::from_secs(12);
    let _guard = loop {
        if let Some(guard) =
            crate::single_instance::acquire_named(&name).map_err(|e| e.to_string())?
        {
            break guard;
        }
        if Instant::now() >= deadline {
            return Err("Another vEdit is importing the profile. Try again shortly.".into());
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    if !needs_migration(&appdata) {
        return Ok(());
    }
    // An older main window can be starting before its endpoint is written.
    // The compatibility name exists only during the one-time import.
    let _old_guard = crate::single_instance::acquire_named("Local\\Morrow.Editor.SingleInstance")
        .map_err(|e| e.to_string())?
        .ok_or("Close the older editor normally, then start vEdit again to import its settings and drafts.")?;
    migrate_under(&appdata).map_err(|e| format!("Could not import the editor profile: {e}. Original data is retained. Close older editor windows normally and start vEdit again."))
}

fn sources(appdata: &Path) -> [(PathBuf, &'static str); 3] {
    [
        (appdata.join("Morrow"), "previous-profile"),
        (appdata.join("morrow-editor"), "previous-window-state"),
        (appdata.join("vedit-editor"), "previous-vedit-window-state"),
    ]
}

fn needs_migration(appdata: &Path) -> bool {
    appdata.join("vEdit").join(PENDING).exists()
        || sources(appdata).iter().any(|(path, _)| path.exists())
}

fn migrate_under(appdata: &Path) -> Result<(), String> {
    let root = appdata.join("vEdit");
    let target = root.join("recovery");
    let stage = root.join(STAGE);
    let pending = root.join(PENDING);
    let backup = root.join("migration-backup");
    if target.join(COMPLETE).is_file() {
        // Publication succeeded, but the process could have stopped before
        // deleting the journal. No source is ever imported a second time.
        if pending.is_file() {
            fs::remove_file(&pending).map_err(|e| e.to_string())?;
        }
        if sources(appdata).iter().any(|(p, _)| p.exists()) {
            return Err("a previous profile reappeared after the completed import; both profiles have been kept".into());
        }
        return Ok(());
    }
    for directory in [appdata.join("Morrow/recovery"), target.clone()] {
        ensure_closed(&directory)?;
    }
    if root.exists() {
        reject_link(&root)?;
    }
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    if !pending.exists() {
        let legacy = appdata.join("Morrow/recovery");
        if has_entries(&target)? && has_entries(&legacy)? {
            return Err(format!(
                "both {} and {} contain sessions; neither was replaced",
                target.display(),
                legacy.display()
            ));
        }
        if backup.exists() {
            return Err(format!(
                "the import backup already exists at {}; it was not replaced",
                backup.display()
            ));
        }
        // An interrupted preparation has not moved any original files yet.
        // This reserved staging directory is strictly below the profile root.
        if stage.exists() {
            reject_link(&stage)?;
            fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
        }
        fs::create_dir(&stage).map_err(|e| e.to_string())?;
        let source = if has_entries(&legacy)? {
            &legacy
        } else {
            &target
        };
        if source.exists() {
            copy_tree(source, &stage)?;
        }
        if !stage.join("app.ron").exists() {
            for settings in [
                appdata.join("morrow-editor/data/app.ron"),
                appdata.join("vedit-editor/data/app.ron"),
            ] {
                if settings.is_file() {
                    fs::copy(settings, stage.join("app.ron")).map_err(|e| e.to_string())?;
                    break;
                }
            }
        }
        rewrite_tree(&stage, &legacy, &target)?;
        write_synced(&stage.join(COMPLETE), b"{\"version\":1}\n")?;
        // From this point on, resume publication, never re-prepare from a
        // partially archived source. The journal is itself published atomically.
        let temporary = root.join(".profile-migration-pending.tmp");
        write_synced(&temporary, b"{\"version\":1}\n")?;
        fs::rename(temporary, &pending).map_err(|e| e.to_string())?;
    }
    if !stage.join(COMPLETE).is_file() {
        return Err("the prepared profile is incomplete; the import backups were kept".into());
    }
    fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
    for (source, name) in sources(appdata)
        .into_iter()
        .chain([(target.clone(), "previous-recovery")])
    {
        if source.exists() {
            reject_link(&source)?;
            let destination = backup.join(name);
            if destination.exists() {
                return Err(format!(
                    "{} already exists; no data was replaced",
                    destination.display()
                ));
            }
            fs::rename(source, destination).map_err(|e| e.to_string())?;
        }
    }
    fs::rename(stage, target).map_err(|e| e.to_string())?;
    fs::remove_file(pending).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::File::create(path).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}

fn has_entries(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .map_err(|e| e.to_string())
}

fn reject_link(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "linked profile path {} requires manual import",
                path.display()
            ));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "linked profile path {} requires manual import",
            path.display()
        ));
    }
    Ok(())
}

fn ensure_closed(directory: &Path) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    reject_link(directory)?;
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        reject_link(&path)?;
        if path.is_dir() {
            ensure_closed(&path)?;
        } else if path
            .file_name()
            .is_some_and(|n| n == "control.json" || n == "window-open")
        {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            let pid = if path.ends_with("control.json") {
                serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|v| v["pid"].as_u64())
                    .and_then(|pid| u32::try_from(pid).ok())
            } else {
                std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
            };
            if pid.is_some_and(crate::process_liveness::running) {
                return Err("an editor still owns this profile".into());
            }
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    reject_link(source)?;
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        reject_link(&entry.path())?;
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if entry.file_name() != "control.json"
            && entry.file_name() != "last-active"
            && !(entry.file_name().to_string_lossy().starts_with('.')
                && entry.file_name().to_string_lossy().ends_with(".tmp"))
        {
            fs::copy(entry.path(), target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn rewrite_tree(directory: &Path, old: &Path, new: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            rewrite_tree(&path, old, new)?;
        } else if path.ends_with("session.json") {
            let mut session: Session =
                serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            rewrite_session(&mut session, old, new);
            write_synced(
                &path,
                &serde_json::to_vec(&session).map_err(|e| e.to_string())?,
            )?;
        } else if path.ends_with("app.ron") {
            let mut storage: BTreeMap<String, String> =
                ron::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if let Some(serialized) = storage.get_mut(eframe::APP_KEY) {
                let mut session: Session = ron::from_str(serialized).map_err(|e| e.to_string())?;
                rewrite_session(&mut session, old, new);
                *serialized = ron::to_string(&session).map_err(|e| e.to_string())?;
            }
            write_synced(
                &path,
                ron::to_string(&storage)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )?;
        } else if path.extension().is_some_and(|e| e == "txt") {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            if let Some(record) = bytes.strip_prefix(b"Morrow recovery v1\0") {
                let mut migrated = b"vEdit recovery v1\0".to_vec();
                migrated.extend_from_slice(record);
                write_synced(&path, &migrated)?;
            }
        }
    }
    Ok(())
}

fn rewrite_session(session: &mut Session, old: &Path, new: &Path) {
    for project in &mut session.projects {
        for tab in &mut project.tabs {
            if let Some(path) = &mut tab.recovery_path
                && let Some(relative) = recovery_relative(path, old)
            {
                *path = new.join(relative);
            }
        }
        for command in [
            &mut project.tasks.check,
            &mut project.tasks.test,
            &mut project.tasks.build,
            &mut project.tasks.run_file,
        ] {
            *command = rewrite_command(command);
        }
    }
}

fn recovery_relative(path: &Path, old: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    for expected in old.components() {
        let actual = components.next()?;
        #[cfg(windows)]
        let matches = actual
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy());
        #[cfg(not(windows))]
        let matches = actual == expected;
        if !matches {
            return None;
        }
    }
    Some(components.as_path().to_owned())
}

fn rewrite_command(command: &str) -> String {
    static VARIABLE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let expression = VARIABLE
        .get_or_init(|| regex::Regex::new(r"(\$env:|\$\{|\$)MORROW_(ROOT|FILE|PYTHON)\b").unwrap());
    expression
        .replace_all(command, "${1}VEDIT_${2}")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{SessionProject, SessionTab};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "vedit-profile-migration-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn write(&self, path: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn import_preserves_drafts_baselines_settings_and_independent_windows() {
        let fixture = Fixture::new();
        let old = fixture.0.join("Morrow/recovery");
        let new = fixture.0.join("vEdit/recovery");
        let original_path = fixture.0.join("projects/Morrow/keep-name.py");
        let original = b"original\r\n";
        fixture.write("projects/Morrow/keep-name.py", original);
        let baseline = crate::workspace::read_bytes(&original_path, 100).unwrap().1;
        let encoded = crate::recovery::encode_document(
            "draft 東京\r\n",
            Some(&baseline),
            crate::workspace::LineEnding::CrLf,
            true,
        )
        .unwrap();
        let mut legacy = b"Morrow recovery v1\0".to_vec();
        legacy.extend(encoded.strip_prefix(b"vEdit recovery v1\0").unwrap());
        fixture.write("Morrow/recovery/123-1.txt", &legacy);
        let mut session = Session::default();
        let mut project = SessionProject {
            root: original_path.parent().unwrap().to_owned(),
            tabs: vec![SessionTab {
                path: Some(original_path.clone()),
                title: "Morrow source".into(),
                recovery_path: Some(old.join("123-1.txt")),
                cursor: 7,
                agent_unsaved: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        project.tasks.run_file = "& $env:MORROW_PYTHON $env:MORROW_FILE".into();
        session.projects.push(project);
        let serialized = serde_json::to_vec(&session).unwrap();
        fixture.write("Morrow/recovery/session.json", &serialized);
        fixture.write("Morrow/recovery/windows/w1/session.json", &serialized);
        fixture.write("Morrow/recovery/windows/w1/window-open", "4294967295");
        fixture.write(
            "Morrow/recovery/control.json",
            r#"{"pid":4294967295,"token":"obsolete"}"#,
        );
        fixture.write(
            "Morrow/recovery/.session.json.morrow-10-1.tmp",
            b"unfinished",
        );
        let storage = BTreeMap::from([
            (
                eframe::APP_KEY.to_owned(),
                ron::to_string(&session).unwrap(),
            ),
            ("window".to_owned(), "original window geometry".to_owned()),
            ("egui".to_owned(), "original egui preferences".to_owned()),
        ]);
        let old_storage = ron::to_string(&storage).unwrap();
        fixture.write("morrow-editor/data/app.ron", &old_storage);
        migrate_under(&fixture.0).unwrap();
        for relative in ["session.json", "windows/w1/session.json"] {
            let migrated: Session =
                serde_json::from_slice(&fs::read(new.join(relative)).unwrap()).unwrap();
            let tab = &migrated.projects[0].tabs[0];
            assert_eq!(tab.path.as_ref(), Some(&original_path));
            assert_eq!(tab.title, "Morrow source");
            assert_eq!(tab.cursor, 7);
            assert!(tab.agent_unsaved);
            assert_eq!(tab.recovery_path, Some(new.join("123-1.txt")));
            assert_eq!(
                migrated.projects[0].tasks.run_file,
                "& $env:VEDIT_PYTHON $env:VEDIT_FILE"
            );
        }
        let recovered = crate::recovery::read_document(&new.join("123-1.txt"), 100).unwrap();
        assert_eq!(recovered.text, "draft 東京\r\n");
        assert_eq!(recovered.baseline, Some(baseline));
        assert_eq!(
            recovered.line_ending,
            Some(crate::workspace::LineEnding::CrLf)
        );
        assert_eq!(recovered.bom, Some(true));
        assert!(!new.join("control.json").exists());
        assert!(!new.join(".session.json.morrow-10-1.tmp").exists());
        assert!(new.join("windows/w1/window-open").exists());
        assert_eq!(fs::read(&original_path).unwrap(), original);
        let saved: BTreeMap<String, String> =
            ron::from_str(&fs::read_to_string(new.join("app.ron")).unwrap()).unwrap();
        assert_eq!(saved["window"], storage["window"]);
        assert_eq!(saved["egui"], storage["egui"]);
        assert!(!saved[eframe::APP_KEY].contains("MORROW_FILE"));
        assert_eq!(
            fs::read_to_string(
                fixture
                    .0
                    .join("vEdit/migration-backup/previous-window-state/data/app.ron")
            )
            .unwrap(),
            old_storage
        );
        assert_eq!(
            fs::read(
                fixture
                    .0
                    .join("vEdit/migration-backup/previous-profile/recovery/123-1.txt")
            )
            .unwrap(),
            legacy
        );
        assert!(!needs_migration(&fixture.0));
        migrate_under(&fixture.0).unwrap();
    }

    #[test]
    fn merges_existing_vedit_window_geometry_into_current_profile() {
        let fixture = Fixture::new();
        fixture.write("vEdit/recovery/session.json", b"{}");
        fixture.write("vedit-editor/data/app.ron", r#"{"window":"geometry"}"#);
        migrate_under(&fixture.0).unwrap();
        assert!(fixture.0.join("vEdit/recovery/session.json").exists());
        assert!(fixture.0.join("vEdit/recovery/app.ron").exists());
        assert!(!fixture.0.join("vedit-editor").exists());
        assert!(
            fixture
                .0
                .join("vEdit/migration-backup/previous-recovery/session.json")
                .exists()
        );
    }

    #[test]
    fn refuses_a_live_secondary_window_without_moving_originals() {
        let fixture = Fixture::new();
        fixture.write(
            "Morrow/recovery/windows/w1/window-open",
            std::process::id().to_string(),
        );
        fixture.write("Morrow/recovery/session.json", b"{}");
        assert!(
            migrate_under(&fixture.0)
                .unwrap_err()
                .contains("still owns")
        );
        assert!(fixture.0.join("Morrow/recovery/session.json").exists());
        assert!(!fixture.0.join("vEdit/recovery").exists());
    }

    #[test]
    fn conflicting_profiles_and_corrupt_manifests_never_replace_originals() {
        let fixture = Fixture::new();
        fixture.write("Morrow/recovery/session.json", b"{broken");
        assert!(migrate_under(&fixture.0).is_err());
        assert_eq!(
            fs::read(fixture.0.join("Morrow/recovery/session.json")).unwrap(),
            b"{broken"
        );
        fixture.write("vEdit/recovery/session.json", b"{new");
        assert!(migrate_under(&fixture.0).unwrap_err().contains("both"));
        assert_eq!(
            fs::read(fixture.0.join("vEdit/recovery/session.json")).unwrap(),
            b"{new"
        );
    }

    #[test]
    fn resumes_publication_after_sources_were_partially_archived() {
        let fixture = Fixture::new();
        fixture.write(&format!("vEdit/{STAGE}/session.json"), b"{}");
        fixture.write(&format!("vEdit/{STAGE}/{COMPLETE}"), b"{\"version\":1}");
        fixture.write(&format!("vEdit/{PENDING}"), b"{\"version\":1}");
        fixture.write(
            "vEdit/migration-backup/previous-profile/recovery/session.json",
            b"old bytes",
        );
        fixture.write("morrow-editor/data/app.ron", b"original window data");
        migrate_under(&fixture.0).unwrap();
        assert_eq!(
            fs::read(fixture.0.join("vEdit/recovery/session.json")).unwrap(),
            b"{}"
        );
        assert!(!needs_migration(&fixture.0));
        // Resume once more from the tiny interval after atomic publication.
        fixture.write(&format!("vEdit/{PENDING}"), b"{\"version\":1}");
        migrate_under(&fixture.0).unwrap();
        assert!(!needs_migration(&fixture.0));
    }

    #[test]
    fn task_rename_changes_variable_references_without_changing_other_names() {
        assert_eq!(
            rewrite_command("$MORROW_FILE ${MORROW_ROOT} $env:MORROW_PYTHON"),
            "$VEDIT_FILE ${VEDIT_ROOT} $env:VEDIT_PYTHON"
        );
        assert_eq!(
            rewrite_command("MORROW_FILE $MORROW_FILE_SUFFIX C:/Morrow/file.py"),
            "MORROW_FILE $MORROW_FILE_SUFFIX C:/Morrow/file.py"
        );
    }

    #[cfg(windows)]
    #[test]
    fn recovery_prefix_matches_windows_case_and_separators_not_similar_folder_names() {
        let old = Path::new(r"C:\Users\Viro\AppData\Roaming\Morrow\recovery");
        assert_eq!(
            recovery_relative(
                Path::new("c:/users/VIRO/appdata/roaming/morrow/recovery/draft.txt"),
                old
            ),
            Some(PathBuf::from("draft.txt"))
        );
        assert!(
            recovery_relative(
                Path::new(r"C:\Users\Viro\AppData\Roaming\Morrow\recovery-other\draft.txt"),
                old
            )
            .is_none()
        );
    }
}
