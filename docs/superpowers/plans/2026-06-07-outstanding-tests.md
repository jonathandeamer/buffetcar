# Outstanding Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the two unbuilt tests from the multi-user Nex server design — a deterministic write-timeout test and a multi-user name-swap race stress test.

**Architecture:** The write-timeout test lives in `src/conn.rs`'s unit module and drives `conn::handle` over a real loopback socket whose buffers are shrunk via `libc::setsockopt`, with a client that never reads. The stress test lives in `tests/buffetcar_contract.rs` and drives the public `serve_selector` from several reader threads while one mutator thread cycles a name through file/symlink/FIFO/directory variants via staging-then-`rename`.

**Tech Stack:** Rust, `std::thread::scope`, `std::net` loopback sockets, `libc` (already a dependency: `setsockopt`, `mkfifo`), `rustix`-based resolver under test.

**Spec:** `docs/superpowers/specs/2026-06-07-outstanding-tests-design.md`

**Note on TDD for characterization tests:** These tests assert behavior that *already exists*, so each one PASSES on first run rather than failing first. Non-vacuousness is established by a deliberate-break sanity check step (mutate a constant so the test fails, observe the failure, then revert). Do not skip the sanity check — it is what proves the test actually guards the property.

---

### Task 1: Deterministic write-timeout test

**Files:**
- Modify (test module only): `src/conn.rs` — add to `#[cfg(test)] mod tests` (ends at line 241)

- [ ] **Step 1: Add the socket-buffer-shrinking helper and the test**

In `src/conn.rs`, inside `mod tests` (after `read_timeout_fires_for_a_silent_client`, before the closing `}` of the module), add:

```rust
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
```

- [ ] **Step 2: Run the test, expect PASS**

Run: `cargo test --lib conn::tests::write_timeout_fires_for_a_stalled_reader -- --nocapture`
Expected: `test result: ok. 1 passed`. Should finish in well under a second (≈200ms for the timeout).

- [ ] **Step 3: Sanity check — prove the test is non-vacuous**

Temporarily change the write timeout in the test from `Duration::from_millis(200)` to `Duration::from_secs(30)`.
Run: `cargo test --lib conn::tests::write_timeout_fires_for_a_stalled_reader`
Expected: the test now HANGS (no timeout fires, the write blocks indefinitely) — confirming the assertion depends on the timeout actually firing. Cancel with Ctrl-C.
Then **revert** the value back to `Duration::from_millis(200)` and re-run Step 2 to confirm PASS.

- [ ] **Step 4: Run clippy and fmt on the change**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: no warnings, no diff complaints.

- [ ] **Step 5: Commit**

```bash
git add src/conn.rs
git commit -m "test(conn): assert the write timeout fires for a stalled reader"
```

---

### Task 2: Multi-user name-swap race stress test

**Files:**
- Modify (test code only): `tests/buffetcar_contract.rs` — add one `#[cfg(unix)]` test plus free-function helpers (alongside `make_public`/`make_chain_public` near the end of the file)

Reuses existing free functions in that file: `make_public(&Path, u32)`, `unique_name(prefix, suffix)`.

- [ ] **Step 1: Add the swap helpers**

In `tests/buffetcar_contract.rs`, after `make_chain_public` (end of file), add:

```rust
/// Remove whatever node currently occupies `target` (file, symlink, FIFO, or
/// directory). One of the two calls succeeds; both errors are ignored.
#[cfg(unix)]
fn remove_target(target: &Path) {
    let _ = fs::remove_file(target);
    let _ = fs::remove_dir_all(target);
}

/// Create a self-cleaning sibling directory of `root` (same filesystem, so
/// `rename` works, but outside the served tree).
#[cfg(unix)]
fn sibling_dir(root: &Path, suffix: &str) -> PathBuf {
    let name = format!(
        "{}-{suffix}",
        root.file_name().unwrap().to_str().unwrap()
    );
    let dir = root.parent().unwrap().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).expect("create sibling dir");
    make_public(&dir, 0o755);
    dir
}

#[cfg(unix)]
struct DirGuard(PathBuf);

#[cfg(unix)]
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn make_fifo(p: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).expect("cstring");
    let ret = unsafe { libc::mkfifo(c.as_ptr(), 0o644) };
    assert_eq!(ret, 0, "mkfifo failed");
}

/// Stage one fully-formed variant under `stage/node`, then atomically rename it
/// onto `target`. Staging-then-rename guarantees readers see either an absent
/// `target` or a complete node — never a half-written file or empty directory
/// at construction time.
#[cfg(unix)]
fn swap_in(stage: &Path, secret: &Path, target: &Path, variant: usize) {
    let p = stage.join("node");
    let _ = fs::remove_file(&p);
    let _ = fs::remove_dir_all(&p);
    match variant % 4 {
        0 => {
            fs::write(&p, b"SAFE\n").expect("write staged file");
            make_public(&p, 0o644);
        }
        1 => {
            std::os::unix::fs::symlink(secret, &p).expect("stage symlink");
        }
        2 => {
            make_fifo(&p);
        }
        _ => {
            fs::create_dir(&p).expect("stage dir");
            let child = p.join("child.txt");
            fs::write(&child, b"hi\n").expect("write child");
            make_public(&child, 0o644);
            make_public(&p, 0o755);
        }
    }
    fs::rename(&p, target).expect("rename staged node onto target");
}

/// The only bodies a request for `target` may legitimately return while it is
/// being swapped: the safe file, the directory's listing, a transient empty
/// listing (reader raced the directory teardown), or `document not found`.
fn is_allowed(body: &[u8]) -> bool {
    body == b"SAFE\n"
        || body == b"=> child.txt\n"
        || body.is_empty()
        || body == b"document not found"
}
```

