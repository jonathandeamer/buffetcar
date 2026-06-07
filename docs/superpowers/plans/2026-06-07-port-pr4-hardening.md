# Port PR #4 Hardening Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the remaining hardening improvements from stale PR #4 to main, covering selector CRLF/double-CR validation, execute-only directory exclusion from listings, and listing pre-allocation bounds checks.

**Architecture:** Modify `src/selector.rs` to validate trailing and embedded CRs. Modify `src/root.rs` to verify child directories are `listable()` (world-readable + world-executable) before listing/diagnosing them, and ensure consistent search-only fd usage. Update `src/listing.rs` to run count and size checks before allocating/appending. Add/update contract tests to assert new policies.

**Tech Stack:** Rust (standard library, `rustix`)

---

### Task 1: Selector Carriage Return Validation

**Files:**
- Modify: `src/selector.rs:46-76`
- Test: `src/selector.rs:120-170`

- [ ] **Step 1: Write the failing tests**
  Add unit tests to check that format of 1024-byte path with trailing CR is accepted (at limit) and double CR `\r\r` is rejected.
  Replace the existing tests or add/modify in `src/selector.rs`:
  ```rust
      #[test]
      fn rejects_nul_and_oversized_selectors() {
          assert_eq!(parse("a\0b"), None);
          let oversized = "a".repeat(1025);
          assert_eq!(parse(&oversized), None);
          let at_limit = "a".repeat(1024);
          assert_eq!(parse(&at_limit), req(&[&at_limit], false));
          // 1024-byte path + trailing CR = 1025 wire bytes: CR is stripped first,
          // leaving 1024 bytes at the limit -> accepted.
          let at_limit_cr = format!("{}\r", "a".repeat(1024));
          assert_eq!(parse(&at_limit_cr), req(&[&"a".repeat(1024)], false));
      }

      #[test]
      fn tolerates_one_trailing_carriage_return() {
          assert_eq!(parse("plain.txt\r"), req(&["plain.txt"], false));
          // double-CR: stripping one leaves an embedded CR -> rejected
          assert_eq!(parse("plain.txt\r\r"), None);
      }
  ```

- [ ] **Step 2: Run test to verify it fails**
  Run: `cargo test selector::tests`
  Expected: FAIL (either panic on assertion of `at_limit_cr` or `plain.txt\r\r`)

- [ ] **Step 3: Implement selector CR validation**
  Modify `parse_diagnostic` in `src/selector.rs` to strip CR first, then check limit, then reject remaining `\r` (mapping to `SelectorReject::Nul` or similar):
  ```rust
  pub(crate) fn parse_diagnostic(selector: &str) -> Result<Request, SelectorReject> {
      // Strip exactly one trailing CR before the length check so that a 1024-byte
      // path sent with Windows CRLF line endings (1025 wire bytes) is accepted.
      let selector = selector.strip_suffix('\r').unwrap_or(selector);
      if selector.len() > MAX_SELECTOR_BYTES {
          return Err(SelectorReject::TooLong);
      }
      // Reject NUL and any remaining CR (e.g. double-CR from a misbehaving client).
      if selector.contains('\0') || selector.contains('\r') {
          return Err(SelectorReject::Nul);
      }
  ```

