use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

static SIGNAL: AtomicI32 = AtomicI32::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) const INTERRUPTED: &str = "__PIRA_CTX_INTERRUPTED__";

pub(crate) fn requested() -> bool {
    SIGNAL.load(Ordering::Relaxed) != 0
}

pub(crate) fn exit_code() -> i32 {
    128 + SIGNAL.load(Ordering::Relaxed)
}

pub(crate) fn check() -> Result<(), String> {
    if requested() {
        Err(INTERRUPTED.into())
    } else {
        Ok(())
    }
}

pub(crate) fn wait<T>(receiver: mpsc::Receiver<T>) -> Result<T, String> {
    loop {
        check()?;
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(value) => {
                check()?;
                return Ok(value);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => return Err(format!("stdio worker: {error}")),
        }
    }
}

type OutputRequest = (bool, Vec<u8>, mpsc::Sender<io::Result<()>>);
static OUTPUT: OnceLock<Result<mpsc::Sender<OutputRequest>, String>> = OnceLock::new();

// PIRA: CLI-only workers own bytes/stdio, never children or checkpoint leases.
// A blocked stdio operation is abandoned on shutdown and released by process exit;
// no inherited descriptor flags are changed and no blocked writer is joined.
pub(crate) fn write_output(stderr: bool, bytes: &[u8]) -> io::Result<()> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return direct_write(stderr, bytes);
    }
    check().map_err(io::Error::other)?;
    let sender = OUTPUT
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<OutputRequest>();
            std::thread::Builder::new()
                .name("ctx-output".into())
                .spawn(move || {
                    for (stderr, bytes, reply) in receiver {
                        let _ = reply.send(direct_write(stderr, &bytes));
                    }
                })
                .map_err(|e| format!("start output worker: {e}"))?;
            Ok(sender)
        })
        .as_ref()
        .map_err(|e| io::Error::other(e.clone()))?;
    let (reply, receiver) = mpsc::channel();
    sender
        .send((stderr, bytes.to_vec(), reply))
        .map_err(io::Error::other)?;
    wait(receiver).map_err(io::Error::other)?
}

fn direct_write(stderr: bool, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        // A blocked worker must not hold Rust's global stdio mutex during exit.
        // dup owns a descriptor, not a new file description; flags stay unchanged.
        let fd = unsafe { libc::dup(if stderr { 2 } else { 1 }) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut output = unsafe { std::fs::File::from_raw_fd(fd) };
        return output.write_all(bytes);
    }
    #[cfg(not(unix))]
    if stderr {
        let mut output = io::stderr().lock();
        output.write_all(bytes)?;
        output.flush()
    } else {
        let mut output = io::stdout().lock();
        output.write_all(bytes)?;
        output.flush()
    }
}

pub(crate) struct Output(pub bool);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        write_output(self.0, bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
extern "C" fn request(signal: libc::c_int) {
    // No allocation, I/O, locks or process signalling in the signal handler.
    let _ = SIGNAL.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

pub(crate) struct Guard {
    #[cfg(unix)]
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

/// Install in capturing/watch CLI routes, through final state publication.
pub(crate) fn install() -> Result<Guard, String> {
    #[allow(unused_mut)]
    let mut guard = Guard {
        #[cfg(unix)]
        previous: Vec::new(),
    };
    SIGNAL.store(0, Ordering::Relaxed);
    #[cfg(unix)]
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: initialized actions and a handler using only a lock-free atomic.
        // The guard restores already-installed actions if a later install fails.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut previous = std::mem::zeroed();
            action.sa_sigaction = request as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, &mut previous) != 0 {
                return Err(format!(
                    "install capture shutdown handler: {}",
                    std::io::Error::last_os_error()
                ));
            }
            guard.previous.push((signal, previous));
        }
    }
    #[cfg(unix)]
    ACTIVE.store(true, Ordering::Relaxed);
    Ok(guard)
}

impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Relaxed);
        #[cfg(unix)]
        for (signal, previous) in self.previous.iter().rev() {
            // SAFETY: restoring the disposition saved by this CLI-only guard.
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
    }
}
