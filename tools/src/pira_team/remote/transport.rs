//! Cancellable I/O for one owned local SSH process, never a remote-work signal.
use std::{
    io::{self, Read, Write},
    process::{Child, ChildStdin, ChildStdout},
    time::Duration,
};

fn check(cancelled: &impl Fn() -> bool, stage: &str) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            format!("SSH exchange cancelled during {stage}"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn configure(input: &ChildStdin, output: &ChildStdout) -> io::Result<()> {
    nonblocking(input)?;
    nonblocking(output)
}
#[cfg(windows)]
mod windows {
    use super::*;
    use std::{ffi::c_void, os::windows::io::AsRawHandle, ptr};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetNamedPipeHandleState(
            pipe: *mut c_void,
            mode: *const u32,
            count: *const u32,
            timeout: *const u32,
        ) -> i32;
        fn PeekNamedPipe(
            pipe: *mut c_void,
            buffer: *mut c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }
    pub(super) fn configure(input: &ChildStdin, _: &ChildStdout) -> io::Result<()> {
        // PIRA: anonymous stdio is synchronous. PIPE_NOWAIT bounds writes;
        // PeekNamedPipe bounds the sole reader without requiring read-handle write access.
        if unsafe { SetNamedPipeHandleState(input.as_raw_handle(), &1, ptr::null(), ptr::null()) }
            == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn read(output: &mut ChildStdout, buffer: &mut [u8]) -> io::Result<usize> {
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                output.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(109) | Some(233) => Ok(0), // all writers closed
                _ => Err(error),
            };
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let size = buffer.len().min(available as usize);
        output.read(&mut buffer[..size])
    }
}
#[cfg(windows)]
use windows::configure;
#[cfg(not(any(unix, windows)))]
fn configure(_: &ChildStdin, _: &ChildStdout) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cancellable SSH pipes unavailable on this platform",
    ))
}

fn read(output: &mut ChildStdout, buffer: &mut [u8]) -> io::Result<usize> {
    #[cfg(windows)]
    {
        windows::read(output, buffer)
    }
    #[cfg(not(windows))]
    {
        output.read(buffer)
    }
}

fn write_request(
    input: &mut ChildStdin,
    mut request: &[u8],
    cancelled: &impl Fn() -> bool,
) -> io::Result<()> {
    while !request.is_empty() {
        check(cancelled, "write")?;
        match input.write(request) {
            Ok(0) => {
                #[cfg(not(windows))]
                return Err(io::ErrorKind::WriteZero.into());
                #[cfg(windows)]
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(size) => request = &request[size..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn wait(child: &mut Child, cancelled: &impl Fn() -> bool) -> io::Result<()> {
    loop {
        check(cancelled, "wait")?;
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other("SSH process failed"))
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn exchange(
    child: &mut Child,
    request: &[u8],
    cancelled: impl Fn() -> bool,
) -> Result<Vec<u8>, String> {
    let outcome = (|| -> io::Result<Vec<u8>> {
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("SSH stdin unavailable"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("SSH stdout unavailable"))?;
        configure(&input, &output)?;
        write_request(&mut input, request, &cancelled)?;
        drop(input);
        let mut bytes = Vec::new();
        let mut buffer = [0; 16384];
        loop {
            check(&cancelled, "read")?;
            let size = buffer.len().min(super::LIMIT as usize + 1 - bytes.len());
            match read(&mut output, &mut buffer[..size]) {
                Ok(0) => break,
                Ok(count) => {
                    bytes.extend_from_slice(&buffer[..count]);
                    if bytes.len() as u64 > super::LIMIT {
                        return Err(io::Error::other("remote reply exceeds protocol limit"));
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }
        wait(child, &cancelled)?;
        check(&cancelled, "completion")?;
        Ok(bytes)
    })();
    match outcome {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            // This handle is the spawned local SSH, not a retained PID/group or remote job.
            let _ = child.kill();
            child
                .wait()
                .map_err(|reap| format!("{error}; reap owned local SSH: {reap}"))?;
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests;
