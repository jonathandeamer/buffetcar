use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subcommand {
    Serve,
    Check,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HelpArgs {
    pub(crate) subcommand: Option<Subcommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Serve(ServeArgs),
    Check(CheckArgs),
    Help(HelpArgs),
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
    pub(crate) hint: Option<Subcommand>,
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub(crate) fn with_hint(mut self, sub: Subcommand) -> Self {
        self.hint = Some(sub);
        self
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

    if rest.is_empty() {
        return Ok(Command::Help(HelpArgs::default()));
    }

    let has_help_flag = rest.iter().any(|a| a == "-h" || a == "--help");

    if rest.first().and_then(|a| a.to_str()) == Some("help") {
        let sub = match rest.get(1).and_then(|a| a.to_str()) {
            Some("serve") => Some(Subcommand::Serve),
            Some("check") => Some(Subcommand::Check),
            _ => None,
        };
        return Ok(Command::Help(HelpArgs { subcommand: sub }));
    }

    if has_help_flag {
        return Ok(Command::Help(HelpArgs {
            subcommand: infer_help_subcommand(&rest),
        }));
    }

    let mode = match rest.first().and_then(|arg| arg.to_str()) {
        Some("serve") => {
            rest.remove(0);
            Subcommand::Serve
        }
        Some("check") => {
            rest.remove(0);
            Subcommand::Check
        }
        _ => Subcommand::Serve,
    };

    match mode {
        Subcommand::Serve => parse_serve(&rest)
            .map(Command::Serve)
            .map_err(|e| e.with_hint(Subcommand::Serve)),
        Subcommand::Check => parse_check(&rest)
            .map(Command::Check)
            .map_err(|e| e.with_hint(Subcommand::Check)),
    }
}

/// Decide which subcommand's help to show when `-h`/`--help` appears among
/// otherwise-unstructured args. An explicit leading `serve`/`check` wins; failing
/// that, a `check` token anywhere selects check. Otherwise we default to serve
/// help whenever the args already look like serve flags (more than one arg and at
/// least one `--flag`), mirroring the parser's "unknown leading token means serve"
/// default. Anything else falls back to the general help.
fn infer_help_subcommand(rest: &[OsString]) -> Option<Subcommand> {
    match rest.first().and_then(|a| a.to_str()) {
        Some("check") => return Some(Subcommand::Check),
        Some("serve") => return Some(Subcommand::Serve),
        _ => {}
    }
    if rest.iter().any(|a| a.to_str() == Some("check")) {
        return Some(Subcommand::Check);
    }
    let has_serve = rest.iter().any(|a| a.to_str() == Some("serve"));
    let looks_like_serve_flags = rest.len() > 1
        && rest
            .iter()
            .any(|a| a.to_str().is_some_and(|s| s.starts_with("--")));
    if has_serve || looks_like_serve_flags {
        return Some(Subcommand::Serve);
    }
    None
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

use std::io::IsTerminal;

pub(crate) struct Styler {
    use_color: bool,
}

impl Styler {
    fn with_atty(is_atty: bool) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        Self {
            use_color: is_atty && !no_color,
        }
    }

    pub(crate) fn new_stdout() -> Self {
        Self::with_atty(std::io::stdout().is_terminal())
    }

    pub(crate) fn new_stderr() -> Self {
        Self::with_atty(std::io::stderr().is_terminal())
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.use_color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub(crate) fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }

    pub(crate) fn green(&self, text: &str) -> String {
        self.wrap("32", text)
    }

    pub(crate) fn red_bold(&self, text: &str) -> String {
        self.wrap("1;31", text)
    }

    pub(crate) fn yellow(&self, text: &str) -> String {
        self.wrap("33", text)
    }
}

pub(crate) fn general_help(styler: &Styler) -> String {
    format!(
        "\
buffetcar
A hardened, single-binary Nex server.

{usage_hdr}
    buffetcar [{command_opt}] [{options_opt}]

{commands_hdr}
    {serve_cmd}       Start the Nex server daemon (default)
    {check_cmd}       Run local file and path policy diagnostics
    {help_cmd}        Print this message or the help for the given subcommand

For detailed help on a command, run:
    buffetcar help {serve_cmd}
    buffetcar help {check_cmd}
",
        usage_hdr = styler.bold("USAGE:"),
        command_opt = styler.green("COMMAND"),
        options_opt = styler.green("OPTIONS"),
        commands_hdr = styler.bold("COMMANDS"),
        serve_cmd = styler.green("serve"),
        check_cmd = styler.green("check"),
        help_cmd = styler.green("help"),
    )
}

pub(crate) fn serve_help(styler: &Styler) -> String {
    format!(
        "\
buffetcar {serve_cmd}
Start the Nex server daemon.

{usage_hdr}
    buffetcar [{serve_cmd}] {root_flag} {path_val} [{listen_flag} {addr_val}] [{workers_flag} {n_val}] [{write_timeout_flag} {secs_val}]

{options_hdr}
    {root_flag} {path_val}           (Required) Absolute path to the directory to serve.
                            Must be world-executable (e.g., 0755). Symlinks are
                            never followed and files inside must be world-readable.
                            Refuses to serve if run as root (UID 0).

    {listen_flag} {addr_val}         Socket address to bind to (default: 127.0.0.1:1900).
                            * 127.0.0.1:1900 - Local loopback only (private development).
                            * 0.0.0.0:1900   - All interfaces (accessible over network).
                            * <IP>:1900      - Bind to specific network card or VPN (e.g. Tailscale).

    {workers_flag} {n_val}           Number of worker threads (between 1 and 1024, default: 128).
                            Limits the maximum concurrent connections the server handles.

    {write_timeout_flag} {secs_val}  Socket write timeout in seconds (between 1 and 300, default: 30).
                            Stalled connections are dropped after this time.
",
        serve_cmd = styler.green("serve"),
        usage_hdr = styler.bold("USAGE:"),
        root_flag = styler.green("--root"),
        path_val = styler.yellow("<PATH>"),
        listen_flag = styler.green("--listen"),
        addr_val = styler.yellow("<ADDR>"),
        workers_flag = styler.green("--workers"),
        n_val = styler.yellow("<N>"),
        write_timeout_flag = styler.green("--write-timeout"),
        secs_val = styler.yellow("<SECS>"),
        options_hdr = styler.bold("OPTIONS:"),
    )
}

pub(crate) fn check_help(styler: &Styler) -> String {
    format!(
        "\
buffetcar {check_cmd}
Run local diagnostics on paths against the served root directory without binding sockets.

{usage_hdr}
    buffetcar {check_cmd} {root_flag} {path_val} {selector_val}...

{arguments_hdr}
    {selector_val}...           One or more relative selector paths to diagnose.
                            For example: 'index', 'about.txt', 'logs/'.

{options_hdr}
    {root_flag} {path_val}           (Required) Absolute path to the directory to check.

{policy_hdr}
    * Regular files must be world-readable (0644) and have a link count of 1.
    * Directories must be world-executable (0755) to traverse.
    * Symlinks, hardlinks, FIFOs, and special/device files are rejected.
    * Path components starting with a dot (dotfiles/directories) are rejected.
    * Mount crossing (crossing filesystem boundaries) is rejected.
",
        check_cmd = styler.green("check"),
        usage_hdr = styler.bold("USAGE:"),
        root_flag = styler.green("--root"),
        path_val = styler.yellow("<PATH>"),
        selector_val = styler.yellow("<selector>"),
        arguments_hdr = styler.bold("ARGUMENTS:"),
        options_hdr = styler.bold("OPTIONS:"),
        policy_hdr = styler.bold("POLICY RULES VERIFIED:"),
    )
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

    #[test]
    fn parse_help_triggers() {
        assert_eq!(
            parse(args(&["buffetcar", "help"])),
            Ok(Command::Help(HelpArgs { subcommand: None }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "--help"])),
            Ok(Command::Help(HelpArgs { subcommand: None }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "-h"])),
            Ok(Command::Help(HelpArgs { subcommand: None }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "help", "serve"])),
            Ok(Command::Help(HelpArgs {
                subcommand: Some(Subcommand::Serve)
            }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "serve", "--help"])),
            Ok(Command::Help(HelpArgs {
                subcommand: Some(Subcommand::Serve)
            }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "help", "check"])),
            Ok(Command::Help(HelpArgs {
                subcommand: Some(Subcommand::Check)
            }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "check", "-h"])),
            Ok(Command::Help(HelpArgs {
                subcommand: Some(Subcommand::Check)
            }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "--help", "check"])),
            Ok(Command::Help(HelpArgs {
                subcommand: Some(Subcommand::Check)
            }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "--root", "/srv", "-h"])),
            Ok(Command::Help(HelpArgs {
                subcommand: Some(Subcommand::Serve)
            }))
        );
    }
}
