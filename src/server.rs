//! TCP listener and fixed worker-thread pool.
//!
//! The `Root` capability is opened once and shared across workers behind an
//! `Arc`. A rendezvous `sync_channel` plus exactly `workers` threads caps active
//! handlers at `workers`; the single accept loop can hold one additional accepted
//! stream while blocked in `send`. There is no unbounded spawn-per-connection and
//! no async runtime.

use crate::config::{self, ServeConfig};
use crate::conn;
use crate::limiter::{ConnPermit, PerIpLimiter};
use crate::root::Root;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::FromRawFd as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A startup failure for `serve`. Carries enough context to print one
/// actionable `error:` line without leaking internal type names.
pub(crate) enum ServeError {
    Bind(SocketAddr, io::Error),
    Root(io::Error),
    Sandbox(io::Error),
    Serve(io::Error),
}

impl ServeError {
    pub(crate) fn message(&self) -> String {
        match self {
            ServeError::Bind(addr, err) => format!("could not bind {addr}: {}", bind_reason(err)),
            ServeError::Root(err) => format!("could not open root: {err}"),
            ServeError::Sandbox(err) => format!("could not apply sandbox: {err}"),
            ServeError::Serve(err) => format!("server error: {err}"),
        }
    }
}

fn bind_reason(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::AddrInUse => "address already in use".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::AddrNotAvailable => "address not available".to_string(),
        _ => err.to_string(),
    }
}

/// Open the root, apply the sandbox, install signal handlers, bind the
/// listener, print the success banner, then serve until a shutdown signal
/// arrives. Returns only on shutdown, a fatal accept-loop error, or a startup
/// error; the banner is written only after a successful bind.
pub(crate) fn run(config: &ServeConfig, mut banner: impl Write) -> Result<(), ServeError> {
    let root = Root::open(&config.root).map_err(ServeError::Root)?;
    let listener =
        bind_with_backlog(config.listen).map_err(|err| ServeError::Bind(config.listen, err))?;
    // The address the OS actually bound, which differs from the requested
    // `config.listen` when an ephemeral port (`:0`) was requested. The banner
    // reports this concrete address so operators (and tests) learn the live port.
    let bound = listener
        .local_addr()
        .map_err(|err| ServeError::Bind(config.listen, err))?;
    crate::sandbox::apply(&config.root).map_err(ServeError::Sandbox)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let wake_fd = crate::signal::install(&shutdown);

    // Bind succeeded: this is the startup-success banner.
    let _ = config::write_banner(
        &config.root,
        bound,
        crate::version::version_line(),
        &mut banner,
    );

    let settings = ServeSettings {
        workers: config.workers,
        max_conns_per_ip: config.max_conns_per_ip,
        read_timeout: Duration::from_secs(config::READ_TIMEOUT_SECS),
        write_timeout: config.write_timeout,
    };
    serve(listener, root, settings, &shutdown, wake_fd).map_err(ServeError::Serve)
}

/// TCP listen backlog: number of fully-established connections the kernel
/// will queue before refusing new ones with RST / connection timeout.
///
/// 128 is a conservative, portable baseline; it also matches the value
/// `std`'s `TcpListener::bind` hard-codes, so this preserves the previous
/// behaviour while making the number explicit. The kernel clamps the
/// request down to its own limit (`net.core.somaxconn`, which defaults to
/// 4096 on modern Linux) and never raises it, so 128 is safe everywhere.
const LISTEN_BACKLOG: i32 = 128;

