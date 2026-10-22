//! Start the independent GUI without retaining a CLI caller's captured pipes.
use std::{io, path::Path};

#[cfg(windows)]
mod platform {
    use std::{
        ffi::{OsStr, c_void},
        io,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        path::Path,
        ptr,
    };

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;

    #[repr(C)]
    struct StartupInfo {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_count_chars: u32,
        y_count_chars: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        reserved2_size: u16,
        reserved2: *mut u8,
        standard_input: *mut c_void,
        standard_output: *mut c_void,
        standard_error: *mut c_void,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: *mut c_void,
        thread: *mut c_void,
        process_id: u32,
        thread_id: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateProcessW(
            application_name: *const u16,
            command_line: *mut u16,
            process_attributes: *const c_void,
            thread_attributes: *const c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *const c_void,
            current_directory: *const u16,
            startup_info: *const StartupInfo,
            process_information: *mut ProcessInformation,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    }

    pub(crate) struct ApplicationProcess {
        process: OwnedHandle,
    }
    impl ApplicationProcess {
        pub(crate) fn has_exited(&mut self) -> io::Result<bool> {
            // Waiting avoids confusing an actual exit code 259 with STILL_ACTIVE.
            match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }

    pub(super) fn spawn(executable: &Path) -> io::Result<ApplicationProcess> {
        spawn_arguments(executable, &[])
    }

