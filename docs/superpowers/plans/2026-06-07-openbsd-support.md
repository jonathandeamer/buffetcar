# OpenBSD Support and Sandboxing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement OpenBSD compatibility, sandboxing via `pledge` and `unveil`, and automated CI testing inside an OpenBSD VM.

**Architecture:** The path resolver fallback uses `O_RDONLY` for directory descent on OpenBSD because OpenBSD lacks execute-only descriptors (`O_PATH`/`O_SEARCH`). The daemon sandboxing calls `unveil` to restrict access to the absolute root directory, locks it, and pledges `"stdio rpath inet"`. This is triggered in `server::run` after `TcpListener::bind` to avoid polluting the in-process test runner.

**Tech Stack:** Rust standard library, `rustix`, `libc` for OpenBSD FFI, GitHub Actions with `vmactions/openbsd-vm`.

---

### Task 1: OpenBSD Resolver Adaptation

**Files:**
- Modify: `src/root.rs:43-63`

- [ ] **Step 1: Modify resolver compilation guards**
  Replace the existing OS compilation guards and `TRAVERSE_DIR` flags in `src/root.rs` to add OpenBSD and define `TRAVERSE_DIR` as `O_RDONLY`.

  Code to replace:
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
  ```

  Replacement code:
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

- [ ] **Step 2: Run local test suite**
  Verify the project compiles and all tests pass on the host platform.
  Run: `make check`
  Expected: SUCCESS

- [ ] **Step 3: Commit**
  ```bash
  git add src/root.rs
  git commit -m "feat(root): support O_RDONLY directory traversal on OpenBSD"
  ```

---

### Task 2: Sandbox Implementation

**Files:**
- Modify: `src/sandbox.rs`

- [ ] **Step 1: Rewrite `src/sandbox.rs`**
  Modify `src/sandbox.rs` to implement OpenBSD `unveil` and `pledge` sandboxing for target OS OpenBSD, while remaining a no-op on other platforms and when compiling under unit tests.

  Replacement code:
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

- [ ] **Step 2: Run local test suite**
  Run: `make check`
  Expected: SUCCESS

- [ ] **Step 3: Commit**
  ```bash
  git add src/sandbox.rs
  git commit -m "feat(sandbox): implement OpenBSD pledge/unveil sandboxing"
  ```

---

### Task 3: Server Startup Integration

**Files:**
- Modify: `src/server.rs:48-65`

- [ ] **Step 1: Update the startup order and sandbox call**
  In `src/server.rs`, update `run` to invoke `TcpListener::bind` *before* `sandbox::apply` and pass the root path to `apply`.

  Code to replace:
  ```rust
  pub(crate) fn run(config: &ServeConfig, mut banner: impl Write) -> Result<(), ServeError> {
      let root = Root::open(&config.root).map_err(ServeError::Root)?;
      crate::sandbox::apply();
      let listener =
          TcpListener::bind(config.listen).map_err(|err| ServeError::Bind(config.listen, err))?;

      // Bind succeeded: this is the startup-success banner.
      let _ = config::write_banner(config, &mut banner);
  ```

  Replacement code:
  ```rust
  pub(crate) fn run(config: &ServeConfig, mut banner: impl Write) -> Result<(), ServeError> {
      let root = Root::open(&config.root).map_err(ServeError::Root)?;
      let listener =
          TcpListener::bind(config.listen).map_err(|err| ServeError::Bind(config.listen, err))?;
      crate::sandbox::apply(&config.root);

      // Bind succeeded: this is the startup-success banner.
      let _ = config::write_banner(config, &mut banner);
  ```

- [ ] **Step 2: Run local test suite**
  Verify that the unit test `run_reports_bind_conflict` runs successfully (returns early on bind failure without calling the sandbox) and all other tests pass on the host platform.
  Run: `make check`
  Expected: SUCCESS

- [ ] **Step 3: Commit**
  ```bash
  git add src/server.rs
  git commit -m "feat(server): bind socket before applying sandbox to protect unit tests"
  ```

---

### Task 4: Startup Banner Verification

**Files:**
- Modify: `src/config.rs:58-66` (write_banner) and `src/config.rs:450-469` (formats_startup_banner)

- [ ] **Step 1: Modify banner formatting**
  In `src/config.rs`, modify `write_banner` to append `(sandbox: pledge/unveil active)` to the status line on OpenBSD.

  Code to replace:
  ```rust
  pub(crate) fn write_banner(config: &ServeConfig, mut err: impl Write) -> io::Result<()> {
      writeln!(
          err,
          "serving {} on {}",
          config.root.display(),
          config.listen
      )?;
      Ok(())
  }
  ```

  Replacement code:
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

- [ ] **Step 2: Update startup banner unit test**
  Update the `formats_startup_banner` test in `src/config.rs` to support both OpenBSD and non-OpenBSD expected outcomes.

  Code to replace:
  ```rust
      #[test]
      fn formats_startup_banner() {
          let site = TempSite::new();
          let config = ServeConfig {
              root: site.path().to_path_buf(),
              listen: DEFAULT_LISTEN,
              workers: DEFAULT_WORKERS,
              write_timeout: Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS),
          };
          let mut stderr = Vec::new();
          write_banner(&config, &mut stderr).expect("write banner");
          let stderr = String::from_utf8(stderr).expect("banner utf8");

          assert_eq!(
              stderr,
              format!("serving {} on 127.0.0.1:1900\n", site.path().display())
          );
      }
  ```

  Replacement code:
  ```rust
      #[test]
      fn formats_startup_banner() {
          let site = TempSite::new();
          let config = ServeConfig {
              root: site.path().to_path_buf(),
              listen: DEFAULT_LISTEN,
              workers: DEFAULT_WORKERS,
              write_timeout: Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS),
          };
          let mut stderr = Vec::new();
          write_banner(&config, &mut stderr).expect("write banner");
          let stderr = String::from_utf8(stderr).expect("banner utf8");

          #[cfg(target_os = "openbsd")]
          assert_eq!(
              stderr,
              format!(
                  "serving {} on 127.0.0.1:1900 (sandbox: pledge/unveil active)\n",
                  site.path().display()
              )
          );
          #[cfg(not(target_os = "openbsd"))]
          assert_eq!(
              stderr,
              format!("serving {} on 127.0.0.1:1900\n", site.path().display())
          );
      }
  ```

- [ ] **Step 3: Run local test suite**
  Run: `make check`
  Expected: SUCCESS

- [ ] **Step 4: Commit**
  ```bash
  git add src/config.rs
  git commit -m "feat(config): print active sandbox status in banner on OpenBSD"
  ```

---

### Task 5: CI/CD Workflow Setup

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add OpenBSD VM job**
  Modify `.github/workflows/ci.yml` to include a new `openbsd` job using the pinned `vmactions/openbsd-vm` action.

  Add this block directly under the `check` job in `.github/workflows/ci.yml`:
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

- [ ] **Step 2: Run local test suite to ensure syntax correctness**
  Run: `make check`
  Expected: SUCCESS

- [ ] **Step 3: Commit**
  ```bash
  git add .github/workflows/ci.yml
  git commit -m "ci: add OpenBSD QEMU workflow test suite job"
  ```
