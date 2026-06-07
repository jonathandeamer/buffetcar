use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn serves_files_directory_indexes_listings_and_not_found() {
    let site = TempSite::new();
    site.write("index", b"root index\n");
    site.write("plain.txt", b"plain file\n");
    site.write("docs/index", b"docs index\n");
    site.dir("listing/subdir");
    site.write("listing/apple.txt", b"apple\n");
    site.write("listing/banana.txt", b"banana\n");

    assert_eq!(respond(site.path(), ""), b"root index\n");
    assert_eq!(respond(site.path(), "/"), b"root index\n");
    assert_eq!(respond(site.path(), "plain.txt"), b"plain file\n");
    assert_eq!(respond(site.path(), "/plain.txt"), b"plain file\n");
    assert_eq!(respond(site.path(), "docs"), b"docs index\n");
    assert_eq!(respond(site.path(), "docs/"), b"docs index\n");
    assert_eq!(
        respond(site.path(), "listing"),
        b"=> apple.txt\n=> banana.txt\n=> subdir/\n"
    );
    assert_eq!(respond(site.path(), "missing.txt"), b"document not found");
}

#[test]
fn directory_listings_sort_by_name_independent_of_trailing_slash() {
    let site = TempSite::new();
    site.write("listing/sub/inner.txt", b"inner\n");
    site.write("listing/sub.txt", b"file\n");

    // "sub" < "sub.txt" by name, so the directory lists first even though its
    // rendered "sub/" would sort after "sub.txt" if the slash were included.
    assert_eq!(respond(site.path(), "listing"), b"=> sub/\n=> sub.txt\n");
}

#[cfg(unix)]
#[test]
fn omits_non_utf8_names_from_listings() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let site = TempSite::new();
    site.write("listing/ok.txt", b"ok\n");
    let bad = site
        .path()
        .join("listing")
        .join(OsStr::from_bytes(b"bad\xffname"));
    // Some filesystems (e.g. APFS on macOS) reject non-UTF-8 names outright; on
    // those there is nothing to skip, so the assertion only runs where the
    // fixture can actually be created.
    if fs::write(bad, b"bad\n").is_err() {
        return;
    }

    assert_eq!(respond(site.path(), "listing"), b"=> ok.txt\n");
}

#[test]
fn preserves_binary_file_bytes() {
    let site = TempSite::new();
    let bytes = [0, 1, 2, b'\n', 0xff, b'n', b'e', b'x'];
    site.write("blob.bin", &bytes);

    assert_eq!(respond(site.path(), "blob.bin"), bytes);
}

#[test]
fn rejects_dotfiles_by_default_and_omits_them_from_listings() {
    let site = TempSite::new();
    site.write(".secret", b"secret\n");
    site.write("listing/.hidden", b"hidden\n");
    site.write("listing/public.txt", b"public\n");

    assert_eq!(respond(site.path(), ".secret"), b"document not found");
    assert_eq!(
        respond(site.path(), "listing/.hidden"),
        b"document not found"
    );
    assert_eq!(respond(site.path(), "listing"), b"=> public.txt\n");
}

#[test]
fn allows_balanced_parent_components_but_rejects_above_root_escape() {
    let site = TempSite::new();
    site.dir("a/b");
    site.write("a/c.txt", b"inside root\n");
    let outside_name = unique_name("buffetcar-outside", ".txt");
    let outside = site
        .path()
        .parent()
        .expect("site has parent")
        .join(&outside_name);
    fs::write(&outside, b"outside root\n").expect("write outside fixture");

    assert_eq!(respond(site.path(), "a/b/../c.txt"), b"inside root\n");
    assert_eq!(
        respond(site.path(), &format!("../{outside_name}")),
        b"document not found"
    );

    fs::remove_file(outside).expect("remove outside fixture");
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape_outside_the_root() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    let outside_name = unique_name("buffetcar-symlink-target", ".txt");
    let outside = site
        .path()
        .parent()
        .expect("site has parent")
        .join(outside_name);
    fs::write(&outside, b"symlink target\n").expect("write symlink target");
    symlink(&outside, site.path().join("leak.txt")).expect("create symlink fixture");

    assert_eq!(respond(site.path(), "leak.txt"), b"document not found");

    fs::remove_file(outside).expect("remove symlink target");
}

#[cfg(unix)]
#[test]
fn refuses_in_root_symlink_to_ordinary_target() {
    let site = TempSite::new();
    site.write("real.txt", b"real\n");
    site.symlink("real.txt", "alias.txt");

    assert_eq!(respond(site.path(), "alias.txt"), b"document not found");
}

#[cfg(unix)]
#[test]
fn refuses_in_root_symlink_to_dotfile_target() {
    let site = TempSite::new();
    site.write(".secret", b"top secret\n");
    site.symlink(".secret", "public");

    assert_eq!(respond(site.path(), "public"), b"document not found");
}

