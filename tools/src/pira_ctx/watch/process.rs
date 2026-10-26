use std::io;
use std::process::{Child, Command, Stdio};

/// A child isolated so its descendants can be terminated as one attempt.
pub struct ProcessTree {
    pub child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    cleaned: bool,
}

impl ProcessTree {
    pub fn spawn(command: &mut Command, label: &str) -> Result<Self, String> {
        configure(command);
        let child = command
            .spawn()
            .map_err(|error| format!("start {label}: {error}"))?;
        Self::isolate(child, label)
    }

    pub fn spawn_capture(
        cmd: &[String],
        redirected: Option<crate::model::StreamKind>,
    ) -> Result<Self, String> {
        let mut command = Command::new(&cmd[0]);
        command
            .args(&cmd[1..])
            .stdin(Stdio::inherit())
            .stdout(if redirected == Some(crate::model::StreamKind::Stdout) {
                Stdio::inherit()
            } else {
                Stdio::piped()
            })
            .stderr(if redirected == Some(crate::model::StreamKind::Stderr) {
                Stdio::inherit()
            } else {
                Stdio::piped()
            });
        configure(&mut command);
        let child = command.spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!("__EXIT127__ command not found: {}", cmd[0])
            } else if error.kind() == io::ErrorKind::PermissionDenied {
                format!("__EXIT126__ permission denied/not executable: {}", cmd[0])
            } else {
                format!("failed to spawn {}: {error}", cmd[0])
            }
        })?;
        Self::isolate(child, "capture")
    }

    fn isolate(child: Child, _label: &str) -> Result<Self, String> {
        #[cfg(windows)]
        let job = match create_kill_job(&child) {
            Ok(job) => job,
            Err(error) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("isolate {_label} process tree: {error}"));
            }
        };
        let tree = Self {
            child,
            #[cfg(windows)]
            job,
            cleaned: false,
        };
        #[cfg(windows)]
        {
            let mut tree = tree;
            if let Err(error) = resume_primary(&tree.child) {
                tree.terminate_tree();
                let _ = tree.child.kill();
                let _ = tree.child.wait();
                return Err(format!("resume isolated {_label}: {error}"));
            }
            return Ok(tree);
        }
        #[cfg(not(windows))]
        Ok(tree)
    }

    /// Ends any descendants that retained stdio after the direct child exited.
    pub fn terminate_tree(&mut self) {
        if self.cleaned {
            return;
        }
        #[cfg(unix)]
        {
            let group = self.child.id().min(i32::MAX as u32) as i32;
            // SAFETY: every attempt is made leader of a new process group before spawn.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            // SAFETY: `job` is a live handle owned by this value.
            unsafe {
                TerminateJobObject(self.job, 1);
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self.child.kill();
        }
        self.cleaned = true;
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate_tree();
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            // SAFETY: `job` is owned by this value and closed exactly once.
            unsafe {
                CloseHandle(self.job);
            }
        }
    }
}

#[cfg(unix)]
fn configure(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure(_command: &mut Command) {}

#[cfg(windows)]
fn create_kill_job(child: &Child) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // SAFETY: APIs receive initialized values of the documented sizes. Failure paths close `job`.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            let error = std::io::Error::last_os_error().to_string();
            CloseHandle(job);
            return Err(error);
        }
        let process = child.as_raw_handle() as HANDLE;
        if AssignProcessToJobObject(job, process) == 0 {
            let error = std::io::Error::last_os_error().to_string();
            CloseHandle(job);
            return Err(error);
        }
        Ok(job)
    }
}

// Command's stable API does not expose the primary thread handle. A newly created,
// suspended process must have exactly one thread; never guess or resume before Job assignment.
#[cfg(windows)]
fn primary_thread(child: &Child) -> Result<std::os::windows::io::OwnedHandle, String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::Threading::*;
    // SAFETY: owned handles close on every return, and entry has the documented size.
    unsafe {
        let raw = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().to_string());
        }
        let snapshot = OwnedHandle::from_raw_handle(raw);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut found = None;
        let mut ok = Thread32First(snapshot.as_raw_handle(), &mut entry);
        while ok != 0 {
            if entry.th32OwnerProcessID == child.id() {
                if found.replace(entry.th32ThreadID).is_some() {
                    return Err(
                        "suspended child has multiple threads; refusing ambiguous resume".into(),
                    );
                }
            }
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            ok = Thread32Next(snapshot.as_raw_handle(), &mut entry);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
            return Err(error.to_string());
        }
        let id = found.ok_or("suspended child primary thread not found")?;
        let raw = OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
            0,
            id,
        );
        if raw.is_null() {
            return Err(io::Error::last_os_error().to_string());
        }
        let thread = OwnedHandle::from_raw_handle(raw);
        if GetProcessIdOfThread(thread.as_raw_handle()) != child.id() {
            return Err("primary thread owner changed".into());
        }
        Ok(thread)
    }
}

#[cfg(windows)]
fn resume_primary(child: &Child) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    let thread = primary_thread(child)?;
    // SAFETY: the verified child thread is owned, and its process is already in our Job.
    let previous =
        unsafe { windows_sys::Win32::System::Threading::ResumeThread(thread.as_raw_handle()) };
    if previous != 1 {
        return Err(format!(
            "unexpected primary thread suspend count {previous}: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    #[test]
    fn launch_is_suspended_until_job_assignment() {
        use windows_sys::Win32::System::Threading::{ResumeThread, SuspendThread};
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "exit 0"]);
        configure(&mut command);
        let mut child = command.spawn().unwrap();
        let thread = primary_thread(&child).unwrap_or_else(|error| {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{error}");
        });
        // Observe the kernel suspend count, not timing or an absent output file.
        let suspended = unsafe { SuspendThread(thread.as_raw_handle()) };
        let resumed = if suspended != u32::MAX {
            unsafe { ResumeThread(thread.as_raw_handle()) }
        } else {
            u32::MAX
        };
        if (suspended, resumed) != (1, 2) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("expected suspended primary thread, observed {suspended}/{resumed}");
        }
        let mut tree = ProcessTree::isolate(child, "test").unwrap();
        let mut member = 0;
        unsafe {
            assert_ne!(
                windows_sys::Win32::System::JobObjects::IsProcessInJob(
                    tree.child.as_raw_handle(),
                    tree.job,
                    &mut member
                ),
                0
            );
        }
        assert_ne!(member, 0);
        assert!(tree.child.wait().unwrap().success());
    }
}
