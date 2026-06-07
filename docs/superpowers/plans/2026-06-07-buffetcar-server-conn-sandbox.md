# Buffetcar Server, Conn, Sandbox, And Daemon Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the validated-but-inert `serve` mode into a working blocking Nex daemon: bind a `TcpListener`, run a fixed worker-thread pool, read one bounded selector per connection, stream the response through the existing fd-relative resolver, and close.

**Architecture:** Open the `Root` capability once at startup and share it across workers behind an `Arc`. A rendezvous `sync_channel` plus a fixed number of worker threads caps concurrency at `--workers` with no async runtime or thread-pool dependency. Each connection sets read/write timeouts, reads one `\n`-terminated selector (bounded at 1024 bytes), resolves it with the landed `selector` + `root` + `listing` modules, and streams a regular file (or directory `index`) in fixed-size chunks while bounded listings are buffered. Every unavailable selector collapses to the literal `document not found`. A no-op `sandbox` hook records where a future platform sandbox would attach.

**Tech Stack:** Rust 2021. Standard library only for networking and threading (`std::net::TcpListener`/`TcpStream`, `std::sync::mpsc::sync_channel`, `std::sync::{Arc, Mutex}`, `std::thread`). The existing `rustix` fd-relative resolver and `libc` UID/flag usage are unchanged. No new dependencies.

---

## Scope

**In scope for Plan 3:**

- `src/sandbox.rs`: no-op platform sandbox hook.
- `src/conn.rs`: per-connection timeouts, bounded selector read, response resolution, and chunked streaming.
- `src/server.rs`: listener bind, fixed worker-thread pool, accept loop, and startup error mapping (`ServeError`).
- A streamable directory response: split `listing::serve` into an `index`-or-`generate` path so a directory `index` can be streamed by fd instead of buffered.
- Wiring `serve` into `run_with_io` so the binary actually serves, replacing the temporary "serve networking is not implemented" guard from Plan 2.
- Server/connection unit tests (loopback serving, concurrency, bind conflict, selector bounds, UTF-8/NUL handling, read timeout).
- An architecture guard test asserting the request path never uses whole-path opens or `join`-then-open.

**Out of scope for Plan 3:**

- OpenBSD `pledge`/`unveil` (the resolver does not support OpenBSD; spec: `sandbox` row).
- A deterministic write-timeout unit test (filling a socket send buffer is not reliably reproducible across platforms). The write timeout is still applied to every connection; it is exercised in production, not asserted in a flaky test.
- README publishing docs and the multi-user race stress test (separate work; the resolver-level race policy already landed in earlier plans).

## File Structure

- Create `src/sandbox.rs`: `apply()` no-op hook; the only place a platform sandbox would later attach.
- Create `src/conn.rs`: one public `handle(stream, root, read_timeout, write_timeout)` plus private selector reading, response resolution, and chunked streaming. No path strings; only fd-relative resolver calls.
- Create `src/server.rs`: `run(config, banner)` for the binary (open root, apply sandbox, bind, print banner, serve forever) and a lower-level `serve(listener, root, workers, read_timeout, write_timeout)` that the tests drive directly with injected timeouts. `ServeError` formats actionable startup errors.
- Modify `src/listing.rs`: split `serve` into `serve` (index-or-`generate`) and `generate` (listing bytes only), and borrow the directory fd so `conn` can reuse it for an `index` lookup before generating a listing.
- Modify `src/selector.rs`: expose `MAX_SELECTOR_BYTES` to `conn` as the single source of the 1024-byte bound.
- Modify `src/lib.rs`: declare the three new modules, update the `listing::serve` call site, and replace the `serve` guard with a real `server::run` dispatch.
- Modify `tests/check_contract.rs`: replace the obsolete "stops before networking" serve test with a deterministic bind-conflict test.
- Create `tests/architecture.rs`: guard the request path against whole-path opens and selector `join`.

## Design Invariants (do not violate)

