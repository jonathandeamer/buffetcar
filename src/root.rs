//! The root directory capability and the fd-relative, no-follow resolver.
//!
//! All filesystem access is relative to an opened root directory descriptor.
//! Each selector component is opened with `openat` + `O_NOFOLLOW` from the
//! current directory fd, so a symlink anywhere on the path fails the open rather
//! than being followed. `O_NONBLOCK` lets a FIFO or device component be opened
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

/// Open flags for probing a file/special path component.
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

fn open_traverse_dir<D: AsFd, P: Arg>(dir: D, path: P) -> io::Result<OwnedFd> {
    Ok(fs::openat(dir, path, TRAVERSE_DIR, Mode::empty())?)
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

        Ok(Some(Resolved::Dir(cur)))
    }

    /// Open `dir`'s `index` if it is a servable regular file. Anything refused
    /// by policy is treated as absent, so the directory falls back to a listing.
    pub(crate) fn open_index(&self, dir: &OwnedFd) -> io::Result<Option<Vec<u8>>> {
        match self.classify_child(dir, "index")? {
            Some(Child::File(fd)) => crate::read_file(fd).map(Some),
            _ => Ok(None),
        }
    }

    /// Re-open a directory entry under no-follow public-content policy.
    pub(crate) fn classify_child<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Option<Child>> {
        if let Some(child) = self.classify_readable_child(dir, name)? {
            return Ok(Some(child));
        }
        // Fallback for directories that can't be opened with O_RDONLY (e.g.
        // execute-only 0o111). Only include the directory if it is also
        // world-readable (listable): an execute-only directory is traversable
        // but its existence must not be revealed in listings.
        match self.open_child_dir(dir, name)? {
            Some(fd) => {
                let st = fs::fstat(&fd)?;
                if self.listable(&st) {
                    Ok(Some(Child::Dir))
                } else {
                    Ok(None)
                }
            }
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

    fn classify_readable_child<P: Arg>(&self, dir: &OwnedFd, name: P) -> io::Result<Option<Child>> {
        let fd = match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => fd,
            Err(_) => return Ok(None),
        };
        let st = fs::fstat(&fd)?;
        if st.st_dev as u64 != self.dev {
            return Ok(None);
        }
        match FileType::from_raw_mode(st.st_mode) {
            FileType::Directory if self.listable(&st) => Ok(Some(Child::Dir)),
            FileType::RegularFile if self.file_ok(&st) => Ok(Some(Child::File(fd))),
            _ => Ok(None),
        }
    }

    fn open_leaf(&self, dir: &OwnedFd, name: &str, dir_only: bool) -> io::Result<Option<Resolved>> {
        if !dir_only {
            let fd = match fs::openat(dir, name, PROBE, Mode::empty()) {
                Ok(fd) => fd,
                Err(_) => return self.open_leaf_dir(dir, name),
            };
            let st = fs::fstat(&fd)?;
            if st.st_dev as u64 != self.dev {
                return Ok(None);
            }
            match FileType::from_raw_mode(st.st_mode) {
                FileType::Directory if self.dir_ok(&st) => {
                    // Drop the PROBE fd and re-open with TRAVERSE_DIR so that
                    // Resolved::Dir always carries a consistent search-only fd,
                    // matching the fd returned by the dir_only and fallback paths.
                    drop(fd);
                    return self.open_leaf_dir(dir, name);
                }
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
            Err(_) => return Ok(None),
        };
        let st = fs::fstat(&fd)?;
        if st.st_dev as u64 != self.dev || !self.dir_ok(&st) {
            return Ok(None);
        }
        Ok(Some(fd))
    }

    /// A traversable / servable directory: a directory, world-executable.
    fn dir_ok(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::Directory
            && Mode::from_raw_mode(st.st_mode).contains(Mode::XOTH)
    }

    /// A listable directory: traversable and world-readable.
    fn listable(&self, st: &Stat) -> bool {
        self.dir_ok(st) && Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH)
    }

    /// A servable regular file: regular, world-readable, and not a hardlink.
    fn file_ok(&self, st: &Stat) -> bool {
        FileType::from_raw_mode(st.st_mode) == FileType::RegularFile
            && Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH)
            && st.st_nlink as u64 == 1
    }
}
