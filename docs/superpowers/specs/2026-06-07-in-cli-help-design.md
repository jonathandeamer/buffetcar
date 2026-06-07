# In-CLI Help Screen Design

Date: 2026-06-07
Status: active design; pending user review
Working name: in-cli-help

This design specification details the implementation of an interactive, color-coded, context-aware in-CLI help screen for the `buffetcar` Nex server. The goal is to provide clear, helpful, and detailed documentation directly within the terminal, with zero extra dependencies and smart environment-based color controls.

## Requirements

### Help Command & Flag Triggers
The server must intercept help triggers in both general and command-specific contexts:
1. **General Help:**
   - `buffetcar help`
   - `buffetcar --help`
   - `buffetcar -h`
2. **Serve Mode Help:**
   - `buffetcar help serve`
   - `buffetcar serve --help`
   - `buffetcar serve -h`
3. **Check Mode Help:**
   - `buffetcar help check`
   - `buffetcar check --help`
   - `buffetcar check -h`

### Smart ANSI Color Styling
Help text must be colorized to highlight commands, options, and constraints:
- Bold, Green, Red, Yellow ANSI codes will be applied to command segments.
- **Smart Auto-Disabling:** Colors must be stripped automatically under the following conditions:
  - If the output stream is not a terminal (e.g., redirected to a file, piped to another tool like `less` without `-R`). Checked via `std::io::IsTerminal`.
  - If the standard `NO_COLOR` environment variable is present.

### Error UX Enhancements
1. **Unknown Flag Fallback:** When an unknown flag or option is passed (e.g., `buffetcar --verbose`), the error is reported on `stderr`, followed by the 2-line short usage summary, plus a line pointing to the detailed help:
   ```text
   error: unknown argument '--verbose'
   usage: buffetcar [serve] --root <PATH> [--listen <ADDR>] [--workers <N>] [--write-timeout <SECS>]
          buffetcar check --root <PATH> <selector>...

   For detailed help, run: buffetcar --help
   ```
2. **Error Highlight:** The prefix `error:` must be styled in bold red if the target stream is a terminal and colors are enabled.

---

## Architectural Changes

### 1. Command Enum Extension
We will update `Command` and introduce a `HelpArgs` struct to capture context:

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

pub(crate) enum Command {
    Serve(ServeArgs),
    Check(CheckArgs),
    Help(HelpArgs),
}
```

### 2. CLI Parsing Integration
In `src/cli.rs`, we will update `parse` to:
- Intercept `help`, `--help`, and `-h` first.
- Handle trailing `-h`/`--help` after subcommands (`serve -h` or `check --help`).

### 3. The `Styler` Utility
A new struct `Styler` will handle ANSI code wrapping based on terminal checks:

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

---

## Output Layouts

### General Help (`buffetcar help`)
```text
buffetcar
A hardened, single-binary Nex server.

USAGE:
    buffetcar [COMMAND] [OPTIONS]

COMMANDS:
    serve       Start the Nex server daemon (default)
    check       Run local file and path policy diagnostics

For detailed help on a command, run:
    buffetcar help serve
    buffetcar help check
```

### Serve Help (`buffetcar help serve`)
```text
buffetcar serve
Start the Nex server daemon.

USAGE:
    buffetcar [serve] --root <PATH> [--listen <ADDR>] [--workers <N>] [--write-timeout <SECS>]

OPTIONS:
    --root <PATH>           (Required) Absolute path to the directory to serve.
                            Must be world-executable (e.g., 0755). Symlinks are
                            never followed and files inside must be world-readable.
                            Refuses to serve if run as root (UID 0).

    --listen <ADDR>         Socket address to bind to (default: 127.0.0.1:1900).
                            * 127.0.0.1:1900 - Local loopback only (private development).
                            * 0.0.0.0:1900   - All interfaces (accessible over network).
                            * <IP>:1900      - Bind to specific network card or VPN (e.g. Tailscale).

    --workers <N>           Number of worker threads (1..1024, default: 128).
                            Limits the maximum concurrent connections the server handles.

    --write-timeout <SECS>  Socket write timeout in seconds (1..300, default: 30).
                            Stalled connections are dropped after this time.
```

### Check Help (`buffetcar help check`)
```text
buffetcar check
Run local diagnostics on paths against the served root directory without binding sockets.

USAGE:
    buffetcar check --root <PATH> <selector>...

ARGUMENTS:
    <selector>...           One or more relative selector paths to diagnose.
                            For example: 'index', 'about.txt', 'logs/'.

OPTIONS:
    --root <PATH>           (Required) Absolute path to the directory to check.

POLICY RULES VERIFIED:
    * Regular files must be world-readable (0644) and have a link count of 1.
    * Directories must be world-executable (0755) to traverse.
    * Symlinks, hardlinks, FIFOs, and special/device files are rejected.
    * Path components starting with a dot (dotfiles/directories) are rejected.
    * Mount crossing (crossing filesystem boundaries) is rejected.
```

---

## Non-Goals
- Adding external CLI dependency frameworks like `clap`.
- Supporting nested/recursive command help beyond `serve` and `check`.
