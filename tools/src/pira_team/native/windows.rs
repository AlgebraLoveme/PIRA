//! Small Win32/NT adapter; handles, not persisted PIDs, own termination and IPC.
use super::Identity;
use std::{
    ffi::c_void,
    io, mem,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::PathBuf,
    ptr,
    time::{Duration, Instant},
};
pub type Raw = *mut c_void;
pub const CREATE_SUSPENDED: u32 = 0x00000004;
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
#[repr(transparent)]
pub struct Handle(pub Raw);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Handle {
    fn checked(raw: Raw) -> io::Result<Self> {
        if raw.is_null() || raw as isize == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
#[repr(C)]
struct FileTime {
    low: u32,
    high: u32,
}
#[repr(C)]
struct Security {
    length: u32,
    descriptor: Raw,
    inherit: i32,
}
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Raw,
}
#[repr(C)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    minimum: usize,
    maximum: usize,
    processes: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
struct JobLimits {
    basic: BasicLimits,
    counters: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    name: [u16; 260],
}
fn parent_pid(pid: u32) -> io::Result<u32> {
    let snapshot = Handle::checked(unsafe { CreateToolhelp32Snapshot(2, 0) })?;
    let mut entry: ProcessEntry = unsafe { mem::zeroed() };
    entry.size = mem::size_of::<ProcessEntry>() as u32;
    let mut present = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    while present != 0 {
        if entry.pid == pid {
            return Ok(entry.parent);
        }
        present = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "process left the native snapshot",
    ))
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CloseHandle(handle: Raw) -> i32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        extension: *const u16,
        length: u32,
        buffer: *mut u16,
        part: *mut *mut u16,
    ) -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Raw;
    fn GetExitCodeProcess(process: Raw, code: *mut u32) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Raw;
    fn Process32FirstW(snapshot: Raw, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: Raw, entry: *mut ProcessEntry) -> i32;
    fn GetCurrentProcess() -> Raw;
    fn GetProcessTimes(
        handle: Raw,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn QueryFullProcessImageNameW(handle: Raw, flags: u32, name: *mut u16, length: *mut u32)
    -> i32;
    fn WaitForSingleObject(handle: Raw, milliseconds: u32) -> u32;
    fn CreateJobObjectW(security: Raw, name: *const u16) -> Raw;
    fn SetInformationJobObject(handle: Raw, class: i32, data: Raw, length: u32) -> i32;
    fn AssignProcessToJobObject(job: Raw, process: Raw) -> i32;
    fn TerminateJobObject(job: Raw, status: u32) -> i32;
    fn OpenJobObjectW(access: u32, inherit: i32, name: *const u16) -> Raw;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn QueryInformationJobObject(
        job: Raw,
        class: i32,
        information: Raw,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    fn IsProcessInJob(process: Raw, job: Raw, result: *mut i32) -> i32;
    fn CreateNamedPipeW(
        name: *const u16,
        open: u32,
        mode: u32,
        instances: u32,
        output: u32,
        input: u32,
        timeout: u32,
        security: *const Security,
    ) -> Raw;
    fn ConnectNamedPipe(pipe: Raw, overlapped: *mut Overlapped) -> i32;
    fn DisconnectNamedPipe(pipe: Raw) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: Raw,
        disposition: u32,
        flags: u32,
        template: Raw,
    ) -> Raw;
    fn WaitNamedPipeW(name: *const u16, milliseconds: u32) -> i32;
    fn GetNamedPipeClientProcessId(pipe: Raw, pid: *mut u32) -> i32;
    fn GetNamedPipeServerProcessId(pipe: Raw, pid: *mut u32) -> i32;
    fn CreateEventW(security: Raw, manual: i32, initial: i32, name: *const u16) -> Raw;
    fn ReadFile(
        handle: Raw,
        data: Raw,
        length: u32,
        count: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn WriteFile(
        handle: Raw,
        data: *const c_void,
        length: u32,
        count: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetOverlappedResult(
        handle: Raw,
        overlapped: *mut Overlapped,
        count: *mut u32,
        wait: i32,
    ) -> i32;
    fn CancelIoEx(handle: Raw, overlapped: *const Overlapped) -> i32;
    fn LocalFree(value: Raw) -> Raw;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Raw, access: u32, token: *mut Raw) -> i32;
    fn GetTokenInformation(
        token: Raw,
        class: i32,
        data: Raw,
        length: u32,
        required: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: Raw, string: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        descriptor: *mut Raw,
        length: *mut u32,
    ) -> i32;
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process: Raw) -> i32;
}
fn checked(ok: i32) -> io::Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn open_process(pid: u32) -> io::Result<Handle> {
    Handle::checked(unsafe { OpenProcess(0x1000, 0, pid) })
}

