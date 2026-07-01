use crate::cli::{CheckArgs, CliError, Command, ServeArgs, Subcommand};
use std::fs;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 1900));
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
    pub(crate) max_conns_per_ip: u32,
    pub(crate) write_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckConfig {
    pub(crate) root: PathBuf,
    pub(crate) selectors: Vec<String>,
}

pub(crate) fn validate(command: Command) -> Result<RunMode, CliError> {
    validate_with_euid(command, effective_uid())
}

pub(crate) fn validate_with_euid(command: Command, euid: u32) -> Result<RunMode, CliError> {
    let mode = match command {
        Command::Serve(args) => {
            RunMode::Serve(validate_serve(args).map_err(|e| e.with_hint(Subcommand::Serve))?)
        }
        Command::Check(args) => {
            RunMode::Check(validate_check(args).map_err(|e| e.with_hint(Subcommand::Check))?)
        }
        Command::Help(_) | Command::Version => {
            return Err(CliError::new("help/version command is not a runnable mode"))
        }
    };

    if euid == 0 {
        return Err(CliError::new(
            "refusing to run as root; run buffetcar as an unprivileged service user",
        ));
    }

    Ok(mode)
}

/// Write the startup-success banner. `listen` is the address the listener
/// actually bound, which the caller reads from `local_addr()` — so when the
/// operator requests an ephemeral port (`--listen 127.0.0.1:0`) the banner
/// still reports the concrete `host:port` that is now accepting connections.
/// Printed after a successful bind, it doubles as a readiness signal.
pub(crate) fn write_banner(
    root: &Path,
    listen: SocketAddr,
    version_line: &str,
    mut err: impl Write,
) -> io::Result<()> {
    writeln!(err, "{version_line}")?;
    match crate::sandbox::status() {
        Some(status) => writeln!(err, "serving {} on {listen} ({status})", root.display()),
        None => writeln!(err, "serving {} on {listen}", root.display()),
    }
}

fn validate_serve(args: ServeArgs) -> Result<ServeConfig, CliError> {
    let root = validate_root(args.root)?;
    let listen = validate_listen(args.listen)?;
    let workers = validate_workers(args.workers)?;
    let max_conns_per_ip = validate_max_conns_per_ip(args.max_conns_per_ip, workers)?;
    let write_timeout = Duration::from_secs(validate_write_timeout(args.write_timeout)?);

    Ok(ServeConfig {
        root,
        listen,
        workers,
        max_conns_per_ip,
        write_timeout,
    })
}

fn validate_check(args: CheckArgs) -> Result<CheckConfig, CliError> {
    let root = validate_root(args.root)?;
    if args.selectors.is_empty() {
        return Err(CliError::new("check requires at least one selector"));
    }
    Ok(CheckConfig {
        root,
        selectors: args.selectors,
    })
}

