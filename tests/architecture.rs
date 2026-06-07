//! Architecture guard: the request path must resolve only through the
//! fd-relative resolver. It must never open a whole request path or assemble
//! selector components into a path with `join`.

use std::fs;
use std::path::Path;

/// Modules on the network request path.
const REQUEST_PATH_MODULES: &[&str] = &[
    "src/lib.rs",
    "src/selector.rs",
    "src/root.rs",
    "src/listing.rs",
    "src/conn.rs",
    "src/server.rs",
];

/// Whole-path open helpers that bypass per-component `openat` + `O_NOFOLLOW`.
const FORBIDDEN_OPENS: &[&str] = &[
    "File::open",
    "fs::read(",
    "read_to_string",
    "read_dir",
    "cap_std",
    "canonicalize",
];

/// Modules that resolve selector components into files; none may build a path
/// with `join`. (`server` is excluded: it handles sockets and worker threads,
/// not selector resolution, so its `JoinHandle::join` is not a path join.)
const SELECTOR_PATH_MODULES: &[&str] = &[
    "src/selector.rs",
    "src/root.rs",
    "src/listing.rs",
    "src/conn.rs",
];

/// Read a source file and return only its production portion (everything before
/// the first `#[cfg(test)]`), so test fixtures using `join`/`fs` do not trip the
/// guard.
fn production_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {relative}: {err}"));
    match text.find("#[cfg(test)]") {
        Some(idx) => text[..idx].to_string(),
        None => text,
    }
}

#[test]
fn request_path_never_uses_whole_path_opens() {
    for module in REQUEST_PATH_MODULES {
        let src = production_source(module);
        for needle in FORBIDDEN_OPENS {
            assert!(
                !src.contains(needle),
                "{module} uses forbidden whole-path open helper `{needle}`"
            );
        }
    }
}

#[test]
fn request_path_never_joins_selector_components() {
    for module in SELECTOR_PATH_MODULES {
        let src = production_source(module);
        assert!(
            !src.contains(".join("),
            "{module} assembles a path with `.join(` on the request path"
        );
    }
}