pub fn identity(pid: u32) -> io::Result<Identity> {
    let handle = open_process(pid)?;
    let parent = parent_pid(pid)?;
    let mut created: FileTime = unsafe { mem::zeroed() };
    let mut unused: [FileTime; 3] = unsafe { mem::zeroed() };
    checked(unsafe {
        GetProcessTimes(
            handle.0,
            &mut created,
            &mut unused[0],
            &mut unused[1],
            &mut unused[2],
        )
    })?;
    let mut executable = vec![0_u16; 32768];
    let mut length = executable.len() as u32;
    checked(unsafe {
        QueryFullProcessImageNameW(handle.0, 0, executable.as_mut_ptr(), &mut length)
    })?;
    use std::os::windows::ffi::OsStringExt;
    let mut exit_code = 0;
    checked(unsafe { GetExitCodeProcess(handle.0, &mut exit_code) })?;
    Ok(Identity {
        pid,
        parent,
        birth: ((created.high as u64) << 32) | created.low as u64,
        executable: PathBuf::from(std::ffi::OsString::from_wide(
            &executable[..length as usize],
        )),
        alive: exit_code == 259,
    })
}

pub struct Job {
    handle: Handle,
    pub name: String,
}
impl Job {
    pub fn assign(child: &std::process::Child) -> io::Result<Self> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| io::Error::other("job entropy unavailable"))?;
        let name = format!(
            "Local\\pira-team-{}-{}",
            std::process::id(),
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let handle =
            Handle::checked(unsafe { CreateJobObjectW(ptr::null_mut(), wide(&name).as_ptr()) })?;
        let mut limits: JobLimits = unsafe { mem::zeroed() };
        limits.basic.flags = 0x2000 | 0x0800; // KILL_ON_JOB_CLOSE, explicit BREAKAWAY_OK.
        checked(unsafe {
            SetInformationJobObject(
                handle.0,
                9,
                (&mut limits as *mut JobLimits).cast(),
                mem::size_of::<JobLimits>() as u32,
            )
        })?;
        checked(unsafe { AssignProcessToJobObject(handle.0, child.as_raw_handle()) })?;
        if unsafe { NtResumeProcess(child.as_raw_handle()) } < 0 {
            return Err(io::Error::other("cannot resume owned backend"));
        }
        Ok(Self { handle, name })
    }
    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.handle.0, 1);
        }
    }
}
pub fn in_job(name: &str) -> io::Result<bool> {
    let job = Handle::checked(unsafe { OpenJobObjectW(4, 0, wide(name).as_ptr()) })?;
    let mut result = 0;
    checked(unsafe { IsProcessInJob(GetCurrentProcess(), job.0, &mut result) })?;
    Ok(result != 0)
}

/// Return the kernel session that owns the process namespace.
pub fn session_id(pid: u32) -> io::Result<u32> {
    let mut session = 0;
    checked(unsafe { ProcessIdToSessionId(pid, &mut session) })?;
    Ok(session)
}

/// Prove the recorded named scope is empty in its original OS session namespace.
pub fn job_empty(name: &str, expected_session: u32) -> io::Result<bool> {
    if session_id(std::process::id())? != expected_session {
        return Err(io::Error::other(
            "backend job belongs to a different OS session",
        ));
    }
    let job = match Handle::checked(unsafe { OpenJobObjectW(4, 0, wide(name).as_ptr()) }) {
        Ok(job) => job,
        // A named job exists until both its handles and associated processes are gone.
        Err(error) if error.raw_os_error() == Some(2) => return Ok(true),
        Err(error) => return Err(error),
    };
    #[repr(C)]
    struct Accounting {
        user_time: i64,
        kernel_time: i64,
        period_user_time: i64,
        period_kernel_time: i64,
        page_faults: u32,
        total_processes: u32,
        active_processes: u32,
        terminated_processes: u32,
    }
    let mut accounting: Accounting = unsafe { mem::zeroed() };
    checked(unsafe {
        QueryInformationJobObject(
            job.0,
            1, // JobObjectBasicAccountingInformation
            (&mut accounting as *mut Accounting).cast(),
            mem::size_of::<Accounting>() as u32,
            ptr::null_mut(),
        )
    })?;
    Ok(accounting.active_processes == 0)
}

