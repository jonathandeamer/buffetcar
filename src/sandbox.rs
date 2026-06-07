//! Platform sandbox hooks.
//!
//! On OpenBSD, this applies `pledge(2)` and `unveil(2)` to restrict filesystem
//! access and process capabilities, and the startup banner reports that the
//! sandbox is active. On other platforms, it is a no-op.

use std::path::Path;

/// Apply any available platform sandbox.
#[cfg(all(target_os = "openbsd", not(test)))]
pub(crate) fn apply(root: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // Convert root path to C string without panic.
    let path_c = match CString::new(root.as_os_str().as_bytes()) {
        Ok(s) => s,
        Err(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "root path contains NUL bytes",
            ));
        }
    };
    let r_mode = CString::new("r").unwrap();

    // Safety: calling OS system calls via FFI.
    unsafe {
        // 1. Unveil the root directory with read-only permission.
        // We unveil the absolute, normalized config.root path. Since validate_root()
        // guarantees the final component is not a symlink, this is robust defense-in-depth.
        if libc::unveil(path_c.as_ptr(), r_mode.as_ptr()) != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // 2. Lock unveil to prevent future unveil calls.
        if libc::unveil(std::ptr::null(), std::ptr::null()) != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // 3. Pledge: stdio, rpath, and inet.
        // - stdio: standard I/O and pthread creation/synchronization.
        // - rpath: read files under the unveiled root.
        // - inet: bind socket, listen, and accept network connections.
        let promises = CString::new("stdio rpath inet").unwrap();
        if libc::pledge(promises.as_ptr(), std::ptr::null()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

/// Apply any available platform sandbox. No-op on non-OpenBSD platforms,
/// and in unit tests to avoid sandboxing the test runner process.
#[cfg(any(not(target_os = "openbsd"), test))]
pub(crate) fn apply(_root: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_is_callable() {
        // Safe to call on all platforms during tests.
        assert!(apply(Path::new("/")).is_ok());
    }
}
