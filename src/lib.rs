//! Buffetcar Nex server.

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

#[allow(dead_code)] // wired into serve_selector in Task 2
mod selector;

const NOT_FOUND: &[u8] = b"document not found";
const DEFAULT_INDEX: &str = "index";

pub fn serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>> {
    let root = Dir::open_ambient_dir(root, ambient_authority())?;
    let selector = clean_selector(selector);

    if has_dotfile_component(&selector) {
        return Ok(NOT_FOUND.to_vec());
    }

    // cap-std is the load-bearing containment guarantee: it structurally
    // refuses `..` and symlink escapes out of the root with no TOCTOU window.
    // The dotfile check above deliberately does not cover `..`
    // (Component::ParentDir falls through), so containment of relative
    // traversal rests entirely on cap-std — do not weaken that dependency.
    match resolve(&root, &selector)? {
        Some(response) => Ok(response),
        None => Ok(NOT_FOUND.to_vec()),
    }
}

fn clean_selector(selector: &str) -> PathBuf {
    let trimmed = selector.trim_matches('/');
    if trimmed.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(trimmed)
    }
}

fn has_dotfile_component(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => is_dotfile_name(name),
        _ => false,
    })
}

fn is_dotfile_name(name: &OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

/// Resolve a contained selector to its response bytes.
///
/// Returns `Ok(None)` when the target is unavailable — missing, permission
/// denied, or refused by cap-std as an escape. These are indistinguishable to a
/// client by design, so all collapse to a "not found" body and none leak why.
/// Genuine operational faults on an already-opened handle (a failed read, for
/// example) propagate as `Err` so the server layer can log them rather than
/// masquerade them as a missing document.
fn resolve(root: &Dir, path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match root.open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };

    if file.metadata()?.is_dir() {
        return serve_directory(root, path).map(Some);
    }

    read_all(file).map(Some)
}

fn serve_directory(root: &Dir, path: &Path) -> io::Result<Vec<u8>> {
    if let Ok(index) = root.open(path.join(DEFAULT_INDEX)) {
        return read_all(index);
    }

    let dir = root.open_dir(path)?;
    let mut entries = Vec::new();
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        if is_dotfile_name(&name) {
            continue;
        }
        // A Nex selector is text; a non-UTF-8 name could not round-trip to a
        // fetchable link, so omit it rather than emit a lossy placeholder.
        let Some(name) = name.to_str().map(str::to_owned) else {
            continue;
        };
        entries.push((name, entry.file_type()?.is_dir()));
    }

    // Sort by name alone so a directory and a file sharing a prefix order
    // alphabetically; the trailing slash is presentation, applied on render.
    entries.sort();

    let mut listing = String::new();
    for (name, is_dir) in entries {
        listing.push_str("=> ");
        listing.push_str(&name);
        if is_dir {
            listing.push('/');
        }
        listing.push('\n');
    }
    Ok(listing.into_bytes())
}

fn read_all(mut file: cap_std::fs::File) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}
