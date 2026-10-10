//! Preserve existing sessions across the Morrow -> vEdit product rename.
//!
//! Reuse an existing profile in place: recovery manifests contain absolute paths,
//! and an older editor can still be running while the new package is installed.

use std::path::{Path, PathBuf};

fn existing_or_new(legacy: PathBuf, current: PathBuf) -> PathBuf {
    if legacy.is_dir() { legacy } else { current }
}

fn recovery_under(appdata: &Path) -> PathBuf {
    existing_or_new(appdata.join("Morrow"), appdata.join("vEdit")).join("recovery")
}

pub fn recovery_directory() -> PathBuf {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    recovery_under(&appdata)
}

pub fn persistence_path() -> Option<PathBuf> {
    // The explicit eframe override takes a file, despite its folder-oriented
    // field documentation. Fresh profiles use eframe's default app-id path.
    let legacy = eframe::storage_dir("morrow-editor")?;
    persistence_override(legacy)
}

fn persistence_override(legacy: PathBuf) -> Option<PathBuf> {
    legacy
        .join("app.ron")
        .is_file()
        .then(|| legacy.join("app.ron"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_keeps_recovery_paths_and_existing_window_settings() {
        let root = std::env::temp_dir().join(format!("vedit-profile-{}", std::process::id()));
        let legacy = root.join("Morrow");
        std::fs::create_dir_all(legacy.join("recovery")).unwrap();
        let draft = legacy.join("recovery/draft.txt");
        std::fs::write(&draft, "unsaved draft").unwrap();
        // Even a pre-created new directory must not hide the existing session.
        std::fs::create_dir_all(root.join("vEdit")).unwrap();
        assert_eq!(recovery_under(&root), legacy.join("recovery"));
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), "unsaved draft");
        let settings = root.join("morrow-editor/data");
        std::fs::create_dir_all(&settings).unwrap();
        let state_file = settings.join("app.ron");
        std::fs::write(&state_file, "persisted window settings").unwrap();
        let restored = persistence_override(settings).unwrap();
        assert_eq!(restored, state_file);
        assert_eq!(
            std::fs::read_to_string(restored).unwrap(),
            "persisted window settings"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fresh_profile_uses_vedit_names() {
        let root = Path::new("nonexistent-vedit-profile-fixture");
        assert_eq!(recovery_under(root), root.join("vEdit/recovery"));
        assert_eq!(persistence_override(root.join("morrow-editor/data")), None);
    }
}
