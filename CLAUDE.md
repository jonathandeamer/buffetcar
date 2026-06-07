# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`buffetcar` (crate name) is a hardened, single-binary **Nex** server in Rust. Nex is a minimal protocol: the client sends one selector line over TCP (port 1900 by default), the server replies with the raw text/binary bytes of a file — or a generated directory listing using `=> ` links — then closes. No status codes, no connection state, no URL decoding.

The project is **design-led**: the authoritative specs and plans live under `docs/superpowers/`. The full server is now implemented across focused modules in `src/` (`cli`, `config`, `server`, `conn`, `selector`, `root`, `listing`, `sandbox`, plus `lib`, `main`, `check`). Read the authoritative design before making non-trivial changes (see below).

## Commands

```sh
make check          # full local gate: fmt --check, clippy -D warnings, test
make fmt            # cargo fmt --all --check
make clippy         # cargo clippy --all-targets -- -D warnings
make test           # cargo test (default suite only)
make deny           # cargo deny check (needs: cargo install cargo-deny)
make hooks          # install the Conventional Commits commit-msg hook (run once per clone)
make nexd-contract  # optional reference suite — NOT part of `make check` (see below)
```

Run a single test: `cargo test <name_substring>` (default suite), or `cargo test --features nexd-contract --test nexd_contract <name_substring>` for the reference suite.

Commits are enforced as **Conventional Commits** by `.githooks/commit-msg` once `make hooks` is run (`<type>(<scope>)!: <desc>`; scope optional). Bypass once with `git commit --no-verify`.

## Git workflow

`main` is protected with **3 required status checks (CI)**. Do **not** push directly to `main`, even when you hold bypass permission — bypassing skips CI and can land a red `main`. Land work via a pull request: push a feature branch and `gh pr create`, let the checks run, then merge. Keep local commits green with `make check` before pushing, but treat CI as the gate that actually guards `main`.

## Design authority

The plan of record is **`docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`**. It supersedes the earlier `2026-06-05-buffetcar-design.md`; where the two disagree, the 2026-06-06 design wins.

That design is now **implemented**. The earlier static-site choices it reversed are gone: `cap-std` has been removed in favour of an explicit fd-relative resolver (`rustix` `openat` + `O_NOFOLLOW` + `fstat`; `openat2` is Linux-only optional hardening), the `serve`/`check` CLI is hand-parsed instead of using `clap`, and resolution is split across `selector` + `root` (+ `listing`). The daemon (`server::run`) opens the root **once at startup** and shares it across a fixed worker-thread pool. `serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>>` survives as a thin test/library wrapper that opens a `Root` per call.

Outstanding spec items (not yet built; none blocking): a multi-user race **stress test** (swap a name between file/symlink/FIFO/dir under load and assert outside/special content is never served — the protection exists structurally, the asserting test does not), a **README** stating the publishing rules in `check`'s terms, a deterministic **write-timeout test** (the timeout is applied, just not asserted), and the **OpenBSD `pledge`/`unveil`** sandbox (out of scope until the resolver supports OpenBSD; `sandbox::apply()` is a deliberate no-op everywhere today).

## Core principle (drives most decisions)

Security and minimalism are co-equal; **when they genuinely conflict, prefer security.** Safety properties are *invariants, not configurable defaults* — there is intentionally no flag/config to serve dotfiles, follow symlinks, change the index name, disable timeouts, or run as root. The resolver is allowed to be more complex than the protocol because check-then-open path validation is unsafe under the multi-user threat model.

Two consequences that repeatedly matter when editing:

- **No information leakage to visitors.** Every unavailable selector — missing, permission-denied, dotfile, symlink, special file, mount crossing, root escape, oversized — returns the identical literal body `document not found` (no trailing newline, no reason). Operational faults on an already-opened handle propagate as `Err` (for server-side logging), not as a fake "not found".
- **Containment is structural, not a string check.** The fd-relative resolver in `root` opens each selector component with `openat` + `O_NOFOLLOW` from the current directory fd and `fstat`-checks the result, so there is no TOCTOU window and a symlink anywhere on the path fails the open rather than being followed. Do not introduce `PathBuf::join`-then-open or whole-path opens (`File::open`, `std::fs::read*`, `read_dir`) on the request path — that's the bug class the design forbids, and `tests/architecture.rs` asserts against it (it scans the production source of the request-path modules for forbidden opens and selector `.join(`).

## Tests

Default-suite tests (all run by `make check`):

- **Module unit tests** (`#[cfg(test)] mod tests` inside `src/*.rs`) — closest to the code; cover the resolver, selector parsing, config validation, `check` diagnostics, per-connection handling (`conn`), and the worker pool (`server`, which binds ephemeral `127.0.0.1:0` ports and drives real sockets).
- **`tests/buffetcar_contract.rs`** — buffetcar's *own* policy contract over `serve_selector`: dotfile rejection, symlink-escape rejection, listing rules, balanced-`..` handling, mode/hardlink/special-file/device policy, listing bounds, etc.
- **`tests/check_contract.rs`** — black-box tests of the compiled binary's `check` and `serve` startup behavior (stdout/stderr, exit codes, bind-conflict error).
- **`tests/architecture.rs`** — guards the request path against whole-path opens and selector `.join(` (see Core principle).

Optional reference suite (NOT in `make check`):

- **`tests/nexd_contract.rs`** — *optional, reference-only* characterization of the Go `nexd` server, gated behind the `nexd-contract` feature and excluded from `make check`. It builds the local Go `nexd` and sends real TCP requests. Tests named `nexd_legacy_behavior_*` pin unsafe behavior buffetcar **deliberately inverts**; the rest pin protocol-compatible behavior to preserve. Requires a Go toolchain and the `nexd` checkout at `../nexd` (or `NEXD_REPO=/path`); the first build fetches the Mercurial-hosted `hg.sr.ht/~m15o/nex-pfm` module (`go mod download` to pre-warm). The suite binds a fixed `127.0.0.1:1900`, so back-to-back runs can intermittently hit a `TIME_WAIT` port-reuse race. `tests/common/mod.rs` is shared harness only for this suite.
