//! Keep Windows launches of Morrow in one editor window.

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use std::thread;
    use std::time::Duration;

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn GetLastError() -> u32;
        fn SetLastError(error: u32);
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut c_void;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
    }

    pub struct SingleInstanceGuard {
        handle: *mut c_void,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            // The handle is owned by this guard and closed exactly once.
            unsafe { CloseHandle(self.handle) };
        }
    }

    pub fn acquire() -> io::Result<Option<SingleInstanceGuard>> {
        let name: Vec<u16> = std::ffi::OsStr::new("Local\\Morrow.Editor.SingleInstance")
            .encode_wide()
            .chain(Some(0))
            .collect();
        // A named mutex is shared by every Morrow process in the current logon session.
        unsafe { SetLastError(0) };
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            activate_existing_window();
            Ok(None)
        } else {
            Ok(Some(SingleInstanceGuard { handle }))
        }
    }

    fn activate_existing_window() {
        let title: Vec<u16> = std::ffi::OsStr::new("Morrow")
            .encode_wide()
            .chain(Some(0))
            .collect();
        // The first process can own the mutex before eframe has created its window.
        for _ in 0..60 {
            let window = unsafe { FindWindowW(ptr::null(), title.as_ptr()) };
            if !window.is_null() {
                unsafe {
                    ShowWindow(window, SW_RESTORE);
                    SetForegroundWindow(window);
                }
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU64, Ordering};

        #[test]
        fn named_mutex_allows_one_owner() {
            static TEST_ID: AtomicU64 = AtomicU64::new(1);
            let suffix = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let first = acquire_named(&format!(
                "Local\\Morrow.Test.{}.{}",
                std::process::id(),
                suffix
            ))
            .expect("first mutex acquisition");
            assert!(first.is_some());
            let second = acquire_named(&format!(
                "Local\\Morrow.Test.{}.{}",
                std::process::id(),
                suffix
            ))
            .expect("second mutex acquisition");
            assert!(second.is_none());
        }

        fn acquire_named(name: &str) -> io::Result<Option<SingleInstanceGuard>> {
            let name: Vec<u16> = std::ffi::OsStr::new(name)
                .encode_wide()
                .chain(Some(0))
                .collect();
            unsafe { SetLastError(0) };
            let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe { CloseHandle(handle) };
                Ok(None)
            } else {
                Ok(Some(SingleInstanceGuard { handle }))
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    pub struct SingleInstanceGuard;

    pub fn acquire() -> io::Result<Option<SingleInstanceGuard>> {
        Ok(Some(SingleInstanceGuard))
    }
}

pub use platform::acquire;
