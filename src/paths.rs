//! Human-readable paths. Filesystem operations continue to use the original paths.

use std::path::Path;

pub fn display(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        if rest
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
        {
            return format!(r"\\{}", &rest[4..]);
        }
        let bytes = rest.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
        {
            return rest.to_owned();
        }
    }
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_drive_path_is_readable_without_changing_identity() {
        let original = Path::new(r"\\?\C:\Development\Pläne\main.rs");
        assert_eq!(display(original), r"C:\Development\Pläne\main.rs");
        assert_eq!(
            original.to_str().unwrap(),
            r"\\?\C:\Development\Pläne\main.rs"
        );
    }

    #[test]
    fn canonical_network_path_keeps_its_network_root() {
        assert_eq!(
            display(Path::new(r"\\?\UNC\server\share\plan.md")),
            r"\\server\share\plan.md"
        );
        assert_eq!(
            display(Path::new(r"\\?\unc\server\share")),
            r"\\server\share"
        );
    }

    #[test]
    fn relative_and_special_device_paths_are_unchanged() {
        for path in [
            r"src\main.rs",
            r"C:\projects\main.rs",
            r"\\server\share",
            r"\\?\Volume{1234}\file",
            r"\\.\pipe\morrow",
            "/tmp/notes.md",
        ] {
            assert_eq!(display(Path::new(path)), path);
        }
    }
}
