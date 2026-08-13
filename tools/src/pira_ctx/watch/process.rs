use std::process::{Child, Command};

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
        #[cfg(windows)]
        let job = match create_kill_job(&child) {
            Ok(job) => job,
            Err(error) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("isolate {label} process tree: {error}"));
            }
        };
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
            cleaned: false,
        })
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

#[cfg(not(unix))]
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
