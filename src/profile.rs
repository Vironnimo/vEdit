//! Preserve existing sessions across the Morrow -> vEdit product rename.
//!
//! Reuse an existing profile in place: recovery manifests contain absolute paths,
//! and an older editor can still be running while the new package is installed.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static WINDOW: OnceLock<String> = OnceLock::new();

pub fn valid_window(id: &str) -> bool {
    id == "main"
        || id.strip_prefix('w').is_some_and(|number| {
            !number.starts_with('0')
                && !number.is_empty()
                && number.len() <= 18
                && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub fn select_window(id: String) -> Result<(), String> {
    if !valid_window(&id) {
        return Err("Window must be main or an ID from vedit-cli windows.".into());
    }
    WINDOW
        .set(id)
        .map_err(|_| "Window was already selected.".into())
}

pub fn selected_window() -> Option<&'static str> {
    WINDOW.get().map(String::as_str)
}
pub fn window_id() -> &'static str {
    selected_window().unwrap_or("main")
}

pub fn window_directory(base: &Path, id: &str) -> PathBuf {
    assert!(valid_window(id));
    if id == "main" {
        base.to_owned()
    } else {
        base.join("windows").join(id)
    }
}

pub fn base_recovery_directory() -> PathBuf {
    if let Some(root) = std::env::var_os("VEDIT_PROFILE_DIR") {
        return PathBuf::from(root).join("recovery");
    }
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    recovery_under(&appdata)
}

pub fn external_open_new_window() -> bool {
    read_open_mode(&base_recovery_directory())
}

pub fn read_open_mode(base: &Path) -> bool {
    std::fs::read(base.join("external-open.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<bool>(&bytes).ok())
        .unwrap_or(false)
}

fn existing_or_new(legacy: PathBuf, current: PathBuf) -> PathBuf {
    if legacy.is_dir() { legacy } else { current }
}

fn recovery_under(appdata: &Path) -> PathBuf {
    existing_or_new(appdata.join("Morrow"), appdata.join("vEdit")).join("recovery")
}

pub fn recovery_directory() -> PathBuf {
    window_directory(&base_recovery_directory(), window_id())
}

pub fn persistence_path() -> Option<PathBuf> {
    if window_id() != "main" || std::env::var_os("VEDIT_PROFILE_DIR").is_some() {
        return Some(recovery_directory().join("app.ron"));
    }
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
