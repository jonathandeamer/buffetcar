use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const NEXD_ADDR: &str = "127.0.0.1:1900";

pub struct TempSite {
    path: PathBuf,
}

impl TempSite {
    pub fn new() -> Self {
        let path = env::temp_dir().join(unique_name("buffetcar-nexd-contract", ""));
        fs::create_dir(&path).expect("create temp site root");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, relative: &str, content: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, content).expect("write fixture file");
        make_world_readable(&path);
    }

    pub fn dir(&self, relative: &str) {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create fixture directory");
        make_world_readable(&path);
    }
}

impl Drop for TempSite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub struct Nexd {
    child: Child,
    _port_guard: MutexGuard<'static, ()>,
}

impl Nexd {
    pub fn start(root: &Path) -> Self {
        let port_guard = nexd_port_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_port_available();

        let bin = nexd_binary();
        let mut child = Command::new(&bin)
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("start {}: {err}", bin.display()));

        wait_until_listening(&mut child);
        Self {
            child,
            _port_guard: port_guard,
        }
    }
}

impl Drop for Nexd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn request(selector: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(NEXD_ADDR).expect("connect to nexd");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");
    stream
        .write_all(selector.as_bytes())
        .expect("write selector");
    stream.write_all(b"\n").expect("write selector newline");
    stream
        .shutdown(Shutdown::Write)
        .expect("shutdown request write side");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read nexd response");
    response
}

fn assert_port_available() {
    if TcpStream::connect(NEXD_ADDR).is_ok() {
        panic!(
            "{NEXD_ADDR} is already accepting connections; stop that server before running nexd contract tests"
        );
    }
}

fn wait_until_listening(child: &mut Child) {
    let deadline = Duration::from_secs(10);
    let started = std::time::Instant::now();

    loop {
        if let Some(status) = child.try_wait().expect("poll nexd process") {
            let mut stderr = String::new();
            if let Some(pipe) = child.stderr.as_mut() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!("nexd exited before listening: {status}; stderr: {stderr}");
        }

        if TcpStream::connect(NEXD_ADDR).is_ok() {
            return;
        }

        if started.elapsed() > deadline {
            panic!("timed out waiting for nexd to listen on {NEXD_ADDR}");
        }

        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn make_world_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = if path.is_dir() { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture permissions");
}

#[cfg(not(unix))]
fn make_world_readable(_path: &Path) {}

fn nexd_port_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn nexd_binary() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let repo = nexd_repo();
        let output_dir = env::current_dir()
            .expect("current directory")
            .join("target")
            .join("nexd-contract");
        fs::create_dir_all(&output_dir).expect("create nexd test binary directory");
        let bin = output_dir.join("nexd");

        let status = Command::new("go")
            .args(["build", "-o"])
            .arg(&bin)
            .arg(".")
            .current_dir(&repo)
            .status()
            .unwrap_or_else(|err| panic!("build nexd from {}: {err}", repo.display()));

        assert!(
            status.success(),
            "go build failed for nexd from {}: {status}",
            repo.display()
        );
        bin
    })
    .clone()
}

fn nexd_repo() -> PathBuf {
    if let Some(repo) = env::var_os("NEXD_REPO") {
        return PathBuf::from(repo);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest directory has parent")
        .join("nexd")
}

pub fn unique_name(prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}-{}-{}{suffix}",
        std::process::id(),
        unique_suffix()
    )
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos()
}
