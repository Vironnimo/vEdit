//! Task-owned process trees always terminate with their guard.

use super::*;

#[cfg(windows)]
pub(super) fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

pub(super) struct ProcessGuard {
    pub(super) child: Child,
    tree: ProcessTree,
}

impl ProcessGuard {
    pub(super) fn new(child: Child) -> Self {
        Self {
            tree: ProcessTree::new(&child),
            child,
        }
    }

    pub(super) fn terminate(&mut self) {
        self.tree.terminate(&mut self.child);
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
        let _ = self.child.wait();
    }
}

struct ProcessTree {
    #[cfg(windows)]
    job: Option<windows_job::Job>,
    #[cfg(unix)]
    id: u32,
    terminated: bool,
}

impl ProcessTree {
    fn new(child: &Child) -> Self {
        Self {
            #[cfg(windows)]
            job: windows_job::Job::assign(child),
            #[cfg(unix)]
            id: child.id(),
            terminated: false,
        }
    }

    fn terminate(&mut self, child: &mut Child) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        #[cfg(windows)]
        {
            if let Some(job) = self.job.take() {
                // Closing the job kills every descendant, including children
                // whose parent shell has already exited.
                drop(job);
            } else if matches!(child.try_wait(), Ok(None)) {
                let mut killer = Command::new("taskkill.exe");
                killer.args(["/PID", &child.id().to_string(), "/T", "/F"]);
                killer
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                hide_window(&mut killer);
                let _ = killer.status();
            }
        }
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.id)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: u32, data: *const c_void, size: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub(super) struct Job(Handle);

    impl Job {
        pub(super) fn assign(child: &Child) -> Option<Self> {
            // All handles stay on this worker. The pointers refer to live,
            // correctly laid out Win32 structures for the duration of calls.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                let mut limits = ExtendedLimits::default();
                limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                if SetInformationJobObject(
                    handle,
                    9, // JobObjectExtendedLimitInformation
                    (&limits as *const ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, child.as_raw_handle().cast()) == 0
                {
                    return None;
                }
                Some(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