#[cfg(unix)]
#[test]
fn symlinked_index_falls_back_to_listing() {
    let site = TempSite::new();
    site.write("docs/.secret", b"secret index\n");
    site.write("docs/page.txt", b"page\n");
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

#[cfg(unix)]
#[test]
fn rejects_and_omits_special_files() {
    let site = TempSite::new();
    site.dir("dev");
    site.write("dev/real.txt", b"real\n");
    let fifo = site.path().join("dev").join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made {
        return;
    }

    assert_eq!(respond(site.path(), "dev/pipe"), b"document not found");
    assert_eq!(respond(site.path(), "dev"), b"=> real.txt\n");
}

#[cfg(unix)]
#[test]
fn rejects_non_world_readable_file() {
    let site = TempSite::new();
    site.write_mode("private.txt", b"private\n", 0o600);

    assert_eq!(respond(site.path(), "private.txt"), b"document not found");
}

#[cfg(unix)]
#[test]
fn rejects_hardlinked_file() {
    let site = TempSite::new();
    site.write("original.txt", b"shared\n");
    fs::hard_link(
        site.path().join("original.txt"),
        site.path().join("alias.txt"),
    )
    .expect("create hardlink fixture");

    assert_eq!(respond(site.path(), "original.txt"), b"document not found");
    assert_eq!(respond(site.path(), "alias.txt"), b"document not found");
}

#[cfg(unix)]
#[test]
fn rejects_non_world_executable_directory() {
    let site = TempSite::new();
    site.write("locked/inside.txt", b"inside\n");
    site.dir_mode("locked", 0o600);

    assert_eq!(
        respond(site.path(), "locked/inside.txt"),
        b"document not found"
    );
    assert_eq!(respond(site.path(), "locked"), b"document not found");
}

#[cfg(unix)]
#[test]
fn rejects_non_world_executable_root() {
    let site = TempSite::new();
    site.write("public.txt", b"public\n");
    make_public(site.path(), 0o700);

    assert_eq!(respond(site.path(), "public.txt"), b"document not found");
    assert_eq!(respond(site.path(), ""), b"document not found");
}

#[cfg(unix)]
#[test]
fn does_not_list_non_world_readable_directory() {
    let site = TempSite::new();
    site.write("hidden/inside.txt", b"inside\n");
    site.dir_mode("hidden", 0o111);

    assert_eq!(respond(site.path(), "hidden/inside.txt"), b"inside\n");
    assert_eq!(respond(site.path(), "hidden"), b"document not found");
    // execute-only directory must not appear in the parent listing -
    // leaking its name would violate the no-information-leakage invariant.
    assert_eq!(respond(site.path(), ""), b"");
}

#[test]
fn rejects_directory_listing_exceeding_entry_bound() {
    let site = TempSite::new();
    for i in 0..4097 {
        site.write(&format!("big/f{i:05}.txt"), b"x\n");
    }

    assert_eq!(respond(site.path(), "big"), b"document not found");
}

#[test]
fn serves_listing_at_the_entry_bound() {
    let site = TempSite::new();
    for i in 0..4096 {
        site.write(&format!("ok/f{i:05}.txt"), b"x\n");
    }

    let listing = respond(site.path(), "ok");
    assert_ne!(listing, b"document not found");
    assert_eq!(listing.iter().filter(|&&b| b == b'\n').count(), 4096);
}

fn respond(root: &Path, selector: &str) -> Vec<u8> {
    buffetcar::serve_selector(root, selector).expect("serve selector")
}

struct TempSite {
    path: PathBuf,
}

impl TempSite {
    fn new() -> Self {
        let path = std::env::temp_dir().join(unique_name("buffetcar-contract", ""));
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
        }
        fs::write(&path, content).expect("write fixture file");
        #[cfg(unix)]
        {
            make_public(&path, 0o644);
            if let Some(parent) = path.parent() {
                make_chain_public(&self.path, parent);
            }
        }
    }

    fn dir(&self, relative: &str) {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create fixture directory");
        #[cfg(unix)]
        make_chain_public(&self.path, &path);
    }

    #[cfg(unix)]
    fn write_mode(&self, relative: &str, content: &[u8], mode: u32) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
            make_chain_public(&self.path, parent);
        }
        fs::write(&path, content).expect("write fixture file");
        make_public(&path, mode);
    }

    #[cfg(unix)]
    fn dir_mode(&self, relative: &str, mode: u32) {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create fixture directory");
        if let Some(parent) = path.parent() {
            make_chain_public(&self.path, parent);
        }
        make_public(&path, mode);
    }

    #[cfg(unix)]
    fn symlink(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(target, self.path.join(link)).expect("create symlink fixture");
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
    use std::os::unix::fs::PermissionsExt;

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
