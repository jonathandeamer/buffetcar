# Buffetcar fd-relative Resolver Core — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate buffetcar's request path off `cap-std` onto an explicit, fd-relative `rustix` resolver (`openat` + `O_NOFOLLOW` + `fstat`) that enforces the multi-user public-content policy, so containment and the no-symlink/no-special/no-hardlink/world-readable-file/world-executable-directory invariants hold structurally under a local untrusted-writer threat model.

**Architecture:** Split the request path into three modules behind the existing `serve_selector(root, selector)` wrapper. `selector` parses and *lexically* normalizes the selector into a list of plain components plus directory intent (no filesystem access). `root` owns the root directory fd and walks one component at a time with `openat(O_NOFOLLOW | O_NONBLOCK)`, using search-only directory opens for descent so execute-only public directories remain traversable. Every opened descriptor, including the root at the start of each request, is `fstat`-checked for type, device, mode bits, and link count before use. `listing` reuses those fd-relative checks to find a public `index`; it opens a separate readable directory fd only after the directory is world-readable. No whole-path open and no `PathBuf::join`-then-open ever touches the request path.

**Tech Stack:** Rust 2021. `rustix` 1.1 (feature `fs`) for `openat`/`fstat`/`Dir`; direct `libc` only for platform `O_SEARCH` constants not exposed by `rustix::fs::OFlags`. Standard library for `OwnedFd` → `File` reads. Tests use the existing `tests/buffetcar_contract.rs` harness (`TempSite`, `respond`, `unique_name`).

## Plan of Record

This is **Plan 1 of 3** implementing `docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`. It replaces the superseded `cap-std` hardening plan (`2026-06-06-buffetcar-no-symlink-hardening.md`).

- **Plan 1 (this plan):** `selector` + `root` + `listing` — the resolver/security core, fully testable through `serve_selector` with no networking.
- **Plan 2 (later):** `cli` + `config` + `check` local-diagnostics mode — a runnable binary, still no sockets.
- **Plan 3 (later):** `server` + `conn` + `sandbox` + `main` — listener, worker pool, timeouts, and platform sandbox hooks. OpenBSD `pledge`/`unveil` remains desirable, but OpenBSD is not a supported resolver target until execute-only directory traversal can be implemented without weakening the public-directory policy.

**In scope here:** spec sections "Filesystem Resolver", "Public Content Policy", "Directory Listings", the `selector`/`root`/`listing` modules, and the resolver/containment/listing portions of "Testing". The public `serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>>` entry point is preserved as the thin library/test wrapper the spec calls for (it opens a `Root` per call; the daemon will open one at startup).

**Out of scope here (later plans):** the binary, `main.rs`, CLI parsing, config validation, the startup banner, worker pool, socket/read/write timeouts, raw-byte selector reading from the wire, `check` mode, `sandbox`, run-as-root refusal, and the architecture-guard tests (added with the daemon).

**Threat-model note:** mode/link-count/type/device checks are point-in-time checks on the *opened* descriptor, which is the commit point. The spec accepts that a file public when its fd is accepted may keep streaming if chmodded mid-stream. `O_NOFOLLOW` refuses a symlink as a path component atomically at open; `O_NONBLOCK` ensures a FIFO/device component is opened (for its type check) without blocking. Directory descent must not request read permission; otherwise a world-executable but non-world-readable directory would be incorrectly unavailable before policy checks can accept it for direct child access. Because `selector` balances `..` lexically and `root` never opens `..` or follows a symlink, lexical and physical paths cannot diverge.

---

## File Structure

- `Cargo.toml` (modify) — drop `cap-std`; add `rustix = { version = "1.1", features = ["fs"] }` and `libc = "0.2"` for target-specific search-only directory flags.
- `src/lib.rs` (rewrite) — public `serve_selector`; `NOT_FOUND` const; `read_file` helper; `mod selector; mod root; mod listing;`. Dispatches a resolved file/dir to a read or a listing.
- `src/selector.rs` (create) — `Request` struct and `parse()`: byte-length cap, NUL rejection, trailing-CR trim, dotfile rejection, lexical `..` balancing/escape, trailing-slash directory intent. Pure; no filesystem access. Inline unit tests.
- `src/root.rs` (create) — `Root` capability (root dir fd + device id), `Resolved`/`Child` enums, the no-follow component walk, and the `fstat`-based public-content predicates reused by `listing`.
- `src/listing.rs` (create) — `serve()`: public-`index` lookup, then a bounded, sorted plain-text listing that re-opens each entry fd-relative under the same policy.
- `tests/buffetcar_contract.rs` (modify) — make fixtures set deterministic permissions; add `symlink`/`write_mode`/`dir_mode`/`fifo` helpers; add containment, policy, and listing-bound tests.

---

## Task 1: `selector` — parse and lexically normalize (pure, no filesystem)

`selector` turns a selector string into a `Request` (a list of normal components + directory intent) or rejects it. It is pure string logic so it is unit-tested in isolation and does not touch `cap-std`/`rustix`, keeping the tree compiling and green while the filesystem rewrite waits for Task 2.

