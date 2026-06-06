use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        fs::write(path, content).expect("write fixture file");
    }

    fn dir(&self, relative: &str) {
        fs::create_dir_all(self.path.join(relative)).expect("create fixture directory");
    }
}

impl Drop for TempSite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn unique_name(prefix: &str, suffix: &str) -> String {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    format!("{prefix}-{}-{unique}{suffix}", std::process::id())
}