    pub(super) fn spawn_arguments(
        executable: &Path,
        arguments: &[&OsStr],
    ) -> io::Result<ApplicationProcess> {
        let mut application: Vec<u16> = executable.as_os_str().encode_wide().collect();
        if application.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Executable path contains NUL",
            ));
        }
        application.push(0);
        let mut command_line = quoted(executable.as_os_str())?;
        for argument in arguments {
            command_line.push(u16::from(b' '));
            command_line.extend(quoted(argument)?);
        }
        command_line.push(0);
        // Every member is an integer or raw pointer, so zero initialization is valid.
        let mut startup: StartupInfo = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<StartupInfo>() as u32;
        let mut information: ProcessInformation = unsafe { std::mem::zeroed() };
        // std::process::Command on Windows enables general handle inheritance,
        // even with Stdio::null. The caller may own OTHER inheritable capture
        // pipe handles. This independent GUI needs none of them. No std handles
        // are supplied, and detachment also prevents console association.
        let success = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                DETACHED_PROCESS,
                ptr::null(),
                ptr::null(),
                &startup,
                &mut information,
            )
        };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        // CreateProcessW returned owned, valid handles. The GUI continues when
        // these parent-side observation handles close; it is never terminated.
        unsafe { CloseHandle(information.thread) };
        let process = unsafe { OwnedHandle::from_raw_handle(information.process) };
        Ok(ApplicationProcess { process })
    }

    fn quoted(value: &OsStr) -> io::Result<Vec<u16>> {
        let mut result = vec![u16::from(b'"')];
        let mut slashes = 0;
        for character in value.encode_wide() {
            if character == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Argument contains NUL",
                ));
            }
            if character == u16::from(b'\\') {
                slashes += 1;
                continue;
            }
            let count = if character == u16::from(b'"') {
                slashes * 2 + 1
            } else {
                slashes
            };
            result.extend(std::iter::repeat_n(u16::from(b'\\'), count));
            result.push(character);
            slashes = 0;
        }
        result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
        result.push(u16::from(b'"'));
        Ok(result)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::windows::process::CommandExt;
        use std::{
            process::{Command, Stdio},
            thread,
            time::{Duration, Instant},
        };

        #[repr(C)]
        struct SecurityAttributes {
            length: u32,
            descriptor: *mut c_void,
            inherit: i32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreatePipe(
                read: *mut *mut c_void,
                write: *mut *mut c_void,
                attributes: *const SecurityAttributes,
                size: u32,
            ) -> i32;
            fn PeekNamedPipe(
                pipe: *mut c_void,
                buffer: *mut c_void,
                buffer_size: u32,
                read: *mut u32,
                available: *mut u32,
                remaining: *mut u32,
            ) -> i32;
        }
        fn inheritable_pipe() -> (OwnedHandle, OwnedHandle) {
            let attributes = SecurityAttributes {
                length: std::mem::size_of::<SecurityAttributes>() as u32,
                descriptor: ptr::null_mut(),
                inherit: 1,
            };
            let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
            assert_ne!(
                unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) },
                0,
                "{}",
                io::Error::last_os_error()
            );
            unsafe {
                (
                    OwnedHandle::from_raw_handle(read),
                    OwnedHandle::from_raw_handle(write),
                )
            }
        }
        fn broken_pipe(read: &OwnedHandle) -> bool {
            let success = unsafe {
                PeekNamedPipe(
                    read.as_raw_handle(),
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if success == 0 {
                assert_eq!(io::Error::last_os_error().raw_os_error(), Some(109));
                true
            } else {
                false
            }
        }
        #[test]
        fn win32_structures_and_command_line_match_abi() {
            assert_eq!(
                std::mem::size_of::<StartupInfo>(),
                if cfg!(target_pointer_width = "64") {
                    104
                } else {
                    68
                }
            );
            assert_eq!(
                std::mem::size_of::<ProcessInformation>(),
                if cfg!(target_pointer_width = "64") {
                    24
                } else {
                    16
                }
            );
            assert_eq!(
                String::from_utf16(&quoted(OsStr::new("C:\\space name\\vedit.exe")).unwrap())
                    .unwrap(),
                "\"C:\\space name\\vedit.exe\""
            );
            assert_eq!(
                String::from_utf16(&quoted(OsStr::new("trailing\\")).unwrap()).unwrap(),
                "\"trailing\\\\\""
            );
            assert!(quoted(OsStr::new("nul\0value")).is_err());
        }
        #[test]
        #[ignore = "Launches hidden test helpers; run isolated with --ignored --test-threads=1"]
        fn detached_start_does_not_inherit_unrelated_capture_pipe() {
            // No vEdit window is opened. A hidden, silent PowerShell helper
            // remains alive while the test observes the actual pipe lifetime.
            let executable = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let arguments = [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[System.Threading.Thread]::Sleep(2200)",
            ];
            let (old_read, old_write) = inheritable_pipe();
            let mut old_child = Command::new(&executable)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .spawn()
                .unwrap();
            drop(old_write);
            assert!(old_child.try_wait().unwrap().is_none());
            assert!(
                !broken_pipe(&old_read),
                "Negative control should reproduce std::Command's inherited pipe"
            );

            let (read, write) = inheritable_pipe();
            let arguments: Vec<_> = arguments.iter().map(OsStr::new).collect();
            let mut child = spawn_arguments(&executable, &arguments).unwrap();
            drop(write);
            assert!(
                !child.has_exited().unwrap(),
                "Native helper must remain alive during the EOF assertion"
            );
            assert!(
                broken_pipe(&read),
                "Captured pipe must reach EOF while the independent child is alive"
            );
            assert!(!child.has_exited().unwrap());
            let deadline = Instant::now() + Duration::from_secs(8);
            while !child.has_exited().unwrap() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            assert!(child.has_exited().unwrap(), "Test helper did not exit");
            assert!(old_child.wait().unwrap().success());
            assert!(
                broken_pipe(&old_read),
                "Old capture pipe reaches EOF only when the inherited child exits"
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::{
        io,
        path::Path,
        process::{Child, Command, Stdio},
    };
    pub(crate) struct ApplicationProcess {
        process: Child,
    }
    impl ApplicationProcess {
        pub(crate) fn has_exited(&mut self) -> io::Result<bool> {
            self.process.try_wait().map(|status| status.is_some())
        }
    }
    pub(super) fn spawn_window(executable: &Path, id: &str) -> io::Result<ApplicationProcess> {
        Command::new(executable)
            .args(["--window", id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|process| ApplicationProcess { process })
    }
    pub(super) fn spawn(executable: &Path) -> io::Result<ApplicationProcess> {
        Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|process| ApplicationProcess { process })
    }
}

pub(super) use platform::ApplicationProcess;
pub(super) fn spawn(executable: &Path) -> io::Result<ApplicationProcess> {
    platform::spawn(executable)
}

pub(super) fn spawn_window(executable: &Path, id: &str) -> io::Result<ApplicationProcess> {
    #[cfg(windows)]
    {
        platform::spawn_arguments(
            executable,
            &[std::ffi::OsStr::new("--window"), std::ffi::OsStr::new(id)],
        )
    }
    #[cfg(not(windows))]
    {
        platform::spawn_window(executable, id)
    }
}
