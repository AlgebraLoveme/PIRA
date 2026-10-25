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
    let root = PathBuf::from(format!("/proc/{pid}"));
    let stat = std::fs::read_to_string(root.join("stat"))?;
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
    Ok(Identity {
        pid,
        parent: u32::try_from(number(1)?).map_err(io::Error::other)?,
        birth: number(19)?,
        executable: if alive {
            std::fs::read_link(root.join("exe"))?
        } else {
            PathBuf::new()
        },
        alive,
    })
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
        return Err(io::Error::last_os_error());
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
        #[cfg(windows)]
        assert!(!super::windows::arguments(first.pid).unwrap().is_empty());
    }
}
