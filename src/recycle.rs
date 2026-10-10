//! Windows Shell recycling. Never fall back to permanent deletion.
use std::path::Path;

#[cfg(windows)]
pub(super) fn delete(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{System::Com::*, UI::Shell::*},
        core::PCWSTR,
    };

    // The shell does not accept canonical verbatim paths. Preserve UNC prefixes
    // when converting, although network paths have no local Recycle Bin.
    let path = crate::paths::display(path);
    if path.starts_with(r"\\") {
        return Err("This network location has no Windows Recycle Bin; nothing was deleted".into());
    }
    let wide: Vec<u16> = std::ffi::OsStr::new(&path)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = (|| -> windows::core::Result<()> {
        // Each filesystem worker has its own thread. COM is initialized and
        // released on that thread, after all Shell interfaces have been dropped.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = Apartment;
        unsafe {
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
            // RECYCLEONDELETE overrides the user's permanent-delete preference.
            // WANTNUKEWARNING prevents silently accepting permanent destruction;
            // NOERRORUI + EARLYFAILURE abort when recycling is unavailable.
            operation.SetOperationFlags(
                FOF_NO_UI
                    | FOF_WANTNUKEWARNING
                    | FOFX_RECYCLEONDELETE
                    | FOFX_ADDUNDORECORD
                    | FOFX_EARLYFAILURE
                    | FOF_NO_CONNECTED_ELEMENTS,
            )?;
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
            operation.DeleteItem(&item, None)?;
            operation.PerformOperations()?;
            if operation.GetAnyOperationsAborted()?.as_bool() {
                return Err(windows::core::Error::from_hresult(
                    COPYENGINE_E_USER_CANCELLED,
                ));
            }
        }
        Ok(())
    })();
    result.map_err(|error| {
        format!(
            "Could not move {path} to the Recycle Bin: {error}. Close applications using this item or check that this drive supports recycling."
        )
    })
}

#[cfg(not(windows))]
pub(super) fn delete(_path: &Path) -> Result<(), String> {
    Err("Recycling requires Windows; nothing was deleted".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{fs, os::windows::ffi::OsStrExt, time::SystemTime};
    use windows::{
        Win32::{System::Com::*, UI::Shell::*},
        core::PCWSTR,
    };

    #[test]
    fn recycle_directory_can_be_restored_with_contents_intact() {
        let name = format!(
            "vedit-recycle-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let parent = std::env::temp_dir();
        let root = parent.join(&name);
        fs::create_dir(&root).unwrap();
        fs::write(root.join("saved.txt"), "recoverable contents").unwrap();
        let target = parent.join(format!("{name}-junction-target"));
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), "junction target stays intact").unwrap();
        super::test_junction(&root.join("linked"), &target);
        delete(&fs::canonicalize(&root).unwrap()).unwrap();
        assert!(!root.exists());
        assert_eq!(
            fs::read_to_string(target.join("keep.txt")).unwrap(),
            "junction target stays intact"
        );

        // Restore only this test's uniquely named item, never empty the bin or
        // operate on any existing user item. This verifies native recoverability.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            {
                let bin: IShellItem =
                    SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None)
                        .unwrap();
                let items: IEnumShellItems = bin.BindToHandler(None, &BHID_EnumItems).unwrap();
                let mut recycled = None;
                loop {
                    let mut item = [None];
                    let mut fetched = 0;
                    items.Next(&mut item, Some(&mut fetched)).unwrap();
                    if fetched == 0 {
                        break;
                    }
                    let item = item[0].take().unwrap();
                    let display = item.GetDisplayName(SIGDN_PARENTRELATIVE).unwrap();
                    let display_name = display.to_string().unwrap();
                    CoTaskMemFree(Some(display.0.cast()));
                    if display_name == name {
                        recycled = Some(item);
                        break;
                    }
                }
                let recycled =
                    recycled.expect("test directory must be recoverable in the Recycle Bin");
                let wide: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
                let destination: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).unwrap();
                let operation: IFileOperation =
                    CoCreateInstance(&FileOperation, None, CLSCTX_ALL).unwrap();
                operation
                    .SetOperationFlags(FOF_NO_UI | FOFX_EARLYFAILURE)
                    .unwrap();
                operation
                    .MoveItem(&recycled, &destination, PCWSTR::null(), None)
                    .unwrap();
                operation.PerformOperations().unwrap();
                assert!(!operation.GetAnyOperationsAborted().unwrap().as_bool());
            }
            CoUninitialize();
        }
        assert_eq!(
            fs::read_to_string(root.join("saved.txt")).unwrap(),
            "recoverable contents"
        );
        assert_eq!(
            fs::read_to_string(target.join("keep.txt")).unwrap(),
            "junction target stays intact"
        );
        fs::remove_dir(root.join("linked")).unwrap();
        fs::remove_file(root.join("saved.txt")).unwrap();
        fs::remove_dir(root).unwrap();
        fs::remove_file(target.join("keep.txt")).unwrap();
        fs::remove_dir(target).unwrap();
    }

    #[test]
    fn network_paths_are_refused_without_trying_permanent_deletion() {
        let error = delete(Path::new(r"\\server\share\document.txt")).unwrap_err();
        assert!(error.contains("nothing was deleted"));
    }
}

#[cfg(all(test, windows))]
pub(super) fn test_junction(link: &Path, target: &Path) {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
