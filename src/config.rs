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

pub(crate) fn write_banner(
    config: &ServeConfig,
    version_line: &str,
    mut err: impl Write,
) -> io::Result<()> {
    writeln!(err, "{version_line}")?;
    match crate::sandbox::status() {
        Some(status) => writeln!(
            err,
            "serving {} on {} ({status})",
            config.root.display(),
            config.listen
        ),
        None => writeln!(
            err,
            "serving {} on {}",
            config.root.display(),
            config.listen
        ),
    }
}

fn validate_serve(args: ServeArgs) -> Result<ServeConfig, CliError> {
    Ok(ServeConfig {
        root: validate_root(args.root)?,
        listen: validate_listen(args.listen)?,
        workers: validate_workers(args.workers)?,
        write_timeout: Duration::from_secs(validate_write_timeout(args.write_timeout)?),
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
        .map_err(|_| CliError::new(format!("--root '{}': not a directory", root.display())))?;
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
        write_banner(&config, "buffetcar 0.1.0", &mut stderr).expect("write banner");
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
