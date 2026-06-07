# Bare Run Help Trigger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a bare `buffetcar` execution print the general help screen and exit with status 0, and list the `help` command in the `COMMANDS` list.

**Architecture:** Intercept empty argument vectors (excluding program name) inside `cli::parse` and return `Command::Help`. Update `cli::general_help` to list the `help` command. Add integration tests.

**Tech Stack:** Rust (standard library and unit/integration test framework).

---

### Task 1: Update CLI General Help Template

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: Modify `general_help` to include the `help` command**

In `src/cli.rs`, update the `general_help` template:

Target code (approx lines 266-290):
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
```

Replacement code:
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

- [ ] **Step 2: Run existing tests to verify compiling**

Run: `make check`
Expected: Compile succeeds, but some integration tests checking help screen output might fail (asserting lack of "help" command text).

- [ ] **Step 3: Commit changes**

Run: `git commit -am "feat(cli): add help command to the general help template"`
Expected: Success.

---

### Task 2: Intercept Empty Arguments in Parser

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: Update `cli::parse` function to intercept empty args**

In `src/cli.rs`, check if `rest` is empty and return `Ok(Command::Help(HelpArgs::default()))` immediately:

Target code (approx lines 58-70):
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
```

Replacement code:
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

    let has_help_flag = rest.iter().any(|a| a == "-h" || a == "--help");
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Success.

- [ ] **Step 3: Commit changes**

Run: `git commit -am "feat(cli): return help command on empty arguments in parser"`
Expected: Success.

---

### Task 3: Update and Add Integration Tests

**Files:**
- Modify: `tests/check_contract.rs`

- [ ] **Step 1: Update the existing general help integration test**

Modify `help_screen_triggers_and_content` in `tests/check_contract.rs` to assert that the help output contains `help        Print this message`.

Target code range in `tests/check_contract.rs` (approx lines 244-249):
```rust
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("A hardened, single-binary Nex server."));
    assert!(out_str.contains("USAGE:"));
    assert!(out_str.contains("COMMANDS"));
    assert!(!out_str.contains("0.1.0")); // No hardcoded version number
```

Replacement code:
```rust
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("A hardened, single-binary Nex server."));
    assert!(out_str.contains("USAGE:"));
    assert!(out_str.contains("COMMANDS"));
    assert!(out_str.contains("help        Print this message or the help for the given subcommand"));
    assert!(!out_str.contains("0.1.0")); // No hardcoded version number
```

- [ ] **Step 2: Add integration test for bare run trigger**

Add `bare_run_displays_help_and_exits_zero` to `tests/check_contract.rs`:

Add at the end of the file:
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

- [ ] **Step 3: Run full local gate**

Run: `make check`
Expected: PASS (all 52 tests green)

- [ ] **Step 4: Commit test changes**

Run: `git commit -am "test(cli): verify help screen contains help command and bare run triggers it"`
Expected: Success.