- **One response body for every unavailable selector.** Missing, dotfile, symlink, special file, oversized, invalid-UTF-8, NUL, and root-escape selectors all return exactly `document not found` (no trailing newline) over the wire. Only `check` keeps reason-specific output.
- **No whole-path opens in the request path.** `conn` and `server` resolve only through `selector::parse` + `Root::resolve` + `listing`. Never `File::open(path)`, `std::fs::read*`, `read_dir`, `cap_std`, or `PathBuf::join`-then-open on request data.
- **Root opened once, rechecked per request.** The daemon opens `Root` at startup and shares it; `Root::resolve` already re-stats the root fd on every request, so a root chmod stops new requests without restarting.
- **Concurrency is structurally capped.** Exactly `--workers` threads consume from a rendezvous channel; there is no unbounded spawn-per-connection.

### Task 1: No-Op Sandbox Hook

**Files:**

- Create: `src/sandbox.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Declare the module**

Add this module declaration to the module list near the top of `src/lib.rs` (keep the existing alphabetical grouping):

```rust
mod sandbox;
```

- [ ] **Step 2: Write the failing test**

Create `src/sandbox.rs` with the no-op hook and a test that simply calls it:

```rust
//! Platform sandbox hooks.
//!
//! There is no supported platform sandbox in this build: the banner reports
//! "platform sandbox unavailable" and containment is provided entirely by the
//! fd-relative, no-follow resolver in `root`. OpenBSD `pledge`/`unveil` is out
//! of scope until the resolver supports execute-only traversal on OpenBSD.

/// Apply any available platform sandbox. Currently a deliberate no-op; this is
/// the single attachment point for a future `pledge`/`unveil` or seccomp layer.
pub(crate) fn apply() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_is_callable() {
        apply();
    }
}
```

- [ ] **Step 3: Run the test and verify it passes**

Run:

```bash
cargo fmt --all --check
cargo test sandbox::tests
```

Expected: `apply_is_callable` compiles and passes. (`apply` is unused by non-test code until Task 4 wires it into `server::run`; that is intentional and the full clippy gate runs after wiring in Task 5.)

- [ ] **Step 4: Commit**

Run:

```bash
git add src/lib.rs src/sandbox.rs
git commit -m "feat: add no-op platform sandbox hook"
```

### Task 2: Streamable Directory Response

This splits `listing::serve` so a directory `index` can be streamed by fd later (Task 3) while preserving today's visitor-facing behavior. It is a behavior-preserving refactor gated by the existing contract tests.

**Files:**

- Modify: `src/listing.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Replace `serve` and add `generate`**

In `src/listing.rs`, replace the existing `pub(crate) fn serve(...)` function (the one taking `dir: OwnedFd`) with these two functions. The directory fd is now borrowed so a caller can probe `index` and then generate a listing from the same fd:

```rust
/// Serve a directory: stream its `index` if one is servable, otherwise a
/// generated listing. Used by the `serve_selector` library/compatibility path,
/// which buffers everything into a `Vec`.
pub(crate) fn serve(root: &Root, dir: &OwnedFd) -> io::Result<Vec<u8>> {
    if let Some(bytes) = root.open_index(dir)? {
        return Ok(bytes);
    }
    generate(root, dir)
}

/// Generate a bounded plain-text listing for `dir`, or `document not found`
/// bytes if the directory is not listable or exceeds the listing bounds.
pub(crate) fn generate(root: &Root, dir: &OwnedFd) -> io::Result<Vec<u8>> {
    let Some(list_dir) = root.open_listable_dir(dir)? else {
        return Ok(crate::NOT_FOUND.to_vec());
    };

    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in Dir::read_from(&list_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes.first() == Some(&b'.') {
            continue;
        }
        let Some(child) = root.classify_child(&list_dir, name)? else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
        if entries.len() > MAX_ENTRIES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
    }

    entries.sort();

    let mut out = String::new();
    for (name, is_dir) in entries {
        out.push_str("=> ");
        out.push_str(&name);
        if is_dir {
            out.push('/');
        }
        out.push('\n');
        if out.len() > MAX_BYTES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
    }
    Ok(out.into_bytes())
}
```

