# Port PR #4 Hardening Improvements Design

Date: 2026-06-07
Status: active design; approved
Working name: buffetcar

This design document outlines the port of the four remaining hardening improvements from the stale PR #4 onto the current `main` branch.

## 1. Selector Parsing (`src/selector.rs`)
To handle CRLF safely at the 1024-byte boundary, we will update `parse_diagnostic` to:
* Strip exactly one trailing `\r` carriage return before performing the length check against `MAX_SELECTOR_BYTES` (1024).
* Reject any selector containing `\0` (NUL) or any *remaining* `\r` (carriage return). This detects and rejects double-carriage-returns (e.g. `\r\r` at the end or embedded `\r`), preventing raw carriage returns from reaching the resolver/filesystem calls.
* Port these checks to the unit tests, adding `at_limit_cr` (1024 bytes + `\r` = 1025 wire bytes) and double-CR tests.

## 2. Listing and Resolution of Execute-Only Directories (`src/root.rs`)
To prevent execute-only (`0o111`) subdirectories from leaking their existence in parent listings:
* In `classify_child`, when the fallback `open_child_dir` is triggered (for directories that cannot be opened with `O_RDONLY`), we will verify that the opened child directory fd is `self.listable(&st)` (meaning it is both world-executable and world-readable). If it is not listable, we return `Ok(None)` to exclude it from parent listings.
* In `classify_readable_child`, we will require `self.listable(&st)` rather than just `self.dir_ok(&st)` for directories.
* In `classify_child_diagnostic`, we will similarly align the directory check to verify `self.listable(&st)` and return `Err(RejectReason::DirectoryNotWorldReadable)` when a directory is not listable.
* In `open_leaf`, when resolving a final directory component, we will ensure that directories resolved via `open_leaf_dir` carry a consistent search-only descriptor. In `open_leaf`, if we probed the directory successfully with `O_RDONLY` (`PROBE` fd), we drop the `PROBE` fd and re-open it with `TRAVERSE_DIR` so that all resolved directories (`Resolved::Dir`) carry a consistent search-only descriptor.

## 3. Listing Resource Allocation Bounds (`src/listing.rs`)
To prevent transient over-allocation when building listings:
* In `listing::generate` and `listing::diagnose`, we will pre-check the entry count *before* pushing to the `entries` vector (`if entries.len() >= MAX_ENTRIES`).
* In `listing::generate`, we will pre-calculate the byte footprint of each line before formatting/appending to the output buffer (`out.push_str...`) using the formula `4 + name.len() + usize::from(is_dir)`. If appending would exceed the byte cap, we abort early.

## 4. Testing (`tests/buffetcar_contract.rs`)
* Add a new assertion in `does_not_list_non_world_readable_directory` to verify that listing the parent directory returns an empty body `b""` (meaning the execute-only `0o111` directory is completely omitted from the parent's directory listing).
