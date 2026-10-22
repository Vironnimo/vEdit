//! One vEdit profile, with independently persisted editor windows.

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
    appdata.join("vEdit/recovery")
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

pub fn recovery_directory() -> PathBuf {
    window_directory(&base_recovery_directory(), window_id())
}

pub fn persistence_path() -> Option<PathBuf> {
    Some(recovery_directory().join("app.ron"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_window_state_lives_in_its_recovery_directory() {
        assert_eq!(
            persistence_path(),
            Some(recovery_directory().join("app.ron"))
        );
        assert_eq!(
            window_directory(Path::new("profile/recovery"), "w2"),
            Path::new("profile/recovery/windows/w2")
        );
    }
}
