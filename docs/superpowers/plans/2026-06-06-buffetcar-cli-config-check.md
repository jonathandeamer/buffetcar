# Buffetcar CLI Config And Check Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a runnable `buffetcar` binary with hand-parsed `serve`/`check` commands, validated startup configuration, and local `check` diagnostics that explain the same filesystem policy enforced by `serve_selector`.

**Architecture:** Keep the existing library wrapper and resolver as the security core. Add a tiny `src/main.rs` that delegates into a public `run_with_io` library function, so the new `cli`, `config`, and `check` modules can stay inside the library crate and reuse private `selector`, `root`, and `listing` internals. `check` gets reason-specific diagnostics for local operators, while the existing `serve_selector` network-facing compatibility path continues to collapse every unavailable selector to `document not found`.

**Tech Stack:** Rust 2021. Standard library for argument parsing, `SocketAddr`, `Duration`, process exit codes, and test subprocesses. Existing `rustix` `fs` APIs remain the only request-path filesystem primitive; add `rustix::fs::statat`/`AtFlags::SYMLINK_NOFOLLOW` only for fd-relative local diagnostics. Existing `libc` is used for effective UID checks.

---

## Scope

**In scope for Plan 2:**

- `src/main.rs` binary entry point.
- `src/cli.rs` hand parser for `serve`, omitted-`serve`, and `check`.
- `src/config.rs` validation for `--root`, `--listen`, `--workers`, `--write-timeout`, and effective UID root refusal.
- `src/check.rs` local diagnostics for selectors.
- Diagnostic reason APIs in `selector`, `root`, and `listing`.
- Integration tests for `buffetcar check` stdout/stderr and exit codes.
- Unit tests for CLI/config edge cases that should not require subprocesses.

**Temporary Plan 2 boundary:**

- `check` is fully runnable.
- `serve` parses and validates config, prints the configured startup banner, then exits with code `2` and `error: serve networking is not implemented in this build`. The server/listener plan removes this guard and binds sockets. This avoids silently pretending that a daemon is running before `server` and `conn` exist.

**Out of scope for Plan 2:**

- `server`, `conn`, `sandbox`, listener binding, worker pool, read timeout, write timeout, streaming file chunks, bind-failure behavior, daemon signal behavior, access logging policy tests, and architecture guard tests.
- README publication docs. Add those with the server plan so the top-level docs describe a daemon that actually exists.

## File Structure

- Create `src/main.rs`: binary entry point; calls `buffetcar::run_with_io` and exits with the returned status code.
- Create `src/cli.rs`: hand-parse `serve`/`check` command-line syntax and return raw, unvalidated argument structs.
- Create `src/config.rs`: convert raw CLI structs into validated `ServeConfig` / `CheckConfig`; format banner and actionable startup errors.
- Create `src/check.rs`: run local diagnostics for selectors using the selector parser, root capability, and listing/index policy.
- Modify `src/lib.rs`: declare new modules; expose `run_with_io`; keep `serve_selector` behavior unchanged.
- Modify `src/selector.rs`: add diagnostic parse reasons; keep `parse(selector) -> Option<Request>` as the network/library collapse API.
- Modify `src/root.rs`: add fd-relative diagnostic resolver methods and public-content reject reasons; keep `resolve` returning `Option<Resolved>`.
- Modify `src/listing.rs`: add directory diagnostic checks for `index`, listing eligibility, and listing bounds; keep `serve` returning bytes or `NOT_FOUND`.
- Create `tests/check_contract.rs`: black-box tests for the `buffetcar` binary's `check` behavior.

## Diagnostic Invariant

The implementation must never report `ok:` from a pathname-only check. Every `ok:` result comes from an opened fd that passed the same descriptor checks used by `serve_selector`.

For failed selectors, diagnostics may use fd-relative no-follow metadata probes (`statat(dirfd, name, SYMLINK_NOFOLLOW)`) to explain a rejection. Those probes must never turn a rejected selector into an accepted one, and they must never use `PathBuf::join` or whole-selector opens.

### Task 1: Hand-Parsed CLI

**Files:**