- [ ] **Step 4: Run test to verify it passes**
  Run: `cargo test selector::tests`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/selector.rs
  git commit -m "fix(selector): validate CR boundaries and reject double-CR"
  ```

---

### Task 2: Listing and Resolution of Execute-Only Directories

**Files:**
- Modify: `src/root.rs`

- [ ] **Step 1: Check existing resolver tests**
  Run: `cargo test root::tests`
  Expected: PASS

- [ ] **Step 2: Modify root resolution and classification logic**
  Implement the listability check on child directories to exclude them if they are not world-readable, and drop/re-open probed directories with a consistent search-only fd.
  Modify `src/root.rs`:
  - In `classify_child`:
    ```rust
        if let Some(child) = self.classify_readable_child(dir, name)? {
            return Ok(Some(child));
        }
        match self.open_child_dir(dir, name)? {
            Some(fd) => {
                let st = fs::fstat(&fd)?;
                if self.listable(&st) {
                    Ok(Some(Child::Dir))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    ```
  - In `classify_readable_child`:
    ```rust
        match FileType::from_raw_mode(st.st_mode) {
            FileType::Directory if self.listable(&st) => Ok(Some(Child::Dir)),
            FileType::RegularFile if self.file_ok(&st) => Ok(Some(Child::File(fd))),
            _ => Ok(None),
        }
    ```
  - In `classify_child_diagnostic`:
    ```rust
        match fs::openat(dir, name, PROBE, Mode::empty()) {
            Ok(fd) => {
                let st = fs::fstat(&fd)?;
                match FileType::from_raw_mode(st.st_mode) {
                    FileType::Directory => {
                        if !self.listable(&st) {
                            return Ok(Err(RejectReason::DirectoryNotWorldReadable));
                        }
                        if let Err(reason) = self.accept_dir(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(Child::Dir))
                    }
                    FileType::RegularFile => {
                        if let Err(reason) = self.accept_file(&st) {
                            return Ok(Err(reason));
                        }
                        Ok(Ok(Child::File(fd)))
                    }
                    _ => Ok(Err(self.reject_for_stat(&st, DiagnosticContext::Leaf))),
                }
            }
            Err(_) => match self.open_child_dir_diagnostic(dir, name)? {
                Ok(fd) => {
                    let st = fs::fstat(&fd)?;
                    if !self.listable(&st) {
                        Ok(Err(RejectReason::DirectoryNotWorldReadable))
                    } else {
                        Ok(Ok(Child::Dir))
                    }
                }
                Err(reason) => Ok(Err(reason)),
            },
        }
    ```
  - In `open_leaf` (around directory resolution branch):
    ```rust
                FileType::Directory if self.dir_ok(&st) => {
                    // Drop the PROBE fd and re-open with TRAVERSE_DIR so that
                    // Resolved::Dir always carries a consistent search-only fd,
                    // matching the fd returned by the dir_only and fallback paths.
                    drop(fd);
                    return self.open_leaf_dir(dir, name);
                }
    ```

- [ ] **Step 3: Run tests to verify**
  Run: `cargo test root::tests`
  Expected: PASS

- [ ] **Step 4: Commit**
  ```bash
  git add src/root.rs
  git commit -m "fix(root): exclude non-listable directories from child classification and use consistent fd for directories"
  ```

---

### Task 3: Resource Allocation Pre-Checks in Listings

**Files:**
- Modify: `src/listing.rs`

- [ ] **Step 1: Check existing listing tests**
  Run: `cargo test listing`
  Expected: PASS (if no specific test, just runs all compiling/matching tests)

- [ ] **Step 2: Implement entry-bound and byte-bound pre-checks**
  Modify `src/listing.rs` to check bounds before vector/string allocations:
  - In `generate`:
    ```rust
        if entries.len() >= MAX_ENTRIES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
    ```
    And:
    ```rust
    let mut out = String::new();
    for (name, is_dir) in entries {
        // Pre-check avoids appending past the byte cap.
        let extra = 4 + name.len() + usize::from(is_dir); // "=> " + name + "\n" + optional "/"
        if out.len() + extra > MAX_BYTES {
            return Ok(crate::NOT_FOUND.to_vec());
        }
        out.push_str("=> ");
        out.push_str(&name);
        if is_dir {
            out.push('/');
        }
        out.push('\n');
    }
    ```
  - In `diagnose`:
    ```rust
        if entries.len() >= MAX_ENTRIES {
            return Ok(Err(RejectReason::ListingTooManyEntries));
        }
        entries.push((name.to_owned(), matches!(child, Child::Dir)));
    ```

- [ ] **Step 3: Run tests to verify**
  Run: `make check`
  Expected: PASS

- [ ] **Step 4: Commit**
  ```bash
  git add src/listing.rs
  git commit -m "fix(listing): perform entry-bound and byte-bound checks as pre-checks"
  ```

---

### Task 4: Contract Integration Test For Execute-Only Directory Listing

**Files:**
- Modify: `tests/buffetcar_contract.rs`

- [ ] **Step 1: Write/update the contract test**
  Modify the `does_not_list_non_world_readable_directory` test in `tests/buffetcar_contract.rs` to verify the parent listing is empty.
  ```rust
  #[cfg(unix)]
  #[test]
  fn does_not_list_non_world_readable_directory() {
      let site = TempSite::new();
      site.write("hidden/inside.txt", b"inside\n");
      site.dir_mode("hidden", 0o111);

      assert_eq!(respond(site.path(), "hidden/inside.txt"), b"inside\n");
      assert_eq!(respond(site.path(), "hidden"), b"document not found");
      // execute-only directory must not appear in the parent listing -
      // leaking its name would violate the no-information-leakage invariant.
      assert_eq!(respond(site.path(), ""), b"");
  }
  ```

- [ ] **Step 2: Run check to verify everything passes**
  Run: `make check`
  Expected: PASS

- [ ] **Step 3: Commit**
  ```bash
  git add tests/buffetcar_contract.rs
  git commit -m "test: assert that non-listable directories are omitted from parent listings"
  ```
