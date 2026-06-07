use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn check_returns_zero_when_every_selector_is_servable() {
    let site = TempSite::new();
    site.write("public.txt", b"public\n");
    site.write("listing/page.txt", b"page\n");

    let output = buffetcar(&[
        "check",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "public.txt",
        "listing/",
    ]);

    assert!(output.status.success(), "output: {output:?}");
    assert_eq!(
        stdout(&output),
        "\
ok: public.txt: regular file, public
ok: listing/: directory, public listing
"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn check_returns_one_when_any_selector_is_rejected() {
    let site = TempSite::new();
    site.write("public.txt", b"public\n");
    site.write(".secret", b"secret\n");

    let output = buffetcar(&[
        "check",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "public.txt",
        ".secret",
        "missing.txt",
    ]);

    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    assert_eq!(
        stdout(&output),
        "\
ok: public.txt: regular file, public
reject: .secret: dotfile component
reject: missing.txt: not found
"
    );
    assert_eq!(stderr(&output), "");
}

#[cfg(unix)]
#[test]
fn check_reports_policy_reasons() {
    let site = TempSite::new();
    site.write("private.txt", b"private\n");
    site.chmod("private.txt", 0o600);
    site.write("linked.txt", b"linked\n");
    fs::hard_link(
        site.path().join("linked.txt"),
        site.path().join("alias.txt"),
    )
    .expect("create hardlink");
    site.symlink("linked.txt", "link.txt");
    site.write("hidden/inside.txt", b"inside\n");
    site.chmod("hidden", 0o111);

    let output = buffetcar(&[
        "check",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "private.txt",
        "linked.txt",
        "link.txt",
        "hidden/inside.txt",
        "hidden",
    ]);

    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    assert_eq!(
        stdout(&output),
        "\
reject: private.txt: not world-readable
reject: linked.txt: hardlink count 2
reject: link.txt: symlink
ok: hidden/inside.txt: regular file, public
reject: hidden: directory is not world-readable
"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn usage_errors_return_two_on_stderr() {
    let output = buffetcar(&["check", "index"]);

    assert_eq!(output.status.code(), Some(2), "output: {output:?}");
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("error: --root is required\n"));
}

#[test]
fn invalid_listen_returns_two_before_serve_networking_guard() {
    let site = TempSite::new();
    let output = buffetcar(&[
        "serve",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "--listen",
        "localhost:1900",
    ]);

    assert_eq!(output.status.code(), Some(2), "output: {output:?}");
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "error: invalid --listen 'localhost:1900': expected an IP socket address\n"
    );
}

#[test]
fn serve_reports_bind_conflict_with_actionable_error() {
    let site = TempSite::new();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupy port");
    let addr = occupied.local_addr().expect("addr");

    let output = buffetcar(&[
        "serve",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "--listen",
        &addr.to_string(),
    ]);

    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!("error: could not bind {addr}: address already in use\n")
    );
}

fn buffetcar(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_buffetcar"))
        .args(args)
        .output()
        .expect("run buffetcar")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout utf8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr utf8")
}

struct TempSite {
    path: PathBuf,
}

impl TempSite {
    fn new() -> Self {
        let path = std::env::temp_dir().join(unique_name("buffetcar-check-contract", ""));
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
            #[cfg(unix)]
            make_chain_public(&self.path, parent);
        }
        fs::write(&path, content).expect("write fixture file");
        #[cfg(unix)]
        make_public(&path, 0o644);
    }

    #[cfg(unix)]
    fn chmod(&self, relative: &str, mode: u32) {
        make_public(&self.path.join(relative), mode);
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
