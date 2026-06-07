# In-CLI Help System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an interactive, color-coded, context-aware in-CLI help system for buffetcar with zero extra dependencies and smart environment-based color controls.

**Architecture:** We will intercept help command line arguments (`help`, `--help`, `-h`) in `cli::parse`. If help is requested, we bypass normal config validation and route directly to printing the help templates. A lightweight `Styler` utility is added to apply ANSI colors dynamically based on terminal capability and the `NO_COLOR` environment variable.

**Tech Stack:** Rust (standard library: `std::io::IsTerminal`, `std::env`).

---

## File Structure

- Modify: [src/cli.rs](file:///Users/jonathan/nex-server/src/cli.rs)
  - Introduce `Subcommand` enum and `HelpArgs` struct.
  - Extend `Command` enum with a `Help` variant.
  - Implement help argument parsing logic.
  - Implement `Styler` and help screen string template functions.
- Modify: [src/lib.rs](file:///Users/jonathan/nex-server/src/lib.rs)
  - Handle `Command::Help` inside `run_with_io` and print templates to the stdout stream.
  - Use `Styler` to format and print errors to the stderr stream.
- Modify: [tests/check_contract.rs](file:///Users/jonathan/nex-server/tests/check_contract.rs)
  - Add integration tests verifying help trigger outputs and error formatting.

---

### Task 1: CLI Data Structures and Triggers

**Files:**
- Modify: [src/cli.rs](file:///Users/jonathan/nex-server/src/cli.rs)

- [ ] **Step 1: Define Subcommand, HelpArgs, and update Command enum**
  Add the data structures at the top of the file, and add the `Help` variant to `Command`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subcommand {
    Serve,
    Check,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HelpArgs {
    pub(crate) subcommand: Option<Subcommand>,
}
```

And update `Command`:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Serve(ServeArgs),
    Check(CheckArgs),
    Help(HelpArgs),
}
```

- [ ] **Step 2: Add help interception to `cli::parse`**
  Modify `parse` function to intercept `help`, `--help`, and `-h` inputs.

```rust
pub(crate) fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    let mut rest: Vec<OsString> = args.collect();

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
        let first_str = rest.first().and_then(|a| a.to_str());
        let sub = match first_str {
            Some("check") => Some(Subcommand::Check),
            Some("serve") => Some(Subcommand::Serve),
            _ => {
                if rest.iter().any(|a| a.to_str() == Some("check")) {
                    Some(Subcommand::Check)
                } else if rest.iter().any(|a| a.to_str() == Some("serve"))
                    || (rest.iter().any(|a| a.to_str().map_or(false, |s| s.starts_with("--")))
                        && rest.iter().all(|a| a.to_str() != Some("check"))
                        && rest.len() > 1)
                {
                    Some(Subcommand::Serve)
                } else {
                    None
                }
            }
        };
        return Ok(Command::Help(HelpArgs { subcommand: sub }));
    }

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
```

- [ ] **Step 3: Add unit tests for help parsing**
  Add unit tests inside the `tests` module in [src/cli.rs](file:///Users/jonathan/nex-server/src/cli.rs) to verify help command and flag triggers.

```rust
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
            Ok(Command::Help(HelpArgs { subcommand: Some(Subcommand::Serve) }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "serve", "--help"])),
            Ok(Command::Help(HelpArgs { subcommand: Some(Subcommand::Serve) }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "help", "check"])),
            Ok(Command::Help(HelpArgs { subcommand: Some(Subcommand::Check) }))
        );
        assert_eq!(
            parse(args(&["buffetcar", "check", "-h"])),
            Ok(Command::Help(HelpArgs { subcommand: Some(Subcommand::Check) }))
        );
    }
```

- [ ] **Step 4: Run unit tests**
  Run: `cargo test cli::tests::parse_help_triggers`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/cli.rs
  git commit -m "feat(cli): add help subcommand and flag triggers to parser"
  ```

---

### Task 2: Implement Styler and Help Templates

**Files:**
- Modify: [src/cli.rs](file:///Users/jonathan/nex-server/src/cli.rs)

- [ ] **Step 1: Implement the Styler struct**
  Add `Styler` near the bottom of [src/cli.rs](file:///Users/jonathan/nex-server/src/cli.rs).

```rust
use std::io::IsTerminal;

pub(crate) struct Styler {
    use_color: bool,
}

impl Styler {
    pub(crate) fn new<W: IsTerminal>(stream: &W) -> Self {
        let is_atty = stream.is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        Self {
            use_color: is_atty && !no_color,
        }
    }

    pub(crate) fn bold(&self, text: &str) -> String {
        if self.use_color { format!("\x1b[1m{text}\x1b[0m") } else { text.to_string() }
    }

    pub(crate) fn green(&self, text: &str) -> String {
        if self.use_color { format!("\x1b[32m{text}\x1b[0m") } else { text.to_string() }
    }

    pub(crate) fn red_bold(&self, text: &str) -> String {
        if self.use_color { format!("\x1b[1;31m{text}\x1b[0m") } else { text.to_string() }
    }

    pub(crate) fn yellow(&self, text: &str) -> String {
        if self.use_color { format!("\x1b[33m{text}\x1b[0m") } else { text.to_string() }
    }
}
```

- [ ] **Step 2: Add template generation functions**
  Add `general_help`, `serve_help`, and `check_help` functions at the bottom of [src/cli.rs](file:///Users/jonathan/nex-server/src/cli.rs).

```rust
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
```

- [ ] **Step 3: Run full tests to verify no syntax/compilation issues**
  Run: `make check`
  Expected: PASS

- [ ] **Step 4: Commit**
  ```bash
  git add src/cli.rs
  git commit -m "feat(cli): implement Styler and help screen text templates"
  ```

---

### Task 3: Integrate Help and Colorized Errors into the Execution Path

**Files:**
- Modify: [src/lib.rs](file:///Users/jonathan/nex-server/src/lib.rs)

- [ ] **Step 1: Intercept `Command::Help` and update error printing in `run_with_io`**
  Modify the `run_with_io` function to output help screens and format errors.

```rust
pub fn run_with_io<I, S, O, E>(args: I, out: &mut O, err: &mut E) -> i32
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    O: Write,
    E: Write,
{
    // Enable std::io::IsTerminal and cli::Styler
    let command = match cli::parse(args) {
        Ok(command) => command,
        Err(error) => {
            let styler = cli::Styler::new(err);
            let _ = writeln!(err, "{} {}", styler.red_bold("error:"), error.message());
            let _ = write!(err, "{}", cli::USAGE);
            let _ = writeln!(err, "\nFor detailed help, run: {}", styler.bold("buffetcar --help"));
            return 2;
        }
    };

    if let cli::Command::Help(help_args) = command {
        let styler = cli::Styler::new(out);
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
                let styler = cli::Styler::new(err);
                let _ = writeln!(err, "{} {error}", styler.red_bold("error:"));
                2
            }
        },
        Ok(config::RunMode::Serve(config)) => match server::run(&config, &mut *err) {
            Ok(()) => 0,
            Err(error) => {
                let styler = cli::Styler::new(err);
                let _ = writeln!(err, "{} {}", styler.red_bold("error:"), error.message());
                1
            }
        },
        Err(error) => {
            let styler = cli::Styler::new(err);
            let _ = writeln!(err, "{} {}", styler.red_bold("error:"), error.message());
            2
        }
    }
}
```

- [ ] **Step 2: Run all tests**
  Run: `make check`
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add src/lib.rs
  git commit -m "feat(cli): wire up help screen rendering and colorized errors to run_with_io"
  ```

---

### Task 4: Add Verification Integration Tests

**Files:**
- Modify: [tests/check_contract.rs](file:///Users/jonathan/nex-server/tests/check_contract.rs)

- [ ] **Step 1: Write integration tests verifying triggers and output**
  Open [tests/check_contract.rs](file:///Users/jonathan/nex-server/tests/check_contract.rs) and add tests for the help screens and error styling.

```rust
#[test]
fn help_screen_triggers_and_content() {
    let mut out = Vec::new();
    let mut err = Vec::new();

    // General help
    let code = buffetcar::run_with_io(vec!["buffetcar", "help"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("A hardened, single-binary Nex server."));
    assert!(out_str.contains("COMMANDS:"));
    assert!(!out_str.contains("0.1.0")); // No hardcoded version number

    // Serve help
    let mut out_serve = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar", "serve", "-h"], &mut out_serve, &mut err);
    assert_eq!(code, 0);
    let out_serve_str = String::from_utf8(out_serve).unwrap();
    assert!(out_serve_str.contains("Start the Nex server daemon."));
    assert!(out_serve_str.contains("--root <PATH>"));
    assert!(out_serve_str.contains("between 1 and 1024"));

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
    assert!(err_str.contains("For detailed help, run: buffetcar --help"));
}
```

- [ ] **Step 2: Run all tests**
  Run: `make check`
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add tests/check_contract.rs
  git commit -m "test(cli): add integration tests for CLI help triggers and formatted error outputs"
  ```