- [ ] **Step 2: Update the `serve_selector` call site**

In `src/lib.rs`, the `serve_selector` match now borrows the directory fd. Change the `Resolved::Dir` arm:

```rust
        Some(Resolved::Dir(fd)) => listing::serve(&root, &fd),
```

- [ ] **Step 3: Run the existing contract tests**

Run:

```bash
cargo fmt --all --check
cargo test buffetcar_contract
```

Expected: all `buffetcar_contract` tests still pass (index serving, sorted listings, dotfile/symlink/special omission, listing bounds). Behavior is unchanged; only the internal split and fd borrowing changed.

- [ ] **Step 4: Commit**

Run:

```bash
git add src/listing.rs src/lib.rs
git commit -m "refactor: split streamable directory response"
```

### Task 3: Per-Connection Handling

**Files:**

- Create: `src/conn.rs`
- Modify: `src/selector.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Expose the selector bound and declare the module**

In `src/selector.rs`, make the byte bound visible to `conn` (single source of truth). Change:

```rust
const MAX_SELECTOR_BYTES: usize = 1024;
```

to:

```rust
pub(crate) const MAX_SELECTOR_BYTES: usize = 1024;
```

In `src/lib.rs`, add the module declaration to the module list near the top:

```rust
mod conn;
```

- [ ] **Step 2: Create `conn.rs` with stubs and failing tests**

Create `src/conn.rs` with stub `handle`/`read_selector` plus the full test module below. The stubs compile but the tests fail.

```rust
//! Per-connection handling: timeouts, one bounded selector, streamed response.
//!
//! This module resolves only through `selector::parse` + `Root::resolve` +
//! `listing`. It never opens a whole request path, never joins selector bytes
//! into a path, and collapses every unavailable selector to `document not
//! found`. Regular files and directory `index` files stream in fixed-size
//! chunks; bounded directory listings are buffered.

use crate::root::Root;
use std::io::{self, Read};
use std::net::TcpStream;
use std::time::Duration;

pub(crate) fn handle(
    mut stream: TcpStream,
    _root: &Root,
    read_timeout: Duration,
    write_timeout: Duration,
) -> io::Result<()> {
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(write_timeout))?;
    Ok(())
}

