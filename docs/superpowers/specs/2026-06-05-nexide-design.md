# nexide — design

Date: 2026-06-05
Status: approved (design phase)

A hardened, single-binary Nex server in Rust. Crate and binary are both named
`nexide`. This document is the design spec; an implementation plan follows
separately.

## Background

Nex is a tiny protocol for distributed document retrieval (Gopher/Gemini
lineage): a client connects on TCP port 1900, sends one selector line, receives
text or binary data, and the server closes the connection. No state is retained,
there is no TLS by design, and it targets public, read-only, low-stakes personal
publishing. Empty paths and paths ending in `/` denote directories; directory
maps are plain text where lines beginning with `=> ` are links, which may be
relative.

The reference server (`nexd`, ~46 lines of Go) reads one line and serves from
`os.DirFS(root)` with a goroutine per connection. It is useful as a reference
but unsafe to expose: no root containment against symlink escape, no read
deadlines, no connection or request-size limits, and it binds all interfaces.
`nexide` keeps the protocol small while making the default hosting posture safe.

See `docs/2026-06-05-context.md` for the full protocol notes and security
analysis that informed this design.

## Goals

- Strict site-root containment, including `../` traversal and symlink escape,
  with no time-of-check/time-of-use (TOCTOU) window.
- Secure by default: loopback bind, read/write deadlines, bounded request,
  bounded concurrency, dotfiles rejected — all without configuration.
- Small, auditable codebase and dependency tree.
- Behavior compatible with the Nex spec, including relative links in directory
  listings.

## Non-goals (v1)

TLS (the protocol has none by design), config file, rate limiting (rely on
firewall plus the connection cap), virtual hosts, caching, metrics, log
rotation, and a client (`rex` already exists).

## Decisions

These were settled during brainstorming and are fixed for v1:

- **Concurrency: threaded, std only.** Blocking `std::net` with a fixed pool of
  worker threads. No async runtime. The protocol is one short request per
  short-lived connection at hobby scale; threads match the workload, keep the
  dependency/audit surface minimal, and keep the security-critical path linear
  and auditable. The thread count is the concurrency cap.
- **Containment: `cap-std`.** A capability-based `Dir` opened on the root makes
  every subsequent open relative and structurally refuses escapes, including via
  symlinks, with no TOCTOU window — the Rust analog of Go's `os.Root`. This is
  the one dependency worth spending: hand-rolling symlink- and TOCTOU-safe
  containment is a class of bug, not a one-liner.
- **Config: CLI flags only (`clap`), secure defaults.** The config surface is
  small; a config file is unnecessary machinery. Defaults are safe out of the
  box (see §3).
- **Platform: portable, with OpenBSD `pledge`/`unveil` compiled into v1** behind
  `#[cfg(target_os = "openbsd")]`. `cap-std` is the in-process containment
  guarantee on every platform; `pledge`/`unveil` add an OS-level second wall on
  OpenBSD. Because development is on macOS, the OpenBSD path is compile-checked
  locally / in CI and exercised only on OpenBSD; the cap-std containment is what
  local tests cover.

## 1. Architecture & components

`lib.rs` plus a thin `main.rs`, so logic is integration-testable.

| Module    | Responsibility                                                                                          | Depends on |
|-----------|--------------------------------------------------------------------------------------------------------|------------|
| `config`  | Parse and validate CLI flags into a `Config` (root exists and is a directory, etc.)                     | `clap`     |
| `server`  | Bind `TcpListener`; run a fixed pool of N worker threads, each looping `accept()` → `conn::handle`      | std        |
| `conn`    | Per-connection: set read/write timeouts, read one bounded selector line, dispatch, write response, close | std       |
| `resolve` | Map selector → file/dir inside the root via `cap_std::fs::Dir`; enforce dotfile policy                  | `cap-std`  |
| `listing` | Directory handling: serve `index` if present, else generate a plain-text listing with `=> ` links       | std        |
| `sandbox` | `#[cfg(target_os = "openbsd")]` `pledge`/`unveil`; no-op stub elsewhere                                 | `libc`     |

**Concurrency model:** N worker threads share the listener (`try_clone`); the OS
load-balances `accept()`. No channel or thread-pool crate is needed — the thread
count *is* the cap. Each worker handles one short-lived connection at a time;
connections beyond N wait in the kernel listen backlog and are then refused.

## 2. Data flow

```
accept → set read/write timeouts → read selector line (bounded length)
  → normalize selector → cap-std resolve within root
      ├─ escapes root / dotfile  → reject
      ├─ directory               → serve `index`, else generate listing
      ├─ regular file            → stream bytes
      └─ not found               → short "not found" body
  → close connection
```

