//! Buffetcar Nex server.

mod check;
mod cli;
mod config;
mod conn;
mod limiter;
mod listing;
mod root;
mod sandbox;
mod selector;
mod server;
mod signal;
mod version;

#[cfg(test)]
mod test_support;

use root::{Resolved, Root};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::path::Path;

const NOT_FOUND: &[u8] = b"document not found";

/// Resolve `selector` against `root` and return the response bytes.
///
/// Every unavailable selector returns the same body: missing paths, rejected
/// dotfiles, symlinks, special files, escapes, and policy failures are
/// intentionally indistinguishable to clients.
pub fn serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>> {
    let Some(request) = selector::parse(selector) else {
        return Ok(NOT_FOUND.to_vec());
    };
    let Ok(root) = Root::open(root) else {
        return Ok(NOT_FOUND.to_vec());
    };

    match root.resolve(&request)? {
        Some(Resolved::File(fd)) => read_file(fd),
        Some(Resolved::Dir(fd)) => listing::serve(&root, &fd),
        None => Ok(NOT_FOUND.to_vec()),
    }
}

/// Run the buffetcar CLI with injectable output streams.
///
/// Public so the binary and integration tests share the same dispatch path.
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
            print_cli_error(err, error.message(), error.hint);
            return 2;
        }
    };

    if let cli::Command::Version = command {
        let _ = writeln!(out, "{}", version::version_line());
        return 0;
    }

    if let cli::Command::Help(help_args) = command {
        let styler = cli::Styler::new_stdout();
        match help_args.subcommand {
            None => {
                let _ = write!(
                    out,
                    "{}",
                    cli::general_help(&styler, version::version_line())
                );
            }
            Some(cli::Subcommand::Serve) => {
                let _ = write!(out, "{}", cli::serve_help(&styler));
            }
            Some(cli::Subcommand::Check) => {
                let _ = write!(out, "{}", cli::check_help(&styler));
            }
        }
        return 0;
    }

    match config::validate(command) {
        Ok(config::RunMode::Check(config)) => match check::run(&config, out) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(error) => {
                eprint_error_line(err, &error.to_string());
                2
            }
        },
        Ok(config::RunMode::Serve(config)) => match server::run(&config, &mut *err) {
            Ok(()) => 0,
            Err(error) => {
                eprint_error_line(err, &error.message());
                1
            }
        },
        Err(error) => {
            print_cli_error(err, error.message(), error.hint);
            2
        }
    }
}

/// Print a CLI error to stderr: the `error:` line, the usage block, and a
/// subcommand-scoped "for detailed help" pointer derived from `hint`.
fn print_cli_error(err: &mut impl Write, message: &str, hint: Option<cli::Subcommand>) {
    eprint_error_line(err, message);
    let styler = cli::Styler::new_stderr();
    let _ = write!(err, "{}", cli::USAGE);
    let help_cmd = match hint {
        Some(cli::Subcommand::Serve) => "buffetcar help serve",
        Some(cli::Subcommand::Check) => "buffetcar help check",
        None => "buffetcar --help",
    };
    let _ = writeln!(err, "\nFor detailed help, run: {}", styler.bold(help_cmd));
}

/// Write a styled `error: <message>` line to stderr.
fn eprint_error_line(err: &mut impl Write, message: &str) {
    let styler = cli::Styler::new_stderr();
    let _ = writeln!(err, "{} {}", styler.red_bold("error:"), message);
}

pub(crate) fn read_file(fd: OwnedFd) -> io::Result<Vec<u8>> {
    let mut file = File::from(fd);
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}
