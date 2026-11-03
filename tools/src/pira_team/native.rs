//! Kernel process identity. Birth times distinguish recycled process identifiers.
use std::{io, path::PathBuf};
#[cfg(windows)]
#[path = "native_windows.rs"]
pub mod windows;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pid: u32,
    pub parent: u32,
    pub birth: u64,
    pub executable: PathBuf,
    pub alive: bool,
}

#[cfg(target_os = "linux")]
pub fn identity(pid: u32) -> io::Result<Identity> {
    metadata(pid)?.identify(executable)
}

// Keep ancestry metadata distinct from executable identity: protected Linux
// ancestors can expose stat/cmdline without permitting ptrace-gated exe access.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub pid: u32,
    pub parent: u32,
    pub birth: u64,
    pub alive: bool,
}

#[cfg(any(target_os = "linux", test))]
impl Metadata {
    pub fn with_executable(self, executable: PathBuf) -> Identity {
        Identity {
            pid: self.pid,
            parent: self.parent,
            birth: self.birth,
            alive: self.alive,
            executable,
        }
    }

    fn identify(self, executable: impl FnOnce(u32) -> io::Result<PathBuf>) -> io::Result<Identity> {
        let path = if self.alive {
            executable(self.pid)?
        } else {
            PathBuf::new()
        };
        Ok(self.with_executable(path))
    }
}

#[cfg(target_os = "linux")]
pub fn metadata(pid: u32) -> io::Result<Metadata> {
    parse_stat(pid, &std::fs::read_to_string(format!("/proc/{pid}/stat"))?)
}

#[cfg(any(target_os = "linux", test))]
fn parse_stat(pid: u32, stat: &str) -> io::Result<Metadata> {
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("invalid process stat"))?
        .1
        .split_whitespace()
        .collect();
    let number = |index: usize| -> io::Result<u64> {
        fields
            .get(index)
            .ok_or_else(|| io::Error::other("short process stat"))?
            .parse()
            .map_err(|_| io::Error::other("invalid process identity"))
    };
    let alive = fields
        .first()
        .is_some_and(|state| !["Z", "X"].contains(state));
    Ok(Metadata {
        pid,
        parent: u32::try_from(number(1)?).map_err(io::Error::other)?,
        birth: number(19)?,
        alive,
    })
}

#[cfg(target_os = "linux")]
pub fn executable(pid: u32) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
}

#[cfg(target_os = "macos")]
pub fn identity(pid: u32) -> io::Result<Identity> {
    #[repr(C)]
    struct BsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        ppid: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        reserved: u32,
        comm: [u8; 16],
        name: [u8; 32],
        files: u32,
        pgid: u32,
        jobc: u32,
        tty: u32,
        tty_pgid: u32,
        nice: i32,
        start_sec: u64,
        start_usec: u64,
    }
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            size: i32,
        ) -> i32;
        fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, size: u32) -> i32;
    }
    let mut info: BsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<BsdInfo>() as i32;
    if unsafe { proc_pidinfo(pid as i32, 3, 0, (&mut info as *mut BsdInfo).cast(), size) } != size {
        return Err(io::Error::last_os_error());
    }
    let alive = info.status != 5; // SZOMB: no executable remains to stop.
    let mut buffer = [0_u8; 4096];
    let count = if alive {
        unsafe { proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) }
    } else {
        0
    };
    if alive && count <= 0 {
        // A removed executable does not mean this PID exited. Keep identity
        // failure distinct from missing-process evidence used by settlement.
        return Err(unavailable_executable(pid, io::Error::last_os_error()));
    }
    let bytes = &buffer[..buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len())];
    use std::os::unix::ffi::OsStrExt;
    Ok(Identity {
        pid,
        parent: info.ppid,
        birth: info
            .start_sec
            .checked_mul(1_000_000)
            .and_then(|v| v.checked_add(info.start_usec))
            .ok_or_else(|| io::Error::other("invalid process birth time"))?,
        executable: std::ffi::OsStr::from_bytes(bytes).into(),
        alive,
    })
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn unavailable_executable(pid: u32, error: io::Error) -> io::Error {
    // Executable inspection failure is not evidence that the live PID exited.
    io::Error::other(format!(
        "cannot identify executable for live process {pid}: {error}"
    ))
}

