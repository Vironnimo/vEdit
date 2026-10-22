//! One owner per independently persisted editor window.

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    const ERROR_ALREADY_EXISTS: u32 = 183;

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
        acquire_named(&mutex_name())
    }

    pub fn acquire_named(name: &str) -> io::Result<Option<SingleInstanceGuard>> {
        let name: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        // A named mutex is shared by every vEdit process in the current logon session.
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

    fn mutex_name() -> String {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        crate::profile::recovery_directory()
            .to_string_lossy()
            .to_lowercase()
            .hash(&mut hash);
        format!("Local\\vEdit.Window.{:016x}", hash.finish())
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
                "Local\\vEdit.Test.{}.{}",
                std::process::id(),
                suffix
            ))
            .expect("first mutex acquisition");
            assert!(first.is_some());
            let second = acquire_named(&format!(
                "Local\\vEdit.Test.{}.{}",
                std::process::id(),
                suffix
            ))
            .expect("second mutex acquisition");
            assert!(second.is_none());
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

    pub fn acquire_named(_name: &str) -> io::Result<Option<SingleInstanceGuard>> {
        acquire()
    }
}

pub use platform::{SingleInstanceGuard, acquire, acquire_named};
