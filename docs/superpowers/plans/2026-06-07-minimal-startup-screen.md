# Minimal Startup Screen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Simplify the startup-success banner printed by `buffetcar` to a single clean line.

**Architecture:** Update `config::write_banner` to output only `serving <root> on <listen>` and update the corresponding unit tests in `src/config.rs`.

**Tech Stack:** Rust (standard library and unit test framework).

---

### Task 1: Update write_banner Implementation and Unit Test

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Modify the banner test to reflect the new expected format**

Modify the `formats_startup_banner` test in `src/config.rs` to assert the single-line format instead of the old multi-line format.

Target code range in `src/config.rs` (approx lines 488-500):
```rust
        assert!(stderr.contains("buffetcar 0.1.0\n"));
        assert!(stderr.contains(&format!("  root:     {}\n", site.path().display())));
        assert!(stderr.contains("  listen:   127.0.0.1:1900\n"));
        assert!(stderr.contains("  workers:  128\n"));
        assert!(stderr.contains("  timeouts: read 5s, write 30s\n"));
        assert!(stderr.contains(
            "  policy:   no dotfiles, symlinks, hardlinks, special files, or mount crossing\n"
        ));
        assert!(
            stderr.contains("  sandbox:  fd-relative containment (platform sandbox unavailable)\n")
        );
```

Replacement code:
```rust
        assert_eq!(
            stderr,
            format!("serving {} on 127.0.0.1:1900\n", site.path().display())
        );
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test config::tests::formats_startup_banner`
Expected: FAIL, asserting that the old multi-line banner did not match the single-line expected string.

- [ ] **Step 3: Update `write_banner` implementation**

Target code in `src/config.rs` (approx lines 71-91):
```rust
pub(crate) fn write_banner(config: &ServeConfig, mut err: impl Write) -> io::Result<()> {
    writeln!(err, "buffetcar {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(err, "  root:     {}", config.root.display())?;
    writeln!(err, "  listen:   {}", config.listen)?;
    writeln!(err, "  workers:  {}", config.workers)?;
    writeln!(
        err,
        "  timeouts: read {}s, write {}s",
        READ_TIMEOUT_SECS,
        config.write_timeout.as_secs()
    )?;
    writeln!(
        err,
        "  policy:   no dotfiles, symlinks, hardlinks, special files, or mount crossing"
    )?;
    writeln!(
        err,
        "  sandbox:  fd-relative containment (platform sandbox unavailable)"
    )?;
    Ok(())
}
```

Replacement code:
```rust
pub(crate) fn write_banner(config: &ServeConfig, mut err: impl Write) -> io::Result<()> {
    writeln!(err, "serving {} on {}", config.root.display(), config.listen)?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test config::tests::formats_startup_banner`
Expected: PASS

- [ ] **Step 5: Run full local gate to ensure no regressions**

Run: `make check`
Expected: PASS (all tests green)

- [ ] **Step 6: Commit changes**

Run: `git commit -am "feat(config): simplify startup banner to a single serving line"`
Expected: Success, conventional commit message hook accepts the commit.
