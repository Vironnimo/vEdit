//! Whether a process ID still belongs to a running process.
//!
//! vEdit names temporary and recovery files after its process ID. A file whose
//! process is gone can be cleaned up; when the answer is uncertain, for example
//! because access is denied, the process counts as running and its files stay.

/// `true` while a process with this ID runs. Windows can reuse an ID, so a
/// `true` answer may refer to an unrelated process; callers then keep files.
pub fn running(pid: u32) -> bool {
    imp::running(pid)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const STILL_ACTIVE: u32 = 259;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub(super) fn running(pid: u32) -> bool {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // An unknown ID is reported as an invalid parameter. Any other
            // failure, such as access denied, means the process exists.
            return std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER);
        }
        let mut code = 0;
        let queried = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
        unsafe {
            CloseHandle(handle);
        }
        !queried || code == STILL_ACTIVE
    }
}

#[cfg(unix)]
mod imp {
    pub(super) fn running(pid: u32) -> bool {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // Signal 0 checks for existence; EPERM means it exists for another user.
        unsafe { kill(pid, 0) == 0 }
        || std::io::Error::last_os_error().raw_os_error() == Some(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_runs_and_an_exited_child_does_not() {
        assert!(running(std::process::id()));
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // The handle is closed after `wait`; an ID is reused only much later.
        drop(child);
        assert!(!running(pid));
    }
}
