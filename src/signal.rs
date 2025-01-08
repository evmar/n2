//! Unix signal handling (SIGINT).
//!
//! We let the first SIGINT reach child processes, which ought to build-fail
//! and let the parent properly print that progress.  This also lets us still
//! write out pending debug traces, too.

use std::sync::atomic::AtomicBool;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn sigint_handler(_sig: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
    // SA_RESETHAND should clear the handler.
}

#[cfg(unix)]
pub fn register_sigint() {
    // Safety: registering a signal handler is libc unsafe code.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = sigint_handler as libc::sighandler_t;
        sa.sa_flags = libc::SA_RESETHAND;
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

pub fn was_interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed)
}
