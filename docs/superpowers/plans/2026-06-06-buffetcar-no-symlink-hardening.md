# Buffetcar No-Symlink Hardening & Library Split — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce buffetcar's no-symlink-following invariant uniformly — across selector resolution, the implicit directory-`index` open, and directory-listing enumeration — so a symlinked target can never bypass dotfile rejection, and split the resolution/listing logic out of `lib.rs` into focused modules.

**Architecture:** Today `serve_selector` calls `cap_std::fs::Dir::open(path)`, which *follows* symlinks; an in-root symlink (e.g. `public -> .secret` or `index -> .secret`) therefore serves an otherwise-blocked target. We replace the single `open` with a component-by-component walk under cap-std that refuses any component whose `symlink_metadata` reports a symlink (cap-std still handles `..` containment and escapes). The directory `index` is served only when `symlink_metadata` shows a real regular file, and listing generation skips entries whose `DirEntry::file_type()` is a symlink. The logic moves into `resolve.rs` (containment + dotfile/symlink policy) and `listing.rs` (index lookup + listing), with `lib.rs` reduced to the public entry point, shared helpers, and module declarations.

**Tech Stack:** Rust 2021, `cap-std` 4.0.2. Relevant API (verified against the installed crate): `Dir::try_clone`, `Dir::symlink_metadata` (no-follow), `Dir::open_dir`, `Dir::open`, `Dir::entries`; `DirEntry::file_type` / `file_name`; `Metadata::is_symlink/is_dir/is_file`; `FileType::is_symlink/is_dir`. Tests use the existing `tests/buffetcar_contract.rs` harness (`TempSite`, `respond`, `unique_name`).

**Scope:** This plan covers spec sections §4 (listing omits symlinks), §5 (symlinked-index falls back to listing), §6 (no-symlink invariant), §7 (symlink tests), and the §9 `resolve.rs` / `listing.rs` split. It does **not** cover `config`/`server`/`conn`/`sandbox`, the startup banner, timeouts, or CLI — those are later plans. After this plan the library still exposes the same public `serve_selector(root, selector)` and the full existing contract stays green.

**Threat-model note:** no-follow is enforced via `symlink_metadata` checks rather than an atomic no-follow open (cap-std 4.0.2 exposes no public no-follow open without the extra `cap-fs-ext` dependency, which the minimalism budget rejects). This leaves a check-then-open TOCTOU window only against a concurrent writer inside the served tree — outside buffetcar's read-only-static-publishing threat model, and cap-std still structurally blocks root escapes regardless.

---

## File Structure

- `src/lib.rs` (modify) — public `serve_selector`; shared helpers `clean_selector`, `has_dotfile_component`, `is_dotfile_name`, `read_all`; constants `NOT_FOUND`, `DEFAULT_INDEX`; `mod resolve; mod listing;`. Dispatches a resolved entry to a file read or a listing.
- `src/resolve.rs` (create) — `Resolved` enum and `resolve()`: the cap-std containment walk enforcing the dotfile (pre-checked in `lib.rs`) and no-symlink policy. Owns the "cap-std is load-bearing for `..`" reasoning.
- `src/listing.rs` (create) — `serve_directory()`: serve a real `index` file if present (no-follow), else render a sorted plain-text listing omitting dotfiles, symlinks, and non-UTF-8 names.
- `tests/buffetcar_contract.rs` (modify) — add a `symlink` helper to `TempSite` and four `#[cfg(unix)]` tests for the new behavior.

---

## Task 1: Split `lib.rs` into `resolve.rs` + `listing.rs` (behavior-preserving)

This is a pure refactor: it introduces the final module boundaries and the `Resolved` enum while keeping today's symlink-following behavior, so the entire existing test suite must stay green. No new tests.

**Files:**
- Create: `src/resolve.rs`
- Create: `src/listing.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/resolve.rs` with the current (follow) behavior expressed as a `Resolved` enum**

