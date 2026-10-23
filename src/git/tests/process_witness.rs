//! Observe the actual Windows writer, rather than sleeping past a late write.

use super::*;
use std::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    fn TerminateProcess(handle: *mut c_void, exit_code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

/// The Git-for-Windows shell already runs the hooks under test. Its procfs
/// exposes the Windows PID of this exact writing subshell; `$$` alone is an
/// MSYS PID and would not identify the process to OpenProcess. Shell builtins
/// publish the PID, so readiness needs no PowerShell startup inside a timeout.
pub(super) fn writer(name: &str, marker: &str) -> String {
    format!(
        "(\nread -r native_pid < /proc/$BASHPID/winpid || exit 1\nprintf '%s\\n' \"$native_pid\" > .git/{name}.pid\nwhile [ ! -e .git/{name}.release ]; do sleep 0.02; done\nprintf 'alive' > {marker}\n)"
    )
}

pub(super) struct ProcessWitness {
    handle: *mut c_void,
    release: PathBuf,
}

impl ProcessWitness {
    pub(super) fn open(root: &Path, name: &str) -> Self {
        let path = root.join(format!(".git/{name}.pid"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let id = loop {
            if let Ok(value) = fs::read_to_string(&path)
                && let Ok(id) = value.trim().parse::<u32>()
            {
                break id;
            }
            assert!(
                Instant::now() < deadline,
                "writer never became ready: {path:?}"
            );
            thread::sleep(Duration::from_millis(5));
        };
        // The writer is held at its release gate, so it cannot finish and
        // recycle this PID before we acquire a stable process-object handle.
        let handle = unsafe { OpenProcess(0x0010_0001, 0, id) }; // SYNCHRONIZE | TERMINATE
        assert!(
            !handle.is_null(),
            "writer exited before its handle was opened"
        );
        let witness = Self {
            handle,
            release: root.join(format!(".git/{name}.release")),
        };
        witness.assert_running();
        witness
    }

    pub(super) fn assert_running(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle, 0) },
            258,
            "the exact descendant writer was not alive"
        );
    }

    pub(super) fn release(&self) {
        fs::write(&self.release, "continue").unwrap();
    }

    pub(super) fn assert_stopped(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle, 5_000) },
            0,
            "the exact descendant writer survived Git shutdown"
        );
    }
}

impl Drop for ProcessWitness {
    fn drop(&mut self) {
        // Failure cleanup only touches the helper represented by our handle.
        // Releasing it also lets its short-lived sleep child exit naturally.
        let _ = fs::write(&self.release, "continue");
        unsafe {
            if WaitForSingleObject(self.handle, 5_000) != 0 {
                TerminateProcess(self.handle, 1);
                WaitForSingleObject(self.handle, 5_000);
            }
            CloseHandle(self.handle);
        }
    }
}