**Files:**
- Create: `src/selector.rs`
- Modify: `src/lib.rs` (add `mod selector;` only)

- [ ] **Step 1: Create `src/selector.rs` with the parser and its failing-by-absence unit tests**

```rust
//! Parse and lexically normalize a Nex selector into safe path components.
//!
//! This module performs no filesystem access. It produces a list of *normal*
//! components (never empty, `.`, or `..`) plus directory intent. Lexical `..`
//! balancing is sound only because the resolver in `root` never follows a
//! symlink, so the lexical parent of a component is always its physical parent.

/// Hardcoded selector byte bound (spec: "Networking And Resource Bounds").
const MAX_SELECTOR_BYTES: usize = 1024;

/// A normalized request: normal path components in order, plus whether a
/// trailing slash expressed directory intent (which forbids a regular-file
/// resolution).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) components: Vec<String>,
    pub(crate) dir_only: bool,
}

/// Parse a selector into a `Request`, or `None` when it is unavailable by policy:
/// over the byte bound, containing a NUL, naming a dotfile component, or escaping
/// above the root via unbalanced `..`.
pub(crate) fn parse(selector: &str) -> Option<Request> {
    if selector.len() > MAX_SELECTOR_BYTES {
        return None;
    }
    // Tolerate one trailing CR (a CRLF remnant); an interior NUL is never a
    // valid path byte.
    let selector = selector.strip_suffix('\r').unwrap_or(selector);
    if selector.contains('\0') {
        return None;
    }

    let dir_only = selector.ends_with('/');

    let mut components: Vec<String> = Vec::new();
    for raw in selector.split('/') {
        match raw {
            "" | "." => continue,
            ".." => {
                // Balanced `..` is allowed; `..` above the root is an escape.
                if components.pop().is_none() {
                    return None;
                }
            }
            name => {
                // The dotfile rule applies to each *original* normal component,
                // before any later `..` could cancel it, so a dotfile probe
                // cannot hide behind a trailing parent component.
                if name.starts_with('.') {
                    return None;
                }
                components.push(name.to_owned());
            }
        }
    }

    Some(Request {
        components,
        dir_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(components: &[&str], dir_only: bool) -> Option<Request> {
        Some(Request {
            components: components.iter().map(|s| s.to_string()).collect(),
            dir_only,
        })
    }

    #[test]
    fn empty_and_slash_resolve_to_root_directory() {
        assert_eq!(parse(""), req(&[], false));
        assert_eq!(parse("/"), req(&[], true));
    }

    #[test]
    fn trims_leading_and_interior_empty_components() {
        assert_eq!(parse("/plain.txt"), req(&["plain.txt"], false));
        assert_eq!(parse("a//b"), req(&["a", "b"], false));
    }

    #[test]
    fn trailing_slash_sets_directory_intent() {
        assert_eq!(parse("docs/"), req(&["docs"], true));
        assert_eq!(parse("docs"), req(&["docs"], false));
    }

    #[test]
    fn balances_parent_components_and_rejects_escape() {
        assert_eq!(parse("a/b/../c.txt"), req(&["a", "c.txt"], false));
        assert_eq!(parse("../outside"), None);
        assert_eq!(parse("a/../../escape"), None);
    }

    #[test]
    fn rejects_dotfile_components_before_parent_cancellation() {
        assert_eq!(parse(".secret"), None);
        assert_eq!(parse("listing/.hidden"), None);
        // `.secret` is seen and rejected even though `..` would cancel it.
        assert_eq!(parse(".secret/../public"), None);
    }

    #[test]
    fn rejects_nul_and_oversized_selectors() {
        assert_eq!(parse("a\0b"), None);
        let oversized = "a".repeat(1025);
        assert_eq!(parse(&oversized), None);
        let at_limit = "a".repeat(1024);
        assert_eq!(parse(&at_limit), req(&[&at_limit], false));
    }

    #[test]
    fn tolerates_one_trailing_carriage_return() {
        assert_eq!(parse("plain.txt\r"), req(&["plain.txt"], false));
    }
}
```

- [ ] **Step 2: Declare the module in `src/lib.rs`**

Add this line to `src/lib.rs` alongside the existing top-of-file items (above `pub fn serve_selector`):

```rust
mod selector;
```

The module is `dead_code` until Task 2 wires it in. Add the allow at the `mod` site so the build stays warning-clean for exactly one task:

```rust
#[allow(dead_code)] // wired into serve_selector in Task 2
mod selector;
```

- [ ] **Step 3: Run the selector unit tests**

Run: `cargo test --lib selector`
Expected: PASS — all seven `selector::tests::*` functions green.

- [ ] **Step 4: Verify formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS, no clippy output.

- [ ] **Step 5: Commit**

```bash
git add src/selector.rs src/lib.rs
git commit -m "feat: add lexical selector parser"
```

---

## Task 2: `root` + `listing` — port the request path onto the fd-relative resolver

Replace `cap-std` with `rustix`. The resolver walks components with `openat(O_NOFOLLOW)`, so symlinks are refused atomically; `fstat` discriminates files from directories and rejects special files and cross-device entries by type/device. This task ports *all* existing contract behavior onto the new primitive (type + device policy only — mode/link-count checks land in Task 3) and adds the symlink/special-file containment tests the new design enables.

