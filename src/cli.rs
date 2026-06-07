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
    value.into_string().map_err(|value| {
        CliError::new(format!(
            "{flag} value '{}' is not valid UTF-8",
            display_arg(&value)
        ))
    })
}

fn display_arg(arg: &OsString) -> String {
    arg.to_string_lossy().into_owned()
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