- Create: `src/cli.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Declare the module**

Add this module declaration near the top of `src/lib.rs`:

```rust
mod cli;
```

- [ ] **Step 2: Write failing parser tests**

Create `src/cli.rs` with only the types needed for tests plus the tests below. The first run should fail because `parse` returns an error for all inputs.

```rust
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Serve(ServeArgs),
    Check(CheckArgs),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServeArgs {
    pub(crate) root: Option<PathBuf>,
    pub(crate) listen: Option<String>,
    pub(crate) workers: Option<String>,
    pub(crate) write_timeout: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CheckArgs {
    pub(crate) root: Option<PathBuf>,
    pub(crate) selectors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliError {
    message: String,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) const USAGE: &str = "\
usage: buffetcar [serve] --root <PATH> [--listen <ADDR>] [--workers <N>] [--write-timeout <SECS>]
       buffetcar check --root <PATH> <selector>...
";

pub(crate) fn parse<I, S>(_args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    Err(CliError::new("parser not wired"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn omitted_mode_is_serve() {
        assert_eq!(
            parse(args(&["buffetcar", "--root", "/srv/nex"])),
            Ok(Command::Serve(ServeArgs {
                root: Some(PathBuf::from("/srv/nex")),
                listen: None,
                workers: None,
                write_timeout: None,
            }))
        );
    }

    #[test]
    fn explicit_serve_accepts_all_serve_flags() {
        assert_eq!(
            parse(args(&[
                "buffetcar",
                "serve",
                "--root",
                "/srv/nex",
                "--listen",
                "127.0.0.1:1900",
                "--workers",
                "64",
                "--write-timeout",
                "10",
            ])),
            Ok(Command::Serve(ServeArgs {
                root: Some(PathBuf::from("/srv/nex")),
                listen: Some("127.0.0.1:1900".to_string()),
                workers: Some("64".to_string()),
                write_timeout: Some("10".to_string()),
            }))
        );
    }

    #[test]
    fn check_collects_selectors_and_root() {
        assert_eq!(
            parse(args(&[
                "buffetcar",
                "check",
                "--root",
                "/srv/nex",
                "index",
                "users/alice/",
            ])),
            Ok(Command::Check(CheckArgs {
                root: Some(PathBuf::from("/srv/nex")),
                selectors: vec!["index".to_string(), "users/alice/".to_string()],
            }))
        );
    }

    #[test]
    fn check_allows_root_after_selectors() {
        assert_eq!(
            parse(args(&[
                "buffetcar",
                "check",
                "index",
                "--root",
                "/srv/nex",
                "users/alice/",
            ])),
            Ok(Command::Check(CheckArgs {
                root: Some(PathBuf::from("/srv/nex")),
                selectors: vec!["index".to_string(), "users/alice/".to_string()],
            }))
        );
    }

    #[test]
    fn missing_flag_value_is_actionable() {
        let err = parse(args(&["buffetcar", "serve", "--root"])).unwrap_err();
        assert_eq!(err.message(), "--root requires a value");
    }

    #[test]
    fn unknown_serve_argument_is_rejected() {
        let err = parse(args(&["buffetcar", "serve", "--verbose"])).unwrap_err();
        assert_eq!(err.message(), "unknown argument '--verbose'");
    }

    #[test]
    fn positional_serve_argument_is_rejected() {
        let err = parse(args(&["buffetcar", "serve", "index"])).unwrap_err();
        assert_eq!(err.message(), "unexpected argument 'index'");
    }

    #[test]
    fn check_rejects_server_only_flags() {
        let err = parse(args(&[
            "buffetcar",
            "check",
            "--root",
            "/srv/nex",
            "--listen",
            "127.0.0.1:1900",
        ]))
        .unwrap_err();
        assert_eq!(err.message(), "unknown argument '--listen' for check");
    }
}
```

- [ ] **Step 3: Run parser tests and verify failure**

Run:

```bash
cargo test cli::tests
```

Expected: tests compile and fail because `parse` returns `Err("parser not wired")`.

- [ ] **Step 4: Implement the parser**

Replace the `parse` stub and add these helper functions in `src/cli.rs` above the test module:

```rust
pub(crate) fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    let mut rest: Vec<OsString> = args.collect();

    let mode = match rest.first().and_then(|arg| arg.to_str()) {
        Some("serve") => {
            rest.remove(0);
            ModeName::Serve
        }
        Some("check") => {
            rest.remove(0);
            ModeName::Check
        }
        _ => ModeName::Serve,
    };

    match mode {
        ModeName::Serve => parse_serve(&rest).map(Command::Serve),
        ModeName::Check => parse_check(&rest).map(Command::Check),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModeName {
    Serve,
    Check,
}

fn parse_serve(args: &[OsString]) -> Result<ServeArgs, CliError> {
    let mut parsed = ServeArgs::default();
    let mut i = 0;
    while i < args.len() {
        let Some(arg) = args[i].to_str() else {
            return Err(CliError::new(format!(
                "invalid non-UTF-8 argument '{}'",
                display_arg(&args[i])
            )));
        };
        match arg {
            "--root" => {
                i += 1;
                parsed.root = Some(PathBuf::from(take_value(args, i, "--root")?));
            }
            "--listen" => {
                i += 1;
                parsed.listen = Some(take_utf8_value(args, i, "--listen")?);
            }
            "--workers" => {
                i += 1;
                parsed.workers = Some(take_utf8_value(args, i, "--workers")?);
            }
            "--write-timeout" => {
                i += 1;
                parsed.write_timeout = Some(take_utf8_value(args, i, "--write-timeout")?);
            }
            other if other.starts_with("--") => {
                return Err(CliError::new(format!("unknown argument '{other}'")));
            }
            other => return Err(CliError::new(format!("unexpected argument '{other}'"))),
        }
        i += 1;
    }
    Ok(parsed)
}

fn parse_check(args: &[OsString]) -> Result<CheckArgs, CliError> {
    let mut parsed = CheckArgs::default();
    let mut i = 0;
    while i < args.len() {
        let Some(arg) = args[i].to_str() else {
            return Err(CliError::new(format!(
                "invalid non-UTF-8 argument '{}'",
                display_arg(&args[i])
            )));
        };
        match arg {
            "--root" => {
                i += 1;
                parsed.root = Some(PathBuf::from(take_value(args, i, "--root")?));
            }
            other if other.starts_with("--") => {
                return Err(CliError::new(format!(
                    "unknown argument '{other}' for check"
                )));
            }
            selector => parsed.selectors.push(selector.to_string()),
        }
        i += 1;
    }
    Ok(parsed)
}

fn take_value(args: &[OsString], index: usize, flag: &str) -> Result<OsString, CliError> {
    args.get(index)
        .cloned()
        .ok_or_else(|| CliError::new(format!("{flag} requires a value")))
}

fn take_utf8_value(args: &[OsString], index: usize, flag: &str) -> Result<String, CliError> {
    let value = take_value(args, index, flag)?;
    value
        .into_string()
        .map_err(|value| CliError::new(format!("{flag} value '{}' is not valid UTF-8", display_arg(&value))))
}

fn display_arg(arg: &OsString) -> String {
    arg.to_string_lossy().into_owned()
}
```

- [ ] **Step 5: Run parser tests and formatting**

Run:

```bash
cargo fmt --all --check
cargo test cli::tests
```

Expected: all `cli::tests` pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add src/lib.rs src/cli.rs
git commit -m "feat: add hand-parsed cli modes"
```

### Task 2: Config Validation And Root Refusal

**Files:**

- Create: `src/config.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Declare the module**

Add this module declaration near the top of `src/lib.rs`:

```rust
mod config;
```

- [ ] **Step 2: Write failing config tests**

Create `src/config.rs` with the type stubs and tests below. The first run should fail because `validate_with_euid` returns a stub error.

```rust
use crate::cli::{CheckArgs, Command, ServeArgs};
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const DEFAULT_LISTEN: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 1900));
pub(crate) const DEFAULT_WORKERS: usize = 128;
pub(crate) const DEFAULT_WRITE_TIMEOUT_SECS: u64 = 30;
pub(crate) const READ_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunMode {
    Serve(ServeConfig),
    Check(CheckConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServeConfig {
    pub(crate) root: PathBuf,
    pub(crate) listen: SocketAddr,
    pub(crate) workers: usize,
    pub(crate) write_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckConfig {
    pub(crate) root: PathBuf,
    pub(crate) selectors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) fn validate(command: Command) -> Result<RunMode, ConfigError> {
    validate_with_euid(command, effective_uid())
}

pub(crate) fn validate_with_euid(_command: Command, _euid: u32) -> Result<RunMode, ConfigError> {
    Err(ConfigError::new("config validator not wired"))
}

pub(crate) fn write_banner(_config: &ServeConfig, _err: impl Write) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe { libc::geteuid() as u32 }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn validates_serve_defaults() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: None,
                write_timeout: None,
            }),
            1000,
        )
        .expect("valid serve config");

        assert_eq!(
            mode,
            RunMode::Serve(ServeConfig {
                root: site.path().to_path_buf(),
                listen: DEFAULT_LISTEN,
                workers: DEFAULT_WORKERS,
                write_timeout: Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS),
            })
        );
    }

    #[test]
    fn validates_serve_overrides() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: Some("127.0.0.1:1901".to_string()),
                workers: Some("1".to_string()),
                write_timeout: Some("300".to_string()),
            }),
            1000,
        )
        .expect("valid serve config");

        assert_eq!(
            mode,
            RunMode::Serve(ServeConfig {
                root: site.path().to_path_buf(),
                listen: "127.0.0.1:1901".parse().unwrap(),
                workers: 1,
                write_timeout: Duration::from_secs(300),
            })
        );
    }

    #[test]
    fn validates_check_config() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Check(CheckArgs {
                root: Some(site.path().to_path_buf()),
                selectors: vec!["index".to_string()],
            }),
            1000,
        )
        .expect("valid check config");

        assert_eq!(
            mode,
            RunMode::Check(CheckConfig {
                root: site.path().to_path_buf(),
                selectors: vec!["index".to_string()],
            })
        );
    }

    #[test]
    fn rejects_missing_root() {
        let err = validate_with_euid(
            Command::Check(CheckArgs {
                root: None,
                selectors: vec!["index".to_string()],
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(err.message(), "--root is required");
    }

    #[test]
    fn rejects_relative_root() {
        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(PathBuf::from("site")),
                listen: None,
                workers: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(err.message(), "--root 'site': not an absolute path");
    }

    #[test]
    fn rejects_non_directory_root() {
        let site = TempSite::new();
        let file = site.path().join("file.txt");
        fs::write(&file, b"file").expect("write file root");
        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(file.clone()),
                listen: None,
                workers: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            format!("--root '{}': not a directory", file.display())
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_root() {
        let site = TempSite::new();
        let target = site.path().join("target");
        fs::create_dir(&target).expect("create target dir");
        let link = site.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("create root symlink");

        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(link.clone()),
                listen: None,
                workers: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            format!("--root '{}': final path component is a symlink", link.display())
        );
    }

    #[test]
    fn rejects_root_execution_after_flag_validation() {
        let site = TempSite::new();
        let err = validate_with_euid(
            Command::Check(CheckArgs {
                root: Some(site.path().to_path_buf()),
                selectors: vec!["index".to_string()],
            }),
            0,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            "refusing to run as root; run buffetcar as an unprivileged service user"
        );
    }

    #[test]
    fn rejects_invalid_listen_address() {
        let site = TempSite::new();
        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: Some("localhost:1900".to_string()),
                workers: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            "invalid --listen 'localhost:1900': expected an IP socket address"
        );
    }

    #[test]
    fn rejects_invalid_workers() {
        let site = TempSite::new();
        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: Some("0".to_string()),
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            "--workers '0': expected a value from 1 to 1024"
        );
    }

    #[test]
    fn rejects_invalid_write_timeout() {
        let site = TempSite::new();
        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: None,
                write_timeout: Some("999".to_string()),
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            "--write-timeout '999': expected a value from 1 to 300 seconds"
        );
    }

    #[test]
    fn rejects_check_without_selectors() {
        let site = TempSite::new();
        let err = validate_with_euid(
            Command::Check(CheckArgs {
                root: Some(site.path().to_path_buf()),
                selectors: Vec::new(),
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(err.message(), "check requires at least one selector");
    }

    #[test]
    fn formats_startup_banner() {
        let site = TempSite::new();
        let config = ServeConfig {
            root: site.path().to_path_buf(),
            listen: DEFAULT_LISTEN,
            workers: DEFAULT_WORKERS,
            write_timeout: Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS),
        };
        let mut stderr = Vec::new();
        write_banner(&config, &mut stderr).expect("write banner");
        let stderr = String::from_utf8(stderr).expect("banner utf8");

        assert!(stderr.contains("buffetcar 0.1.0\n"));
        assert!(stderr.contains(&format!("  root:     {}\n", site.path().display())));
        assert!(stderr.contains("  listen:   127.0.0.1:1900\n"));
        assert!(stderr.contains("  workers:  128\n"));
        assert!(stderr.contains("  timeouts: read 5s, write 30s\n"));
        assert!(stderr.contains("  policy:   no dotfiles, symlinks, hardlinks, special files, or mount crossing\n"));
        assert!(stderr.contains("  sandbox:  fd-relative containment (platform sandbox unavailable)\n"));
    }

    struct TempSite {
        path: PathBuf,
    }

    impl TempSite {
        fn new() -> Self {
            let path = std::env::temp_dir().join(unique_name("buffetcar-config", ""));
            fs::create_dir(&path).expect("create temp site root");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
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
}
```

- [ ] **Step 3: Run config tests and verify failure**

Run:

```bash
cargo test config::tests
```

Expected: tests fail because `validate_with_euid` returns `config validator not wired`, and the banner test fails because `write_banner` writes nothing.

- [ ] **Step 4: Implement config validation**

Replace the `validate_with_euid` and `write_banner` stubs, then add the helper functions below before the test module:

```rust
pub(crate) fn validate_with_euid(command: Command, euid: u32) -> Result<RunMode, ConfigError> {
    let mode = match command {
        Command::Serve(args) => RunMode::Serve(validate_serve(args)?),
        Command::Check(args) => RunMode::Check(validate_check(args)?),
    };

    if euid == 0 {
        return Err(ConfigError::new(
            "refusing to run as root; run buffetcar as an unprivileged service user",
        ));
    }

    Ok(mode)
}

pub(crate) fn write_banner(config: &ServeConfig, mut err: impl Write) -> io::Result<()> {
    writeln!(err, "buffetcar {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(err, "  root:     {}", config.root.display())?;
    writeln!(err, "  listen:   {}", config.listen)?;
    writeln!(err, "  workers:  {}", config.workers)?;
    writeln!(
        err,
        "  timeouts: read {}s, write {}s",
        READ_TIMEOUT_SECS,
        config.write_timeout.as_secs()
    )?;
    writeln!(
        err,
        "  policy:   no dotfiles, symlinks, hardlinks, special files, or mount crossing"
    )?;
    writeln!(
        err,
        "  sandbox:  fd-relative containment (platform sandbox unavailable)"
    )?;
    Ok(())
}

fn validate_serve(args: ServeArgs) -> Result<ServeConfig, ConfigError> {
    Ok(ServeConfig {
        root: validate_root(args.root)?,
        listen: validate_listen(args.listen)?,
        workers: validate_workers(args.workers)?,
        write_timeout: Duration::from_secs(validate_write_timeout(args.write_timeout)?),
    })
}

fn validate_check(args: CheckArgs) -> Result<CheckConfig, ConfigError> {
    let root = validate_root(args.root)?;
    if args.selectors.is_empty() {
        return Err(ConfigError::new("check requires at least one selector"));
    }
    Ok(CheckConfig {
        root,
        selectors: args.selectors,
    })
}

fn validate_root(root: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
    let root = root.ok_or_else(|| ConfigError::new("--root is required"))?;
    if !root.is_absolute() {
        return Err(ConfigError::new(format!(
            "--root '{}': not an absolute path",
            root.display()
        )));
    }

    let metadata = fs::symlink_metadata(&root).map_err(|_| {
        ConfigError::new(format!("--root '{}': not a directory", root.display()))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ConfigError::new(format!(
            "--root '{}': final path component is a symlink",
            root.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(ConfigError::new(format!(
            "--root '{}': not a directory",
            root.display()
        )));
    }

    Ok(root)
}

fn validate_listen(listen: Option<String>) -> Result<SocketAddr, ConfigError> {
    match listen {
        Some(raw) => raw.parse::<SocketAddr>().map_err(|_| {
            ConfigError::new(format!(
                "invalid --listen '{raw}': expected an IP socket address"
            ))
        }),
        None => Ok(DEFAULT_LISTEN),
    }
}

fn validate_workers(workers: Option<String>) -> Result<usize, ConfigError> {
    parse_range("--workers", workers, DEFAULT_WORKERS, 1, 1024, "")
}

fn validate_write_timeout(timeout: Option<String>) -> Result<u64, ConfigError> {
    parse_range(
        "--write-timeout",
        timeout,
        DEFAULT_WRITE_TIMEOUT_SECS,
        1,
        300,
        " seconds",
    )
}

fn parse_range<T>(
    flag: &str,
    raw: Option<String>,
    default: T,
    min: T,
    max: T,
    suffix: &str,
) -> Result<T, ConfigError>
where
    T: Copy + Ord + std::str::FromStr + std::fmt::Display,
{
    let Some(raw) = raw else {
        return Ok(default);
    };

    let parsed = raw.parse::<T>().ok();
    match parsed {
        Some(value) if value >= min && value <= max => Ok(value),
        _ => Err(ConfigError::new(format!(
            "{flag} '{raw}': expected a value from {min} to {max}{suffix}"
        ))),
    }
}
```

- [ ] **Step 5: Run config tests and formatting**

Run:

```bash
cargo fmt --all --check
cargo test config::tests
```

Expected: all `config::tests` pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add src/lib.rs src/config.rs
git commit -m "feat: validate cli configuration"
```

### Task 3: Selector Diagnostic Reasons

**Files:**

- Modify: `src/selector.rs`

- [ ] **Step 1: Write failing diagnostic parser tests**

Add these tests inside `src/selector.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn diagnostic_parse_preserves_reject_reasons() {
        assert_eq!(parse_diagnostic(".secret"), Err(SelectorReject::DotfileComponent));
        assert_eq!(
            parse_diagnostic("listing/.hidden"),
            Err(SelectorReject::DotfileComponent)
        );
        assert_eq!(parse_diagnostic("a\0b"), Err(SelectorReject::Nul));
        assert_eq!(parse_diagnostic("../outside"), Err(SelectorReject::EscapesRoot));

        let oversized = "a".repeat(1025);
        assert_eq!(parse_diagnostic(&oversized), Err(SelectorReject::TooLong));
    }

    #[test]
    fn selector_reject_reason_messages_are_stable() {
        assert_eq!(SelectorReject::TooLong.message(), "selector exceeds 1024 bytes");
        assert_eq!(SelectorReject::Nul.message(), "selector contains NUL");
        assert_eq!(SelectorReject::DotfileComponent.message(), "dotfile component");
        assert_eq!(SelectorReject::EscapesRoot.message(), "selector escapes root");
    }
```

- [ ] **Step 2: Run selector tests and verify failure**

Run:

```bash
cargo test selector::tests
```

Expected: compile fails because `parse_diagnostic` and `SelectorReject` are not defined.

- [ ] **Step 3: Add diagnostic parser API**

In `src/selector.rs`, add this enum after `Request`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectorReject {
    TooLong,
    Nul,
    DotfileComponent,
    EscapesRoot,
}

impl SelectorReject {
    pub(crate) fn message(self) -> &'static str {
        match self {
            SelectorReject::TooLong => "selector exceeds 1024 bytes",
            SelectorReject::Nul => "selector contains NUL",
            SelectorReject::DotfileComponent => "dotfile component",
            SelectorReject::EscapesRoot => "selector escapes root",
        }
    }
}
```

Replace `parse` with these two functions:

```rust
pub(crate) fn parse(selector: &str) -> Option<Request> {
    parse_diagnostic(selector).ok()
}

pub(crate) fn parse_diagnostic(selector: &str) -> Result<Request, SelectorReject> {
    if selector.len() > MAX_SELECTOR_BYTES {
        return Err(SelectorReject::TooLong);
    }
    let selector = selector.strip_suffix('\r').unwrap_or(selector);
    if selector.contains('\0') {
        return Err(SelectorReject::Nul);
    }

    let dir_only = selector.ends_with('/');
    let mut components = Vec::new();
    for raw in selector.split('/') {
        match raw {
            "" | "." => continue,
            ".." => {
                components.pop().ok_or(SelectorReject::EscapesRoot)?;
            }
            name => {
                if name.starts_with('.') {
                    return Err(SelectorReject::DotfileComponent);
                }
                components.push(name.to_owned());
            }
        }
    }

    Ok(Request {
        components,
        dir_only,
    })
}
```

- [ ] **Step 4: Run selector tests and formatting**

Run:

```bash
cargo fmt --all --check
cargo test selector::tests
```

Expected: all selector tests pass, including existing `parse(...) == None` tests.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/selector.rs
git commit -m "feat: preserve selector reject reasons"
```

### Task 4: Root And Listing Diagnostic API

**Files:**

- Modify: `src/root.rs`
- Modify: `src/listing.rs`

- [ ] **Step 1: Write failing root/listing diagnostic tests**

Add these tests at the bottom of `src/root.rs`. They live in `root` because they need private diagnostic methods and do not require a binary subprocess.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::{self, DirectoryResponse};
    use crate::selector::parse_diagnostic;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn diagnostic_resolve_reports_file_and_directory_success() {
        let site = TempSite::new();
        site.write("public.txt", b"public\n");
        site.write("listing/page.txt", b"page\n");

        let root = Root::open(site.path()).expect("open root");
        let file = parse_diagnostic("public.txt").expect("parse file");
        assert!(matches!(
            root.resolve_diagnostic(&file).expect("diagnose file"),
            Ok(DiagnosticTarget::File(_))
        ));

        let dir = parse_diagnostic("listing/").expect("parse dir");
        let target = root
            .resolve_diagnostic(&dir)
            .expect("diagnose dir")
            .expect("dir target");
        let DiagnosticTarget::Dir(fd) = target else {
            panic!("expected directory target");
        };
        assert_eq!(
            listing::diagnose(&root, fd).expect("diagnose listing"),
            Ok(DirectoryResponse::Listing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_resolve_reports_stable_reject_reasons() {
        let site = TempSite::new();
        site.write("private.txt", b"private\n");
        site.chmod("private.txt", 0o600);
        site.write("linked.txt", b"linked\n");
        fs::hard_link(site.path().join("linked.txt"), site.path().join("alias.txt"))
            .expect("create hardlink");
        site.write("locked/inside.txt", b"inside\n");
        site.chmod("locked", 0o600);
        site.symlink("linked.txt", "link.txt");

        let root = Root::open(site.path()).expect("open root");

        assert!(matches!(
            diagnose_selector(&root, "private.txt"),
            Err(RejectReason::NotWorldReadable)
        ));
        assert!(matches!(
            diagnose_selector(&root, "linked.txt"),
            Err(RejectReason::Hardlink(2))
        ));
        assert!(matches!(
            diagnose_selector(&root, "locked/inside.txt"),
            Err(RejectReason::DirectoryNotWorldExecutable)
        ));
        assert!(matches!(
            diagnose_selector(&root, "link.txt"),
            Err(RejectReason::Symlink)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_directory_listing_rejects_non_world_readable_directory() {
        let site = TempSite::new();
        site.write("hidden/inside.txt", b"inside\n");
        site.chmod("hidden", 0o111);

        let root = Root::open(site.path()).expect("open root");
        assert!(matches!(
            diagnose_selector(&root, "hidden/inside.txt"),
            Ok(DiagnosticTarget::File(_))
        ));

        let target = diagnose_selector(&root, "hidden").expect("hidden dir target");
        let DiagnosticTarget::Dir(fd) = target else {
            panic!("expected hidden directory target");
        };
        assert_eq!(
            listing::diagnose(&root, fd).expect("diagnose hidden dir"),
            Err(RejectReason::DirectoryNotWorldReadable)
        );
    }

    fn diagnose_selector(
        root: &Root,
        selector: &str,
    ) -> Result<DiagnosticTarget, RejectReason> {
        let request = parse_diagnostic(selector).expect("parse selector");
        root.resolve_diagnostic(&request).expect("diagnose selector")
    }

    struct TempSite {
        path: PathBuf,
    }

    impl TempSite {
        fn new() -> Self {
            let path = std::env::temp_dir().join(unique_name("buffetcar-root", ""));
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
            std::os::unix::fs::symlink(target, self.path.join(link))
                .expect("create symlink fixture");
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
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test root::tests
```

Expected: compile fails because `DiagnosticTarget`, `RejectReason`, `resolve_diagnostic`, `listing::diagnose`, and `DirectoryResponse` are not defined.

- [ ] **Step 3: Add root diagnostic types and messages**

In `src/root.rs`, change the `use rustix::fs` line to include `AtFlags`:

```rust
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, Stat};
```

Add these types after `Child`:

```rust
/// A diagnostic resolution target that has still been accepted by descriptor policy.
pub(crate) enum DiagnosticTarget {
    File(OwnedFd),
    Dir(OwnedFd),
}

/// Local-only reject reasons for `buffetcar check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RejectReason {
    Missing,
    Symlink,
    SpecialFile,
    CrossDevice,
    Hardlink(u64),
    NotWorldReadable,
    DirectoryNotWorldExecutable,
    DirectoryNotWorldReadable,
    NotADirectory,
    TrailingSlashOnFile,
    ListingTooManyEntries,
    ListingTooManyBytes,
}

impl RejectReason {
    pub(crate) fn message(&self) -> String {
        match self {
            RejectReason::Missing => "not found".to_string(),
            RejectReason::Symlink => "symlink".to_string(),
            RejectReason::SpecialFile => "special file".to_string(),
            RejectReason::CrossDevice => "crosses filesystem boundary".to_string(),
            RejectReason::Hardlink(count) => format!("hardlink count {count}"),
            RejectReason::NotWorldReadable => "not world-readable".to_string(),
            RejectReason::DirectoryNotWorldExecutable => {
                "directory is not world-executable".to_string()
            }
            RejectReason::DirectoryNotWorldReadable => {
                "directory is not world-readable".to_string()
            }
            RejectReason::NotADirectory => "not a directory".to_string(),
            RejectReason::TrailingSlashOnFile => "trailing slash on regular file".to_string(),
            RejectReason::ListingTooManyEntries => {
                "directory listing exceeds 4096 entries".to_string()
            }
            RejectReason::ListingTooManyBytes => {
                "directory listing exceeds 262144 bytes".to_string()
            }
        }
    }
}
```

- [ ] **Step 4: Keep `Root::open` as capability setup, not request policy**

Replace `Root::open` with this version. The root is still opened `O_NOFOLLOW`, but public-directory policy remains enforced by `open_root_dir` and the new diagnostic resolver on each request.

```rust
    pub(crate) fn open(path: &Path) -> io::Result<Root> {
        let fd = fs::open(path, TRAVERSE_DIR, Mode::empty())?;
        let st = fs::fstat(&fd)?;
        Ok(Root {
            fd,
            dev: st.st_dev as u64,
        })
    }
```

- [ ] **Step 5: Add diagnostic resolver methods**

Add these methods inside `impl Root`, keeping the existing `resolve`, `open_index`, `classify_child`, and serving helpers unchanged:

```rust
    pub(crate) fn resolve_diagnostic(
        &self,
        request: &Request,
    ) -> io::Result<Result<DiagnosticTarget, RejectReason>> {
        let mut cur = match self.open_root_dir_diagnostic()? {
            Ok(fd) => fd,
            Err(reason) => return Ok(Err(reason)),
        };

        let total = request.components.len();
        for (i, name) in request.components.iter().enumerate() {
            if i + 1 == total {
                return self.open_leaf_diagnostic(&cur, name, request.dir_only);
            }

            cur = match self.open_child_dir_diagnostic(&cur, name.as_str())? {
                Ok(fd) => fd,
                Err(reason) => return Ok(Err(reason)),
            };
        }

        Ok(Ok(DiagnosticTarget::Dir(cur)))
    }

    pub(crate) fn classify_child_diagnostic<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Result<Child, RejectReason>> {
        match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => {
                let st = fs::fstat(&fd)?;
                match FileType::from_raw_mode(st.st_mode) {
                    FileType::Directory => {
                        if let Err(reason) = self.accept_dir(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(Child::Dir))
                    }
                    FileType::RegularFile => {
                        if let Err(reason) = self.accept_file(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(Child::File(fd)))
                    }
                    _ => Ok(Err(self.reject_for_stat(&st, DiagnosticContext::Leaf))),
                }
            }
            Err(_) => match self.open_child_dir_diagnostic(dir, name)? {
                Ok(_) => Ok(Ok(Child::Dir)),
                Err(reason) => Ok(Err(reason)),
            },
        }
    }

    pub(crate) fn open_listable_dir_diagnostic(
        &self,
        dir: &OwnedFd,
    ) -> io::Result<Result<OwnedFd, RejectReason>> {
        let st = fs::fstat(dir)?;
        if let Err(reason) = self.accept_dir(&st) {
            return Ok(Err(reason));
        }
        if !Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH) {
            return Ok(Err(RejectReason::DirectoryNotWorldReadable));
        }

        let fd = match fs::openat(dir, ".", LIST_DIR, Mode::empty()) {
            Ok(fd) => fd,
            Err(_) => return Ok(Err(RejectReason::DirectoryNotWorldReadable)),
        };
        let st = fs::fstat(&fd)?;
        if let Err(reason) = self.accept_dir(&st) {
            return Ok(Err(reason));
        }
        if Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH) {
            Ok(Ok(fd))
        } else {
            Ok(Err(RejectReason::DirectoryNotWorldReadable))
        }
    }

    fn open_root_dir_diagnostic(&self) -> io::Result<Result<OwnedFd, RejectReason>> {
        let fd = match open_traverse_dir(&self.fd, ".") {
            Ok(fd) => fd,
            Err(_) => return Ok(Err(RejectReason::Missing)),
        };
        let st = fs::fstat(&fd)?;
        match self.accept_dir(&st) {
            Ok(()) => Ok(Ok(fd)),
            Err(reason) => Ok(Err(reason)),
        }
    }

    fn open_leaf_diagnostic(
        &self,
        dir: &OwnedFd,
        name: &str,
        dir_only: bool,
    ) -> io::Result<Result<DiagnosticTarget, RejectReason>> {
        if dir_only {
            return match self.open_child_dir_diagnostic(dir, name)? {
                Ok(fd) => Ok(Ok(DiagnosticTarget::Dir(fd))),
                Err(_) => Ok(Err(self.diagnose_child(dir, name, DiagnosticContext::DirOnly)?)),
            };
        }

        match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => {
                let st = fs::fstat(&fd)?;
                match FileType::from_raw_mode(st.st_mode) {
                    FileType::Directory => {
                        if let Err(reason) = self.accept_dir(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(DiagnosticTarget::Dir(fd)))
                    }
                    FileType::RegularFile => {
                        if let Err(reason) = self.accept_file(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(DiagnosticTarget::File(fd)))
                    }
                    _ => Ok(Err(self.reject_for_stat(&st, DiagnosticContext::Leaf))),
                }
            }
            Err(_) => match self.open_child_dir_diagnostic(dir, name)? {
                Ok(fd) => Ok(Ok(DiagnosticTarget::Dir(fd))),
                Err(_) => Ok(Err(self.diagnose_child(dir, name, DiagnosticContext::Leaf)?)),
            },
        }
    }

    fn open_child_dir_diagnostic<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
    ) -> io::Result<Result<OwnedFd, RejectReason>> {
        let fd = match open_traverse_dir(dir, name) {
            Ok(fd) => fd,
            Err(_) => return Ok(Err(self.diagnose_child(dir, name, DiagnosticContext::Intermediate)?)),
        };
        let st = fs::fstat(&fd)?;
        match self.accept_dir(&st) {
            Ok(()) => Ok(Ok(fd)),
            Err(reason) => Ok(Err(reason)),
        }
    }

    fn diagnose_child<P: Arg + Copy>(
        &self,
        dir: &OwnedFd,
        name: P,
        context: DiagnosticContext,
    ) -> io::Result<RejectReason> {
        match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => Ok(self.reject_for_stat(&st, context)),
            Err(_) => Ok(RejectReason::Missing),
        }
    }

    fn accept_dir(&self, st: &Stat) -> Result<(), RejectReason> {
        if st.st_dev as u64 != self.dev {
            return Err(RejectReason::CrossDevice);
        }
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
            return Err(RejectReason::SpecialFile);
        }
        if !Mode::from_raw_mode(st.st_mode).contains(Mode::XOTH) {
            return Err(RejectReason::DirectoryNotWorldExecutable);
        }
        Ok(())
    }

    fn accept_file(&self, st: &Stat) -> Result<(), RejectReason> {
        if st.st_dev as u64 != self.dev {
            return Err(RejectReason::CrossDevice);
        }
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(RejectReason::SpecialFile);
        }
        if !Mode::from_raw_mode(st.st_mode).contains(Mode::ROTH) {
            return Err(RejectReason::NotWorldReadable);
        }
        if st.st_nlink as u64 != 1 {
            return Err(RejectReason::Hardlink(st.st_nlink as u64));
        }
        Ok(())
    }

    fn reject_for_stat(&self, st: &Stat, context: DiagnosticContext) -> RejectReason {
        if st.st_dev as u64 != self.dev {
            return RejectReason::CrossDevice;
        }

        match FileType::from_raw_mode(st.st_mode) {
            FileType::Symlink => RejectReason::Symlink,
            FileType::Directory => self
                .accept_dir(st)
                .err()
                .unwrap_or(RejectReason::DirectoryNotWorldReadable),
            FileType::RegularFile if context == DiagnosticContext::Intermediate => {
                RejectReason::NotADirectory
            }
            FileType::RegularFile if context == DiagnosticContext::DirOnly => {
                self.accept_file(st).err().unwrap_or(RejectReason::TrailingSlashOnFile)
            }
            FileType::RegularFile => self
                .accept_file(st)
                .err()
                .unwrap_or(RejectReason::NotWorldReadable),
            _ => RejectReason::SpecialFile,
        }
    }
```

Add this private enum after `impl RejectReason`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticContext {
    Intermediate,
    Leaf,
    DirOnly,
}
```

- [ ] **Step 6: Add listing diagnostics**

In `src/listing.rs`, change the imports:

```rust
use crate::root::{Child, RejectReason, Root};
```

Add this enum after the bounds constants:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryResponse {
    Index,
    Listing,
}
```

Add this function below `serve`:

```rust
pub(crate) fn diagnose(
    root: &Root,
    dir: OwnedFd,
) -> io::Result<Result<DirectoryResponse, RejectReason>> {
    if matches!(
        root.classify_child_diagnostic(&dir, "index")?,
        Ok(Child::File(_))
    ) {
        return Ok(Ok(DirectoryResponse::Index));
    }

    let list_dir = match root.open_listable_dir_diagnostic(&dir)? {
        Ok(fd) => fd,
        Err(reason) => return Ok(Err(reason)),
    };

    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in Dir::read_from(&list_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes.first() == Some(&b'.') {
            continue;
        }
        let Ok(child) = root.classify_child_diagnostic(&list_dir, name)? else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
        if entries.len() > MAX_ENTRIES {
            return Ok(Err(RejectReason::ListingTooManyEntries));
        }
    }

    entries.sort();

    let mut rendered_bytes = 0usize;
    for (name, is_dir) in entries {
        rendered_bytes += 3 + name.len() + usize::from(is_dir) + 1;
        if rendered_bytes > MAX_BYTES {
            return Ok(Err(RejectReason::ListingTooManyBytes));
        }
    }

    Ok(Ok(DirectoryResponse::Listing))
}
```

- [ ] **Step 7: Run diagnostics tests and existing contract tests**

Run:

```bash
cargo fmt --all --check
cargo test root::tests
cargo test buffetcar_contract
```

Expected: root diagnostic tests pass and the existing visitor-facing contract tests still pass.

- [ ] **Step 8: Commit**

Run:

```bash
git add src/root.rs src/listing.rs
git commit -m "feat: add fd-relative check diagnostics"
```

### Task 5: Check Runner And Binary Dispatch

**Files:**

- Create: `src/check.rs`
- Create: `src/main.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Declare the check module and public runner**

Add this module declaration near the top of `src/lib.rs`:

```rust
mod check;
```

Add these imports in `src/lib.rs`:

```rust
use std::ffi::OsString;
use std::io::Write;
```

Add this public runner below `serve_selector`:

```rust
/// Run the buffetcar CLI with injectable output streams.
///
/// This is public so the binary and integration tests can exercise the same
/// dispatch path without exposing the resolver internals.
pub fn run_with_io<I, S, O, E>(args: I, out: &mut O, err: &mut E) -> i32
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    O: Write,
    E: Write,
{
    let command = match cli::parse(args) {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(err, "error: {}", error.message());
            let _ = write!(err, "{}", cli::USAGE);
            return 2;
        }
    };

    match config::validate(command) {
        Ok(config::RunMode::Check(config)) => match check::run(&config, out) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(error) => {
                let _ = writeln!(err, "error: {error}");
                2
            }
        },
        Ok(config::RunMode::Serve(config)) => {
            let _ = config::write_banner(&config, err);
            let _ = writeln!(err, "error: serve networking is not implemented in this build");
            2
        }
        Err(error) => {
            let _ = writeln!(err, "error: {}", error.message());
            2
        }
    }
}
```

- [ ] **Step 2: Write failing check runner tests**

Create `src/check.rs` with the stubs and tests below:

```rust
use crate::config::CheckConfig;
use std::io::{self, Write};

pub(crate) fn run(_config: &CheckConfig, _out: impl Write) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn check_outputs_ok_and_reject_lines() {
        let site = TempSite::new();
        site.write("public.txt", b"public\n");
        site.write("listing/page.txt", b"page\n");
        site.write(".secret", b"secret\n");

        let config = CheckConfig {
            root: site.path().to_path_buf(),
            selectors: vec![
                "public.txt".to_string(),
                "listing/".to_string(),
                ".secret".to_string(),
                "missing.txt".to_string(),
            ],
        };
        let mut out = Vec::new();
        let all_ok = run(&config, &mut out).expect("run check");

        assert!(!all_ok);
        assert_eq!(
            String::from_utf8(out).expect("stdout utf8"),
            "\
ok: public.txt: regular file, public
ok: listing/: directory, public listing
reject: .secret: dotfile component
reject: missing.txt: not found
"
        );
    }

    #[cfg(unix)]
    #[test]
    fn check_outputs_policy_reasons() {
        let site = TempSite::new();
        site.write("private.txt", b"private\n");
        site.chmod("private.txt", 0o600);
        site.write("linked.txt", b"linked\n");
        fs::hard_link(site.path().join("linked.txt"), site.path().join("alias.txt"))
            .expect("create hardlink");
        site.symlink("linked.txt", "link.txt");
        site.write("hidden/inside.txt", b"inside\n");
        site.chmod("hidden", 0o111);

        let config = CheckConfig {
            root: site.path().to_path_buf(),
            selectors: vec![
                "private.txt".to_string(),
                "linked.txt".to_string(),
                "link.txt".to_string(),
                "hidden".to_string(),
            ],
        };
        let mut out = Vec::new();
        let all_ok = run(&config, &mut out).expect("run check");

        assert!(!all_ok);
        assert_eq!(
            String::from_utf8(out).expect("stdout utf8"),
            "\
reject: private.txt: not world-readable
reject: linked.txt: hardlink count 2
reject: link.txt: symlink
reject: hidden: directory is not world-readable
"
        );
    }

    #[test]
    fn check_returns_true_when_every_selector_is_servable() {
        let site = TempSite::new();
        site.write("public.txt", b"public\n");

        let config = CheckConfig {
            root: site.path().to_path_buf(),
            selectors: vec!["public.txt".to_string()],
        };
        let mut out = Vec::new();
        let all_ok = run(&config, &mut out).expect("run check");

        assert!(all_ok);
        assert_eq!(
            String::from_utf8(out).expect("stdout utf8"),
            "ok: public.txt: regular file, public\n"
        );
    }

    struct TempSite {
        path: PathBuf,
    }

    impl TempSite {
        fn new() -> Self {
            let path = std::env::temp_dir().join(unique_name("buffetcar-check", ""));
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
            std::os::unix::fs::symlink(target, self.path.join(link))
                .expect("create symlink fixture");
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
}
```

- [ ] **Step 3: Run check runner tests and verify failure**

Run:

```bash
cargo test check::tests
```

Expected: tests fail because the stub emits no result lines and returns `false`.

- [ ] **Step 4: Implement check runner**

Replace the stub in `src/check.rs` with this implementation:

```rust
use crate::config::CheckConfig;
use crate::listing::{self, DirectoryResponse};
use crate::root::{DiagnosticTarget, RejectReason, Root};
use crate::selector;
use std::io::{self, Write};

pub(crate) fn run(config: &CheckConfig, mut out: impl Write) -> io::Result<bool> {
    let root = Root::open(&config.root)?;
    let mut all_ok = true;

    for selector in &config.selectors {
        match selector::parse_diagnostic(selector) {
            Ok(request) => match root.resolve_diagnostic(&request)? {
                Ok(DiagnosticTarget::File(_)) => {
                    writeln!(
                        out,
                        "ok: {}: regular file, public",
                        display_selector(selector)
                    )?;
                }
                Ok(DiagnosticTarget::Dir(fd)) => match listing::diagnose(&root, fd)? {
                    Ok(DirectoryResponse::Index) => {
                        writeln!(
                            out,
                            "ok: {}: directory, public index",
                            display_selector(selector)
                        )?;
                    }
                    Ok(DirectoryResponse::Listing) => {
                        writeln!(
                            out,
                            "ok: {}: directory, public listing",
                            display_selector(selector)
                        )?;
                    }
                    Err(reason) => {
                        all_ok = false;
                        write_reject(&mut out, selector, &reason)?;
                    }
                },
                Err(reason) => {
                    all_ok = false;
                    write_reject(&mut out, selector, &reason)?;
                }
            },
            Err(reason) => {
                all_ok = false;
                writeln!(
                    out,
                    "reject: {}: {}",
                    display_selector(selector),
                    reason.message()
                )?;
            }
        }
    }

    Ok(all_ok)
}

fn write_reject(
    mut out: impl Write,
    selector: &str,
    reason: &RejectReason,
) -> io::Result<()> {
    writeln!(
        out,
        "reject: {}: {}",
        display_selector(selector),
        reason.message()
    )
}

fn display_selector(selector: &str) -> String {
    selector
        .chars()
        .flat_map(|ch| match ch {
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\0' => "\\0".chars().collect::<Vec<_>>(),
            ch => vec![ch],
        })
        .collect()
}
```

- [ ] **Step 5: Add binary entry point**

Create `src/main.rs`:

```rust
fn main() {
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = buffetcar::run_with_io(std::env::args_os(), &mut stdout, &mut stderr);
    std::process::exit(code);
}
```

- [ ] **Step 6: Run runner tests and formatting**

Run:

```bash
cargo fmt --all --check
cargo test check::tests
```

Expected: all `check::tests` pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/lib.rs src/check.rs src/main.rs
git commit -m "feat: add check command runner"
```

### Task 6: Binary Contract Tests

**Files:**

- Create: `tests/check_contract.rs`

- [ ] **Step 1: Write failing binary tests**

Create `tests/check_contract.rs`:

```rust
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
    fs::hard_link(site.path().join("linked.txt"), site.path().join("alias.txt"))
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
fn serve_validates_config_and_stops_before_networking() {
    let site = TempSite::new();
    let output = buffetcar(&[
        "serve",
        "--root",
        site.path().to_str().expect("utf8 temp path"),
        "--workers",
        "1",
        "--write-timeout",
        "1",
    ]);

    assert_eq!(output.status.code(), Some(2), "output: {output:?}");
    assert_eq!(stdout(&output), "");
    let err = stderr(&output);
    assert!(err.contains("buffetcar 0.1.0\n"));
    assert!(err.contains("  listen:   127.0.0.1:1900\n"));
    assert!(err.contains("  workers:  1\n"));
    assert!(err.contains("  timeouts: read 5s, write 1s\n"));
    assert!(err.ends_with("error: serve networking is not implemented in this build\n"));
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
        std::os::unix::fs::symlink(target, self.path.join(link))
            .expect("create symlink fixture");
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
```

- [ ] **Step 2: Run binary tests and verify failure**

Run:

```bash
cargo test --test check_contract
```

Expected: failures if any previous task is incomplete. If all earlier tasks are complete, these tests pass.

- [ ] **Step 3: Run binary tests after fixes**

Run:

```bash
cargo fmt --all --check
cargo test --test check_contract
```

Expected: all binary check contract tests pass.

- [ ] **Step 4: Commit**

Run:

```bash
git add tests/check_contract.rs
git commit -m "test: cover check command contract"
```

### Task 7: Final Integration Gate

**Files:**

- Modify only files changed by Tasks 1-6 if verification exposes issues.

- [ ] **Step 1: Run the full local gate**

Run:

```bash
make check
```

Expected:

```text
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

All commands pass.

- [ ] **Step 2: Run dependency policy if the tool is installed**

Run:

```bash
make deny
```

Expected if `cargo-deny` is installed: pass.

Expected in this workspace if `cargo-deny` is not installed:

```text
error: no such command: `deny`
```

If `cargo-deny` is missing, record that in the final verification notes instead of treating it as an implementation failure.

- [ ] **Step 3: Inspect the diff for temporary serve boundary clarity**

Run:

```bash
git diff --stat HEAD~4..HEAD
rg -n "serve networking is not implemented in this build|run_with_io|resolve_diagnostic|parse_diagnostic" src tests
```

Expected: the temporary serve guard appears only in `src/lib.rs` and `tests/check_contract.rs`; `check` paths use `parse_diagnostic` and `resolve_diagnostic`; `serve_selector` still uses `selector::parse` and `Root::resolve`.

- [ ] **Step 4: Confirm visitor-facing behavior remains collapsed**

Run:

```bash
cargo test --test buffetcar_contract rejects_dotfiles_by_default_and_omits_them_from_listings
cargo test --test buffetcar_contract rejects_non_world_readable_file
cargo test --test buffetcar_contract rejects_hardlinked_file
```

Expected: each test passes and unavailable selectors still produce exactly `document not found`.

- [ ] **Step 5: Commit any verification fixes**

If Step 1 or Step 4 required code changes, commit them:

```bash
git add src tests
git commit -m "fix: align check diagnostics with serve policy"
```

If no fixes were needed, do not create an empty commit.

## Self-Review

**Spec coverage:**

- Architecture `cli` and `config`: Tasks 1-2 add hand parsing, config validation, defaults, and usage/startup errors.
- Binary modes: Task 5 adds `src/main.rs`, omitted `serve`, explicit `serve`, and `check`; Task 6 tests those paths. Plan 2 intentionally stops valid `serve` before sockets with a clear error because `server`/`conn` are out of scope.
- Filesystem resolver reuse for diagnostics: Tasks 3-5 add diagnostic reasons without changing `serve_selector`'s visitor-facing collapse.
- Public content policy diagnostics: Task 4 reports symlink, hardlink count, private file, private directory, non-listable directory, special file, cross-device, trailing slash mismatch, and oversized listing reasons through fd-relative no-follow probes.
- Logging/operator UX: Task 2 formats the startup banner and config errors; Task 5 sends usage/startup errors to stderr; Task 6 asserts stdout/stderr separation for `check`.
- Exit status: Task 5 implements `0` for all servable, `1` for selector rejection, `2` for usage/startup errors and the temporary serve guard.
- Networking/resource bounds: config values are validated in Task 2, but listener binding, read timeout, write timeout enforcement, fixed worker pool, and bind-failure errors are deferred to the server/connection plan.
- README: deferred to the server/connection plan because this repository currently has no README and Plan 2 does not yet provide a real daemon.

**Placeholder scan:** The plan has concrete file paths, function names, test bodies, commands, expected outcomes, and commit messages. It avoids open-ended implementation instructions and does not rely on undefined task-local types.

**Type consistency:**

- `cli::Command::{Serve, Check}` feeds `config::validate`.
- `config::RunMode::{Serve, Check}` feeds `run_with_io`.
- `selector::parse_diagnostic` returns `Result<Request, SelectorReject>`; `selector::parse` remains `Option<Request>`.
- `root::resolve_diagnostic` returns `Result<DiagnosticTarget, RejectReason>` inside `io::Result`.
- `listing::diagnose` returns `Result<DirectoryResponse, RejectReason>` inside `io::Result`.
- `check::run` returns `io::Result<bool>` where `true` means every selector was servable.
