# Minimal Startup Screen Design

Date: 2026-06-07
Status: active design; pending user review
Working name: minimal-startup-screen

This design specification details the simplification of the `buffetcar` server's startup-success banner. The goal is to make the startup output extremely clean, less verbose, and more user-friendly by printing a single status line and moving static security/containment invariants to documentation.

## Requirements

### Single-Line Output
When `buffetcar` successfully starts and binds to its listening port, it must output exactly one line to `stderr` (or the configured banner writer):

```text
serving <root> on <listen>
```

For example:
```text
serving /home/jonathan/nex-root on 127.0.0.1:1900
```

This replaces the previous multi-line layout:
```text
buffetcar 0.1.0
  root:     /home/jonathan/nex-root
  listen:   0.0.0.0:1900
  workers:  128
  timeouts: read 5s, write 30s
  policy:   no dotfiles, symlinks, hardlinks, special files, or mount crossing
  sandbox:  fd-relative containment (platform sandbox unavailable)
```

### Removal of Unchangeable Information
- **Security Policy:** Details such as "no dotfiles, symlinks, hardlinks, special files, or mount crossing" are invariants enforced by design and cannot be changed via config flags. Therefore, they are removed from the interactive startup log.
- **Sandbox Details:** Details about "fd-relative containment" are removed as they are static architectural features.
- **Configuration Defaults:** Non-default or default workers count and timeout values can be configured via CLI flags, but are omitted from the basic startup screen to minimize visual clutter.

## Architectural Changes

### 1. Update `write_banner`
In `src/config.rs`, the `write_banner` function signature is preserved but its output is changed to only print the single serving message:

```rust
pub(crate) fn write_banner(config: &ServeConfig, mut err: impl Write) -> io::Result<()> {
    writeln!(err, "serving {} on {}", config.root.display(), config.listen)?;
    Ok(())
}
```

### 2. Update Unit Tests
In `src/config.rs`, the unit test `formats_startup_banner` must be updated to match the new simplified output:

```rust
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
    write_banner(&config, &mut stderr).expect("write banner");
    let stderr = String::from_utf8(stderr).expect("banner utf8");

    assert_eq!(stderr, format!("serving {} on 127.0.0.1:1900\n", site.path().display()));
}
```

### 3. Update CLI Integration Tests
We must verify if there are any other integration tests (such as in `tests/check_contract.rs` or elsewhere) checking the banner format, and adjust them accordingly.
