//! Buffetcar Nex server.

mod cli;
mod config;
mod listing;
mod root;
mod selector;

use root::{Resolved, Root};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::path::Path;

const NOT_FOUND: &[u8] = b"document not found";

/// Resolve `selector` against `root` and return the response bytes.
///
/// Every unavailable selector returns the same body: missing paths, rejected
/// dotfiles, symlinks, special files, escapes, and policy failures are
/// intentionally indistinguishable to clients.
pub fn serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>> {
    let Some(request) = selector::parse(selector) else {
        return Ok(NOT_FOUND.to_vec());
    };
    let Ok(root) = Root::open(root) else {
        return Ok(NOT_FOUND.to_vec());
    };

    match root.resolve(&request)? {
        Some(Resolved::File(fd)) => read_file(fd),
        Some(Resolved::Dir(fd)) => listing::serve(&root, fd),
        None => Ok(NOT_FOUND.to_vec()),
    }
}

pub(crate) fn read_file(fd: OwnedFd) -> io::Result<Vec<u8>> {
    let mut file = File::from(fd);
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}
