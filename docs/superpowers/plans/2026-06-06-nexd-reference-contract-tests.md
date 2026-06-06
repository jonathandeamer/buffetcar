# Nexd Reference Contract Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore and expand optional reference-only contract tests for the Go `nexd` server so buffetcar can distinguish Nex-compatible behavior to preserve from unsafe legacy behavior to deliberately invert.

**Architecture:** Add a feature-gated `nexd_contract` integration test target that builds and runs the local Go `nexd` repository, sends real TCP Nex selectors, and records observable responses. Keep this outside normal `cargo test`/`make check`; it is a characterization suite, not buffetcar TDD. Use deterministic fixture permissions so results do not depend on process umask.

**Tech Stack:** Rust integration tests, local Go toolchain, local `/Users/jonathan/nexd` or `NEXD_REPO`, TCP port `127.0.0.1:1900`, feature flag `nexd-contract`.

---

## Scope

This plan only adds optional reference tests for `nexd`. It does not implement buffetcar's new resolver, daemon, CLI, or red/green buffetcar tests. The reference tests should be run before implementation planning when we need to confirm what `nexd` actually does, and then again only when changing the characterization suite.

This plan does not modify the existing `tests/buffetcar_contract.rs` suite. The
later buffetcar implementation plan must update those fixture helpers to chmod
servable files/directories deterministically before mode-bit enforcement lands,
but that is not part of the optional `nexd` reference suite.

`nexd` tests must be explicit about whether a behavior is:

- **Protocol-compatible:** buffetcar should preserve it unless the active spec says otherwise.
- **Legacy unsafe:** buffetcar should intentionally reject or change it.
- **Reference-only:** useful history, but not a buffetcar requirement.

Do not add dangerous `nexd` tests that can hang indefinitely, such as direct FIFO reads. Special-file safety belongs in buffetcar tests later.

## File Structure

- `Cargo.toml` (modify) — add `nexd-contract` feature and a gated `nexd_contract` integration test target.
- `Makefile` (modify) — add an optional `nexd-contract` target that runs only the feature-gated suite.
- `tests/common/mod.rs` (create) — shared reference-test harness: temp site, permission helpers, `nexd` build/start/stop, TCP request helper, port lock.
- `tests/nexd_contract.rs` (create) — reference behavior assertions split into protocol-compatible and legacy-unsafe sections.

## Task 1: Restore The Feature-Gated Nexd Harness

**Files:**
- Modify: `Cargo.toml`
- Create: `tests/common/mod.rs`

- [ ] **Step 1: Add the optional test feature and target**

Edit `Cargo.toml` to include the feature and gated test target while keeping current dependencies unchanged:

```toml
[package]
name = "buffetcar"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "A hardened, single-binary Nex server in Rust"

[features]
nexd-contract = []

[dependencies]
cap-std = "4.0.2"

[[test]]
name = "nexd_contract"
path = "tests/nexd_contract.rs"
required-features = ["nexd-contract"]
```

- [ ] **Step 2: Create the shared `nexd` test harness**

Create `tests/common/mod.rs`:

```rust
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const NEXD_ADDR: &str = "127.0.0.1:1900";

pub struct TempSite {
    path: PathBuf,
}

impl TempSite {
    pub fn new() -> Self {
        let path = env::temp_dir().join(unique_name("buffetcar-nexd-contract", ""));
        fs::create_dir(&path).expect("create temp site root");
        make_public_dir(&path);
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, relative: &str, content: impl AsRef<[u8]>) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
            make_public_dir_tree(&self.path, parent);
        }
        fs::write(&path, content).expect("write fixture file");
        make_public_file(&path);
    }

    pub fn write_private(&self, relative: &str, content: impl AsRef<[u8]>) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
            make_public_dir_tree(&self.path, parent);
        }
        fs::write(&path, content).expect("write private fixture file");
        make_private_file(&path);
    }

    pub fn dir(&self, relative: &str) {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create fixture directory");
        make_public_dir_tree(&self.path, &path);
    }
}

impl Drop for TempSite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A uniquely named, world-readable fixture created in the site root's *parent*
/// directory, for symlink/hardlink targets and parent-traversal probes. Removed
/// on drop so a panicking assertion never leaks fixtures into the shared temp
/// directory.
pub struct OutsideFile {
    path: PathBuf,
}

impl OutsideFile {
    pub fn new(site: &TempSite, prefix: &str, content: impl AsRef<[u8]>) -> Self {
        let path = site
            .path()
            .parent()
            .expect("site has parent")
            .join(unique_name(prefix, ".txt"));
        fs::write(&path, content).expect("write outside fixture");
        make_public_file(&path);
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("outside fixture has a UTF-8 name")
    }
}

impl Drop for OutsideFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct Nexd {
    child: Child,
    _port_guard: MutexGuard<'static, ()>,
}

impl Nexd {
    pub fn start(root: &Path) -> Self {
        let port_guard = nexd_port_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_port_available();

        let bin = nexd_binary();
        let mut child = Command::new(&bin)
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("start {}: {err}", bin.display()));

        wait_until_listening(&mut child);
        Self {
            child,
            _port_guard: port_guard,
        }
    }
}

impl Drop for Nexd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn request(selector: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(NEXD_ADDR).expect("connect to nexd");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");
    stream
        .write_all(selector.as_bytes())
        .expect("write selector");
    stream.write_all(b"\n").expect("write selector newline");
    stream
        .shutdown(Shutdown::Write)
        .expect("shutdown request write side");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read nexd response");
    response
}

fn assert_port_available() {
    if TcpStream::connect(NEXD_ADDR).is_ok() {
        panic!(
            "{NEXD_ADDR} is already accepting connections; stop that server before running nexd contract tests"
        );
    }
}

fn wait_until_listening(child: &mut Child) {
    let deadline = Duration::from_secs(10);
    let started = std::time::Instant::now();

    loop {
        if let Some(status) = child.try_wait().expect("poll nexd process") {
            let mut stderr = String::new();
            if let Some(pipe) = child.stderr.as_mut() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!("nexd exited before listening: {status}; stderr: {stderr}");
        }

        if TcpStream::connect(NEXD_ADDR).is_ok() {
            return;
        }

        if started.elapsed() > deadline {
            panic!("timed out waiting for nexd to listen on {NEXD_ADDR}");
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn nexd_port_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn nexd_binary() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let repo = nexd_repo();
        let output_dir = env::current_dir()
            .expect("current directory")
            .join("target")
            .join("nexd-contract");
        fs::create_dir_all(&output_dir).expect("create nexd test binary directory");
        let bin = output_dir.join("nexd");

        let status = Command::new("go")
            .args(["build", "-o"])
            .arg(&bin)
            .arg(".")
            .current_dir(&repo)
            .status()
            .unwrap_or_else(|err| panic!("build nexd from {}: {err}", repo.display()));

        assert!(
            status.success(),
            "go build failed for nexd from {}: {status}",
            repo.display()
        );
        bin
    })
    .clone()
}

fn nexd_repo() -> PathBuf {
    if let Some(repo) = env::var_os("NEXD_REPO") {
        return PathBuf::from(repo);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest directory has parent")
        .join("nexd")
}

#[cfg(unix)]
pub fn make_public_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
        .expect("set public file permissions");
}

#[cfg(not(unix))]
pub fn make_public_file(_path: &Path) {}

#[cfg(unix)]
pub fn make_private_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("set private file permissions");
}

#[cfg(not(unix))]
pub fn make_private_file(_path: &Path) {}

#[cfg(unix)]
pub fn make_public_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("set public directory permissions");
}

#[cfg(not(unix))]
pub fn make_public_dir(_path: &Path) {}

fn make_public_dir_tree(root: &Path, dir: &Path) {
    make_public_dir(root);
    let relative = dir.strip_prefix(root).expect("fixture directory under root");
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        make_public_dir(&current);
    }
}

pub fn unique_name(prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}-{}-{}{suffix}",
        std::process::id(),
        unique_suffix()
    )
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos()
}
```