**Files:**
- Modify: `Cargo.toml`
- Create: `src/root.rs`
- Create: `src/listing.rs`
- Modify: `src/lib.rs`
- Modify: `tests/buffetcar_contract.rs`

- [ ] **Step 1: Swap the dependency in `Cargo.toml`**

Replace the `[dependencies]` section:

```toml
[dependencies]
cap-std = "4.0.2"
```

with:

```toml
[dependencies]
rustix = { version = "1.1", features = ["fs"] }
libc = "0.2"
```

Leave `[features]`, `[package]`, and the `[[test]]` block unchanged.

- [ ] **Step 2: Make test fixtures set deterministic permissions, and add a `symlink` helper**

The new policy reads mode bits (in Task 3), so positive fixtures must be world-readable/executable regardless of the test process umask. In `tests/buffetcar_contract.rs`, add a private free function and rewrite `TempSite::new`/`write`/`dir`, then add a `symlink` helper.

Add this free function near `unique_name`:

```rust
#[cfg(unix)]
fn make_public(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod fixture");
}

/// Chmod every directory from `leaf` up to (and including) `root` to `0o755`, so
/// a restrictive umask cannot turn an intended-public fixture into a policy
/// rejection.
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
```

Replace `TempSite::new`, `write`, and `dir` with:

```rust
    fn new() -> Self {
        let path = std::env::temp_dir().join(unique_name("buffetcar-contract", ""));
        fs::create_dir(&path).expect("create temp site root");
        #[cfg(unix)]
        make_public(&path, 0o755);
        Self { path }
    }

    fn write(&self, relative: &str, content: &[u8]) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, content).expect("write fixture file");
        #[cfg(unix)]
        {
            make_public(&path, 0o644);
            if let Some(parent) = path.parent() {
                make_chain_public(&self.path, parent);
            }
        }
    }

    fn dir(&self, relative: &str) {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create fixture directory");
        #[cfg(unix)]
        make_chain_public(&self.path, &path);
    }

    /// Create a symlink at `link` (relative to the root) pointing at the raw
    /// `target` string (kept relative so it stays inside the root unless the
    /// test deliberately escapes).
    #[cfg(unix)]
    fn symlink(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(target, self.path.join(link)).expect("create symlink fixture");
    }
```

- [ ] **Step 3: Write the new containment tests (they will not compile/pass until the rewrite lands)**

Add these tests to `tests/buffetcar_contract.rs`:

```rust
#[cfg(unix)]
#[test]
fn refuses_in_root_symlink_to_ordinary_target() {
    let site = TempSite::new();
    site.write("real.txt", b"real\n");
    site.symlink("real.txt", "alias.txt");

    assert_eq!(respond(site.path(), "alias.txt"), b"document not found");
}

#[cfg(unix)]
#[test]
fn refuses_in_root_symlink_to_dotfile_target() {
    let site = TempSite::new();
    site.write(".secret", b"top secret\n");
    site.symlink(".secret", "public");

    assert_eq!(respond(site.path(), "public"), b"document not found");
}

#[cfg(unix)]
#[test]
fn symlinked_index_falls_back_to_listing() {
    let site = TempSite::new();
    site.write("docs/.secret", b"secret index\n");
    site.write("docs/page.txt", b"page\n");
    site.symlink(".secret", "docs/index");

    assert_eq!(respond(site.path(), "docs"), b"=> page.txt\n");
}

#[cfg(unix)]
#[test]
fn omits_symlink_entries_from_listings() {
    let site = TempSite::new();
    site.write("links/real.txt", b"real\n");
    site.symlink("real.txt", "links/alias.txt");

    assert_eq!(respond(site.path(), "links"), b"=> real.txt\n");
}

#[cfg(unix)]
#[test]
fn rejects_and_omits_special_files() {
    let site = TempSite::new();
    site.dir("dev");
    site.write("dev/real.txt", b"real\n");
    let fifo = site.path().join("dev").join("pipe");
    // `mkfifo` is present on Linux and macOS; skip where it is unavailable.
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made {
        return;
    }

    assert_eq!(respond(site.path(), "dev/pipe"), b"document not found");
    assert_eq!(respond(site.path(), "dev"), b"=> real.txt\n");
}
```

- [ ] **Step 4: Create `src/root.rs`**

