# Bare Run Help Trigger Design

Date: 2026-06-07
Status: active design; pending user review
Working name: bare-run-help

This design specification details the implementation of a bare-run help trigger and the explicit inclusion of the `help` command in the general help screen. The goal is to make the `buffetcar` CLI more user-friendly and standard by displaying the general help screen instead of a required-argument error when executed with no arguments, and explicitly listing `help` in the commands list.

## Requirements

### 1. Bare Run Behavior
When `buffetcar` is executed with no arguments/flags (e.g. `./buffetcar`), it must:
- Print the general help screen to `stdout`.
- Exit with status code `0`.

If a subcommand is explicitly provided but is missing its required arguments (e.g. `buffetcar serve` without `--root`), it must fail normally with exit code `2` and report the error to `stderr`.

### 2. Help Command in Commands List
The general help screen must explicitly list `help` as one of the commands:
```text
COMMANDS
    serve       Start the Nex server daemon (default)
    check       Run local file and path policy diagnostics
    help        Print this message or the help for the given subcommand
```

## Architectural Changes

### 1. CLI Parser Interception
In `src/cli.rs`, we will modify the `parse` function to return `Command::Help` if the argument vector (excluding the program name) is empty:

```rust
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
    // ... rest of parse logic
}
```

### 2. General Help Template Update
In `src/cli.rs`, we will update the `general_help` function to include the `help` command:

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
```

## Verification

### Unit/Integration Tests
We will add a new test in `tests/check_contract.rs`:
```rust
#[test]
fn bare_run_displays_help_and_exits_zero() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = buffetcar::run_with_io(vec!["buffetcar"], &mut out, &mut err);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("buffetcar"));
    assert!(out_str.contains("A hardened, single-binary Nex server."));
    assert!(out_str.contains("help        Print this message or the help for the given subcommand"));
}
```

We will also update the existing general help integration tests to verify the inclusion of the `help` command in the `COMMANDS` list.