```rust
//! Selector → file/dir resolution inside the site root.
//!
//! `cap-std` is the load-bearing containment guarantee: it structurally refuses
//! `..` escapes and symlink escapes out of the root with no TOCTOU window. The
//! dotfile check in `lib.rs` runs before resolution and deliberately does not
//! cover `..` (a `ParentDir` component), so containment of relative traversal
//! rests entirely on cap-std — do not weaken that dependency.

use cap_std::fs::{Dir, File};
use std::io;
use std::path::Path;

/// A resolved, already-opened target within the root.
pub(crate) enum Resolved {
    File(File),
    Dir(Dir),
}

/// Resolve a contained selector to an open file or directory handle.
///
/// Returns `Ok(None)` when the target is unavailable — missing, permission
/// denied, or refused by cap-std as an escape. These are indistinguishable to a
/// client by design.
pub(crate) fn resolve(root: &Dir, path: &Path) -> io::Result<Option<Resolved>> {
    let file = match root.open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };

    if file.metadata()?.is_dir() {
        return match root.open_dir(path) {
            Ok(dir) => Ok(Some(Resolved::Dir(dir))),
            Err(_) => Ok(None),
        };
    }

    Ok(Some(Resolved::File(file)))
}
```

- [ ] **Step 2: Create `src/listing.rs` operating on an opened `Dir` (behavior-preserving)**

```rust
//! Directory handling: serve the `index` file if present, else generate a
//! plain-text Nex listing (`=> ` links, trailing `/` on subdirectories).

use crate::{is_dotfile_name, read_all, DEFAULT_INDEX};
use cap_std::fs::Dir;
use std::io;

pub(crate) fn serve_directory(dir: &Dir) -> io::Result<Vec<u8>> {
    if let Ok(index) = dir.open(DEFAULT_INDEX) {
        return read_all(index);
    }

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
```

- [ ] **Step 3: Replace `src/lib.rs` with the dispatcher + shared helpers**

```rust
//! Buffetcar Nex server.

use cap_std::ambient_authority;
use cap_std::fs::{Dir, File};
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

mod listing;
mod resolve;

pub(crate) const NOT_FOUND: &[u8] = b"document not found";
pub(crate) const DEFAULT_INDEX: &str = "index";

pub fn serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>> {
    let root = Dir::open_ambient_dir(root, ambient_authority())?;
    let selector = clean_selector(selector);

    if has_dotfile_component(&selector) {
        return Ok(NOT_FOUND.to_vec());
    }

    match resolve::resolve(&root, &selector)? {
        Some(resolve::Resolved::File(file)) => read_all(file),
        Some(resolve::Resolved::Dir(dir)) => listing::serve_directory(&dir),
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

pub(crate) fn is_dotfile_name(name: &OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

pub(crate) fn read_all(mut file: File) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}
```

- [ ] **Step 4: Run the full suite to verify behavior is unchanged**

Run: `cargo test`
Expected: PASS — all existing contract tests green (`serves_files_directory_indexes_listings_and_not_found`, `directory_listings_sort_by_name_independent_of_trailing_slash`, `omits_non_utf8_names_from_listings`, the binary-preservation test, `rejects_dotfiles_by_default_and_omits_them_from_listings`, `allows_balanced_parent_components_but_rejects_above_root_escape`, `rejects_symlink_escape_outside_the_root`).

- [ ] **Step 5: Verify formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS, no output from clippy.

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/resolve.rs src/listing.rs
git commit -m "refactor: split resolve and listing out of lib.rs"
```

---

## Task 2: No-follow selector resolution

Replace the `open`-based resolution with a component walk that refuses any symlink component, closing the `public -> .secret` bypass while preserving `..`/escape behavior.

**Files:**
- Modify: `tests/buffetcar_contract.rs`
- Modify: `src/resolve.rs`

- [ ] **Step 1: Add a `symlink` helper to `TempSite`**

Add this method inside the `impl TempSite { ... }` block in `tests/buffetcar_contract.rs`, after the existing `dir` method:

```rust
    /// Create a symlink at `link` (relative to the site root) pointing at
    /// `target` (a raw link string, kept relative so it stays inside the root).
    #[cfg(unix)]
    fn symlink(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(target, self.path.join(link)).expect("create symlink fixture");
    }
```

- [ ] **Step 2: Write the failing test for an in-root symlink to a dotfile target**

Add this test function to `tests/buffetcar_contract.rs`:

```rust
#[cfg(unix)]
#[test]
fn refuses_in_root_symlink_to_dotfile_target() {
    let site = TempSite::new();
    site.write(".secret", b"top secret\n");
    // `public` is a non-dot name, so it passes the dotfile component check, but
    // it is a symlink whose (relative, in-root) target is a dotfile. cap-std
    // would follow it; no-follow must refuse it.
    site.symlink(".secret", "public");

    assert_eq!(respond(site.path(), "public"), b"document not found");
}