```rust
//! The root directory capability and the fd-relative, no-follow resolver.
//!
//! All filesystem access is relative to an opened root directory descriptor.
//! Each selector component is opened with `openat` + `O_NOFOLLOW` from the
//! current directory fd, so a symlink anywhere on the path fails the open rather
//! than being followed; `O_NONBLOCK` lets a FIFO or device component be opened
//! for its type check without blocking. Every opened fd is `fstat`-checked
//! before use. `selector` has already balanced `..` lexically, so the walk only
//! ever sees normal components and never opens `..` (which could climb above the
//! root). Whole-path opens and `PathBuf::join`-then-open are never used here.

use crate::selector::Request;
use rustix::fs::{self, FileType, Mode, OFlags, Stat};
use rustix::path::Arg;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

/// Open flags for probing a file/special path component: read-only, never follow
/// a final symlink, never block on a FIFO/device, close-on-exec.
const PROBE: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

/// Open flags for a directory that will be enumerated after listing policy
/// accepts it. This intentionally requests read permission.
const LIST_DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Open flags for directory descent. This must not request read permission:
/// world-executable but non-world-readable directories are traversable but not
/// listable. Linux exposes this as `O_PATH`; Darwin/BSD targets expose it as
/// `O_SEARCH`/`O_EXEC` through libc rather than rustix's portable `OFlags`.
#[cfg(any(target_os = "linux", target_os = "android"))]
const TRAVERSE_DIR: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
))]
const TRAVERSE_DIR: OFlags = OFlags::from_bits_retain(
    (libc::O_SEARCH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) as u32,
);

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
)))]
compile_error!(
    "buffetcar requires O_PATH or O_SEARCH so execute-only directories can be traversed"
);

fn open_traverse_dir<D: AsFd, P: Arg>(dir: D, path: P) -> io::Result<OwnedFd> {
    fs::openat(dir, path, TRAVERSE_DIR, Mode::empty())
}

/// A resolved, already-opened and policy-checked target inside the root.
pub(crate) enum Resolved {
    File(OwnedFd),
    Dir(OwnedFd),
}

/// The kind of a re-opened directory entry that passed policy.
pub(crate) enum Child {
    File(OwnedFd),
    Dir,
}

/// An opened root directory descriptor and its device id.
pub(crate) struct Root {
    fd: OwnedFd,
    dev: u64,
}

impl Root {
    /// Open `path` as the served root. The final component must be a real
    /// directory and not a symlink (`O_NOFOLLOW`); intermediate symlinks in the
    /// operator-chosen absolute path are resolved by the kernel at startup.
    pub(crate) fn open(path: &Path) -> io::Result<Root> {
        let fd = fs::open(path, TRAVERSE_DIR, Mode::empty())?;
        let st = fs::fstat(&fd)?;
        let dev = st.st_dev as u64;
        let root = Root { fd, dev };
        if !root.dir_ok(&st) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "served root is not public",
            ));
        }
        Ok(root)
    }

    /// Resolve a parsed request to an opened, policy-checked file or directory.
    pub(crate) fn resolve(&self, request: &Request) -> io::Result<Option<Resolved>> {
        // The root is the first directory in every selector path. Re-open and
        // policy-check it for each request so a root chmod from public to private
        // stops new resolutions, even though the daemon keeps the startup fd.
        let Some(mut cur) = self.open_root_dir()? else {
            return Ok(None);
        };

        let total = request.components.len();
        for (i, name) in request.components.iter().enumerate() {
            if i + 1 == total {
                return self.open_leaf(&cur, name, request.dir_only);
            }
            cur = match self.open_child_dir(&cur, name.as_str())? {
                Some(child) => child,
                None => return Ok(None),
            };
        }

        // Empty selector (or fully balanced `..`): the root directory itself.
        Ok(Some(Resolved::Dir(cur)))
    }

    /// Open `dir`'s `index` if it is a servable regular file (same policy as a
    /// typed selector). A symlinked, special, cross-device, or non-public
    /// `index` is treated as absent, so the directory falls back to a listing.
    pub(crate) fn open_index(&self, dir: &OwnedFd) -> io::Result<Option<Vec<u8>>> {
        match self.classify_child(dir, "index")? {
            Some(Child::File(fd)) => crate::read_file(fd).map(Some),
            _ => Ok(None),
        }
    }

    /// Re-open a directory entry under no-follow public-content policy, reporting
    /// whether it is a listable directory or a servable file. Anything else
    /// (symlink, special file, cross-device, or — from Task 3 — non-public mode
    /// or hardlink) is `None`.
    pub(crate) fn classify_child<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Option<Child>> {
        if let Some(child) = self.classify_readable_child(dir, name)? {
            return Ok(Some(child));
        }
        match self.open_child_dir(dir, name)? {
            Some(_) => Ok(Some(Child::Dir)),
            None => Ok(None),
        }
    }

    /// Re-open `dir` for enumeration after confirming the policy bit. The
    /// resolver may hold only a search-only fd, which is enough for `openat` but
    /// not for `Dir::read_from`.
    pub(crate) fn open_listable_dir(&self, dir: &OwnedFd) -> io::Result<Option<OwnedFd>> {
        let st = fs::fstat(dir)?;
        if st.st_dev as u64 != self.dev || !self.listable(&st) {
            return Ok(None);
        }
        let fd = match fs::openat(dir, ".", LIST_DIR, Mode::empty()) {
            Ok(fd) => fd,
            Err(_) => return Ok(None),
        };
        let st = fs::fstat(&fd)?;
        if st.st_dev as u64 == self.dev && self.listable(&st) {
            Ok(Some(fd))
        } else {
            Ok(None)
        }
    }

    fn open_root_dir(&self) -> io::Result<Option<OwnedFd>> {
        let fd = match open_traverse_dir(&self.fd, ".") {
            Ok(fd) => fd,
            Err(_) => return Ok(None),
        };
        let st = fs::fstat(&fd)?;
        if st.st_dev as u64 != self.dev || !self.dir_ok(&st) {
            return Ok(None);
        }
        Ok(Some(fd))
    }

    fn classify_readable_child<P: Arg>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Option<Child>> {
        let fd = match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => fd,
            Err(_) => return Ok(None),
        };
        let st = fs::fstat(&fd)?;
        if st.st_dev as u64 != self.dev {
            return Ok(None);
        }
        match FileType::from_raw_mode(st.st_mode) {
            FileType::Directory if self.dir_ok(&st) => Ok(Some(Child::Dir)),
            FileType::RegularFile if self.file_ok(&st) => Ok(Some(Child::File(fd))),
            _ => Ok(None),
        }
    }

    fn open_leaf(&self, dir: &OwnedFd, name: &str, dir_only: bool) -> io::Result<Option<Resolved>> {
        if !dir_only {
            // No `O_DIRECTORY`: the final component may be a readable file, a
            // special file opened for type rejection, or a readable directory.
            let fd = match fs::openat(dir, name, PROBE, Mode::empty()) {
                Ok(fd) => fd,
                Err(_) => return self.open_leaf_dir(dir, name),
            };
            let st = fs::fstat(&fd)?;
            if st.st_dev as u64 != self.dev {
                return Ok(None);
            }
            match FileType::from_raw_mode(st.st_mode) {
                FileType::Directory if self.dir_ok(&st) => return Ok(Some(Resolved::Dir(fd))),
                FileType::RegularFile if self.file_ok(&st) => {
                    return Ok(Some(Resolved::File(fd)));
                }
                _ => return Ok(None),
            }
        }
        self.open_leaf_dir(dir, name)
    }

    fn open_leaf_dir(&self, dir: &OwnedFd, name: &str) -> io::Result<Option<Resolved>> {
        match self.open_child_dir(dir, name)? {
            Some(fd) => Ok(Some(Resolved::Dir(fd))),
            None => Ok(None),
        }
    }

    fn open_child_dir<P: Arg>(&self, dir: &OwnedFd, name: P) -> io::Result<Option<OwnedFd>> {
        let fd = match open_traverse_dir(dir, name) {
            Ok(fd) => fd,
            // Missing, a symlink (ELOOP), not a directory, or no search permission.
            Err(_) => return Ok(None),
        };
        let st = fs::fstat(&fd)?;
        if st.st_dev as u64 != self.dev || !self.dir_ok(&st) {
            return Ok(None);
        }
        Ok(Some(fd))
    }

    /// A traversable / servable directory. (Mode bits added in Task 3.)
    fn dir_ok(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::Directory
    }

    /// A listable directory. (World-readable requirement added in Task 3.)
    fn listable(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::Directory
    }

    /// A servable regular file. (World-readable + link-count added in Task 3.)
    fn file_ok(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::RegularFile
    }
}
```

