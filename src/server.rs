//! TCP listener and fixed worker-thread pool.
//!
//! The `Root` capability is opened once and shared across workers behind an
//! `Arc`. A rendezvous `sync_channel` plus exactly `workers` threads caps
//! concurrency: a connection is dispatched only when a worker is waiting to
//! receive, so there is no unbounded spawn-per-connection and no async runtime.

use crate::config::{self, ServeConfig};
use crate::conn;
use crate::root::Root;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
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
        TcpListener::bind(config.listen).map_err(|err| ServeError::Bind(config.listen, err))?;
    crate::sandbox::apply(&config.root).map_err(ServeError::Sandbox)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    crate::signal::install(&shutdown);

    // Bind succeeded: this is the startup-success banner.
    let _ = config::write_banner(config, crate::version::version_line(), &mut banner);

    serve(
        listener,
        root,
        config.workers,
        Duration::from_secs(config::READ_TIMEOUT_SECS),
        config.write_timeout,
        &shutdown,
    )
    .map_err(ServeError::Serve)
}

fn serve(
    listener: TcpListener,
    root: Root,
    workers: usize,
    read_timeout: Duration,
    write_timeout: Duration,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let root = Arc::new(root);
    // Rendezvous channel: a send blocks until a worker is waiting to receive,
    // so at most `workers` connections are in flight at once.
    let (tx, rx) = mpsc::sync_channel::<TcpStream>(0);
    let rx = Arc::new(Mutex::new(rx));

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let rx = Arc::clone(&rx);
        let root = Arc::clone(&root);
        handles.push(thread::spawn(move || {
            worker_loop(&rx, &root, read_timeout, write_timeout)
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

            let mut pollfd = libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let res = unsafe { libc::poll(&mut pollfd, 1, -1) };
            if res < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }

            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    if tx.send(stream).is_err() {
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
        for stream in listener.incoming() {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            match stream {
                Ok(stream) => {
                    if tx.send(stream).is_err() {
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

fn worker_loop(
    rx: &Mutex<Receiver<TcpStream>>,
    root: &Root,
    read_timeout: Duration,
    write_timeout: Duration,
) {
    crate::signal::block_signals_on_current_thread();
    loop {
        // Hold the lock only across `recv`; release it before handling so other
        // workers can pick up the next connection. Recover from a poisoned lock
        // rather than panicking, so one failed worker cannot collapse the pool.
        let stream = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
            Ok(stream) => stream,
            Err(_) => return, // channel closed: shut the worker down
        };
        // Connection-level errors are silent; the connection is simply dropped.
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

    #[test]
    fn serves_files_over_loopback() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            let _ = serve(
                listener,
                root,
                4,
                Duration::from_secs(5),
                Duration::from_secs(5),
                &NEVER_SHUTDOWN,
            );
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
            let _ = serve(
                listener,
                root,
                4,
                Duration::from_secs(5),
                Duration::from_secs(5),
                &NEVER_SHUTDOWN,
            );
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
    fn run_reports_bind_conflict() {
        let site = TempSite::new();
        let occupied = TcpListener::bind("127.0.0.1:0").expect("occupy port");
        let addr = occupied.local_addr().expect("addr");
        let config = ServeConfig {
            root: site.path().to_path_buf(),
            listen: addr,
            workers: 1,
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

        let handle = thread::spawn(move || {
            serve(
                listener,
                root,
                4,
                Duration::from_secs(5),
                Duration::from_secs(5),
                &shutdown_clone,
            )
        });

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

        let handle = thread::spawn(move || {
            serve(
                listener,
                root,
                4,
                Duration::from_secs(5),
                Duration::from_secs(5),
                &shutdown_clone,
            )
        });

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
}