- [ ] **Step 2: Add the stress test**

In `tests/buffetcar_contract.rs`, add this test alongside the other `#[cfg(unix)]` tests (e.g. after `rejects_and_omits_special_files`):

```rust
#[cfg(unix)]
#[test]
fn concurrent_name_swaps_never_serve_outside_or_special_content() {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::thread;

    let site = TempSite::new();
    let root = site.path().to_path_buf();
    let target = root.join("target");

    let stage = sibling_dir(&root, "stage");
    let outside = sibling_dir(&root, "outside");
    let _stage_guard = DirGuard(stage.clone());
    let _outside_guard = DirGuard(outside.clone());
    let secret = outside.join("secret.txt");
    fs::write(&secret, b"SECRET\n").expect("write outside secret");

    const READERS: usize = 3;
    const REQUESTS: usize = 2000;

    let stop = Arc::new(AtomicBool::new(false));
    let failures: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));

    thread::scope(|scope| {
        // Mutator: cycle `target` through file / symlink / FIFO / directory.
        {
            let stop = Arc::clone(&stop);
            let stage = stage.clone();
            let secret = secret.clone();
            let target = target.clone();
            scope.spawn(move || {
                let mut i = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    remove_target(&target);
                    swap_in(&stage, &secret, &target, i);
                    i += 1;
                }
                remove_target(&target);
            });
        }

        // Readers: request "target" repeatedly; record any disallowed body.
        let mut readers = Vec::new();
        for _ in 0..READERS {
            let failures = Arc::clone(&failures);
            let root = root.clone();
            readers.push(scope.spawn(move || {
                for _ in 0..REQUESTS {
                    match buffetcar::serve_selector(&root, "target") {
                        Ok(body) => {
                            if !is_allowed(&body) {
                                failures.lock().unwrap().push(body);
                            }
                        }
                        Err(e) => {
                            failures
                                .lock()
                                .unwrap()
                                .push(format!("Err: {e}").into_bytes());
                        }
                    }
                }
            }));
        }

        for reader in readers {
            reader.join().expect("reader thread panicked");
        }
        stop.store(true, Ordering::Relaxed);
    });

    let failures = failures.lock().unwrap();
    assert!(
        failures.is_empty(),
        "disallowed bodies served during swaps: {:?}",
        failures
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<Vec<_>>()
    );
}
```

- [ ] **Step 3: Run the test, expect PASS**

Run: `cargo test --test buffetcar_contract concurrent_name_swaps_never_serve_outside_or_special_content -- --nocapture`
Expected: `test result: ok. 1 passed`. Sub-second runtime.

- [ ] **Step 4: Sanity check — prove the test catches a leak**

Temporarily add `body == b"SECRET\n"` as an allowed body in `is_allowed`, AND temporarily weaken the resolver to expose the symlink? No — instead, prove the *assertion* catches a violation without touching production code: temporarily change the symlink branch in `swap_in` (variant `1`) to write the secret as a real file instead of a symlink:

```rust
        1 => {
            fs::write(&p, b"SECRET\n").expect("stage leak");
            make_public(&p, 0o644);
        }
```

Run: `cargo test --test buffetcar_contract concurrent_name_swaps_never_serve_outside_or_special_content`
Expected: the test FAILS with `disallowed bodies served during swaps: ["SECRET\n", ...]` — confirming a served `SECRET` body is caught.
Then **revert** the `swap_in` variant `1` branch back to the symlink version and re-run Step 3 to confirm PASS.

- [ ] **Step 5: Run clippy and fmt on the change**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add tests/buffetcar_contract.rs
git commit -m "test: stress name swaps to assert no outside or special content is served"
```

---

### Task 3: Full gate and finish

- [ ] **Step 1: Run the full local gate**

Run: `make check`
Expected: fmt clean, clippy clean (`-D warnings`), all default-suite tests pass including the two new ones.

- [ ] **Step 2: Update the design's outstanding-items status**

In `docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`, the test-coverage section lists these as obligations. They are now met; no spec edit is strictly required, but update `CLAUDE.md`'s "Outstanding spec items" paragraph to drop the stress test and write-timeout test (leaving the README and OpenBSD `pledge`/`unveil` items).

The current sentence reads:
> Outstanding spec items (not yet built; none blocking): a multi-user race **stress test** (...), a **README** ..., a deterministic **write-timeout test** (...), and the **OpenBSD `pledge`/`unveil`** sandbox (...).

Rewrite it to list only the README and the OpenBSD sandbox as outstanding.

- [ ] **Step 3: Commit the doc update**

```bash
git add CLAUDE.md
git commit -m "docs: mark race stress and write-timeout tests as built"
```

- [ ] **Step 4: Push the branch and open a PR**

```bash
git push -u origin test/outstanding-race-and-write-timeout
gh pr create --fill
```

Let CI's 3 required checks run; merge via PR (do not push to `main` directly).

---

## Self-Review Notes

- **Spec coverage:** Task 1 ⇒ write-timeout obligation (spec §"Test 2"); Task 2 ⇒ race stress obligation (spec §"Test 1", allowlist incl. empty listing); Task 3 ⇒ keeps `make check` green and updates status. Out-of-scope items (README, OpenBSD sandbox) intentionally untouched.
- **Type/name consistency:** `swap_in(stage, secret, target, variant)`, `is_allowed(&[u8]) -> bool`, `remove_target(&Path)`, `sibling_dir(&Path, &str) -> PathBuf`, `make_fifo(&Path)`, `DirGuard(PathBuf)` are used consistently across steps. `shrink_buf(RawFd, c_int)` used twice in Task 1. Reused existing `make_public`/`unique_name` signatures verified against the file.
- **No placeholders:** every code step shows complete code; every run step shows the command and expected result.
