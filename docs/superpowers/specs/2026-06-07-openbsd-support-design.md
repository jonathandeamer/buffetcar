# OpenBSD Support and Sandboxing Design

Date: 2026-06-07
Status: active design; pending user review
Working name: openbsd-support

This design specification details the implementation of OpenBSD compilation support and sandboxing (`pledge`/`unveil`) for the `buffetcar` Nex server, along with verifying compatibility through a virtualized OpenBSD environment in CI/CD.

## Requirements

1. **OpenBSD Target Compatibility**:
   - `buffetcar`'s path resolver must compile on OpenBSD.
   - OpenBSD does not support execute-only directory open flags (`O_PATH` or `O_SEARCH`). Thus, directory traversal on OpenBSD will fall back to using `O_RDONLY`.
   - On other platforms, the stronger execute-only traversal policy (`O_PATH` on Linux/Android and `O_SEARCH` on macOS/FreeBSD/NetBSD) remains unmodified.
   - **Behavioral Divergence**: Using `O_RDONLY` for traversal means that directories in the served tree on OpenBSD must be world-readable (`0o004` or readable by the daemon user) for traversal to succeed. An execute-only (`--x`) directory tree that is traversable on Linux/macOS will fail with "document not found" on OpenBSD. This is documented in both code comments and the project README.

2. **OpenBSD Sandboxing (`pledge` and `unveil`)**:
   - The sandbox is applied only to the network daemon (`serve`), keeping the diagnostic CLI tool (`check`) unsandboxed (Approach 1).
   - At startup, the daemon must call `unveil` to restrict filesystem access exclusively to the configured `--root` directory with read-only (`"r"`) permissions.
   - The daemon must immediately lock `unveil` by calling it with null pointers to prevent future changes to the filesystem view.
   - The daemon must call `pledge` with `"stdio rpath inet"` promises to restrict allowable system calls to basic I/O/threading, read-only path access, and network operations.
   - Sandbox initialization errors (such as syscall failures or root path validation errors) must be treated as fatal, consistently logging to stderr and exiting with code `1` (no panic).

3. **In-Process Test Isolation (Critical)**:
   - To prevent `pledge` and `unveil` from permanently sandboxing the `cargo test` runner process and causing other unit tests to abort, we implement two safeguards:
     1. In `server::run`, we swap the startup order: we call `TcpListener::bind` *before* invoking `sandbox::apply`. This ensures that unit tests checking bind conflicts (which fail the bind) return early and never execute the sandbox.
     2. We make `sandbox::apply` a compile-time no-op under `cfg(test)` (i.e. `#[cfg(all(target_os = "openbsd", not(test)))]`). The compiled binary target (run in subprocess integration tests) does not have `cfg(test)` and will exercise the real sandboxing path.

4. **Operator Visibility**:
   - The startup success banner must print a one-line status line indicating the sandbox status on OpenBSD:
     - On OpenBSD: `serving <root> on <addr> (sandbox: pledge/unveil active)`
     - On other platforms: `serving <root> on <addr>`
   - The `src/sandbox.rs` module documentation will be updated to match this reality.

5. **OpenBSD CI/CD Verification**:
   - The automated test suite must run and pass on OpenBSD under GitHub Actions.
   - We will use `vmactions/openbsd-vm@fcf799d7ce9c305ad89eabef1fb2fa5c1c42d0ee` (pinned to `v1` by SHA hash for supply-chain security) to execute `make check` inside a virtualized OpenBSD guest VM on an Ubuntu runner.

## Implementation Details

### 1. Resolver Adaptation (`src/root.rs`)

Modify the compilation guards in `src/root.rs` to add OpenBSD and define `TRAVERSE_DIR` using `O_RDONLY`:

```rust
#[cfg(any(target_os = "linux", target_os = "android"))]
const TRAVERSE_DIR: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "netbsd"))]
const TRAVERSE_DIR: OFlags = OFlags::from_bits_retain(
    (libc::O_SEARCH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) as u32,
);

// OpenBSD does not support execute-only directory descriptors (no O_PATH/O_SEARCH).
// We fall back to O_RDONLY. This means directories in the served tree on OpenBSD
// must be readable by the daemon process for traversal to succeed, unlike on other OSs
// where execute-only permissions are sufficient.
#[cfg(target_os = "openbsd")]
const TRAVERSE_DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
)))]
compile_error!(
    "buffetcar requires O_PATH, O_SEARCH, or O_RDONLY (OpenBSD) so directories can be traversed"
);
```

### 2. Sandbox Hook (`src/sandbox.rs`)

Update the signature of `sandbox::apply` to receive a reference to the root path:

```rust
pub(crate) fn apply(root: &std::path::Path)
```

