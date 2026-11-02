use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// A child isolated so its descendants can be terminated as one attempt.
pub struct ProcessTree {
    pub child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    cleaned: bool,
    #[cfg(unix)]
    group_pending: bool,
}

impl ProcessTree {
    pub fn spawn(command: &mut Command, label: &str) -> Result<Self, String> {
        crate::shutdown::check()?;
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
        crate::shutdown::check()?;
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
                if let Err(cleanup) = wait_owned_child(&mut child) {
                    crate::util::diagnostic_line(&format!(
                        "pira_ctx: cleanup isolation failure: {cleanup}"
                    ));
                }
                return Err(format!("isolate {_label} process tree: {error}"));
            }
        };
        let tree = Self {
            child,
            #[cfg(windows)]
            job,
            cleaned: false,
            #[cfg(unix)]
            group_pending: true,
        };
        #[cfg(windows)]
        {
            let mut tree = tree;
            if let Err(error) = resume_primary(&tree.child) {
                if let Err(cleanup) = tree.terminate_and_wait() {
                    crate::util::diagnostic_line(&format!(
                        "pira_ctx: cleanup resume failure: {cleanup}"
                    ));
                }
                return Err(format!("resume isolated {_label}: {error}"));
            }
            return Ok(tree);
        }
        #[cfg(not(windows))]
        Ok(tree)
    }

    /// Observe completion without releasing the Unix PID before group cleanup.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        #[cfg(unix)]
        if self.group_pending {
            // SAFETY: waitid observes only our direct child and WNOWAIT pins its
            // PID even after exit. No other code may reap it before group cleanup.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    // An external reaper invalidates group ownership; never signal it.
                    self.group_pending = false;
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(None);
                }
                return Err(error);
            }
            if unsafe { info.si_pid() } == 0 {
                return Ok(None);
            }
            self.terminate_tree();
        }
        self.child.try_wait()
    }

    /// Ends descendants before reaping the direct child and releasing its PID.
    pub fn terminate_tree(&mut self) {
        if self.cleaned {
            return;
        }
        #[allow(unused_mut)]
        let mut group_cleaned = true;
        #[cfg(unix)]
        if std::mem::take(&mut self.group_pending) {
            let group = self.child.id().min(i32::MAX as u32) as i32;
            // SAFETY: spawn made this child a group leader. Its unreaped PID pins
            // the original group ID, even if the child moved to another group.
            // Never retry the group signal after a subsequent try_wait reaps it.
            let result = unsafe { libc::kill(-group, libc::SIGKILL) };
            if result != 0 {
                let error = io::Error::last_os_error();
                if !group_already_exited(group, &error) {
                    group_cleaned = false;
                    crate::util::diagnostic_line(&format!(
                        "pira_ctx: terminate original process group: {error}"
                    ));
                }
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            // SAFETY: `job` is a live handle owned by this value.
            if unsafe { TerminateJobObject(self.job, 1) } == 0 {
                group_cleaned = false;
                crate::util::diagnostic_line(&format!(
                    "pira_ctx: terminate owned job: {}",
                    io::Error::last_os_error()
                ));
            }
        }
        // A direct child may have left its original group. Kill only that owned
        // child, never its new group (which can contain unrelated processes).
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = self.child.kill() {
                    crate::util::diagnostic_line(&format!(
                        "pira_ctx: terminate owned child: {error}"
                    ));
                    return;
                }
            }
            Err(error) => {
                crate::util::diagnostic_line(&format!("pira_ctx: inspect owned child: {error}"));
                return;
            }
        }
        self.cleaned = group_cleaned;
    }

    /// Kill containment and the owned child, bounding the reap wait to two seconds.
    pub fn terminate_and_wait(&mut self) -> Result<ExitStatus, String> {
        self.terminate_tree();
        wait_owned_child(&mut self.child)
    }
}

#[cfg(unix)]
fn group_already_exited(group: i32, error: &io::Error) -> bool {
    if error.raw_os_error() == Some(libc::ESRCH) {
        return true;
    }
    #[cfg(target_os = "macos")]
    if error.raw_os_error() == Some(libc::EPERM) {
        return zombie_only_group(group);
    }
    let _ = group;
    false
}

#[cfg(target_os = "macos")]
fn zombie_only_group(group: i32) -> bool {
    // Darwin includes zombies in the group list, but kill may return EPERM when
    // no live member remains. Inspect only; never signal an enumerated PID.
    // Unknown/incomplete observations retain the original failure diagnostic.
    // SAFETY: null/zero queries size; subsequent buffers have the supplied size.
    let count = unsafe { libc::proc_listpgrppids(group, std::ptr::null_mut(), 0) };
    if count <= 0 || count > 262_144 {
        return false;
    }
    let mut pids = vec![0_i32; count as usize + 32];
    let capacity = size_of_val(pids.as_slice()) as i32;
    // proc_listpgrppids returns a PID count, although its buffer size is in bytes.
    let count = unsafe { libc::proc_listpgrppids(group, pids.as_mut_ptr().cast(), capacity) };
    if count <= 0 || count as usize >= pids.len() {
        return false;
    }
    pids[..count as usize].iter().all(|&pid| {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: initialized native struct with its exact size; arg=1 explicitly
        // includes zombies in PROC_PIDTBSDINFO. The PID is observed, not signalled.
        let bytes = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                1,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        bytes == size && info.pbi_pgid == group as u32 && info.pbi_status == libc::SZOMB
    })
}

fn wait_owned_child(child: &mut Child) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("reap owned child: {e}"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err("owned child did not exit within 2s of termination".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if let Err(error) = self.terminate_and_wait() {
            crate::util::diagnostic_line(&format!("pira_ctx: cleanup: {error}"));
        }
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

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn live_group_permission_errors_are_not_silenced() {
        let mut command = Command::new("/bin/sleep");
        command.arg("1").current_dir(std::env::temp_dir());
        let tree = ProcessTree::spawn(&mut command, "permission classification fixture").unwrap();
        let group = tree.child.id() as i32;
        assert!(!group_already_exited(
            group,
            &io::Error::from_raw_os_error(libc::EPERM)
        ));
        assert!(!group_already_exited(
            group,
            &io::Error::from_raw_os_error(libc::EACCES)
        ));
        // Drop terminates only this unreaped owned group and waits within the bound.
    }
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