- [ ] **Step 3: Verify default tests do not try to build the feature-gated target**

Run: `cargo test --no-run`

Expected: PASS. The output must not include a `nexd_contract` binary unless the feature is enabled.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml tests/common/mod.rs
git commit -m "test: restore optional nexd contract harness"
```

## Task 2: Restore Baseline Nexd Characterization Tests

**Files:**
- Create: `tests/nexd_contract.rs`

- [ ] **Step 1: Create baseline protocol-compatible and reference-only tests**

Create `tests/nexd_contract.rs` with these tests:

```rust
//! Reference-only characterization tests for the Go `nexd` server.
//!
//! These pin observable behavior of the local reference implementation so
//! buffetcar can deliberately preserve protocol-compatible cases and invert
//! unsafe legacy cases. Run with:
//! `cargo test --features nexd-contract --test nexd_contract`.

mod common;

use common::{request, Nexd, OutsideFile, TempSite};

#[test]
fn nexd_serves_root_index_files_directory_indexes_and_not_found() {
    let site = TempSite::new();
    site.write("index", b"root index\n");
    site.write("plain.txt", b"plain file\n");
    site.write("docs/index", b"docs index\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request(""), b"root index\n");
    assert_eq!(request("/"), b"root index\n");
    assert_eq!(request("plain.txt"), b"plain file\n");
    assert_eq!(request("docs"), b"docs index\n");
    assert_eq!(request("docs/"), b"docs index\n");
    assert_eq!(request("missing.txt"), b"document not found");
}

#[test]
fn nexd_trims_leading_and_trailing_slashes_from_selectors() {
    let site = TempSite::new();
    site.write("plain.txt", b"plain file\n");
    site.write("docs/index", b"docs index\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("/plain.txt"), b"plain file\n");
    assert_eq!(request("plain.txt/"), b"plain file\n");
    assert_eq!(request("/docs/"), b"docs index\n");
}

#[test]
fn nexd_preserves_binary_file_bytes() {
    let site = TempSite::new();
    let bytes = [0, 1, 2, b'\n', 0xff, b'n', b'e', b'x'];
    site.write("blob.bin", bytes);

    let _server = Nexd::start(site.path());

    assert_eq!(request("blob.bin"), bytes);
}

#[test]
fn nexd_generates_ascending_directory_listings_and_hides_dotfiles() {
    let site = TempSite::new();
    site.dir("listing/subdir");
    site.write("listing/apple.txt", b"apple\n");
    site.write("listing/banana.txt", b"banana\n");
    site.write("listing/.hidden", b"hidden\n");

    let _server = Nexd::start(site.path());

    assert_eq!(
        request("listing"),
        b"=> apple.txt\n=> banana.txt\n=> subdir/\n"
    );
}

#[cfg(unix)]
#[test]
fn nexd_omits_entries_without_world_read_permission_from_listings() {
    let site = TempSite::new();
    site.write_private("listing/private.txt", b"private\n");
    site.write("listing/public.txt", b"public\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("listing"), b"=> public.txt\n");
}

#[test]
fn nexd_reverses_directory_listings_when_desc_marker_exists() {
    let site = TempSite::new();
    site.write("listing/apple.txt", b"apple\n");
    site.write("listing/banana.txt", b"banana\n");
    site.write("listing/cherry.txt", b"cherry\n");
    site.write("listing/.desc", b"");

    let _server = Nexd::start(site.path());

    assert_eq!(
        request("listing"),
        b"=> cherry.txt\n=> banana.txt\n=> apple.txt\n"
    );
}

#[test]
fn nexd_rejects_selectors_containing_parent_components_even_when_balanced() {
    let site = TempSite::new();
    site.dir("a/b");
    site.write("a/c.txt", b"inside root\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("a/b/../c.txt"), b"document not found");
}

#[test]
fn nexd_rejects_parent_traversal_selectors() {
    let site = TempSite::new();
    let outside = OutsideFile::new(&site, "buffetcar-outside", b"outside root\n");

    let _server = Nexd::start(site.path());
    let response = request(&format!("../{}", outside.name()));

    assert_eq!(response, b"document not found");
}
```

- [ ] **Step 2: Run the baseline reference tests**

Run: `cargo test --features nexd-contract --test nexd_contract`

Expected: PASS. If this fails because port `127.0.0.1:1900` is busy, stop the existing process and rerun. If this fails because `/Users/jonathan/nexd` is missing, rerun with `NEXD_REPO=/path/to/nexd`.

- [ ] **Step 3: Commit**

```bash
git add tests/nexd_contract.rs
git commit -m "test: characterize baseline nexd behavior"
```

## Task 3: Add Multi-User-Relevant Legacy Behavior Tests

**Files:**
- Modify: `tests/nexd_contract.rs`

- [ ] **Step 1: Append unsafe legacy behavior tests**

> **Unverified characterizations:** Unlike the Task 2 baseline (which was
> previously confirmed against this `nexd`), the in-root symlink, symlinked
> index, same-user private-file, and hardlink cases below are new assumptions.
> Step 2's run is what confirms them. If `nexd` diverges from an assertion,
> correct the *test* to match observed reference behavior — do not change
> `nexd` — and note whether the real behavior is still legacy-unsafe.

Append these tests to `tests/nexd_contract.rs`:

```rust
#[test]
fn nexd_legacy_behavior_serves_direct_dotfile_requests() {
    let site = TempSite::new();
    site.write(".secret", b"secret\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request(".secret"), b"secret\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_follows_symlinks_outside_the_root() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    let outside = OutsideFile::new(&site, "buffetcar-symlink-target", b"symlink target\n");
    symlink(outside.path(), site.path().join("leak.txt")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("leak.txt"), b"symlink target\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_follows_in_root_symlink_to_dotfile() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    site.write(".secret", b"secret\n");
    symlink(".secret", site.path().join("public")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("public"), b"secret\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_serves_symlinked_index() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    site.write("docs/.secret", b"secret index\n");
    site.write("docs/page.txt", b"page\n");
    symlink(".secret", site.path().join("docs/index")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("docs"), b"secret index\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_serves_private_file_when_daemon_user_can_read_it() {
    let site = TempSite::new();
    site.write_private("private.txt", b"private\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("private.txt"), b"private\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_serves_hardlink_to_file_outside_root() {
    let site = TempSite::new();
    let outside = OutsideFile::new(&site, "buffetcar-hardlink-target", b"hardlink target\n");

    let link = site.path().join("published-hardlink.txt");
    if let Err(err) = std::fs::hard_link(outside.path(), &link) {
        eprintln!("skipping hardlink characterization: hard_link unsupported here: {err}");
        return;
    }

    let _server = Nexd::start(site.path());

    assert_eq!(request("published-hardlink.txt"), b"hardlink target\n");
}
```

- [ ] **Step 2: Run the expanded reference tests**

Run: `cargo test --features nexd-contract --test nexd_contract -- --nocapture`

Expected: PASS. The tests named `nexd_legacy_behavior_*` document unsafe or policy-divergent behavior that buffetcar should not copy under the active multi-user spec. If `hard_link` is unsupported on the fixture filesystem, the hardlink test logs a skip notice (visible with `--nocapture`) and returns without asserting.

- [ ] **Step 3: Commit**

```bash
git add tests/nexd_contract.rs
git commit -m "test: characterize unsafe nexd legacy behavior"
```

## Task 4: Add An Optional Make Target And Usage Notes

**Files:**
- Modify: `Makefile`
- Modify: `docs/2026-06-05-context.md`

- [ ] **Step 1: Add an optional `make nexd-contract` target**

Update `Makefile`:

```make
.PHONY: check fmt clippy test deny hooks nexd-contract

check: fmt clippy test ## run the full local gate (fmt, clippy, test)

fmt: ## verify formatting
	cargo fmt --all --check

clippy: ## lint with warnings denied
	cargo clippy --all-targets -- -D warnings

test: ## run the test suite
	cargo test

deny: ## audit dependencies (needs: cargo install cargo-deny)
	cargo deny check advisories licenses bans sources

nexd-contract: ## run optional reference nexd characterization tests (needs Go and NEXD_REPO or ../nexd)
	cargo clippy --features nexd-contract --test nexd_contract -- -D warnings
	cargo test --features nexd-contract --test nexd_contract

hooks: ## install git hooks (commit-msg: Conventional Commits); run once per clone
	git config core.hooksPath .githooks
	@echo "git hooks installed (core.hooksPath -> .githooks)"
```

- [ ] **Step 2: Add a short context note explaining the optional suite**

Append this section to `docs/2026-06-05-context.md`:

````markdown
## Optional Reference Contract Tests

The repository may include a feature-gated `nexd_contract` integration test
suite. It builds the local Go `nexd` reference server and sends real TCP Nex
requests to `127.0.0.1:1900`. Run it with:

```sh
make nexd-contract
```

or, if the reference checkout is not at `../nexd`:

```sh
NEXD_REPO=/path/to/nexd make nexd-contract
```

Building `nexd` resolves `hg.sr.ht/~m15o/nex-pfm`, a Mercurial-hosted Go module,
so the first build needs network access and a `hg` client. Pre-warm the module
cache so later runs work offline and so fetch failures surface up front instead
of looking like a test failure:

```sh
(cd "${NEXD_REPO:-../nexd}" && go mod download)
```

These tests are not part of `make check`. They characterize reference behavior
only. Tests named `nexd_legacy_behavior_*` document behavior buffetcar
intentionally rejects under the active multi-user threat model.
````

- [ ] **Step 3: Run default and optional commands**

Run: `make check`

Expected: PASS. This proves the optional `nexd_contract` target is not part of the default gate.

Run: `make nexd-contract`

Expected: PASS, assuming Go is installed, `127.0.0.1:1900` is free, and `NEXD_REPO` or `../nexd` points at the reference repo.

- [ ] **Step 4: Commit**

```bash
git add Makefile docs/2026-06-05-context.md
git commit -m "docs: document optional nexd contract tests"
```

## Task 5: Confirm The Plan Does Not Expand Into Buffetcar TDD

**Files:**
- Modify: `docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`

- [ ] **Step 1: Add a sentence linking reference tests to later implementation planning**

In the Testing section of `docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`, after the architecture guard tests, add:

```markdown
Optional `nexd_contract` tests are reference characterization only. They do not
define buffetcar's full red/green implementation sequence; the buffetcar TDD
plan is written separately after this reference surface is settled.
```

- [ ] **Step 2: Run doc checks**

Run: `git diff --check`

Expected: PASS, no whitespace errors.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md
git commit -m "docs: scope nexd contracts as reference tests"
```

## Self-Review

**Spec coverage:** This plan covers the reference side of the active spec: Nex-compatible behavior, deliberate divergences, unsafe legacy symlink/dotfile/private-file/hardlink behavior, deterministic fixture permissions, and optional status outside default CI.

**Intentionally deferred:** Buffetcar red/green TDD, the fd-relative resolver implementation, daemon/socket tests, `check` diagnostics, architecture guards, and special-file safety tests are not implemented here. They belong to the later buffetcar implementation plan.

**Unsafe reference cases not tested:** Direct FIFO/special-file reads against `nexd` are intentionally skipped because the Go server may block in `io.Copy`. Buffetcar will test special-file rejection directly without relying on `nexd`.

**Completeness scan:** No placeholder work remains. Every file change has concrete content and commands.

**Type consistency:** `TempSite`, `Nexd`, `OutsideFile`, `request`, `make_public_file`, `write_private`, and `make_public_dir` are defined in `tests/common/mod.rs` before use in `tests/nexd_contract.rs`; `unique_name` is used internally by the harness (`TempSite` and `OutsideFile`) rather than by the tests directly.