#[cfg(windows)]
pub fn identity(pid: u32) -> io::Result<Identity> {
    windows::identity(pid)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn identity(_: u32) -> io::Result<Identity> {
    Err(io::Error::other(
        "native process identity is unavailable on this platform",
    ))
}

pub fn missing(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || cfg!(windows) && error.raw_os_error() == Some(87)
        || {
            #[cfg(unix)]
            {
                error.raw_os_error() == Some(libc::ESRCH)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
}

pub fn current() -> Result<Identity, String> {
    identity(std::process::id()).map_err(|_| "cannot verify launcher process identity".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_stat_metadata_does_not_require_executable_access() {
        let stat = |state| {
            format!("12 (name ) with spaces) {state} 8 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 123")
        };
        for state in ["S", "Z", "X"] {
            let metadata = parse_stat(12, &stat(state)).unwrap();
            assert_eq!(
                (metadata.pid, metadata.parent, metadata.birth),
                (12, 8, 123)
            );
            assert_eq!(metadata.alive, state == "S");
        }
        for stat in ["", "12 (short) S 8", "12 (bad) S invalid"] {
            assert!(parse_stat(12, stat).is_err());
        }
    }

    #[test]
    fn strict_identity_never_swallows_executable_denial() {
        let metadata = Metadata {
            pid: 12,
            parent: 8,
            birth: 123,
            alive: true,
        };
        let error = metadata
            .clone()
            .identify(|_| Err(io::ErrorKind::PermissionDenied.into()))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!missing(&error));
        let dead = Metadata {
            alive: false,
            ..metadata
        };
        assert!(
            !dead
                .identify(|_| panic!("dead process has no executable"))
                .unwrap()
                .alive
        );
    }

    #[test]
    fn current_process_identity_is_kernel_stable() {
        let first = super::current().unwrap();
        let second = super::current().unwrap();
        assert_eq!(first.pid, std::process::id());
        assert_eq!(first.birth, second.birth);
        assert!(first.alive && first.birth > 0);
        assert_eq!(
            first.executable.canonicalize().unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
    }
}

// Claude sandboxes individual shell commands, not its main CLI. Check the actual
// invoking process before delegation rather than waiting for a completed tool result.
#[cfg(target_os = "macos")]
pub(crate) fn claude_shell_unsandboxed(_ancestor: u32) -> io::Result<bool> {
    #[link(name = "sandbox")]
    unsafe extern "C" {
        fn sandbox_check(
            pid: libc::pid_t,
            operation: *const libc::c_char,
            filter: libc::c_int,
            ...
        ) -> libc::c_int;
    }
    match unsafe { sandbox_check(libc::getpid(), std::ptr::null(), 0) } {
        0 => Ok(true),
        value if value > 0 => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn claude_shell_unsandboxed(ancestor: u32) -> io::Result<bool> {
    // Bubblewrap changes namespaces. Preserve a containing VM/container's
    // baseline restrictions, but reject extra isolation on this shell.
    for namespace in ["mnt", "net", "user", "pid"] {
        if std::fs::read_link(format!("/proc/self/ns/{namespace}"))?
            != std::fs::read_link(format!("/proc/{ancestor}/ns/{namespace}"))?
        {
            return Ok(false);
        }
    }
    let current = std::fs::read_to_string("/proc/self/status")?;
    let parent = std::fs::read_to_string(format!("/proc/{ancestor}/status"))?;
    same_execution_filters(&current, &parent)
}

#[cfg(any(target_os = "linux", test))]
fn same_execution_filters(current: &str, parent: &str) -> io::Result<bool> {
    for key in ["NoNewPrivs:", "Seccomp:", "Seccomp_filters:"] {
        let field = |text: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key).map(str::trim).map(str::to_owned))
        };
        match (field(current), field(parent)) {
            (Some(a), Some(b)) if a == b => (),
            (None, None) if key == "Seccomp_filters:" => (), // Older kernels omit the count.
            (Some(_), Some(_)) => return Ok(false),
            _ => return Err(io::Error::other("missing process sandbox metadata")),
        }
    }
    Ok(true)
}

#[cfg(windows)]
pub(crate) fn claude_shell_unsandboxed(_ancestor: u32) -> io::Result<bool> {
    // Native Claude does not implement its shell sandbox on Windows. External
    // process/job restrictions are inherited; WSL uses the Linux check above.
    Ok(true)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn claude_shell_unsandboxed(_ancestor: u32) -> io::Result<bool> {
    Err(io::Error::other(
        "Claude sandbox verification unsupported on this platform",
    ))
}

#[cfg(test)]
mod claude_sandbox_tests {
    use super::*;
    #[test]
    fn rejects_extra_or_missing_linux_restrictions() {
        let base = "NoNewPrivs: 0\nSeccomp: 2\nSeccomp_filters: 1\n";
        assert!(same_execution_filters(base, base).unwrap());
        assert!(!same_execution_filters(&base.replace("filters: 1", "filters: 2"), base).unwrap());
        assert!(!same_execution_filters(&base.replace("Privs: 0", "Privs: 1"), base).unwrap());
        assert!(same_execution_filters("", base).is_err());
    }
    #[test]
    fn current_native_sandbox_probe() {
        let actual = claude_shell_unsandboxed(std::process::id()).unwrap();
        let expected = std::env::var_os("PIRA_TEST_EXPECT_SANDBOX").is_none();
        assert_eq!(actual, expected);
    }
}