/// Create a `TcpListener` on `addr` with an explicit listen backlog.
///
/// `std::net::TcpListener::bind` hard-codes a listen backlog of 128 with
/// no way to change or document it. This function makes the value an
/// explicit, named constant. It also replicates the socket setup
/// `TcpListener::bind` performs on Unix — `SO_REUSEADDR` (so a restart
/// while the port is in `TIME_WAIT` still succeeds) and `SOCK_CLOEXEC`.
fn bind_with_backlog(addr: SocketAddr) -> io::Result<TcpListener> {
    use rustix::net::sockopt;
    use rustix::net::{self, AddressFamily, SocketType};
    use std::os::fd::AsRawFd as _;

    let family = match addr {
        SocketAddr::V4(_) => AddressFamily::INET,
        SocketAddr::V6(_) => AddressFamily::INET6,
    };
    let sock = net::socket(family, SocketType::STREAM, None)
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
    // Match `TcpListener::bind`, which creates the socket close-on-exec; without
    // this the listening fd would leak across an `exec`. `SOCK_CLOEXEC` at
    // creation is not portable (macOS lacks it), so set `FD_CLOEXEC` here. The
    // brief non-atomic window is harmless: this runs once at single-threaded
    // startup, before any worker threads exist to fork/exec.
    if unsafe { libc::fcntl(sock.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    sockopt::set_socket_reuseaddr(&sock, true)
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
    net::bind(&sock, &addr).map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
    net::listen(&sock, LISTEN_BACKLOG)
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;

    // SAFETY: `sock` is a valid, fully-bound, listening TCP socket fd.
    // `into_raw_fd` consumes the OwnedFd so ownership is transferred once.
    unsafe {
        use std::os::fd::IntoRawFd as _;
        Ok(TcpListener::from_raw_fd(sock.into_raw_fd()))
    }
}

#[derive(Clone, Copy)]
struct ServeSettings {
    workers: usize,
    max_conns_per_ip: u32,
    read_timeout: Duration,
    write_timeout: Duration,
}

/// `wake_fd` is the read end of the shutdown self-pipe (see `crate::signal`).
/// It is polled alongside the listener so a shutdown signal wakes the accept
/// loop even with no pending connection; `-1` disables it (tests that drive
/// shutdown by other means, or platforms without the pipe).
fn serve(
    listener: TcpListener,
    root: Root,
    settings: ServeSettings,
    shutdown: &AtomicBool,
    wake_fd: libc::c_int,
) -> io::Result<()> {
    let root = Arc::new(root);
    let limiter = Arc::new(PerIpLimiter::new(settings.max_conns_per_ip));
    // The zero-capacity channel allows at most `workers` handled connections
    // plus one accepted connection parked in `send`.
    let (tx, rx) = mpsc::sync_channel::<(TcpStream, ConnPermit)>(0);
    let rx = Arc::new(Mutex::new(rx));

    let mut handles = Vec::with_capacity(settings.workers);
    for _ in 0..settings.workers {
        let rx = Arc::clone(&rx);
        let root = Arc::clone(&root);
        handles.push(thread::spawn(move || {
            worker_loop(&rx, &root, settings.read_timeout, settings.write_timeout)
        }));
    }

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        listener.set_nonblocking(true)?;

        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }

            // Poll the listener and, when present, the shutdown self-pipe. A
            // signal that sets the flag in the window between the check above
            // and this poll has nothing to interrupt, so EINTR alone is racy;
            // but the handler also writes a byte to the pipe, and a byte already
            // buffered makes poll return immediately. Index 0 is the listener,
            // index 1 the pipe read end (only polled when wake_fd >= 0).
            let mut fds = [
                libc::pollfd {
                    fd: listener.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let nfds: libc::nfds_t = if wake_fd >= 0 { 2 } else { 1 };

            let res = unsafe { libc::poll(fds.as_mut_ptr(), nfds, -1) };
            if res < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }

            // Shutdown self-pipe activity means a signal arrived. On readable,
            // drain it and loop so the flag check at the top breaks out. On a
            // broken pipe (the write end should live for the whole process, so
            // this is defensive), shut down rather than busy-loop on the bit.
            if nfds == 2 && fds[1].revents != 0 {
                if fds[1].revents & libc::POLLIN != 0 {
                    drain_fd(wake_fd);
                    continue;
                }
                break;
            }

            let listener_revents = fds[0].revents;

            // Fatal error or hangup on the listening socket descriptor itself.
            // Treating POLLHUP/POLLERR as fatal prevents infinite busy-looping on persistent bits.
            if listener_revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "fatal listener socket error or hangup",
                ));
            }

            // Only proceed to accept if the socket has pending incoming connections
            if listener_revents & libc::POLLIN == 0 {
                continue;
            }

            match listener.accept() {
                Ok((stream, peer)) => {
                    let Some(permit) = limiter.try_acquire(peer.ip()) else {
                        continue;
                    };
                    stream.set_nonblocking(false)?;
                    if tx.send((stream, permit)).is_err() {
                        break; // all workers have gone away
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = wake_fd; // self-pipe wakeup is a unix-only mechanism
        for stream in listener.incoming() {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            match stream {
                Ok(stream) => {
                    let Ok(peer) = stream.peer_addr() else {
                        continue;
                    };
                    let Some(permit) = limiter.try_acquire(peer.ip()) else {
                        continue;
                    };
                    if tx.send((stream, permit)).is_err() {
                        break; // all workers have gone away
                    }
                }
                Err(_) => continue,
            }
        }
    }

    // Close the channel so workers drain and exit after finishing in-flight
    // connections.
    drop(tx);
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}

/// Drain all pending bytes from a non-blocking fd, discarding them. Used to
/// empty the shutdown self-pipe after `poll` reports it readable so a single
/// wakeup does not report readable forever.
#[cfg(unix)]
fn drain_fd(fd: libc::c_int) {
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
    }
}

