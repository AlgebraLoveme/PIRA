//! Cancellable I/O for one owned local SSH process, never a remote-work signal.
use std::{
    io::{self, Read, Write},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    time::Duration,
};

/// One logical client's successful SSH diagnostics, never persisted or shared between invocations.
#[derive(Default)]
pub(super) struct Diagnostics {
    previous: Vec<u8>,
}

// PIRA: coalesce only complete diagnostics up to 64 KiB. Larger streams pass
// through unchanged instead of truncating output or growing the cache.
const DIAGNOSTIC_CACHE: usize = 64 * 1024;

struct DiagnosticOutput<'a, W> {
    previous: &'a [u8],
    current: Option<Vec<u8>>,
    forwarded: bool,
    output: W,
}

impl<'a, W: Write> DiagnosticOutput<'a, W> {
    fn new(previous: &'a [u8], output: W) -> Self {
        Self {
            previous,
            current: Some(Vec::new()),
            forwarded: false,
            output,
        }
    }

    fn accept(&mut self, bytes: &[u8]) {
        let Some(current) = self.current.as_mut() else {
            let _ = self.output.write_all(bytes);
            return;
        };
        if current.len() + bytes.len() > DIAGNOSTIC_CACHE {
            if !self.forwarded {
                let _ = self.output.write_all(current);
            }
            let _ = self.output.write_all(bytes);
            self.current = None;
            self.forwarded = true;
            return;
        }
        current.extend_from_slice(bytes);
        if self.forwarded {
            let _ = self.output.write_all(bytes);
        } else if !self.previous.starts_with(current) {
            let _ = self.output.write_all(current);
            self.forwarded = true;
        }
    }

    fn finish(mut self, success: bool) -> Vec<u8> {
        if !self.forwarded && (!success || self.current.as_deref() != Some(self.previous)) {
            let _ = self
                .output
                .write_all(self.current.as_deref().unwrap_or_default());
        }
        // As with crate::notice, a closed diagnostic pipe must not bypass cleanup.
        let _ = self.output.flush();
        if success {
            self.current.unwrap_or_default()
        } else {
            Vec::new()
        }
    }
}

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
fn configure(input: &ChildStdin, output: &ChildStdout, stderr: &ChildStderr) -> io::Result<()> {
    nonblocking(input)?;
    nonblocking(output)?;
    nonblocking(stderr)
}

#[cfg(unix)]
fn available(pipe: &ChildStderr) -> io::Result<usize> {
    use std::os::fd::AsRawFd;
    let mut bytes: libc::c_int = 0;
    if unsafe { libc::ioctl(pipe.as_raw_fd(), libc::FIONREAD, &mut bytes) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(bytes.max(0) as usize)
    }
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
    pub(super) fn configure(
        input: &ChildStdin,
        _: &ChildStdout,
        _: &ChildStderr,
    ) -> io::Result<()> {
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
    pub(super) fn available(output: &impl AsRawHandle) -> io::Result<Option<usize>> {
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
                Some(109) | Some(233) => Ok(None), // all writers closed
                _ => Err(error),
            };
        }
        Ok(Some(available as usize))
    }
    pub(super) fn read(
        output: &mut (impl Read + AsRawHandle),
        buffer: &mut [u8],
    ) -> io::Result<usize> {
        let available = match available(output)? {
            None => return Ok(0),
            Some(0) => return Err(io::ErrorKind::WouldBlock.into()),
            Some(size) => size,
        };
        let size = buffer.len().min(available);
        output.read(&mut buffer[..size])
    }
}
#[cfg(windows)]
use windows::configure;
#[cfg(windows)]
fn available(pipe: &ChildStderr) -> io::Result<usize> {
    windows::available(pipe).map(Option::unwrap_or_default)
}
#[cfg(not(any(unix, windows)))]
fn configure(_: &ChildStdin, _: &ChildStdout, _: &ChildStderr) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cancellable SSH pipes unavailable on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn available(_: &ChildStderr) -> io::Result<usize> {
    Err(io::ErrorKind::Unsupported.into())
}

fn drain(
    stderr: &mut ChildStderr,
    output: &mut DiagnosticOutput<impl Write>,
    budget: usize,
) -> io::Result<()> {
    // Snapshot queued bytes: no waiting for EOF from a proxy descendant that
    // retained stderr after our owned SSH process exited.
    let mut remaining = available(stderr)?.min(budget);
    let mut buffer = [0; 16384];
    while remaining > 0 {
        let size = remaining.min(buffer.len());
        #[cfg(windows)]
        let result = windows::read(stderr, &mut buffer[..size]);
        #[cfg(not(windows))]
        let result = stderr.read(&mut buffer[..size]);
        match result {
            Ok(0) => break,
            Ok(count) => {
                output.accept(&buffer[..count]);
                remaining -= count;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
    progress: &mut impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    while !request.is_empty() {
        progress("write")?;
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

fn wait(child: &mut Child, progress: &mut impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
    loop {
        progress("wait")?;
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
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>, String> {
    let mut stderr = child.stderr.take();
    let mut diagnostic_output = DiagnosticOutput::new(&diagnostics.previous, io::stderr());
    let mut configured = false;
    let outcome = (|| -> io::Result<Vec<u8>> {
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("SSH stdin unavailable"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("SSH stdout unavailable"))?;
        let stderr = stderr
            .as_mut()
            .ok_or_else(|| io::Error::other("SSH stderr unavailable"))?;
        configure(&input, &output, stderr)?;
        configured = true;
        let mut progress = |stage: &str| {
            check(&cancelled, stage)?;
            drain(stderr, &mut diagnostic_output, DIAGNOSTIC_CACHE)
        };
        write_request(&mut input, request, &mut progress)?;
        drop(input);
        let mut bytes = Vec::new();
        let mut buffer = [0; 16384];
        loop {
            progress("read")?;
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
        wait(child, &mut progress)?;
        progress("completion")?;
        Ok(bytes)
    })();
    let mut result = match outcome {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            // This handle is the spawned local SSH, not a retained PID/group or remote job.
            let _ = child.kill();
            match child.wait() {
                Ok(_) => Err(error.to_string()),
                Err(reap) => Err(format!("{error}; reap owned local SSH: {reap}")),
            }
        }
    };
    if configured
        && let Err(error) = drain(stderr.as_mut().unwrap(), &mut diagnostic_output, usize::MAX)
    {
        result = Err(match result {
            Ok(_) => format!("read SSH diagnostics: {error}"),
            Err(original) => format!("{original}; read SSH diagnostics: {error}"),
        });
    }
    diagnostics.previous = diagnostic_output.finish(result.is_ok());
    result
}

#[cfg(test)]
#[path = "tests/transport.rs"]
mod tests;
