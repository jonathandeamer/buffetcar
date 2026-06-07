//! Directory `index` lookup and plain-text Nex listings.

use crate::root::{Child, Root};
use rustix::fs::Dir;
use std::io;
use std::os::fd::OwnedFd;

/// Hardcoded listing bounds (spec: "Directory Listings").
const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: usize = 256 * 1024;

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
        if entries.len() >= MAX_ENTRIES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
    }

    entries.sort();

    let mut out = String::new();
    for (name, is_dir) in entries {
        // Pre-check avoids appending past the byte cap.
        let extra = 4 + name.len() + usize::from(is_dir); // "=> " + name + "\n" + optional "/"
        if out.len() + extra > MAX_BYTES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
        out.push_str("=> ");
        out.push_str(&name);
        if is_dir {
            out.push('/');
        }
        out.push('\n');
    }
    Ok(out.into_bytes())
}