fn worker_loop(
    rx: &Mutex<Receiver<(TcpStream, ConnPermit)>>,
    root: &Root,
    read_timeout: Duration,
    write_timeout: Duration,
) {
    crate::signal::block_signals_on_current_thread();
    loop {
        // Hold the lock only across `recv`; release it before handling so other
        // workers can pick up the next connection. Recover from a poisoned lock
        // rather than panicking, so one failed worker cannot collapse the pool.
        let (stream, _permit) = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
            Ok(item) => item,
            Err(_) => return, // channel closed: shut the worker down
        };
        // Connection-level errors are silent; the connection is simply dropped.
        // `_permit` drops at the end of this iteration, including during unwind.
        let _ = conn::handle(stream, root, read_timeout, write_timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::Root;
    use crate::test_support::TempSite;
    use std::io::Read as _;
    use std::net::{Shutdown, SocketAddr, TcpStream};
    use std::time::Instant;

    /// A shutdown flag that is never set, for tests that don't exercise shutdown.
    static NEVER_SHUTDOWN: AtomicBool = AtomicBool::new(false);

    fn request(addr: SocketAddr, selector: &[u8]) -> Vec<u8> {
        let mut client = TcpStream::connect(addr).expect("connect");
        client.write_all(selector).expect("write selector");
        client.shutdown(Shutdown::Write).expect("shutdown write");
        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("read response");
        response
    }

    fn serve_settings(workers: usize, max_conns_per_ip: u32) -> ServeSettings {
        ServeSettings {
            workers,
            max_conns_per_ip,
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn serves_files_over_loopback() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            let _ = serve(listener, root, serve_settings(4, 5), &NEVER_SHUTDOWN, -1);
        });

        assert_eq!(request(addr, b"a.txt\n"), b"hi\n");
        assert_eq!(request(addr, b"missing\n"), b"document not found");
    }

    #[test]
    fn handles_many_concurrent_clients() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            let _ = serve(listener, root, serve_settings(4, 5), &NEVER_SHUTDOWN, -1);
        });

        let mut clients = Vec::new();
        for _ in 0..16 {
            clients.push(thread::spawn(move || request(addr, b"a.txt\n")));
        }
        for client in clients {
            assert_eq!(client.join().expect("client thread"), b"hi\n");
        }
    }

    #[test]
    fn per_ip_cap_refuses_excess_and_releases_permits() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            let _ = serve(listener, root, serve_settings(4, 2), &NEVER_SHUTDOWN, -1);
        });

        let mut first = TcpStream::connect(addr).expect("first held connection");
        let _second = TcpStream::connect(addr).expect("second held connection");

        let mut refused = TcpStream::connect(addr).expect("excess connection");
        refused
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set refused read timeout");
        let mut refused_body = Vec::new();
        refused
            .read_to_end(&mut refused_body)
            .expect("excess connection should close");
        assert!(
            refused_body.is_empty(),
            "refusal must write no response body"
        );

        first.write_all(b"a.txt\n").expect("finish first request");
        first
            .shutdown(Shutdown::Write)
            .expect("shutdown first write");
        let mut first_body = Vec::new();
        first
            .read_to_end(&mut first_body)
            .expect("read first response");
        assert_eq!(first_body, b"hi\n");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let response = request(addr, b"a.txt\n");
            if response == b"hi\n" {
                break;
            }
            assert!(
                response.is_empty(),
                "retry must be served or silently refused"
            );
            assert!(
                Instant::now() < deadline,
                "released permit was not reusable"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn run_reports_bind_conflict() {
        let site = TempSite::new();
        let occupied = TcpListener::bind("127.0.0.1:0").expect("occupy port");
        let addr = occupied.local_addr().expect("addr");
        let config = ServeConfig {
            root: site.path().to_path_buf(),
            listen: addr,
            workers: 1,
            max_conns_per_ip: 2,
            write_timeout: Duration::from_secs(1),
        };

        let mut banner = Vec::new();
        let err = run(&config, &mut banner).expect_err("bind should fail");
        assert_eq!(
            err.message(),
            format!("could not bind {addr}: address already in use")
        );
        assert!(banner.is_empty(), "banner must not print on bind failure");
    }

    #[test]
    fn shutdown_flag_stops_accept_loop() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle =
            thread::spawn(move || serve(listener, root, serve_settings(4, 5), &shutdown_clone, -1));

        // Verify the server is working first.
        assert_eq!(request(addr, b"a.txt\n"), b"hi\n");

        // Set the shutdown flag and connect to wake the accept loop.
        shutdown.store(true, std::sync::atomic::Ordering::Release);
        // A connection attempt wakes the blocked accept() even without EINTR.
        let _ = TcpStream::connect(addr);

        // The server should exit within a reasonable time.
        let result = handle.join().expect("server thread should not panic");
        assert!(result.is_ok(), "server should exit cleanly on shutdown");
    }

    #[test]
    fn in_flight_request_completes_before_shutdown() {
        let site = TempSite::new();
        site.write("big.txt", b"hello from shutdown test\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle =
            thread::spawn(move || serve(listener, root, serve_settings(4, 5), &shutdown_clone, -1));

        // Start a request, then signal shutdown while it's in flight.
        let mut client = TcpStream::connect(addr).expect("connect");
        client.write_all(b"big.txt\n").expect("write selector");
        client.shutdown(Shutdown::Write).expect("shutdown write");

        // Read the first byte of the response to ensure it's in flight.
        let mut first_byte = [0u8; 1];
        client.read_exact(&mut first_byte).expect("read first byte");
        assert_eq!(first_byte[0], b'h');

        // Signal shutdown after the request is in flight.
        shutdown.store(true, std::sync::atomic::Ordering::Release);

        // The in-flight request should still complete with the rest of the response.
        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("read response");
        assert_eq!(response, b"ello from shutdown test\n");

        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(addr);
        let result = handle.join().expect("server thread should not panic");
        assert!(result.is_ok(), "server should exit cleanly on shutdown");
    }

    /// Setting the shutdown flag while the accept loop is blocked in `poll`
    /// must wake the loop through the self-pipe, with **no** client connection
    /// to nudge it. This is the deterministic form of the signal-delivery race:
    /// before the self-pipe, a flag set between the loop's check and `poll(-1)`
    /// left the loop blocked forever (the other shutdown tests have to connect
    /// a client to wake it). Here we feed the pipe exactly as the signal handler
    /// does and require the loop to exit on its own.
    #[test]
    fn signal_pipe_wakes_accept_loop_without_a_connection() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");

        // A self-pipe matching what `signal::install` builds (non-blocking ends).
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        let (read_fd, write_fd) = (fds[0], fds[1]);
        for &fd in &fds {
            unsafe {
                let fl = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let (done_tx, done_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let r = serve(
                listener,
                root,
                serve_settings(4, 5),
                &shutdown_clone,
                read_fd,
            );
            let _ = done_tx.send(r);
        });

        // Ensure the server has reached the accept loop and is blocked in poll.
        assert_eq!(request(addr, b"a.txt\n"), b"hi\n");

        // Trigger shutdown exactly as the signal handler does: set the flag,
        // then write one byte to the pipe. Crucially, NO connection follows.
        shutdown.store(true, Ordering::Release);
        let byte = 1u8;
        let n =
            unsafe { libc::write(write_fd, std::ptr::addr_of!(byte) as *const libc::c_void, 1) };
        assert_eq!(n, 1, "pipe write");

        // The accept loop must exit promptly off the pipe wakeup alone.
        let result = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("serve must exit after self-pipe wakeup without a connection");
        assert!(result.is_ok(), "server should exit cleanly on shutdown");

        let _ = handle.join();
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
    }
}