fn read_selector(_reader: &mut impl Read) -> io::Result<Option<String>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::Root;
    use std::fs;
    use std::io::{Cursor, Read as _, Write as _};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn round_trip(root: &Root, request: &[u8]) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let (stream, _) = listener.accept().expect("accept");
                let _ = handle(stream, root, Duration::from_secs(5), Duration::from_secs(5));
            });
            let mut client = TcpStream::connect(addr).expect("connect");
            client.write_all(request).expect("write selector");
            client.shutdown(Shutdown::Write).expect("shutdown write");
            let mut response = Vec::new();
            client.read_to_end(&mut response).expect("read response");
            response
        })
    }

    #[test]
    fn reads_a_newline_terminated_selector() {
        let mut reader = Cursor::new(b"hello\nextra".to_vec());
        assert_eq!(
            read_selector(&mut reader).expect("read"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn reads_an_empty_selector_at_eof() {
        let mut reader = Cursor::new(Vec::new());
        assert_eq!(read_selector(&mut reader).expect("read"), Some(String::new()));
    }

    #[test]
    fn rejects_oversized_selector() {
        let mut reader = Cursor::new(vec![b'a'; 1025]);
        assert_eq!(read_selector(&mut reader).expect("read"), None);
    }

    #[test]
    fn rejects_non_utf8_selector() {
        let mut reader = Cursor::new(vec![0xff, b'\n']);
        assert_eq!(read_selector(&mut reader).expect("read"), None);
    }

    #[test]
    fn serves_file_bytes() {
        let site = TempSite::new();
        site.write("a.txt", b"hello\n");
        let root = Root::open(site.path()).expect("open root");
        assert_eq!(round_trip(&root, b"a.txt\n"), b"hello\n");
    }

    #[test]
    fn streams_directory_index() {
        let site = TempSite::new();
        site.write("docs/index", b"INDEX\n");
        let root = Root::open(site.path()).expect("open root");
        assert_eq!(round_trip(&root, b"docs/\n"), b"INDEX\n");
    }

    #[test]
    fn generates_directory_listing() {
        let site = TempSite::new();
        site.write("d/x.txt", b"x\n");
        let root = Root::open(site.path()).expect("open root");
        assert_eq!(round_trip(&root, b"d\n"), b"=> x.txt\n");
    }

    #[test]
    fn maps_unavailable_selectors_to_not_found() {
        let site = TempSite::new();
        let root = Root::open(site.path()).expect("open root");
        assert_eq!(round_trip(&root, b"missing\n"), b"document not found");
        assert_eq!(round_trip(&root, b".secret\n"), b"document not found");
    }

    #[test]
    fn read_timeout_fires_for_a_silent_client() {
        let site = TempSite::new();
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let _client = TcpStream::connect(addr).expect("connect"); // connects, never sends
        let (stream, _) = listener.accept().expect("accept");
        let result = handle(stream, &root, Duration::from_millis(100), Duration::from_secs(5));
        assert!(result.is_err(), "a silent client should hit the read timeout");
    }

    struct TempSite {
        path: PathBuf,
    }

    impl TempSite {
        fn new() -> Self {
            let path = std::env::temp_dir().join(unique_name("buffetcar-conn", ""));
            fs::create_dir(&path).expect("create temp site root");
            #[cfg(unix)]
            make_public(&path, 0o755);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write(&self, relative: &str, content: &[u8]) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent directory");
                #[cfg(unix)]
                make_chain_public(&self.path, parent);
            }
            fs::write(&path, content).expect("write fixture file");
            #[cfg(unix)]
            make_public(&path, 0o644);
        }
    }

    impl Drop for TempSite {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn unique_name(prefix: &str, suffix: &str) -> String {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{}-{n}{suffix}", std::process::id())
    }

    #[cfg(unix)]
    fn make_public(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod fixture");
    }

    #[cfg(unix)]
    fn make_chain_public(root: &Path, leaf: &Path) {
        let mut dir = Some(leaf);
        while let Some(d) = dir {
            make_public(d, 0o755);
            if d == root {
                break;
            }
            dir = d.parent();
        }
    }
}
```

- [ ] **Step 3: Run the tests and verify they fail**

Run:

```bash
cargo test conn::tests
```

Expected: `read_selector` tests fail (stub returns `None`), and the `round_trip` tests fail because `handle` writes nothing (client reads empty). `read_timeout_fires_for_a_silent_client` may already pass; that is fine.

- [ ] **Step 4: Implement the connection handler**

Replace the production part of `src/conn.rs` (everything above `#[cfg(test)] mod tests`) with the full implementation below. Leave the `#[cfg(test)] mod tests { ... }` block unchanged.

```rust
//! Per-connection handling: timeouts, one bounded selector, streamed response.
//!
//! This module resolves only through `selector::parse` + `Root::resolve` +
//! `listing`. It never opens a whole request path, never joins selector bytes
//! into a path, and collapses every unavailable selector to `document not
//! found`. Regular files and directory `index` files stream in fixed-size
//! chunks; bounded directory listings are buffered.

use crate::root::{Child, Resolved, Root};
use crate::selector::{self, MAX_SELECTOR_BYTES};
use crate::{listing, NOT_FOUND};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::fd::OwnedFd;
use std::time::Duration;

/// Streaming chunk size for regular files and directory `index` files.
const CHUNK_SIZE: usize = 64 * 1024;

/// A resolved response: either a file descriptor to stream, or buffered bytes
/// (a generated listing or the `document not found` body).
enum Response {
    File(OwnedFd),
    Bytes(Vec<u8>),
}

/// Handle one connection end to end: apply timeouts, read one selector, resolve
/// it, and stream the response. Connection-level conditions (slow client read
/// timeout, disconnect, oversized/invalid selector) are silent. I/O faults on
/// already-opened descriptors propagate as `Err` for the caller to drop.
pub(crate) fn handle(
    mut stream: TcpStream,
    root: &Root,
    read_timeout: Duration,
    write_timeout: Duration,
) -> io::Result<()> {
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(write_timeout))?;

    let Some(selector) = read_selector(&mut stream)? else {
        return stream.write_all(NOT_FOUND);
    };
    let response = resolve(root, &selector)?;
    write_response(&mut stream, response)
}

/// Read one selector line: bytes up to a `\n` (excluded) or EOF, bounded at
/// `MAX_SELECTOR_BYTES`. Returns `None` for an oversized or non-UTF-8 selector,
/// which the caller maps to `document not found`. A trailing `\r` is left in
/// place; `selector::parse` strips it.
fn read_selector(reader: &mut impl Read) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if reader.read(&mut byte)? == 0 {
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        if buf.len() == MAX_SELECTOR_BYTES {
            return Ok(None);
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8(buf).ok())
}

fn resolve(root: &Root, selector: &str) -> io::Result<Response> {
    let Some(request) = selector::parse(selector) else {
        return Ok(Response::Bytes(NOT_FOUND.to_vec()));
    };
    match root.resolve(&request)? {
        Some(Resolved::File(fd)) => Ok(Response::File(fd)),
        Some(Resolved::Dir(dir)) => directory_response(root, dir),
        None => Ok(Response::Bytes(NOT_FOUND.to_vec())),
    }
}

fn directory_response(root: &Root, dir: OwnedFd) -> io::Result<Response> {
    if let Some(Child::File(fd)) = root.classify_child(&dir, "index")? {
        return Ok(Response::File(fd));
    }
    Ok(Response::Bytes(listing::generate(root, &dir)?))
}

fn write_response(out: &mut impl Write, response: Response) -> io::Result<()> {
    match response {
        Response::File(fd) => stream_file(fd, out),
        Response::Bytes(bytes) => out.write_all(&bytes),
    }
}

fn stream_file(fd: OwnedFd, out: &mut impl Write) -> io::Result<()> {
    let mut file = File::from(fd);
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        out.write_all(&buf[..read])?;
    }
    Ok(())
}
```

- [ ] **Step 5: Run the tests and formatting**

Run:

```bash
cargo fmt --all --check
cargo test conn::tests
```

Expected: all `conn::tests` pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add src/selector.rs src/lib.rs src/conn.rs
git commit -m "feat: add per-connection selector handling"
```