struct LocalMemory(Raw);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn pipe_security() -> io::Result<LocalMemory> {
    let mut token = ptr::null_mut();
    checked(unsafe { OpenProcessToken(GetCurrentProcess(), 8, &mut token) })?;
    let token = Handle(token);
    let mut required = 0;
    unsafe {
        GetTokenInformation(token.0, 1, ptr::null_mut(), 0, &mut required);
    }
    if required == 0 || required > 65536 {
        return Err(io::Error::other("invalid user identity size"));
    }
    let mut buffer = vec![0_usize; (required as usize).div_ceil(mem::size_of::<usize>())];
    checked(unsafe {
        GetTokenInformation(
            token.0,
            1,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    })?;
    let sid = unsafe { *(buffer.as_ptr().cast::<Raw>()) };
    let mut text = ptr::null_mut();
    checked(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let memory = LocalMemory(text.cast());
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let user = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| io::Error::other("invalid user SID"))?;
    drop(memory);
    // Same user, restricted-code tokens, and a low-integrity label. Credential
    // authority still requires the admitted peer PID/birth/executable and run binding.
    let sddl = wide(format!("D:P(A;;GA;;;{user})(A;;GRGW;;;RC)S:(ML;;NW;;;LW)"));
    let mut descriptor = ptr::null_mut();
    checked(unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    })?;
    Ok(LocalMemory(descriptor))
}

fn operation(
    handle: Raw,
    deadline: Option<Instant>,
    start: impl FnOnce(*mut Overlapped) -> i32,
) -> io::Result<u32> {
    let event = Handle::checked(unsafe { CreateEventW(ptr::null_mut(), 1, 0, ptr::null()) })?;
    let mut overlapped: Overlapped = unsafe { mem::zeroed() };
    overlapped.event = event.0;
    if start(&mut overlapped) == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(997) {
            return Err(error);
        }
        loop {
            let interrupted =
                deadline.is_none() && crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst);
            let remaining =
                deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            if interrupted || remaining.is_some_and(|remaining| remaining.is_zero()) {
                unsafe {
                    CancelIoEx(handle, &overlapped);
                }
                let mut ignored = 0;
                unsafe {
                    GetOverlappedResult(handle, &mut overlapped, &mut ignored, 1);
                }
                return Err(io::Error::new(
                    if interrupted {
                        io::ErrorKind::ConnectionAborted
                    } else {
                        io::ErrorKind::TimedOut
                    },
                    "local handoff wait interrupted or deadline exceeded",
                ));
            }
            let timeout = remaining
                .unwrap_or(Duration::from_millis(100))
                .as_millis()
                .saturating_add(1)
                .min(u32::MAX as u128) as u32;
            match unsafe { WaitForSingleObject(event.0, timeout) } {
                0 => break,
                258 => continue, // WAIT_TIMEOUT; retain this same overlapped I/O.
                _ => {
                    let error = io::Error::last_os_error();
                    unsafe {
                        CancelIoEx(handle, &overlapped);
                    }
                    let mut ignored = 0;
                    unsafe {
                        GetOverlappedResult(handle, &mut overlapped, &mut ignored, 1);
                    }
                    return Err(error);
                }
            }
        }
    }
    let mut count = 0;
    checked(unsafe { GetOverlappedResult(handle, &mut overlapped, &mut count, 0) })?;
    Ok(count)
}