- [ ] **Step 5: Create `src/listing.rs`**

```rust
//! Directory `index` lookup and plain-text Nex listings, generated from an
//! opened directory fd. Every candidate entry is re-opened fd-relative with the
//! same no-follow public-content checks as a direct request, so a symlink,
//! special file, cross-device, or non-public entry is silently omitted rather
//! than linked.

use crate::root::{Child, Root};
use rustix::fs::Dir;
use std::io;
use std::os::fd::OwnedFd;

pub(crate) fn serve(root: &Root, dir: OwnedFd) -> io::Result<Vec<u8>> {
    if let Some(bytes) = root.open_index(&dir)? {
        return Ok(bytes);
    }
    let Some(list_dir) = root.open_listable_dir(&dir)? else {
        return Ok(crate::NOT_FOUND.to_vec());
    };

    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in Dir::read_from(&list_dir)? {
        let entry = entry?;
        let name = entry.file_name(); // &CStr
        let bytes = name.to_bytes();
        // Skip ".", "..", and any dotfile.
        if bytes.first() == Some(&b'.') {
            continue;
        }
        // Re-open under no-follow public-content policy; omit anything refused.
        // The `CStr` is opened directly to avoid a lossy conversion.
        let Some(child) = root.classify_child(&list_dir, name)? else {
            continue;
        };
        // A Nex selector is text; a non-UTF-8 name cannot round-trip to a
        // fetchable link, so omit it rather than emit a lossy placeholder.
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
    }

    // Sort by name alone so a directory and a file sharing a prefix order
    // alphabetically; the trailing slash is presentation, applied on render.
    entries.sort();

    let mut out = String::new();
    for (name, is_dir) in entries {
        out.push_str("=> ");
        out.push_str(&name);
        if is_dir {
            out.push('/');
        }
        out.push('\n');
    }
    Ok(out.into_bytes())
}
```

- [ ] **Step 6: Rewrite `src/lib.rs`**

Replace the entire contents of `src/lib.rs` with:

