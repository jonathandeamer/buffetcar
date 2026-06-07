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

// `Stat` integer field widths are platform-dependent: macOS/BSD `st_dev` and
// `st_nlink` are narrower than `u64`, so the `as u64` normalization below is
// required there, while on Linux those fields are already `u64` and the cast is
// flagged as unnecessary. Allow the resulting false-positive lint rather than
// removing casts that other targets need.
#![allow(clippy::unnecessary_cast)]

use crate::selector::Request;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, Stat};
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

/// A diagnostic resolution target that has still been accepted by descriptor policy.
pub(crate) enum DiagnosticTarget {
    File(OwnedFd),
    Dir(OwnedFd),
}

/// Local-only reject reasons for `buffetcar check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RejectReason {
    Missing,
    Symlink,
    SpecialFile,
    CrossDevice,
    Hardlink(u64),
    NotWorldReadable,
    DirectoryNotWorldExecutable,
    DirectoryNotWorldReadable,
    NotADirectory,
    TrailingSlashOnFile,
    ListingTooManyEntries,
    ListingTooManyBytes,
}

impl RejectReason {
    pub(crate) fn message(&self) -> String {
        match self {
            RejectReason::Missing => "not found".to_string(),
            RejectReason::Symlink => "symlink".to_string(),
            RejectReason::SpecialFile => "special file".to_string(),
            RejectReason::CrossDevice => "crosses filesystem boundary".to_string(),
            RejectReason::Hardlink(count) => format!("hardlink count {count}"),
            RejectReason::NotWorldReadable => "not world-readable".to_string(),
            RejectReason::DirectoryNotWorldExecutable => {
                "directory is not world-executable".to_string()
            }
            RejectReason::DirectoryNotWorldReadable => {
                "directory is not world-readable".to_string()
            }
            RejectReason::NotADirectory => "not a directory".to_string(),
            RejectReason::TrailingSlashOnFile => "trailing slash on regular file".to_string(),
            RejectReason::ListingTooManyEntries => {
                "directory listing exceeds 4096 entries".to_string()
            }
            RejectReason::ListingTooManyBytes => {
                "directory listing exceeds 262144 bytes".to_string()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticContext {
    Intermediate,
    Leaf,
    DirOnly,
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
        Ok(Root {
            fd,
            dev: st.st_dev as u64,
        })
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

    pub(crate) fn resolve_diagnostic(
        &self,
        request: &Request,
    ) -> io::Result<Result<DiagnosticTarget, RejectReason>> {
        let mut cur = match self.open_root_dir_diagnostic()? {
            Ok(fd) => fd,
            Err(reason) => return Ok(Err(reason)),
        };

        let total = request.components.len();
        for (i, name) in request.components.iter().enumerate() {
            if i + 1 == total {
                return self.open_leaf_diagnostic(&cur, name, request.dir_only);
            }

            cur = match self.open_child_dir_diagnostic(&cur, name.as_str())? {
                Ok(fd) => fd,
                Err(reason) => return Ok(Err(reason)),
            };
        }

        Ok(Ok(DiagnosticTarget::Dir(cur)))
    }

    pub(crate) fn classify_child_diagnostic<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Result<Child, RejectReason>> {
        match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => {
                let st = fs::fstat(&fd)?;
                match FileType::from_raw_mode(st.st_mode) {
                    FileType::Directory => {
                        if !self.listable(&st) {
                            return Ok(Err(RejectReason::DirectoryNotWorldReadable));
                        }
                        if let Err(reason) = self.accept_dir(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(Child::Dir))
                    }
                    FileType::RegularFile => {
                        if let Err(reason) = self.accept_file(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(Child::File(fd)))
                    }
                    _ => Ok(Err(self.reject_for_stat(&st, DiagnosticContext::Leaf))),
                }
            }
            Err(_) => match self.open_child_dir_diagnostic(dir, name)? {
                Ok(fd) => {
                    let st = fs::fstat(&fd)?;
                    if !self.listable(&st) {
                        Ok(Err(RejectReason::DirectoryNotWorldReadable))
                    } else {
                        Ok(Ok(Child::Dir))
                    }
                }
                Err(reason) => Ok(Err(reason)),
            },
        }
    }

    pub(crate) fn open_listable_dir_diagnostic(
        &self,
        dir: &OwnedFd,
    ) -> io::Result<Result<OwnedFd, RejectReason>> {
        let st = fs::fstat(dir)?;
        if let Err(reason) = self.accept_dir(&st) {
            return Ok(Err(reason));
        }
        if !Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH) {
            return Ok(Err(RejectReason::DirectoryNotWorldReadable));
        }

        let fd = match fs::openat(dir, ".", LIST_DIR, Mode::empty()) {
            Ok(fd) => fd,
            Err(_) => return Ok(Err(RejectReason::DirectoryNotWorldReadable)),
        };
        let st = fs::fstat(&fd)?;
        if let Err(reason) = self.accept_dir(&st) {
            return Ok(Err(reason));
        }
        if Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH) {
            Ok(Ok(fd))
        } else {
            Ok(Err(RejectReason::DirectoryNotWorldReadable))
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

    fn open_root_dir_diagnostic(&self) -> io::Result<Result<OwnedFd, RejectReason>> {
        let fd = match open_traverse_dir(&self.fd, ".") {
            Ok(fd) => fd,
            Err(_) => return Ok(Err(RejectReason::Missing)),
        };
        let st = fs::fstat(&fd)?;
        match self.accept_dir(&st) {
            Ok(()) => Ok(Ok(fd)),
            Err(reason) => Ok(Err(reason)),
        }
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

    fn open_leaf_diagnostic(
        &self,
        dir: &OwnedFd,
        name: &str,
        dir_only: bool,
    ) -> io::Result<Result<DiagnosticTarget, RejectReason>> {
        if dir_only {
            return match self.open_child_dir_diagnostic(dir, name)? {
                Ok(fd) => Ok(Ok(DiagnosticTarget::Dir(fd))),
                Err(_) => Ok(Err(self.diagnose_child(
                    dir,
                    name,
                    DiagnosticContext::DirOnly,
                )?)),
            };
        }

        match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => {
                let st = fs::fstat(&fd)?;
                match FileType::from_raw_mode(st.st_mode) {
                    FileType::Directory => {
                        if let Err(reason) = self.accept_dir(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(DiagnosticTarget::Dir(fd)))
                    }
                    FileType::RegularFile => {
                        if let Err(reason) = self.accept_file(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(DiagnosticTarget::File(fd)))
                    }
                    _ => Ok(Err(self.reject_for_stat(&st, DiagnosticContext::Leaf))),
                }
            }
            Err(_) => match self.open_child_dir_diagnostic(dir, name)? {
                Ok(fd) => Ok(Ok(DiagnosticTarget::Dir(fd))),
                Err(_) => Ok(Err(self.diagnose_child(
                    dir,
                    name,
                    DiagnosticContext::Leaf,
                )?)),
            },
        }
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

    fn open_child_dir_diagnostic<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Result<OwnedFd, RejectReason>> {
        let fd = match open_traverse_dir(dir, name) {
            Ok(fd) => fd,
            Err(_) => {
                return Ok(Err(self.diagnose_child(
                    dir,
                    name,
                    DiagnosticContext::Intermediate,
                )?));
            }
        };
        let st = fs::fstat(&fd)?;
        match self.accept_dir(&st) {
            Ok(()) => Ok(Ok(fd)),
            Err(reason) => Ok(Err(reason)),
        }
    }

    fn diagnose_child<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
        context: DiagnosticContext,
    ) -> io::Result<RejectReason> {
        match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => Ok(self.reject_for_stat(&st, context)),
            Err(_) => Ok(RejectReason::Missing),
        }
    }

    fn accept_dir(&self, st: &Stat) -> Result<(), RejectReason> {
        if st.st_dev as u64 != self.dev {
            return Err(RejectReason::CrossDevice);
        }
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
            return Err(RejectReason::SpecialFile);
        }
        if !Mode::from_raw_mode(st.st_mode).contains(Mode::XOTH) {
            return Err(RejectReason::DirectoryNotWorldExecutable);
        }
        Ok(())
    }

    fn accept_file(&self, st: &Stat) -> Result<(), RejectReason> {
        if st.st_dev as u64 != self.dev {
            return Err(RejectReason::CrossDevice);
        }
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(RejectReason::SpecialFile);
        }
        if !Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH) {
            return Err(RejectReason::NotWorldReadable);
        }
        if st.st_nlink as u64 != 1 {
            return Err(RejectReason::Hardlink(st.st_nlink as u64));
        }
        Ok(())
    }

    fn reject_for_stat(&self, st: &Stat, context: DiagnosticContext) -> RejectReason {
        if st.st_dev as u64 != self.dev {
            return RejectReason::CrossDevice;
        }

        match FileType::from_raw_mode(st.st_mode) {
            FileType::Symlink => RejectReason::Symlink,
            FileType::Directory => self
                .accept_dir(st)
                .err()
                .unwrap_or(RejectReason::DirectoryNotWorldReadable),
            FileType::RegularFile if context == DiagnosticContext::Intermediate => {
                RejectReason::NotADirectory
            }
            FileType::RegularFile if context == DiagnosticContext::DirOnly => self
                .accept_file(st)
                .err()
                .unwrap_or(RejectReason::TrailingSlashOnFile),
            FileType::RegularFile => self
                .accept_file(st)
                .err()
                .unwrap_or(RejectReason::NotWorldReadable),
            _ => RejectReason::SpecialFile,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::{self, DirectoryResponse};
    use crate::selector::parse_diagnostic;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn diagnostic_resolve_reports_file_and_directory_success() {
        let site = TempSite::new();
        site.write("public.txt", b"public\n");
        site.write("listing/page.txt", b"page\n");

        let root = Root::open(site.path()).expect("open root");
        let file = parse_diagnostic("public.txt").expect("parse file");
        assert!(matches!(
            root.resolve_diagnostic(&file).expect("diagnose file"),
            Ok(DiagnosticTarget::File(_))
        ));

        let dir = parse_diagnostic("listing/").expect("parse dir");
        let target = root
            .resolve_diagnostic(&dir)
            .expect("diagnose dir")
            .expect("dir target");
        let DiagnosticTarget::Dir(fd) = target else {
            panic!("expected directory target");
        };
        assert_eq!(
            listing::diagnose(&root, fd).expect("diagnose listing"),
            Ok(DirectoryResponse::Listing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_resolve_reports_stable_reject_reasons() {
        let site = TempSite::new();
        site.write("private.txt", b"private\n");
        site.chmod("private.txt", 0o600);
        site.write("linked.txt", b"linked\n");
        fs::hard_link(
            site.path().join("linked.txt"),
            site.path().join("alias.txt"),
        )
        .expect("create hardlink");
        site.write("locked/inside.txt", b"inside\n");
        site.chmod("locked", 0o600);
        site.symlink("linked.txt", "link.txt");

        let root = Root::open(site.path()).expect("open root");

        assert!(matches!(
            diagnose_selector(&root, "private.txt"),
            Err(RejectReason::NotWorldReadable)
        ));
        assert!(matches!(
            diagnose_selector(&root, "linked.txt"),
            Err(RejectReason::Hardlink(2))
        ));
        assert!(matches!(
            diagnose_selector(&root, "locked/inside.txt"),
            Err(RejectReason::DirectoryNotWorldExecutable)
        ));
        assert!(matches!(
            diagnose_selector(&root, "link.txt"),
            Err(RejectReason::Symlink)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_directory_listing_rejects_non_world_readable_directory() {
        let site = TempSite::new();
        site.write("hidden/inside.txt", b"inside\n");
        site.chmod("hidden", 0o111);

        let root = Root::open(site.path()).expect("open root");
        assert!(matches!(
            diagnose_selector(&root, "hidden/inside.txt"),
            Ok(DiagnosticTarget::File(_))
        ));

        let target = diagnose_selector(&root, "hidden").expect("hidden dir target");
        let DiagnosticTarget::Dir(fd) = target else {
            panic!("expected hidden directory target");
        };
        assert_eq!(
            listing::diagnose(&root, fd).expect("diagnose hidden dir"),
            Err(RejectReason::DirectoryNotWorldReadable)
        );
    }

    fn diagnose_selector(root: &Root, selector: &str) -> Result<DiagnosticTarget, RejectReason> {
        let request = parse_diagnostic(selector).expect("parse selector");
        root.resolve_diagnostic(&request)
            .expect("diagnose selector")
    }

    struct TempSite {
        path: PathBuf,
    }

    impl TempSite {
        fn new() -> Self {
            let path = std::env::temp_dir().join(unique_name("buffetcar-root", ""));
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

        #[cfg(unix)]
        fn chmod(&self, relative: &str, mode: u32) {
            make_public(&self.path.join(relative), mode);
        }

        #[cfg(unix)]
        fn symlink(&self, target: &str, link: &str) {
            std::os::unix::fs::symlink(target, self.path.join(link))
                .expect("create symlink fixture");
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
