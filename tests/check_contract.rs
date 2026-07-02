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
    // 0o711, not 0o111: owner read lets OpenBSD's O_RDONLY traversal reach the
    // file inside (it has no O_PATH). The `other` bits are unchanged, so `hidden`
    // is still world-executable-but-not-world-readable for policy purposes.
    site.chmod("hidden", 0o711);

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
    assert!(stderr(&output).contains("error: --root is required"));
    assert!(stderr(&output).contains("For detailed help, run: buffetcar help check"));
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
    assert!(stderr(&output)
        .contains("error: invalid --listen 'localhost:1900': expected an IP socket address"));
    assert!(stderr(&output).contains("For detailed help, run: buffetcar help serve"));
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

#[test]
fn help_screen_triggers_and_content() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    // General help
    let code = buffetcar::run_with_io(vec!["buffetcar", "help"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("A server for Nex, the minimal smallnet protocol."));
    assert!(out_str.contains("USAGE:"));
    assert!(out_str.contains("COMMANDS"));
    assert!(out_str.contains("help        Print this message or the help for the given subcommand"));
    assert!(
        out_str.contains("buffetcar "),
        "help header should include version"
    );

    // Serve help
    let mut out_serve = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "serve", "-h"], &mut out_serve, &mut err);
    assert_eq!(code, 0);
    let out_serve_str = String::from_utf8(out_serve).unwrap();
    assert!(out_serve_str.contains("Start the Nex server daemon."));
    assert!(out_serve_str.contains("--root <PATH>"));
    assert!(out_serve_str.contains("between 1 and 1024"));
    assert!(out_serve_str.contains("--max-conns-per-ip <N>"));
    assert!(out_serve_str.contains("between 1 and workers + 1"));
    assert!(out_serve_str.contains("default: max(1, workers / 8)"));

    // Check help
    let mut out_check = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "help", "check"], &mut out_check, &mut err);
    assert_eq!(code, 0);
    let out_check_str = String::from_utf8(out_check).unwrap();
    assert!(out_check_str.contains("Run local diagnostics"));
    assert!(out_check_str.contains("POLICY RULES VERIFIED:"));
}

#[test]
fn error_output_includes_help_hint() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "--invalid-flag"], &mut out, &mut err);
    assert_eq!(code, 2);
    let err_str = String::from_utf8(err).unwrap();
    assert!(err_str.contains("error: unknown argument '--invalid-flag'"));
    assert!(err_str.contains("usage: buffetcar"));
    assert!(err_str.contains("[--max-conns-per-ip <N>]"));
    assert!(err_str.contains("For detailed help, run: buffetcar help serve"));
}

#[test]
fn bare_run_displays_help_and_exits_zero() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("buffetcar"));
    assert!(out_str.contains("A server for Nex, the minimal smallnet protocol."));
    assert!(out_str.contains("help        Print this message or the help for the given subcommand"));
}
#[test]
fn version_flag_prints_version_and_exits_zero() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "--version"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(
        out_str.starts_with("buffetcar "),
        "version output should start with 'buffetcar ': got '{out_str}'"
    );
    assert!(
        out_str.ends_with('\n'),
        "version output should end with newline"
    );
    assert!(err.is_empty(), "version should produce no stderr");
}

#[test]
fn version_short_flag_prints_version() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "-V"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.starts_with("buffetcar "));
}

#[test]
fn version_subcommand_prints_version() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "version"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.starts_with("buffetcar "));
}

#[test]
fn version_all_triggers_produce_same_output() {
    let triggers: Vec<Vec<&str>> = vec![
        vec!["buffetcar", "--version"],
        vec!["buffetcar", "-V"],
        vec!["buffetcar", "version"],
    ];
    let mut outputs = Vec::new();
    for args in &triggers {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = buffetcar::run_with_io(args.clone(), &mut out, &mut err);
        assert_eq!(code, 0);
        outputs.push(String::from_utf8(out).unwrap());
    }
    assert_eq!(outputs[0], outputs[1], "--version and -V should match");
    assert_eq!(
        outputs[0], outputs[2],
        "--version and version subcommand should match"
    );
}

#[test]
fn help_header_includes_version() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "help"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    // The first line of help should be the version line.
    let first_line = out_str.lines().next().expect("help should have output");
    assert!(
        first_line.starts_with("buffetcar "),
        "help header should start with version: got '{first_line}'"
    );
}

#[cfg(unix)]
#[test]
fn serve_graceful_shutdown_on_sigterm() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    let site = TempSite::new();
    site.write("a.txt", b"hello\n");

    // Ask the OS for an ephemeral port (`:0`) and let the server tell us which
    // one it bound, instead of pre-picking a port, dropping the listener, and
    // racing the child to re-bind it (that race intermittently lost to a foreign
    // bind/TIME_WAIT under load). The startup banner is printed only after a
    // successful bind, so reading it doubles as a readiness signal.
    let mut child = Command::new(env!("CARGO_BIN_EXE_buffetcar"))
        .args([
            "serve",
            "--root",
            site.path().to_str().expect("utf8 temp path"),
            "--listen",
            "127.0.0.1:0",
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn buffetcar");

    // Drain the child's stderr on a thread so it never blocks on a full pipe,
    // forwarding each line so we can find the banner's bound address.
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Banner: "serving <root> on <addr>[ (<status>)]". Wait for the line that
    // carries a parseable socket address; the version line above it has none.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut collected_lines = Vec::new();
    let addr: SocketAddr = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                collected_lines.push(line.clone());
                if let Some(token) = line
                    .rsplit(" on ")
                    .next()
                    .and_then(|rest| rest.split_whitespace().next())
                {
                    if let Ok(parsed_addr) = token.parse::<SocketAddr>() {
                        break parsed_addr;
                    }
                }
            }
            Err(e) => {
                let status = child.try_wait().ok().flatten();
                panic!(
                    "Failed to receive startup banner: {e:?}. \n\
                     Child exit status: {status:?}\n\
                     Stderr lines read:\n{}",
                    collected_lines.join("\n")
                );
            }
        }
    };

    // The banner means the listener is bound; a short retry covers the gap
    // before the accept loop is ready. No port race: the server owns the port.
    let mut connected = false;
    for _ in 0..200 {
        if let Ok(mut stream) = TcpStream::connect(addr) {
            if stream.write_all(b"a.txt\n").is_ok() {
                let mut resp = Vec::new();
                if stream.read_to_end(&mut resp).is_ok() && resp == b"hello\n" {
                    connected = true;
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(connected, "server should start and handle requests");

    // Now send SIGTERM to the child process.
    let pid = child.id() as libc::pid_t;
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }

    // Wait for the child to exit with a timeout.
    let mut exited = false;
    let mut status = None;
    for _ in 0..100 {
        if let Ok(Some(s)) = child.try_wait() {
            exited = true;
            status = Some(s);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    if !exited {
        let _ = child.kill();
        panic!("server failed to shut down within 5 seconds of SIGTERM");
    }

    let status = status.unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "server should exit with code 0 on SIGTERM"
    );
}