```rust
//! Buffetcar Nex server.

use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;

mod listing;
mod root;
mod selector;

use root::{Resolved, Root};

pub(crate) const NOT_FOUND: &[u8] = b"document not found";

/// Resolve `selector` against `root` and return the response bytes.
///
/// Thin library/test wrapper over the resolver: it opens a [`Root`] per call.
/// The daemon path opens the root once at startup and reuses it. Every
/// unavailable selector — missing, rejected by policy, malformed, or an escape —
/// yields the identical `document not found` body with no reason; only genuine
/// I/O faults on an already-opened descriptor propagate as `Err` so the server
/// layer can log them rather than masquerade them as a missing document.
pub fn serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>> {
    let Ok(root) = Root::open(root) else {
        return Ok(NOT_FOUND.to_vec());
    };
    let Some(request) = selector::parse(selector) else {
        return Ok(NOT_FOUND.to_vec());
    };
    match root.resolve(&request)? {
        Some(Resolved::File(fd)) => read_file(fd),
        Some(Resolved::Dir(fd)) => listing::serve(&root, fd),
        None => Ok(NOT_FOUND.to_vec()),
    }
}

pub(crate) fn read_file(fd: OwnedFd) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::from(fd);
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}
```

This removes the Task 1 `#[allow(dead_code)] mod selector;` line (now wired in) and the old `cap-std` implementation entirely.

- [ ] **Step 7: Run the full suite**

Run: `cargo test`
Expected: PASS — every pre-existing contract test (`serves_files_directory_indexes_listings_and_not_found`, `directory_listings_sort_by_name_independent_of_trailing_slash`, `omits_non_utf8_names_from_listings`, `preserves_binary_file_bytes`, `rejects_dotfiles_by_default_and_omits_them_from_listings`, `allows_balanced_parent_components_but_rejects_above_root_escape`, `rejects_symlink_escape_outside_the_root`), the `selector::tests::*` unit tests, and the five new containment tests (`refuses_in_root_symlink_to_ordinary_target`, `refuses_in_root_symlink_to_dotfile_target`, `symlinked_index_falls_back_to_listing`, `omits_symlink_entries_from_listings`, `rejects_and_omits_special_files`).

- [ ] **Step 8: Verify formatting, lints, and dependency policy**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && make deny`
Expected: PASS. `rustix`, direct `libc`, and rustix's remaining transitive deps (`bitflags`, `linux-raw-sys`, `errno`) are MIT/Apache-2.0, which the `deny.toml` allow-list already permits; `cap-std`'s subtree is gone.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/root.rs src/listing.rs tests/buffetcar_contract.rs
git commit -m "feat!: replace cap-std with fd-relative no-follow resolver"
```

---

## Task 3: Public-content policy — world-readable files, link-count, world-executable directories

Tighten the `fstat` predicates so the descriptor checks match the spec's "Public Content Policy": a regular file must be world-readable with link count `1`; a directory must be world-executable to traverse/serve and world-readable to list. These are policy beyond kernel access control: in the multi-user model the daemon may own a user's files, so owner-readable is not the same as public.

**Files:**
- Modify: `tests/buffetcar_contract.rs`
- Modify: `src/root.rs`

- [ ] **Step 1: Add `write_mode`/`dir_mode` fixture helpers**

Add these methods inside `impl TempSite`, after `dir`:

```rust
    /// Write a file with an exact mode, bypassing the public-by-default chmod.
    /// Used to build intentionally non-public fixtures.
    #[cfg(unix)]
    fn write_mode(&self, relative: &str, content: &[u8], mode: u32) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
            make_chain_public(&self.path, parent);
        }
        fs::write(&path, content).expect("write fixture file");
        make_public(&path, mode);
    }

    /// Create a directory with an exact mode, bypassing the public chmod.
    #[cfg(unix)]
    fn dir_mode(&self, relative: &str, mode: u32) {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create fixture directory");
        if let Some(parent) = path.parent() {
            make_chain_public(&self.path, parent);
        }
        make_public(&path, mode);
    }
```

- [ ] **Step 2: Write the failing policy tests**

Add to `tests/buffetcar_contract.rs`:

```rust
#[cfg(unix)]
#[test]
fn rejects_non_world_readable_file() {
    let site = TempSite::new();
    // Owner can read it, but it is not world-readable, so it is not public.
    site.write_mode("private.txt", b"private\n", 0o600);

    assert_eq!(respond(site.path(), "private.txt"), b"document not found");
}

#[cfg(unix)]
#[test]
fn rejects_hardlinked_file() {
    let site = TempSite::new();
    site.write("original.txt", b"shared\n");
    std::fs::hard_link(
        site.path().join("original.txt"),
        site.path().join("alias.txt"),
    )
    .expect("create hardlink fixture");

    // Both names now have link count 2 and are refused.
    assert_eq!(respond(site.path(), "original.txt"), b"document not found");
    assert_eq!(respond(site.path(), "alias.txt"), b"document not found");
}

#[cfg(unix)]
#[test]
fn rejects_non_world_executable_directory() {
    let site = TempSite::new();
    site.write("locked/inside.txt", b"inside\n");
    // Remove world-execute (and the auto-applied 0755) from the directory.
    site.dir_mode("locked", 0o600);

    assert_eq!(respond(site.path(), "locked/inside.txt"), b"document not found");
    assert_eq!(respond(site.path(), "locked"), b"document not found");
}

#[cfg(unix)]
#[test]
fn rejects_non_world_executable_root() {
    let site = TempSite::new();
    site.write("public.txt", b"public\n");
    // The daemon/test user can still open this root as owner, but the served
    // root is itself a directory in the request path and is not public.
    make_public(site.path(), 0o700);

    assert_eq!(respond(site.path(), "public.txt"), b"document not found");
    assert_eq!(respond(site.path(), ""), b"document not found");
}

#[cfg(unix)]
#[test]
fn does_not_list_non_world_readable_directory() {
    let site = TempSite::new();
    site.write("hidden/inside.txt", b"inside\n");
    // World-executable (traversable) but not readable by anyone. This catches
    // accidental `O_RDONLY` use for descent, which would require read permission.
    site.dir_mode("hidden", 0o111);

    // A direct child is still servable; the directory itself yields no listing.
    assert_eq!(respond(site.path(), "hidden/inside.txt"), b"inside\n");
    assert_eq!(respond(site.path(), "hidden"), b"document not found");
}
```

