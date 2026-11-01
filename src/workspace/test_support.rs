//! Isolated filesystem fixtures shared by the workspace contract tests.
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct Fixture(pub(super) PathBuf);

impl Fixture {
    pub(super) fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "vedit-workspace-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(super) fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    #[cfg(any(unix, windows))]
    pub(super) fn link_directory(&self, name: &str, target: &Self) -> DirectoryLink {
        let link = self.0.join(name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target.0, &link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&target.0)
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        DirectoryLink(link)
    }
}

#[cfg(any(unix, windows))]
pub(super) struct DirectoryLink(PathBuf);

#[cfg(any(unix, windows))]
impl Drop for DirectoryLink {
    fn drop(&mut self) {
        // Unlink before either fixture recurses, including on a failed assertion.
        #[cfg(unix)]
        let _ = fs::remove_file(&self.0);
        #[cfg(windows)]
        let _ = fs::remove_dir(&self.0);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