### Task 4: Threaded Listener And Worker Pool

**Files:**

- Create: `src/server.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Declare the module**

In `src/lib.rs`, add the module declaration to the module list near the top:

```rust
mod server;
```

- [ ] **Step 2: Create `server.rs` with stubs and failing tests**

Create `src/server.rs` with stub `run`/`serve` plus the full test module below. The stubs compile but the tests fail.

```rust
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
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A startup failure for `serve`. Carries enough context to print one
/// actionable `error:` line without leaking internal type names.
pub(crate) enum ServeError {
    Bind(SocketAddr, io::Error),
    Root(io::Error),
    Serve(io::Error),
}

impl ServeError {
    pub(crate) fn message(&self) -> String {
        match self {
            ServeError::Bind(addr, err) => format!("could not bind {addr}: {}", bind_reason(err)),
            ServeError::Root(err) => format!("could not open root: {err}"),
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

/// Open the root, apply the sandbox, bind the listener, print the success
/// banner, then serve forever. Returns only on a fatal accept-loop end or a
/// startup error; the banner is written only after a successful bind.
pub(crate) fn run(config: &ServeConfig, _banner: impl Write) -> Result<(), ServeError> {
    Err(ServeError::Root(io::Error::other("server not wired")))
}

fn serve(
    _listener: TcpListener,
    _root: Root,
    _workers: usize,
    _read_timeout: Duration,
    _write_timeout: Duration,
) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::Root;
    use std::fs;
    use std::io::{Read as _, Write as _};
    use std::net::{Shutdown, SocketAddr, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

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

    struct TempSite {
        path: PathBuf,
    }

    impl TempSite {
        fn new() -> Self {
            let path = std::env::temp_dir().join(unique_name("buffetcar-server", ""));
            fs::create_dir(&path).expect("create temp site root");
            #[cfg(unix)]
            make_public(&path, 0o755);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write(&self, relative: &str, content: &[u8]) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent directory");
                #[cfg(unix)]
                make_chain_public(&self.path, parent);
            }
            fs::write(&path, content).expect("write fixture file");
            #[cfg(unix)]
            make_public(&path, 0o644);
        }
    }

    impl Drop for TempSite {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn unique_name(prefix: &str, suffix: &str) -> String {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{}-{n}{suffix}", std::process::id())
    }

    #[cfg(unix)]
    fn make_public(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod fixture");
    }

    #[cfg(unix)]
    fn make_chain_public(root: &Path, leaf: &Path) {
        let mut dir = Some(leaf);
        while let Some(d) = dir {
            make_public(d, 0o755);
            if d == root {
                break;
            }
            dir = d.parent();
        }
    }
}
```

- [ ] **Step 3: Run the tests and verify they fail**

Run:

```bash
cargo test server::tests
```

Expected: `serves_files_over_loopback` and `handles_many_concurrent_clients` fail (stub `serve` accepts nothing, so clients hang on connect/read or get empty responses), and `run_reports_bind_conflict` fails because the stub `run` returns `ServeError::Root`, not `Bind`.

> Note: if the two `serve` tests hang rather than fail fast against the stub, that confirms the accept loop is missing; proceed to the implementation in Step 4, which makes them terminate.

- [ ] **Step 4: Implement the listener and worker pool**

In `src/server.rs`, replace the stub `run` and `serve` functions (keep `ServeError`, `bind_reason`, and the `#[cfg(test)] mod tests` block) with the full implementation and the worker loop:

```rust
pub(crate) fn run(config: &ServeConfig, mut banner: impl Write) -> Result<(), ServeError> {
    let root = Root::open(&config.root).map_err(ServeError::Root)?;
    crate::sandbox::apply();
    let listener =
        TcpListener::bind(config.listen).map_err(|err| ServeError::Bind(config.listen, err))?;

    // Bind succeeded: this is the startup-success banner.
    let _ = config::write_banner(config, &mut banner);

    serve(
        listener,
        root,
        config.workers,
        Duration::from_secs(config::READ_TIMEOUT_SECS),
        config.write_timeout,
    )
    .map_err(ServeError::Serve)
}

fn serve(
    listener: TcpListener,
    root: Root,
    workers: usize,
    read_timeout: Duration,
    write_timeout: Duration,
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

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if tx.send(stream).is_err() {
                    break; // all workers have gone away
                }
            }
            // Per-accept errors (e.g. fd exhaustion) are transient and silent.
            Err(_) => continue,
        }
    }

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
    loop {
        // Hold the lock only across `recv`; release it before handling so other
        // workers can pick up the next connection.
        let stream = match rx.lock().unwrap().recv() {
            Ok(stream) => stream,
            Err(_) => return, // channel closed: shut the worker down
        };
        // Connection-level errors are silent; the connection is simply dropped.
        let _ = conn::handle(stream, root, read_timeout, write_timeout);
    }
}
```

- [ ] **Step 5: Run the tests and formatting**

Run:

```bash
cargo fmt --all --check
cargo test server::tests
```

Expected: all `server::tests` pass. The two loopback/concurrency tests leave a detached server thread running for the rest of the test binary; that is harmless (each binds a distinct ephemeral port).

- [ ] **Step 6: Commit**

Run:

```bash
git add src/lib.rs src/server.rs
git commit -m "feat: add threaded listener and worker pool"
```

### Task 5: Serve The Nex Daemon

**Files:**

- Modify: `src/lib.rs`
- Modify: `tests/check_contract.rs`

- [ ] **Step 1: Wire `serve` into `run_with_io`**

In `src/lib.rs`, replace the temporary serve guard arm:

```rust
        Ok(config::RunMode::Serve(config)) => {
            let _ = config::write_banner(&config, &mut *err);
            let _ = writeln!(
                err,
                "error: serve networking is not implemented in this build"
            );
            2
        }
```

with a real dispatch into `server::run` (the banner is now printed inside `server::run` after a successful bind):

```rust
        Ok(config::RunMode::Serve(config)) => match server::run(&config, &mut *err) {
            Ok(()) => 0,
            Err(error) => {
                let _ = writeln!(err, "error: {}", error.message());
                1
            }
        },
```

- [ ] **Step 2: Replace the obsolete serve integration test**

In `tests/check_contract.rs`, delete the `serve_validates_config_and_stops_before_networking` test entirely (it asserted the removed guard, and serving now blocks forever instead of exiting). Add this deterministic bind-conflict test in its place (it returns immediately because the bind fails before the accept loop):

```rust
#[test]
fn serve_reports_bind_conflict_with_actionable_error() {
    let site = TempSite::new();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupy port");
    let addr = occupied.local_addr().expect("addr");

    let output = buffetcar(&[
        "serve",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "--listen",
        &addr.to_string(),
    ]);

    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!("error: could not bind {addr}: address already in use\n")
    );
}
```

Leave `invalid_listen_returns_two_before_serve_networking_guard` as-is: invalid `--listen` is still rejected during config validation (exit code 2) before any bind.

- [ ] **Step 3: Run the full local gate**

Run:

```bash
make check
```

Expected: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` all pass. With `serve` now wired into non-test code, the new `conn`/`server`/`sandbox` items are reachable and clippy reports no dead code.

- [ ] **Step 4: Commit**

Run:

```bash
git add src/lib.rs tests/check_contract.rs
git commit -m "feat: serve the nex daemon"
```

### Task 6: Architecture Guard Test

**Files:**

- Create: `tests/architecture.rs`

- [ ] **Step 1: Write the failing-by-construction guard test**

Create `tests/architecture.rs`. It reads each request-path module's production source (the text before its `#[cfg(test)]` block) and asserts the forbidden whole-path open helpers and selector `join` never appear.

```rust
//! Architecture guard: the request path must resolve only through the
//! fd-relative resolver. It must never open a whole request path or assemble
//! selector components into a path with `join`.

use std::fs;
use std::path::Path;

/// Modules on the network request path.
const REQUEST_PATH_MODULES: &[&str] = &[
    "src/lib.rs",
    "src/selector.rs",
    "src/root.rs",
    "src/listing.rs",
    "src/conn.rs",
    "src/server.rs",
];

/// Whole-path open helpers that bypass per-component `openat` + `O_NOFOLLOW`.
const FORBIDDEN_OPENS: &[&str] = &[
    "File::open",
    "fs::read(",
    "read_to_string",
    "read_dir",
    "cap_std",
    "canonicalize",
];

/// Modules that consume selector components; none may build a path with `join`.
const SELECTOR_PATH_MODULES: &[&str] = &[
    "src/selector.rs",
    "src/root.rs",
    "src/listing.rs",
    "src/conn.rs",
    "src/server.rs",
];

/// Read a source file and return only its production portion (everything before
/// the first `#[cfg(test)]`), so test fixtures using `join`/`fs` do not trip the
/// guard.
fn production_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {relative}: {err}"));
    match text.find("#[cfg(test)]") {
        Some(idx) => text[..idx].to_string(),
        None => text,
    }
}

#[test]
fn request_path_never_uses_whole_path_opens() {
    for module in REQUEST_PATH_MODULES {
        let src = production_source(module);
        for needle in FORBIDDEN_OPENS {
            assert!(
                !src.contains(needle),
                "{module} uses forbidden whole-path open helper `{needle}`"
            );
        }
    }
}

#[test]
fn request_path_never_joins_selector_components() {
    for module in SELECTOR_PATH_MODULES {
        let src = production_source(module);
        assert!(
            !src.contains(".join("),
            "{module} assembles a path with `.join(` on the request path"
        );
    }
}
```

- [ ] **Step 2: Run the guard test**

Run:

```bash
cargo fmt --all --check
cargo test --test architecture
```

Expected: both guard tests pass. The current production code uses only `openat`/`fstat`/`statat` (`root`), `Dir::read_from` (`listing`), and `File::from(fd)` (`conn`/`lib`) — none of which match the forbidden substrings — and no production module joins selector components.

- [ ] **Step 3: Run the full local gate**

Run:

```bash
make check
```

Expected: full `fmt`, `clippy -D warnings`, and `test` all pass, now including `tests/architecture.rs`.

- [ ] **Step 4: Commit**

Run:

```bash
git add tests/architecture.rs
git commit -m "test: guard request path against whole-path opens"
```

## Self-Review

**Spec coverage (Plan 3 slice):**

- `server` module — listener bind + fixed worker pool: Task 4.
- `conn` module — timeouts, one bounded selector, streamed response: Task 3.
- `sandbox` module — no-op hook where unavailable: Task 1.
- Root opened once at startup, shared, rechecked per request: Task 4 (`run` opens once; `Root::resolve` rechecks).
- Files streamed in fixed-size chunks, not one allocation: Task 3 (`stream_file`, `CHUNK_SIZE`), enabled by Task 2 (`index` streamed by fd).
- Every unavailable selector → `document not found`: Task 3 (`resolve`/`read_selector` failure paths).
- Selector bound (1024), invalid UTF-8, NUL handled: Task 3 (`read_selector` + `selector::parse`).
- Read timeout for slow clients; write timeout applied: Task 3 (`handle` sets both; read timeout asserted). Write-timeout assertion intentionally omitted (see Scope).
- Worker cap / concurrency under many clients: Task 4 (rendezvous channel; concurrency smoke test).
- Loopback default bind: Task 4 (`serves_files_over_loopback`); default `127.0.0.1:1900` comes from landed `config::DEFAULT_LISTEN`.
- Startup banner on success; actionable bind-failure error, non-zero exit: Tasks 4–5 (`run` banner-after-bind; `ServeError`; `serve_reports_bind_conflict_with_actionable_error`).
- Architecture guard tests (no whole-path opens, no selector `join`): Task 6.
- Refusal to run as root, invalid root/listen/workers/write-timeout: already landed in Plan 2 `config` and its tests; unchanged here.

**Deferred to other work (noted in Scope):** OpenBSD sandbox, deterministic write-timeout test, README publishing docs, multi-user race stress test.

**Type/signature consistency:** `listing::serve(&Root, &OwnedFd)` and `listing::generate(&Root, &OwnedFd)` match the `serve_selector` and `conn::directory_response` call sites; `Response::{File(OwnedFd), Bytes(Vec<u8>)}` matches `write_response`/`stream_file`; `server::run(&ServeConfig, impl Write) -> Result<(), ServeError>` matches the `run_with_io` dispatch; `conn::handle(TcpStream, &Root, Duration, Duration)` matches both the worker loop and the unit tests; `selector::MAX_SELECTOR_BYTES` is the single bound used by `read_selector`.

**Placeholder scan:** every code step contains complete code; no TBD/"add error handling"/"similar to" placeholders.
