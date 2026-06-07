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
use std::io::{self, BufReader, Read, Write};
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
/// place; `selector::parse` strips it. One byte past the cap is buffered so a
/// max-length path sent with CRLF line endings survives — `selector::parse`
/// strips the CR and re-checks the authoritative 1024-byte bound. Reads go
/// through a `BufReader` so the byte-at-a-time scan is not a syscall per byte.
fn read_selector(reader: &mut impl Read) -> io::Result<Option<String>> {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if reader.read(&mut byte)? == 0 {
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        if buf.len() > MAX_SELECTOR_BYTES {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::Root;
    use crate::test_support::TempSite;
    use std::io::Cursor;
    use std::net::{Shutdown, TcpListener, TcpStream};

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
        assert_eq!(
            read_selector(&mut reader).expect("read"),
            Some(String::new())
        );
    }

    #[test]
    fn rejects_selector_past_the_buffer_cap() {
        // The reader buffers at most one byte past the 1024 cap (for a trailing
        // CR); anything longer is rejected here rather than buffered unbounded.
        let mut reader = Cursor::new(vec![b'a'; MAX_SELECTOR_BYTES + 2]);
        assert_eq!(read_selector(&mut reader).expect("read"), None);
    }

    #[test]
    fn buffers_a_max_length_selector_with_trailing_cr() {
        // 1024 path bytes + CR (1025 bytes before '\n') are kept here, then the
        // CR is stripped and the 1024 bound re-checked by selector::parse, so a
        // CRLF client at the limit is served instead of spuriously rejected.
        let mut input = vec![b'a'; MAX_SELECTOR_BYTES];
        input.extend_from_slice(b"\r\n");
        let mut reader = Cursor::new(input);
        let expected = format!("{}\r", "a".repeat(MAX_SELECTOR_BYTES));
        assert_eq!(read_selector(&mut reader).expect("read"), Some(expected));
    }

    #[test]
    fn over_limit_selector_is_not_found_end_to_end() {
        let site = TempSite::new();
        let root = Root::open(site.path()).expect("open root");
        let mut selector = vec![b'a'; MAX_SELECTOR_BYTES + 1];
        selector.push(b'\n');
        assert_eq!(round_trip(&root, &selector), b"document not found");
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
        let result = handle(
            stream,
            &root,
            Duration::from_millis(100),
            Duration::from_secs(5),
        );
        assert!(
            result.is_err(),
            "a silent client should hit the read timeout"
        );
    }

    /// Shrink a socket buffer (`SO_SNDBUF` or `SO_RCVBUF`) so the send path
    /// fills quickly. The kernel may enforce a higher floor; combined with the
    /// oversized payload the write still blocks once the reader stops draining.
    #[cfg(unix)]
    fn shrink_buf(fd: std::os::fd::RawFd, opt: libc::c_int) {
        let size: libc::c_int = 4096;
        let ret = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                opt,
                &size as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        assert_eq!(ret, 0, "setsockopt failed");
    }

    #[cfg(unix)]
    #[test]
    fn write_timeout_fires_for_a_stalled_reader() {
        use std::os::fd::AsRawFd;

        let site = TempSite::new();
        // Larger than any socket buffer, so the server's write_all cannot drain
        // into the kernel and must block on a reader that never reads.
        site.write("big", &vec![b'x'; 8 * 1024 * 1024]);
        let root = Root::open(site.path()).expect("open root");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut client = TcpStream::connect(addr).expect("connect");
        shrink_buf(client.as_raw_fd(), libc::SO_RCVBUF);

        let (server, _) = listener.accept().expect("accept");
        shrink_buf(server.as_raw_fd(), libc::SO_SNDBUF);

        let result = std::thread::scope(|scope| {
            let handle_thread = scope.spawn(|| {
                handle(
                    server,
                    &root,
                    Duration::from_secs(5),
                    Duration::from_millis(200),
                )
            });
            // Ask for the big file, then never read the response.
            client.write_all(b"big\n").expect("write selector");
            handle_thread.join().expect("join handle thread")
        });

        assert!(
            result.is_err(),
            "a reader that never reads should trip the write timeout"
        );
    }
}
