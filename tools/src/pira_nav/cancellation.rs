use std::sync::atomic::{AtomicI32, Ordering};
static SIGNAL: AtomicI32 = AtomicI32::new(0);
pub(crate) fn requested() -> bool {
    SIGNAL.load(Ordering::Relaxed) != 0
}
pub(crate) fn exit_code() -> Option<i32> {
    let signal = SIGNAL.load(Ordering::Relaxed);
    (signal != 0).then_some(128 + signal)
}
#[cfg(unix)]
extern "C" fn cancel(signal: libc::c_int) {
    SIGNAL.store(signal, Ordering::Relaxed);
}

pub(crate) fn install() -> Result<(), String> {
    #[cfg(unix)]
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: sigaction receives initialized structs; the handler performs
        // only a lock-free atomic store. Install only in the CLI entry point.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = cancel as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(format!(
                    "cannot install Nav cancellation handler: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }
    Ok(())
}