#[cfg(unix)]
#[test]
fn refuses_in_root_symlink_to_ordinary_target() {
    let site = TempSite::new();
    site.write("real.txt", b"real\n");
    site.symlink("real.txt", "alias.txt");

    assert_eq!(respond(site.path(), "alias.txt"), b"document not found");
}
```

- [ ] **Step 3: Run the new tests to verify they fail**

Run: `cargo test refuses_in_root_symlink -- --nocapture`
Expected: FAIL — `refuses_in_root_symlink_to_dotfile_target` returns `b"top secret\n"` and `refuses_in_root_symlink_to_ordinary_target` returns `b"real\n"`, because the current `open` follows the symlink.

- [ ] **Step 4: Replace the body of `resolve` in `src/resolve.rs` with the no-follow walk**

Replace the entire `pub(crate) fn resolve(...)` function (keep the module doc comment and the `Resolved` enum) with:

```rust
/// Resolve a contained selector to an open file or directory handle, **without
/// following any symlink**.
///
/// The selector is walked one component at a time from the root. Each `Normal`
/// component is checked with `symlink_metadata` (which does not follow the final
/// link) and refused if it is a symlink; `..` is delegated to cap-std, which
/// refuses escapes. Anything unavailable — missing, a symlink, or an escape —
/// collapses to `Ok(None)`.
pub(crate) fn resolve(root: &Dir, path: &Path) -> io::Result<Option<Resolved>> {
    let mut dir = root.try_clone()?;
    let components: Vec<Component> = path.components().collect();

    for (index, component) in components.iter().enumerate() {
        let is_last = index + 1 == components.len();

        let name = match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                // `..` is never a symlink; cap-std refuses escapes above the root.
                match dir.open_dir("..") {
                    Ok(parent) => {
                        dir = parent;
                        continue;
                    }
                    Err(_) => return Ok(None),
                }
            }
            Component::Normal(name) => name,
            // RootDir / Prefix cannot occur: `clean_selector` trims leading `/`.
            _ => return Ok(None),
        };

        // No-follow: refuse any component that is itself a symlink.
        let metadata = match dir.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(None),
        };
        if metadata.is_symlink() {
            return Ok(None);
        }

        if metadata.is_dir() {
            dir = match dir.open_dir(name) {
                Ok(child) => child,
                Err(_) => return Ok(None),
            };
            if is_last {
                return Ok(Some(Resolved::Dir(dir)));
            }
        } else {
            // A non-directory may only be the final component.
            if !is_last {
                return Ok(None);
            }
            return match dir.open(name) {
                Ok(file) => Ok(Some(Resolved::File(file))),
                Err(_) => Ok(None),
            };
        }
    }

    // Empty selector (or only `.`/balanced `..`) resolves to a directory.
    Ok(Some(Resolved::Dir(dir)))
}
```

Then update the imports at the top of `src/resolve.rs` to add `Component`:

```rust
use cap_std::fs::{Dir, File};
use std::io;
use std::path::{Component, Path};
```

- [ ] **Step 5: Run the new tests to verify they pass**

Run: `cargo test refuses_in_root_symlink`
Expected: PASS — both return `b"document not found"`.

- [ ] **Step 6: Run the full suite to verify no regressions**

Run: `cargo test`
Expected: PASS — every test, including `allows_balanced_parent_components_but_rejects_above_root_escape` and `rejects_symlink_escape_outside_the_root` (the escape walk now goes through `dir.open_dir("..")` / `symlink_metadata`, and must still yield `document not found`).

- [ ] **Step 7: Verify formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/resolve.rs tests/buffetcar_contract.rs
git commit -m "feat: refuse symlink components in selector resolution"
```

---

## Task 3: No-follow `index` open and symlink omission in listings

Apply the same rule to the implicit directory-`index` open (a symlinked `index` is treated as missing → listing) and to listing enumeration (symlink entries are omitted, like dotfiles).

**Files:**
- Modify: `tests/buffetcar_contract.rs`
- Modify: `src/listing.rs`

- [ ] **Step 1: Write the failing tests**

Add these test functions to `tests/buffetcar_contract.rs`:

