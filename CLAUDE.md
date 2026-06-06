# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`buffetcar` (crate name) is a hardened, single-binary **Nex** server in Rust. Nex is a minimal protocol: the client sends one selector line over TCP (port 1900 by default), the server replies with the raw text/binary bytes of a file — or a generated directory listing using `=> ` links — then closes. No status codes, no connection state, no URL decoding.

The project is **early-stage and design-led**. Most of the repo is specs and plans under `docs/superpowers/`; the implementation is a single entry point in `src/lib.rs`. Read the authoritative design before making non-trivial changes (see below).

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

## Design authority and the landed-vs-planned gap

The plan of record is **`docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`**. It supersedes the earlier `2026-06-05-buffetcar-design.md`. The two specs disagree, and the disagreement is load-bearing:

- The **landed** `src/lib.rs` uses `cap-std::Dir` for containment (the 2026-06-05 design, which assumed a static public site).
- The **active** design targets a stronger **multi-user** threat model (local untrusted users racing request resolution) and deliberately reverses several earlier choices: replace `cap-std` with an explicit fd-relative resolver (`rustix` `openat` + `O_NOFOLLOW` + `fstat`, portable baseline; `openat2` is Linux-only optional hardening), hand-parse the `serve`/`check` CLI instead of `clap`, and split `resolve` into `selector` + `root` modules (planned modules: `cli`, `config`, `server`, `conn`, `selector`, `root`, `listing`, `sandbox`).

So `src/lib.rs` is a known-interim shape. The public entry point `serve_selector(root: &Path, selector: &str) -> io::Result<Vec<u8>>` is intended to survive as a thin test/library wrapper; the daemon path will open the root once at startup. Don't assume the current cap-std implementation is the target architecture.

## Core principle (drives most decisions)

Security and minimalism are co-equal; **when they genuinely conflict, prefer security.** Safety properties are *invariants, not configurable defaults* — there is intentionally no flag/config to serve dotfiles, follow symlinks, change the index name, disable timeouts, or run as root. The resolver is allowed to be more complex than the protocol because check-then-open path validation is unsafe under the multi-user threat model.

Two consequences that repeatedly matter when editing:

- **No information leakage to visitors.** Every unavailable selector — missing, permission-denied, dotfile, symlink, special file, mount crossing, root escape, oversized — returns the identical literal body `document not found` (no trailing newline, no reason). Operational faults on an already-opened handle propagate as `Err` (for server-side logging), not as a fake "not found".
- **Containment is structural, not a string check.** The current code leans on cap-std to refuse `..`/symlink escape with no TOCTOU window (note the comment in `serve_selector`: the dotfile check deliberately does not cover `..`). The active design moves this guarantee into the fd-relative resolver. Either way, do not introduce `PathBuf::join`-then-open or whole-path opens on the request path — that's the bug class the design forbids (and the planned architecture-guard tests assert against it).

## Tests

Two suites with different purposes:

- **`tests/buffetcar_contract.rs`** — buffetcar's *own* policy contract (default suite, in `make check`). Asserts the hardened behavior: dotfile rejection, symlink-escape rejection, listing rules, balanced-`..` handling, etc.
- **`tests/nexd_contract.rs`** — *optional, reference-only* characterization of the Go `nexd` server, gated behind the `nexd-contract` feature and excluded from `make check`. It builds the local Go `nexd` and sends real TCP requests. Tests named `nexd_legacy_behavior_*` pin unsafe behavior buffetcar **deliberately inverts**; the rest pin protocol-compatible behavior to preserve. Requires a Go toolchain and the `nexd` checkout at `../nexd` (or `NEXD_REPO=/path`); the first build fetches the Mercurial-hosted `hg.sr.ht/~m15o/nex-pfm` module (`go mod download` to pre-warm). The suite binds a fixed `127.0.0.1:1900`, so back-to-back runs can intermittently hit a `TIME_WAIT` port-reuse race. `tests/common/mod.rs` is shared harness only for this suite.
