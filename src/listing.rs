//! Directory `index` lookup and plain-text Nex listings.

use crate::root::{Child, RejectReason, Root};
use rustix::fs::Dir;
use std::io;
use std::os::fd::OwnedFd;

/// Hardcoded listing bounds (spec: "Directory Listings").
const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryResponse {
    Index,
    Listing,
}

/// Serve a directory: stream its `index` if one is servable, otherwise a
/// generated listing. Used by the `serve_selector` library/compatibility path,
/// which buffers everything into a `Vec`.
pub(crate) fn serve(root: &Root, dir: &OwnedFd) -> io::Result<Vec<u8>> {
    if let Some(bytes) = root.open_index(dir)? {
        return Ok(bytes);
    }
    generate(root, dir)
}

/// Generate a bounded plain-text listing for `dir`, or `document not found`
/// bytes if the directory is not listable or exceeds the listing bounds.
pub(crate) fn generate(root: &Root, dir: &OwnedFd) -> io::Result<Vec<u8>> {
    let Some(list_dir) = root.open_listable_dir(dir)? else {
        return Ok(crate::NOT_FOUND.to_vec());
    };

    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in Dir::read_from(&list_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes.first() == Some(&b'.') {
            continue;
        }
        let Some(child) = root.classify_child(&list_dir, name)? else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
        if entries.len() > MAX_ENTRIES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
    }

    entries.sort();

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
}

pub(crate) fn diagnose(
    root: &Root,
    dir: OwnedFd,
) -> io::Result<Result<DirectoryResponse, RejectReason>> {
    if matches!(
        root.classify_child_diagnostic(&dir, "index")?,
        Ok(Child::File(_))
    ) {
        return Ok(Ok(DirectoryResponse::Index));
    }

    let list_dir = match root.open_listable_dir_diagnostic(&dir)? {
        Ok(fd) => fd,
        Err(reason) => return Ok(Err(reason)),
    };

    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in Dir::read_from(&list_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes.first() == Some(&b'.') {
            continue;
        }
        let Ok(child) = root.classify_child_diagnostic(&list_dir, name)? else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
        if entries.len() > MAX_ENTRIES {
            return Ok(Err(RejectReason::ListingTooManyEntries));
        }
    }

    entries.sort();

    let mut rendered_bytes = 0usize;
    for (name, is_dir) in entries {
        rendered_bytes += 3 + name.len() + usize::from(is_dir) + 1;
        if rendered_bytes > MAX_BYTES {
            return Ok(Err(RejectReason::ListingTooManyBytes));
        }
    }

    Ok(Ok(DirectoryResponse::Listing))
}
