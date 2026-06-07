//! Buffetcar Nex server.

mod check;
mod cli;
mod config;
mod conn;
mod listing;
mod root;
mod sandbox;
mod selector;
mod server;

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
            let styler = cli::Styler::new_stderr();
            let _ = writeln!(err, "{} {}", styler.red_bold("error:"), error.message());
            let _ = write!(err, "{}", cli::USAGE);
            let _ = writeln!(
                err,
                "\nFor detailed help, run: {}",
                styler.bold("buffetcar --help")
            );
            return 2;
        }
    };

    if let cli::Command::Help(help_args) = command {
        let styler = cli::Styler::new_stdout();
        match help_args.subcommand {
            None => {
                let _ = write!(out, "{}", cli::general_help(&styler));
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
                let styler = cli::Styler::new_stderr();
                let _ = writeln!(err, "{} {error}", styler.red_bold("error:"));
                2
            }
        },
        Ok(config::RunMode::Serve(config)) => match server::run(&config, &mut *err) {
            Ok(()) => 0,
            Err(error) => {
                let styler = cli::Styler::new_stderr();
                let _ = writeln!(err, "{} {}", styler.red_bold("error:"), error.message());
                1
            }
        },
        Err(error) => {
            let styler = cli::Styler::new_stderr();
            let _ = writeln!(err, "{} {}", styler.red_bold("error:"), error.message());
            2
        }
    }
}

pub(crate) fn read_file(fd: OwnedFd) -> io::Result<Vec<u8>> {
    let mut file = File::from(fd);
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}