fn validate_root(root: Option<PathBuf>) -> Result<PathBuf, CliError> {
    let root = root.ok_or_else(|| CliError::new("--root is required"))?;
    if !root.is_absolute() {
        return Err(CliError::new(format!(
            "--root '{}': not an absolute path",
            root.display()
        )));
    }
    let root = normalize_root(&root);

    let metadata = fs::symlink_metadata(&root)
        .map_err(|err| CliError::new(format!("--root '{}': {err}", root.display())))?;
    if metadata.file_type().is_symlink() {
        return Err(CliError::new(format!(
            "--root '{}': final path component is a symlink",
            root.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(CliError::new(format!(
            "--root '{}': not a directory",
            root.display()
        )));
    }

    Ok(root)
}

fn normalize_root(root: &Path) -> PathBuf {
    root.components().collect()
}

fn validate_listen(listen: Option<String>) -> Result<SocketAddr, CliError> {
    match listen {
        Some(raw) => raw.parse::<SocketAddr>().map_err(|_| {
            CliError::new(format!(
                "invalid --listen '{raw}': expected an IP socket address"
            ))
        }),
        None => Ok(DEFAULT_LISTEN),
    }
}

fn validate_workers(workers: Option<String>) -> Result<usize, CliError> {
    parse_range("--workers", workers, DEFAULT_WORKERS, 1, 1024, "")
}

fn validate_max_conns_per_ip(raw: Option<String>, workers: usize) -> Result<u32, CliError> {
    let default = u32::try_from((workers / 8).max(1)).expect("validated workers fit in u32");
    let max = u32::try_from(workers + 1).expect("validated workers + 1 fit in u32");
    parse_range("--max-conns-per-ip", raw, default, 1, max, "")
}

fn validate_write_timeout(timeout: Option<String>) -> Result<u64, CliError> {
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
) -> Result<T, CliError>
where
    T: Copy + Ord + std::str::FromStr + std::fmt::Display,
{
    let Some(raw) = raw else {
        return Ok(default);
    };

    let parsed = raw.parse::<T>().ok();
    match parsed {
        Some(value) if value >= min && value <= max => Ok(value),
        _ => Err(CliError::new(format!(
            "{flag} '{raw}': expected a value from {min} to {max}{suffix}"
        ))),
    }
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
    use crate::test_support::TempSite;

    #[test]
    fn validates_serve_defaults() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: None,
                max_conns_per_ip: None,
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
                max_conns_per_ip: 16,
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
                max_conns_per_ip: Some("2".to_string()),
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
                max_conns_per_ip: 2,
                write_timeout: Duration::from_secs(300),
            })
        );
    }

    #[test]
    fn derives_max_conns_per_ip_default_from_workers() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: Some("4".to_string()),
                max_conns_per_ip: None,
                write_timeout: None,
            }),
            1000,
        )
        .expect("valid serve config");

        let RunMode::Serve(config) = mode else {
            panic!("expected serve config");
        };
        assert_eq!(config.max_conns_per_ip, 1);
    }

    #[test]
    fn validates_max_conns_per_ip_against_workers_plus_one() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: Some("1024".to_string()),
                max_conns_per_ip: Some("1025".to_string()),
                write_timeout: None,
            }),
            1000,
        )
        .expect("workers + 1 is the neutralizing maximum");

        let RunMode::Serve(config) = mode else {
            panic!("expected serve config");
        };
        assert_eq!(config.max_conns_per_ip, 1025);
    }

    #[test]
    fn rejects_max_conns_per_ip_outside_worker_dependent_range() {
        let site = TempSite::new();
        for value in ["0", "6"] {
            let err = validate_with_euid(
                Command::Serve(ServeArgs {
                    root: Some(site.path().to_path_buf()),
                    listen: None,
                    workers: Some("4".to_string()),
                    max_conns_per_ip: Some(value.to_string()),
                    write_timeout: None,
                }),
                1000,
            )
            .unwrap_err();
            assert_eq!(
                err.message(),
                format!("--max-conns-per-ip '{value}': expected a value from 1 to 5")
            );
        }
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
                max_conns_per_ip: None,
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
                max_conns_per_ip: None,
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

    #[test]
    fn reports_real_reason_for_missing_root() {
        let site = TempSite::new();
        let missing = site.path().join("does-not-exist");
        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(missing.clone()),
                listen: None,
                workers: None,
                max_conns_per_ip: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        // The real stat failure is surfaced (not masked as "not a directory"),
        // so operators can tell "missing" from "not a directory" from "denied".
        assert!(
            err.message()
                .starts_with(&format!("--root '{}': ", missing.display())),
            "message: {}",
            err.message()
        );
        assert!(
            err.message().contains("No such file or directory"),
            "message: {}",
            err.message()
        );
        assert!(
            !err.message().contains("not a directory"),
            "missing root should not be reported as 'not a directory': {}",
            err.message()
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
                max_conns_per_ip: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            format!(
                "--root '{}': final path component is a symlink",
                link.display()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_root_with_trailing_slash() {
        let site = TempSite::new();
        let target = site.path().join("target");
        fs::create_dir(&target).expect("create target dir");
        let link = site.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("create root symlink");
        let root = PathBuf::from(format!("{}/", link.display()));

        let err = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(root),
                listen: None,
                workers: None,
                max_conns_per_ip: None,
                write_timeout: None,
            }),
            1000,
        )
        .unwrap_err();
        assert_eq!(
            err.message(),
            format!(
                "--root '{}': final path component is a symlink",
                link.display()
            )
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
                max_conns_per_ip: None,
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
                max_conns_per_ip: None,
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
                max_conns_per_ip: None,
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
        let mut stderr = Vec::new();
        write_banner(site.path(), DEFAULT_LISTEN, "buffetcar 0.1.0", &mut stderr)
            .expect("write banner");
        let stderr = String::from_utf8(stderr).expect("banner utf8");

        assert!(
            stderr.starts_with("buffetcar 0.1.0\n"),
            "banner should start with version line"
        );

        #[cfg(target_os = "openbsd")]
        assert_eq!(
            stderr,
            format!(
                "buffetcar 0.1.0\nserving {} on 127.0.0.1:1900 (sandbox: pledge/unveil active)\n",
                site.path().display()
            )
        );
        #[cfg(not(target_os = "openbsd"))]
        assert_eq!(
            stderr,
            format!(
                "buffetcar 0.1.0\nserving {} on 127.0.0.1:1900\n",
                site.path().display()
            )
        );
    }
}