pub struct Pipe {
    handle: std::sync::Arc<Handle>,
    pub deadline: Option<Instant>,
    server: bool,
}
impl Pipe {
    pub fn peer(&self) -> io::Result<u32> {
        let mut pid = 0;
        checked(unsafe {
            if self.server {
                GetNamedPipeClientProcessId(self.handle.0, &mut pid)
            } else {
                GetNamedPipeServerProcessId(self.handle.0, &mut pid)
            }
        })?;
        Ok(pid)
    }
}
impl io::Read for Pipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        operation(self.handle.0, self.deadline, |ov| unsafe {
            ReadFile(
                self.handle.0,
                buffer.as_mut_ptr().cast(),
                buffer.len().min(u32::MAX as usize) as u32,
                ptr::null_mut(),
                ov,
            )
        })
        .map(|v| v as usize)
    }
}
impl io::Write for Pipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        operation(self.handle.0, self.deadline, |ov| unsafe {
            WriteFile(
                self.handle.0,
                buffer.as_ptr().cast(),
                buffer.len().min(u32::MAX as usize) as u32,
                ptr::null_mut(),
                ov,
            )
        })
        .map(|v| v as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        if self.server {
            unsafe {
                DisconnectNamedPipe(self.handle.0);
            }
        }
    }
}
pub struct Listener {
    handle: std::sync::Arc<Handle>,
    name: String,
}
impl Listener {
    pub fn bind(name: &str) -> io::Result<Self> {
        Ok(Self {
            handle: Self::instance(name, true)?,
            name: name.to_owned(),
        })
    }
    fn instance(name: &str, first: bool) -> io::Result<std::sync::Arc<Handle>> {
        let descriptor = pipe_security()?;
        let security = Security {
            length: mem::size_of::<Security>() as u32,
            descriptor: descriptor.0,
            inherit: 0,
        };
        let handle = Handle::checked(unsafe {
            CreateNamedPipeW(
                wide(name).as_ptr(),
                0x00000003 | 0x40000000 | if first { 0x00080000 } else { 0 },
                0x00000008,
                255, // PIPE_UNLIMITED_INSTANCES; authentication bounds active handlers.
                65536,
                65536,
                3000,
                &security,
            )
        })?;
        Ok(std::sync::Arc::new(handle))
    }
    pub fn accept(&mut self, deadline: Instant) -> io::Result<Pipe> {
        loop {
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let result = operation(self.handle.0, Some(deadline), |ov| unsafe {
                ConnectNamedPipe(self.handle.0, ov)
            });
            match result {
                // A client may close after connecting but before acceptance.
                // Disconnect that stale instance, not the shared listener.
                Err(error) if error.raw_os_error() == Some(232) => {
                    checked(unsafe { DisconnectNamedPipe(self.handle.0) })?;
                }
                Err(error) if error.raw_os_error() != Some(535) => return Err(error),
                _ => break,
            }
        }
        let pipe = Pipe {
            handle: self.handle.clone(),
            deadline: Some(deadline),
            server: true,
        };
        // Keep the name owned continuously, but never reuse/disconnect a handle
        // whose response is still being consumed by another admitted launcher.
        self.handle = Self::instance(&self.name, false)?;
        Ok(pipe)
    }
}

pub fn connect(name: &str, deadline: Instant) -> io::Result<Pipe> {
    let name = wide(name);
    let handle = loop {
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                0x80000000 | 0x40000000,
                0,
                ptr::null_mut(),
                3,
                0x40000000,
                ptr::null_mut(),
            )
        };
        if raw as isize != -1 {
            break Handle::checked(raw)?;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(231) {
            return Err(error);
        }
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u32::MAX as u128) as u32;
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "local handoff connect deadline exceeded",
            ));
        }
        checked(unsafe { WaitNamedPipeW(name.as_ptr(), remaining) })?;
    };
    Ok(Pipe {
        handle: std::sync::Arc::new(handle),
        deadline: Some(deadline),
        server: false,
    })
}

pub fn resolve_program() -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0_u16; 32768];
    let count = unsafe {
        SearchPathW(
            ptr::null(),
            wide("codex.exe").as_ptr(),
            ptr::null(),
            buffer.len() as u32,
            buffer.as_mut_ptr(),
            ptr::null_mut(),
        )
    };
    if count == 0 || count as usize >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    PathBuf::from(std::ffi::OsString::from_wide(&buffer[..count as usize])).canonicalize()
}