```rust
#[cfg(unix)]
#[test]
fn symlinked_index_is_not_served_and_falls_back_to_listing() {
    let site = TempSite::new();
    site.write("docs/.secret", b"secret index\n");
    site.write("docs/page.txt", b"page\n");
    // `index` is a symlink to a dotfile in the same dir. It must not be served;
    // the directory falls back to a listing that omits both the dotfile target
    // and the symlink entry itself.
    site.symlink(".secret", "docs/index");

    assert_eq!(respond(site.path(), "docs"), b"=> page.txt\n");
}

#[cfg(unix)]
#[test]
fn omits_symlink_entries_from_listings() {
    let site = TempSite::new();
    site.write("links/real.txt", b"real\n");
    site.symlink("real.txt", "links/alias.txt");

    assert_eq!(respond(site.path(), "links"), b"=> real.txt\n");
}
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -- symlinked_index_is_not_served_and_falls_back_to_listing omits_symlink_entries_from_listings`
Expected: FAIL — `symlinked_index_...` serves `b"secret index\n"` (the symlinked index is followed), and `omits_symlink_entries_...` returns `b"=> alias.txt\n=> real.txt\n"` (the symlink entry is listed).

- [ ] **Step 3: Update `serve_directory` in `src/listing.rs` to apply no-follow**

Replace the entire `pub(crate) fn serve_directory(...)` function with:

```rust
pub(crate) fn serve_directory(dir: &Dir) -> io::Result<Vec<u8>> {
    // Serve `index` only when it is a real regular file. `symlink_metadata` does
    // not follow the link, so `is_file()` is false for a symlinked `index`,
    // which then falls back to a listing (never followed).
    if let Ok(metadata) = dir.symlink_metadata(DEFAULT_INDEX) {
        if metadata.is_file() {
            if let Ok(index) = dir.open(DEFAULT_INDEX) {
                return read_all(index);
            }
        }
    }

    let mut entries = Vec::new();
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        if is_dotfile_name(&name) {
            continue;
        }
        let file_type = entry.file_type()?;
        // Omit symlink entries: a symlink is never served (no-follow), so listing
        // it would only emit a dead link and reveal the link's existence.
        if file_type.is_symlink() {
            continue;
        }
        // A Nex selector is text; a non-UTF-8 name could not round-trip to a
        // fetchable link, so omit it rather than emit a lossy placeholder.
        let Some(name) = name.to_str().map(str::to_owned) else {
            continue;
        };
        entries.push((name, file_type.is_dir()));
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
```

- [ ] **Step 4: Run the new tests to verify they pass**

Run: `cargo test -- symlinked_index_is_not_served_and_falls_back_to_listing omits_symlink_entries_from_listings`
Expected: PASS.

- [ ] **Step 5: Run the full suite to verify no regressions**

Run: `cargo test`
Expected: PASS — all tests, including the original `serves_files_directory_indexes_listings_and_not_found` (real `index` files still served) and `rejects_dotfiles_by_default_and_omits_them_from_listings`.

- [ ] **Step 6: Verify formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/listing.rs tests/buffetcar_contract.rs
git commit -m "feat: no-follow index open and omit symlink entries from listings"
```

---

## Self-Review

**Spec coverage:**
- §6 "No symlink following (invariant)" — point 1 (selector resolution) → Task 2; point 2 (implicit `index` open) → Task 3; point 3 (listing omits symlinks) → Task 3. ✓
- §4 "dotfiles and symlink entries omitted" → Task 3. ✓
- §5 "a symlinked `index` … yields the listing" → Task 3 (`symlinked_index_...` test). ✓
- §7 tests: "symlink inside the tree refused (no-follow)" → Task 2; "symlinked index treated as missing" + "symlink entries omitted" → Task 3. ✓
- §9 `resolve.rs` / `listing.rs` split → Task 1. ✓
- Out of scope by design (later plans): `config`, `server`, `conn`, `sandbox`, banner, timeouts, selector-length bound enforcement, bind tests.

**Placeholder scan:** none — every code and command step is concrete.

**Type consistency:** `Resolved` (variants `File`, `Dir`) is defined in Task 1 and used unchanged in Task 2 and in `lib.rs`'s dispatch. `serve_directory(&Dir) -> io::Result<Vec<u8>>` is defined in Task 1 and only has its body changed in Task 3 (signature stable). `read_all(File)`, `is_dotfile_name(&OsStr)`, `DEFAULT_INDEX`, `NOT_FOUND` are declared `pub(crate)` in `lib.rs` (Task 1) and referenced via `crate::` in `listing.rs`. `TempSite::symlink(target, link)` is added in Task 2 and reused in Task 3.
