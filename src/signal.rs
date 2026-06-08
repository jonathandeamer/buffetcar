//! Signal-driven graceful shutdown.
//!
//! Registers handlers for SIGTERM and SIGINT that set a shared shutdown flag.
//! The accept loop checks this flag on interrupted accepts and breaks out,
//! allowing in-flight connections to drain before the process exits.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};
use std::sync::Arc;

/// Pointer to the caller's `AtomicBool`, set once by `install` and read by
/// the signal handler. Initialised to null; the handler is a no-op until
/// `install` stores a valid address.
static SHUTDOWN_PTR: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());

/// Write end of the self-pipe. The signal handler writes one byte here to wake
/// a blocked `poll` in the accept loop; the read end is returned by `install`
/// and polled alongside the listener. `-1` means no pipe (handler skips the
/// write and shutdown relies on `EINTR` alone).
#[cfg(unix)]
static SIGNAL_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// Install signal handlers for SIGTERM and SIGINT that set `shutdown` to `true`
/// and wake the accept loop via a self-pipe. Returns the read end of that pipe
/// (a raw fd) for the accept loop to poll, or `-1` if the pipe could not be
/// created.
///
/// The handlers are registered **without** `SA_RESTART` so that a blocked
/// `poll` returns `EINTR` when a signal arrives. `EINTR` alone is racy — a
/// signal delivered between the loop's flag check and the `poll` call leaves
/// nothing to interrupt — so the handler also writes a byte to the self-pipe.
/// Because the read end is in the same `poll` set as the listener, a byte
/// already buffered makes `poll` return immediately, closing that race.
///
/// One `Arc` clone is intentionally leaked so the `AtomicBool` lives for the
/// process lifetime and the signal handler always has a valid pointer. Call
/// this once at startup.
#[cfg(unix)]
pub(crate) fn install(shutdown: &Arc<AtomicBool>) -> libc::c_int {
    let ptr = Arc::into_raw(Arc::clone(shutdown)) as *mut AtomicBool;
    SHUTDOWN_PTR.store(ptr, Ordering::Release);

    let read_fd = create_self_pipe();

    extern "C" fn handler(_sig: libc::c_int) {
        // Safety: every call here is async-signal-safe. `AtomicBool::store` is
        // a single atomic store; `write(2)` is on the async-signal-safe list.
        // The pointer is valid because `install` leaked an Arc clone that keeps
        // the allocation alive.
        let ptr = SHUTDOWN_PTR.load(Ordering::Acquire);
        if !ptr.is_null() {
            unsafe { &*ptr }.store(true, Ordering::Release);
        }
        // Wake a blocked `poll`. The pipe is non-blocking, so a full buffer
        // (a byte already pending) just returns `EAGAIN`, which is fine.
        let fd = SIGNAL_WRITE_FD.load(Ordering::Acquire);
        if fd >= 0 {
            let byte = 1u8;
            unsafe {
                libc::write(fd, std::ptr::addr_of!(byte) as *const libc::c_void, 1);
            }
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

    read_fd
}

/// Create the shutdown self-pipe, store its write end in `SIGNAL_WRITE_FD`, and
/// return its read end. Both ends are set non-blocking (the handler must never
/// block; draining the read end must never block) and close-on-exec. Returns
/// `-1` on failure; the caller then relies on `EINTR` alone.
///
/// `pipe2` is avoided because macOS lacks it; `pipe` + `fcntl` is portable
/// across the Linux/macOS/OpenBSD targets.
#[cfg(unix)]
fn create_self_pipe() -> libc::c_int {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return -1;
    }
    for &fd in &fds {
        unsafe {
            let fl = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
            let fd_fl = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, fd_fl | libc::FD_CLOEXEC);
        }
    }
    SIGNAL_WRITE_FD.store(fds[1], Ordering::Release);
    fds[0]
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

/// Non-Unix fallback: no-op. Returns `-1` (no self-pipe).
#[cfg(not(unix))]
pub(crate) fn install(_shutdown: &Arc<AtomicBool>) -> i32 {
    -1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_handling_works() {
        let flag = Arc::new(AtomicBool::new(false));
        let _ = install(&flag);
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
