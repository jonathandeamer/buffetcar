//! Shared filesystem scaffolding for the per-module unit tests.
//!
//! Compiled only under `#[cfg(test)]`. `TempSite` creates a throwaway directory
//! tree under the system temp dir and removes it on drop. On Unix the root is
//! created world-traversable (0755) and written files world-readable (0644) so
//! fixtures satisfy buffetcar's publishing policy without per-test chmod.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) struct TempSite {
    path: PathBuf,
}

impl TempSite {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(unique_name());
        fs::create_dir(&path).expect("create temp site root");
        #[cfg(unix)]
        make_public(&path, 0o755);
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn write(&self, relative: &str, content: &[u8]) {
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
    pub(crate) fn chmod(&self, relative: &str, mode: u32) {
        make_public(&self.path.join(relative), mode);
    }

    #[cfg(unix)]
    pub(crate) fn symlink(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(target, self.path.join(link)).expect("create symlink fixture");
    }
}

impl Drop for TempSite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn unique_name() -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("buffetcar-test-{}-{n}", std::process::id())
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
