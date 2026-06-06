//! Buffetcar Nex server.

use cap_std::ambient_authority;
use cap_std::fs::{Dir, FileType};
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

const NOT_FOUND: &[u8] = b"document not found";
const DEFAULT_INDEX: &str = "index";

pub fn serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>> {
    let root = Dir::open_ambient_dir(root, ambient_authority())?;
    let selector = clean_selector(selector);

    if has_dotfile_component(&selector) {
        return Ok(NOT_FOUND.to_vec());
    }

    match serve_path(&root, &selector) {
        Ok(response) => Ok(response),
        Err(_) => Ok(NOT_FOUND.to_vec()),
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

fn serve_path(root: &Dir, path: &Path) -> io::Result<Vec<u8>> {
    let file = root.open(path)?;
    let metadata = file.metadata()?;

    if metadata.is_dir() {
        return serve_directory(root, path);
    }

    read_all(file)
}

fn serve_directory(root: &Dir, path: &Path) -> io::Result<Vec<u8>> {
    let index_path = path.join(DEFAULT_INDEX);
    if let Ok(index) = root.open(&index_path) {
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

        let mut line = String::from("=> ");
        line.push_str(&name.to_string_lossy());
        if is_dir(entry.file_type()?) {
            line.push('/');
        }
        line.push('\n');
        entries.push(line);
    }

    entries.sort();
    Ok(entries.concat().into_bytes())
}

fn is_dir(file_type: FileType) -> bool {
    file_type.is_dir()
}

fn read_all(mut file: cap_std::fs::File) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}