One request per connection, no retained state. `cap-std` refuses `../` escapes
and symlink escapes structurally.

## 3. Config & secure defaults (`clap`)

| Flag                   | Default            | Purpose                                                            |
|------------------------|--------------------|-------------------------------------------------------------------|
| `--root <PATH>`        | *required*         | served site root                                                  |
| `--listen <ADDR>`      | `127.0.0.1:1900`   | loopback by default; public is a deliberate `--listen 0.0.0.0:1900` |
| `--max-conns <N>`      | `128`              | worker threads = concurrency cap                                  |
| `--read-timeout <SECS>`| `5`                | slow-client / slowloris defense                                  |
| `--write-timeout <SECS>`| `30`              | stalled-reader defense while streaming                           |
| `--max-selector <BYTES>`| `1024`            | bound the single request line                                    |
| `--index <NAME>`       | `index`            | directory index filename                                         |
| `--serve-dotfiles`     | off                | dotfiles rejected unless set                                     |
| `-v`, `--verbose`      | off                | enable access logging (see §5)                                    |

`config` validates at startup that `--root` exists and is a directory and that
the listen address parses.

## 4. Directory listings

Per the Nex spec, directories are plain text and links are lines beginning with
`=> `. For a directory with no `index` file, generate one `=> name` line per
child: relative links, a trailing `/` on subdirectories, dotfiles omitted,
entries sorted. This stays compatible with the spec's relative-link examples
(e.g. `../nexlog/`). When an `index` file is present it is served instead of a
generated listing.

## 5. Error handling & logging

- Connection errors (timeout, oversized selector, reset) never crash the server;
  the worker logs and continues.
- Not found / rejected requests return a short plain-text body, then close. Nex
  has no status codes.
- Logging is privacy-conscious: default logs are operational only — startup
  banner, bind address, errors — written to stderr, with **no per-request client
  IPs**. `-v` opts into access logging. Logging is hand-rolled to stderr (no
  logging crate) to keep the dependency surface minimal.

## 6. Security model (consolidated)

- **Containment:** `cap-std` `Dir`, symlink- and TOCTOU-safe; primary guarantee
  on all platforms.
- **Dotfile policy:** reject any path component beginning with `.` (also
  belt-and-suspenders against `.` / `..`).
- **Secure default bind:** loopback; public exposure is explicit.
- **Resource bounds:** read/write deadlines, bounded selector length, fixed
  concurrency cap with kernel-backlog refusal beyond it.
- **OS sandbox (OpenBSD, v1):** after binding the socket and opening the root,
  `unveil` the root read-only and lock, then `pledge` down to `stdio rpath
  inet`. Compile-gated; runs on OpenBSD, compile-checked elsewhere.
- **Non-goal:** TLS — the protocol has none by design; stated explicitly so it
  is not mistaken for an omission.

## 7. Testing

Runnable on macOS via `cap-std`:

- path normalization;
- `../` escape rejected;
- symlink-escape rejected (a symlink inside the tree pointing outside is
  refused);
- dotfile rejection;
- `index` served when present;
- listing format (`=> ` links, trailing slashes on directories, dotfiles
  omitted, sorted);
- selector-length bound enforced;
- slow-client read-timeout (integration test with a stalling socket);
- default loopback bind.

The OpenBSD `sandbox` path is `#[cfg]`-gated and compile-checked in CI; it is
exercised manually on OpenBSD.

## 8. Dependencies

Deliberately small: `clap` (CLI), `cap-std` (containment), `libc` (OpenBSD FFI
only). No async runtime, no logging crate.

The lookit-style toolchain ports over after v1 lands: Cargo plus a `Makefile`
mirroring `make check` (`cargo fmt --check`, `clippy -D warnings`,
`cargo test`), `cargo-audit` / `cargo-deny` for the vulnerability gate,
dependabot, and the Conventional Commits hook already installed in this repo.

## 9. Project layout

```
Cargo.toml
src/
  main.rs        # wire-up: parse config, bind, sandbox, run pool
  lib.rs         # module declarations, shared types
  config.rs      # clap Config + validation
  server.rs      # listener + worker pool, accept loop
  conn.rs        # per-connection: timeouts, read selector, dispatch
  resolve.rs     # cap-std containment, selector → file/dir, dotfile policy
  listing.rs     # directory listing generation, index lookup
  sandbox.rs     # cfg(openbsd) pledge/unveil; no-op elsewhere
tests/
  containment.rs # traversal, symlink, dotfile
  protocol.rs    # selector bounds, index, listing, timeouts, bind
```
