//! Unix signal handling (SIGINT).
//!
//! We let the first SIGINT reach child processes, which ought to build-fail
//! and let the parent properly print that progress.  This also lets us still
//! write out pending debug traces, too.

fn sigint_action(handler: libc::sighandler_t) {
    // Safety: registering a signal handler is libc unsafe code.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as libc::sighandler_t;
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

extern "C" fn sigint_handler(_sig: libc::c_int) {
    // TODO: is it safe to tweak the signal handler in a signal handler?
    sigint_action(libc::SIG_DFL as libc::sighandler_t);
}

pub fn register_sigint() {
    sigint_action(sigint_handler as libc::sighandler_t);
}