Implement the FFI bindings to OpenBSD's `unveil(2)` and `pledge(2)` system calls:

```rust
//! Platform sandbox hooks.
//!
//! On OpenBSD, this applies `pledge(2)` and `unveil(2)` to restrict filesystem
//! access and process capabilities, and the startup banner reports that the
//! sandbox is active. On other platforms, it is a no-op.

use std::path::Path;

/// Apply any available platform sandbox.
#[cfg(all(target_os = "openbsd", not(test)))]
pub(crate) fn apply(root: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // Convert root path to C string without panic.
    let path_c = match CString::new(root.as_os_str().as_bytes()) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("error: root path contains NUL bytes");
            std::process::exit(1);
        }
    };
    let r_mode = CString::new("r").unwrap();

    // Safety: calling OS system calls via FFI.
    unsafe {
        // 1. Unveil the root directory with read-only permission.
        // We unveil the absolute, normalized config.root path. Since validate_root()
        // guarantees the final component is not a symlink, this is robust defense-in-depth.
        if libc::unveil(path_c.as_ptr(), r_mode.as_ptr()) != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("error: unveil failed for '{}': {}", root.display(), err);
            std::process::exit(1);
        }

        // 2. Lock unveil to prevent future unveil calls.
        if libc::unveil(std::ptr::null(), std::ptr::null()) != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("error: unveil lock failed: {}", err);
            std::process::exit(1);
        }

        // 3. Pledge: stdio, rpath, and inet.
        // - stdio: standard I/O and pthread creation/synchronization.
        // - rpath: read files under the unveiled root.
        // - inet: bind socket, listen, and accept network connections.
        let promises = CString::new("stdio rpath inet").unwrap();
        if libc::pledge(promises.as_ptr(), std::ptr::null()) != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("error: pledge failed: {}", err);
            std::process::exit(1);
        }
    }
}

/// Apply any available platform sandbox. No-op on non-OpenBSD platforms,
/// and in unit tests to avoid sandboxing the test runner process.
#[cfg(any(not(target_os = "openbsd"), test))]
pub(crate) fn apply(_root: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_is_callable() {
        // Safe to call on all platforms during tests.
        apply(Path::new("/"));
    }
}
```

### 3. Server Startup hook (`src/server.rs`)

Update the call order and signature in `src/server.rs`:

```rust
pub(crate) fn run(config: &ServeConfig, mut banner: impl Write) -> Result<(), ServeError> {
    let root = Root::open(&config.root).map_err(ServeError::Root)?;
    let listener =
        TcpListener::bind(config.listen).map_err(|err| ServeError::Bind(config.listen, err))?;
    crate::sandbox::apply(&config.root);

    // Bind succeeded: this is the startup-success banner.
    let _ = config::write_banner(config, &mut banner);
...
```

### 4. Banner Output (`src/config.rs`)

Update `write_banner` and its unit test in `src/config.rs`:

```rust
pub(crate) fn write_banner(config: &ServeConfig, mut err: impl Write) -> io::Result<()> {
    #[cfg(target_os = "openbsd")]
    writeln!(
        err,
        "serving {} on {} (sandbox: pledge/unveil active)",
        config.root.display(),
        config.listen
    )?;
    #[cfg(not(target_os = "openbsd"))]
    writeln!(
        err,
        "serving {} on {}",
        config.root.display(),
        config.listen
    )?;
    Ok(())
}
```

### 5. CI/CD Integration (`.github/workflows/ci.yml`)

Add an `openbsd` job to the GitHub Actions workflow to verify formatting, clippy lints, and all tests on OpenBSD:

```yaml
  openbsd:
    name: fmt · clippy · test (openbsd)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10 # v6.0.3
        with:
          persist-credentials: false
      - name: Test in OpenBSD VM
        uses: vmactions/openbsd-vm@fcf799d7ce9c305ad89eabef1fb2fa5c1c42d0ee
        with:
          usesh: true
          prepare: |
            pkg_add -I rust rust-rustfmt rust-clippy
          run: |
            make check
```

## Testing Strategy

- **Zero Host-Side OpenBSD Coverage**: OpenBSD-specific branches (like `TRAVERSE_DIR` with `O_RDONLY` and the FFI calls in `sandbox::apply`) cannot compile or execute on the macOS development host or in the standard Linux/macOS CI runners.
- **OpenBSD QEMU CI Verification**: The newly introduced `openbsd` CI job is the sole gate that compiles the codebase and runs `make check` inside the OpenBSD guest OS.
- **Integration Test Safety**: Subprocess-based integration tests (like the bind conflict test in `tests/check_contract.rs`) will exercise the real sandboxed code path of the compiled binary target without affecting the host environment.