- [ ] **Step 3: Run the new tests to confirm they fail against the permissive predicates**

Run: `cargo test -- rejects_non_world_readable_file rejects_hardlinked_file rejects_non_world_executable_directory rejects_non_world_executable_root does_not_list_non_world_readable_directory`
Expected: FAIL — the permissive Task 2 predicates serve the private file, the hardlinked file, the non-executable directory's child, a child through the non-public root, and a listing for the non-readable directory. If directory descent still uses read-only directory opens, `does_not_list_non_world_readable_directory` fails the other way by refusing `hidden/inside.txt`.

- [ ] **Step 4: Tighten the predicates in `src/root.rs`**

Replace the three predicate functions `dir_ok`, `listable`, and `file_ok` with:

```rust
    /// A traversable / servable directory: a directory, world-executable.
    fn dir_ok(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::Directory
            && Mode::from_raw_mode(st.st_mode).contains(Mode::XOTH)
    }

    /// A listable directory: traversable and world-readable (so a listing exposes
    /// only what is public, not what the daemon user happens to read).
    fn listable(&self, st: &Stat) -> bool {
        self.dir_ok(st) && Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH)
    }

    /// A servable regular file: regular, world-readable, and not a hardlink.
    /// Link count greater than one is refused so a user cannot publish an inode
    /// through a misleading in-tree name (hardlinks are not a Nex feature).
    fn file_ok(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::RegularFile
            && Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH)
            && st.st_nlink as u64 == 1
    }
```

- [ ] **Step 5: Run the new tests to confirm they pass**

Run: `cargo test -- rejects_non_world_readable_file rejects_hardlinked_file rejects_non_world_executable_directory rejects_non_world_executable_root does_not_list_non_world_readable_directory`
Expected: PASS.

- [ ] **Step 6: Run the full suite to confirm no regressions**

Run: `cargo test`
Expected: PASS — every test. The fixture chmods from Task 2 keep all intended-public fixtures (`0644` files, `0755` directories) servable under the stricter predicates.

- [ ] **Step 7: Verify formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/root.rs tests/buffetcar_contract.rs
git commit -m "feat: enforce world-readable, link-count, and world-exec policy"
```

---

## Task 4: Bounded listings

Cap generated listings at the spec's hardcoded bounds — at most 4096 entries and at most 256 KiB of rendered output — so a hostile or huge directory cannot exhaust memory or produce an unbounded response. Exceeding either bound makes the directory unavailable; site authors can provide an `index` for large directories.

**Files:**
- Modify: `tests/buffetcar_contract.rs`
- Modify: `src/listing.rs`

- [ ] **Step 1: Write the failing bound test**

Add to `tests/buffetcar_contract.rs`:

```rust
#[test]
fn rejects_directory_listing_exceeding_entry_bound() {
    let site = TempSite::new();
    // One past the 4096-entry bound.
    for i in 0..4097 {
        site.write(&format!("big/f{i:05}.txt"), b"x\n");
    }

    assert_eq!(respond(site.path(), "big"), b"document not found");
}

#[test]
fn serves_listing_at_the_entry_bound() {
    let site = TempSite::new();
    for i in 0..4096 {
        site.write(&format!("ok/f{i:05}.txt"), b"x\n");
    }

    let listing = respond(site.path(), "ok");
    assert_ne!(listing, b"document not found");
    assert_eq!(listing.iter().filter(|&&b| b == b'\n').count(), 4096);
}
```

- [ ] **Step 2: Run the bound tests to confirm the first fails**

Run: `cargo test -- rejects_directory_listing_exceeding_entry_bound serves_listing_at_the_entry_bound`
Expected: FAIL — `rejects_directory_listing_exceeding_entry_bound` currently serves a 4097-line listing instead of `document not found` (`serves_listing_at_the_entry_bound` already passes).

- [ ] **Step 3: Add the bounds to `src/listing.rs`**

Add these constants above `pub(crate) fn serve`:

```rust
/// Hardcoded listing bounds (spec: "Directory Listings").
const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: usize = 256 * 1024;
```

In `serve`, replace the entry-collection loop's `entries.push(...)` tail and the final render so the bounds are enforced. The loop body's final two lines become:

```rust
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
        if entries.len() > MAX_ENTRIES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
