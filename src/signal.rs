//! Signal-driven graceful shutdown.
//!
//! Registers handlers for SIGTERM and SIGINT that set a shared shutdown flag.
//! The accept loop checks this flag on interrupted accepts and breaks out,
//! allowing in-flight connections to drain before the process exits.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;

/// Pointer to the caller's `AtomicBool`, set once by `install` and read by
/// the signal handler. Initialised to null; the handler is a no-op until
/// `install` stores a valid address.
static SHUTDOWN_PTR: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());

/// Install signal handlers for SIGTERM and SIGINT that set `shutdown` to
/// `true`.
///
/// The handlers are registered **without** `SA_RESTART` so that a blocked
/// `accept()` returns `EINTR` when a signal arrives, giving the accept loop
/// a chance to check the flag.
///
/// One `Arc` clone is intentionally leaked so the `AtomicBool` lives for the
/// process lifetime and the signal handler always has a valid pointer. Call
/// this once at startup.
#[cfg(unix)]
pub(crate) fn install(shutdown: &Arc<AtomicBool>) {
    let ptr = Arc::into_raw(Arc::clone(shutdown)) as *mut AtomicBool;
    SHUTDOWN_PTR.store(ptr, Ordering::Release);

    extern "C" fn handler(_sig: libc::c_int) {
        // Safety: `AtomicBool::store` is async-signal-safe (single atomic
        // store instruction). The pointer is valid because `install` leaked
        // an Arc clone that keeps the allocation alive.
        let ptr = SHUTDOWN_PTR.load(Ordering::Acquire);
        if !ptr.is_null() {
            unsafe { &*ptr }.store(true, Ordering::Release);
        }
    }

    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler as *const () as usize;
        action.sa_flags = 0; // No SA_RESTART: interrupted syscalls return EINTR.
        libc::sigemptyset(&mut action.sa_mask);

        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
}

/// Block SIGTERM and SIGINT on the current thread.
///
/// In a multi-threaded process, process-directed signals can be delivered to
/// any thread that has them unblocked. To ensure signals consistently interrupt
/// the main thread's accept loop (and `poll`), worker threads call this at startup
/// to block these signals.
#[cfg(unix)]
pub(crate) fn block_signals_on_current_thread() {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// Non-Unix fallback: no-op.
#[cfg(not(unix))]
pub(crate) fn block_signals_on_current_thread() {}

/// Non-Unix fallback: no-op.
#[cfg(not(unix))]
pub(crate) fn install(_shutdown: &Arc<AtomicBool>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_handling_works() {
        let flag = Arc::new(AtomicBool::new(false));
        install(&flag);
        assert!(!flag.load(Ordering::Relaxed));

        #[cfg(unix)]
        {
            unsafe { libc::raise(libc::SIGTERM) };
            assert!(
                flag.load(Ordering::Relaxed),
                "SIGTERM should set the shutdown flag"
            );
        }
    }
}