```

and, after `entries.sort();`, guard the rendered size before returning:

```rust
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
```

- [ ] **Step 4: Run the bound tests to confirm both pass**

Run: `cargo test -- rejects_directory_listing_exceeding_entry_bound serves_listing_at_the_entry_bound`
Expected: PASS.

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: PASS — every test.

- [ ] **Step 6: Verify formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/listing.rs tests/buffetcar_contract.rs
git commit -m "feat: bound generated directory listings"
```

---

## Self-Review

**Spec coverage (sections of `2026-06-06-multi-user-nex-server-design.md` in scope):**
- "Nex Compliance" — selector is UTF-8 (`&str` wrapper), trailing-CR trim, NUL/oversized rejection, empty → root, trailing-slash directory intent → Task 1. Directories as `=> ` maps, trailing `/`, `index` served → Tasks 2/4. ✓
- "Filesystem Resolver" — per-component `openat` + `O_NOFOLLOW`, search-only directory descent, `fstat` every fd, lexical `..` (balanced allowed, escape rejected), dotfile-before-`..`, no whole-path open → Tasks 1/2. Root opened no-follow/search-only, device recorded, and policy-checked before every resolution → Tasks 2/3. ✓
- "Public Content Policy" — regular + world-readable + link-count-1 + same-device files; root/directory + world-exec + same-device, world-readable to list; symlinks never followed (`O_NOFOLLOW`); special files rejected by type; cross-device rejected; `index` uses same policy → Tasks 2/3. ✓
- "Directory Listings" — index-or-listing, world-readable to list, readable fd opened only after listing policy, per-entry fd-relative checks, dotfile/symlink/special/non-UTF-8 omission, sorted, 4096-entry / 256 KiB bounds → Tasks 2/3/4. ✓
- "Testing" (resolver/containment/listing rows) — empty/`/`, files, binary, indexes, listings, sorting, trailing-slash-on-file, balanced/escaping `..`, dotfile, symlink (final + intermediate + index + listing), special files, world-readable/-executable, non-world-executable root, execute-only traversal without listing, hardlink, bounds, deterministic fixture permissions → Tasks 1–4. ✓
- Explicitly out of scope (later plans): `cli`/`config`/`server`/`conn`/`sandbox`/`main`, banner, socket/read/write timeouts, worker pool, raw-byte wire selector, `check` mode, run-as-root refusal, architecture-guard tests, cross-device *entry* fixtures (need privileged mounts), and the concurrent name-swap stress test (belongs with the daemon).

**Placeholder scan:** none — every code and command step is concrete and complete.

**Type consistency:**
- `Request { components: Vec<String>, dir_only: bool }` defined in Task 1; consumed by `Root::resolve` and constructed in `selector::tests` (Task 1) — consistent.
- `Resolved { File(OwnedFd), Dir(OwnedFd) }` and `Child { File(OwnedFd), Dir }` defined in Task 2 (`root.rs`); `Resolved` matched in `lib.rs`, `Child` matched in `listing.rs` and `Root::open_index` — consistent.
- `Root` methods `open`, `resolve`, `open_index`, `classify_child<P: Arg + Copy>`, `open_listable_dir`, and private `open_root_dir`/`classify_readable_child`/`open_leaf`/`open_leaf_dir`/`open_child_dir`/`dir_ok`/`listable`/`file_ok` defined in Task 2; Task 3 changes only the bodies of `dir_ok`/`listable`/`file_ok` (signatures stable). ✓
- `crate::read_file(OwnedFd) -> io::Result<Vec<u8>>` and `crate::NOT_FOUND` defined in `lib.rs` (Task 2); referenced from `root.rs` and `listing.rs` — consistent.
- `PROBE` / `LIST_DIR` / `TRAVERSE_DIR` `OFlags` consts (Task 2) split file probing, readable listing opens, and search-only directory descent; `MAX_ENTRIES`/`MAX_BYTES` (Task 4) used only in `listing::serve`. ✓
- Test helpers: `make_public`/`make_chain_public` and `TempSite::{symlink}` (Task 2) reused by `write_mode`/`dir_mode` (Task 3); `respond`/`unique_name`/`TempSite::{new,path,write,dir}` unchanged in signature. ✓

**rustix API grounding:** `openat`/`open`/`fstat`, `OFlags::{RDONLY,NOFOLLOW,NONBLOCK,CLOEXEC,DIRECTORY,PATH}` where target-exposed with const `union`, `OFlags::from_bits_retain` for `libc::O_SEARCH` targets, `Mode::{empty,from_raw_mode,ROTH,XOTH,contains}`, `FileType::from_raw_mode` (derives `PartialEq`), `Stat::{st_mode,st_dev,st_nlink}` (cast `as u64` for portability across libc/linux-raw backends), and `Dir::read_from` + `DirEntry::file_name() -> &CStr` were each verified against the installed `rustix` 1.1.4 source before being written into the steps.
