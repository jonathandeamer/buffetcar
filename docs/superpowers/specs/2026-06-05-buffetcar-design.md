# buffetcar — design

Date: 2026-06-05
Status: superseded on 2026-06-06 by
`docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`
License: MIT OR Apache-2.0

A hardened, single-binary Nex server in Rust. Crate and binary are both named
`buffetcar`. This document is retained as project history for the earlier
static-site threat model. It is no longer the active implementation spec.

## Core principle

Security and minimalism are buffetcar's two **equally high** priorities — neither
is a lesser concern traded away lightly. They usually reinforce each other: a
small, auditable codebase and dependency tree is itself part of being safe to
expose.

- **Ideal state — most secure *is* most minimal.** When the smallest solution is
  also the safest, that is the preferred outcome: delete a knob rather than
  validate it where no legitimate tuning case exists; make the safe value a
  hardcoded invariant rather than a default. (Removing `-v` deleted both code
  *and* the log-injection surface — the ideal in action.)
- **In tension — prefer security.** Where the two genuinely conflict, choose
  security even at the cost of more code. (The no-symlink-following invariant in
  §6 closes a dotfile bypass that the more minimal "just scope the claim" would
  have left open.)

Every flag, feature, and dependency in this spec is justified against this
principle; the §Decisions config rule is its direct application to configuration.

## Implementation status

As of 2026-06-06:

- **Landed:** selector resolution, dotfile policy, and directory listings
  (`serve_selector` in `lib.rs`), cap-std containment, and the contract test
  suite. Toolchain is in place: `make check` (fmt/clippy/test), CI on
  ubuntu-latest and macos-latest, a `cargo-deny` supply-chain gate, dependabot,
  and the Conventional Commits hook. Crate dual-licensed MIT OR Apache-2.0.
- **Not yet built:** the `config`, `server`, `conn`, and `sandbox` modules
  (§1/§9) — CLI flags, the `TcpListener` + worker pool, per-connection
  timeouts and selector bounds, and the OpenBSD `pledge`/`unveil` path. Resolve
  and listing logic currently sit consolidated in `lib.rs`; they will be split
  into modules per §9 as the server layer lands.
- **Refined 2026-06-06 (operator-experience pass):** dropped `--serve-dotfiles`
  (dotfile rejection is now an invariant), hardcoded the selector bound and index
  name, fixed that client IPs are never collected, and specified the startup
  banner, startup-failure messages, and shutdown behavior (§3, §5, §6).
- **Hardened 2026-06-06 (review pass):** added a no-symlink-following invariant
  so the dotfile rule cannot be bypassed by an in-root link (§2, §6, §7); and
  reconciled shutdown with the std-only/`libc`-OpenBSD-only dependency budget by
  relying on default signal handling rather than a goodbye message (§5, §8).
- **Principle pass 2026-06-06:** recorded the Core principle (security and
  minimalism equal; when minimal *is* most secure, that is the ideal; in tension,
  prefer security) and applied it to the config surface — hardcoded the read
  timeout (§3), capped `--max-conns` to a validated `1..=1024` range (§3), and
  removed access logging / `-v` entirely (§5), which also eliminated the
  log-injection surface rather than escaping around it. `clap` and
  `--write-timeout` were reviewed and deliberately kept.
- **Review pass 2 2026-06-06:** closed three gaps against the invariants —
  extended no-symlink to the implicit `index` open and to listing enumeration so
  a symlinked index/entry cannot bypass dotfile rejection (§4, §5, §6, §7); capped
  `--write-timeout` to `1..=300` (a per-write stall left unbounded would pin a
  worker and weaken the concurrency bound) (§3, §6, §7); and reconciled error
  logging with the no-per-request-logging guarantee — client-induced connection
  events are dropped silently, and only rare server-side I/O faults (no selector
  or client data) may reach the log (§5).

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
`buffetcar` keeps the protocol small while making the default hosting posture safe.

See `docs/2026-06-05-context.md` for the full protocol notes and security
analysis that informed this design.

## Goals

- Strict site-root containment, including `../` traversal and symlink escape,
  with no time-of-check/time-of-use (TOCTOU) window.
- Secure by default: loopback bind, read/write deadlines, bounded request,
  bounded concurrency — all without configuration. Properties that must never be
  weakened are invariants, not defaults: dotfiles are structurally unreachable
  (no override flag, and symlinks are never followed, so the rule cannot be
  bypassed by an in-root link), the selector bound and index name are fixed
  constants, and client IPs are never collected.
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
- **Config: CLI flags only (`clap`), secure by construction.** The config
  surface is small; a config file is unnecessary machinery. Guiding rule (the
  §Core principle applied to configuration): a flag exists only if it enables an
  *intended use* (where to serve, which address); a knob that could *weaken* a
  security property is not exposed at all — the safe value is a hardcoded
  invariant. Hence no dotfile-serving flag,
  no configurable selector bound, and no configurable index name. The remaining
  flags' defaults are safe out of the box (see §3).
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
| `resolve` | Map selector → file/dir inside the root via `cap_std::fs::Dir`; enforce dotfile + no-symlink policy     | `cap-std`  |
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
      ├─ escapes root / dotfile / symlink → reject
      ├─ directory               → serve `index`, else generate listing
      ├─ regular file            → stream bytes
      └─ not found               → short "not found" body
  → close connection
```

One request per connection, no retained state. `cap-std` refuses `../` and
symlink escapes structurally — it is the sole guard for `..`; the dotfile rule
(§6) only rejects `.`-prefixed normal components, not `..`; and resolution
additionally refuses any in-root symlink (no-follow, §6) so the dotfile rule
cannot be bypassed via a link.

## 3. Config & secure defaults (`clap`)

| Flag                    | Default          | Purpose                                                            |
|-------------------------|------------------|-------------------------------------------------------------------|
| `--root <PATH>`         | *required*       | served site root                                                  |
| `--listen <ADDR>`       | `127.0.0.1:1900` | loopback by default; public is a deliberate `--listen 0.0.0.0:1900` |
| `--max-conns <N>`       | `128`            | worker threads = concurrency cap (validated `1..=1024`, see below) |
| `--write-timeout <SECS>`| `30`             | stalled-reader defense while streaming (validated `1..=300`)     |

That is the entire surface. Per the §Decisions config rule, security properties
are not configurable; they are fixed invariants rather than flags:

- **Dotfiles are always rejected** — there is no serve-dotfiles flag.
- **Selector bound is `1024` bytes** — a hardcoded constant, so "bounded request"
  cannot be widened into an unbounded-allocation footgun.
- **Index name is `index`** — a hardcoded single path component, so the directory
  index lookup can never be pointed outside the root or at a dotfile.
- **Read timeout is `5s`** — a hardcoded constant, not a flag. A client only
  sends one short selector line, so there is no legitimate reason to lengthen it,
  and a large value would reopen the slowloris window the timeout exists to
  close. Deleting the knob is both the most minimal and the most secure option,
  so per the §Core principle it is hardcoded. (`--write-timeout` stays a flag
  because a genuinely slow reader of a large file can need a longer per-write
  stall allowance — but it is bounded *both* ways. `set_write_timeout` is a
  per-write-operation stall, not a total deadline, so a huge value lets a
  fully-stalled reader pin a worker for that whole duration; enough such readers
  exhaust the fixed pool and weaken the concurrency bound. It is therefore capped,
  not unbounded — see validation below.)
- **No access logging exists** — there is no `-v`/verbosity flag; client IPs and
  selectors are never recorded (see §5).

`config` validates at startup that `--root` exists and is a directory, that the
listen address parses, that `--write-timeout` is within `1..=300` (it can neither
be disabled with a low value nor stretched into a worker-pinning DoS with a high
one), and that `--max-conns` is within `1..=1024` — a
zero/absurd thread count can neither disable the concurrency bound nor turn it
into a resource-exhaustion footgun. The read timeout is a fixed constant, not a
flag (see the invariants above).

## 4. Directory listings

Per the Nex spec, directories are plain text and links are lines beginning with
`=> `. For a directory with no `index` file, generate one `=> name` line per
child: relative links, a trailing `/` on subdirectories, dotfiles and symlink
entries omitted, entries sorted. This stays compatible with the spec's relative-link examples
(e.g. `../nexlog/`). When an `index` file is present it is served instead of a
generated listing.

Specifics fixed by the implementation:

- **Sort by name alone.** The trailing `/` is applied at render time, after
  sorting, so a directory and a file sharing a prefix (e.g. `sub` and
  `sub.txt`) order alphabetically rather than by the `/`-vs-`.` byte.
- **Non-UTF-8 names are omitted** from listings: a lossy name could not
  round-trip to a fetchable text selector, so it is skipped rather than
  rendered with a placeholder.
- **Symlink entries are omitted**, consistent with the no-follow invariant (§6):
  a symlink is never servable, so listing it would only emit a dead link and
  reveal the link's existence.
- **No `.desc` reverse-ordering.** The Go reference reversed a listing when a
  `.desc` marker file was present; buffetcar does not carry this over — `.desc`
  is a dotfile and is rejected by policy.

## 5. Operator experience: startup, logging, errors & shutdown

**Startup.** On a successful bind, print a compact banner to stderr so the
operator sees the active posture at a glance:

```
buffetcar 0.1.0
  root:     /home/jd/site
  listen:   127.0.0.1:1900
  workers:  128
  timeouts: read 5s, write 30s
  sandbox:  cap-std containment (pledge/unveil active on OpenBSD)
```

The `listen:` line is the honest disclosure of public-vs-loopback exposure —
there is deliberately **no "serving publicly" warning**, because a non-loopback
bind is the operator's intended act, not a surprising or dangerous state. With
dotfiles now an invariant and no other safety knob to lower, startup has no
warning lines at all; warnings are reserved for genuinely surprising states so
that, if one ever fires, it is worth reading.

**Startup failures** print a single actionable `error:` line to stderr and exit
non-zero — never a panic or backtrace. Examples: `error: --root '/srv/site': not
a directory`; `error: could not bind 127.0.0.1:1900: address already in use`;
`error: invalid --listen '...:99999': invalid port`.

**Logging.** Operational only, always — the startup banner and operational errors
(startup failures, and the rare server-side I/O faults noted under Request
outcomes; never per-connection client events), written to stderr. **There is no
access logging and no verbosity flag:** buffetcar never emits a per-request line,
so it never records client IPs or selectors, and `peer_addr()` is never called
for logging. Per the §Core principle this is the
preferred outcome because it is simultaneously the most minimal *and* the most
secure: with no selector ever written to a log there is no log-injection surface
to escape against, and client privacy is absolute by construction — you cannot
leak what you never write. An operator who wants per-request visibility has the
firewall, `tcpdump`, or a reverse proxy, kept out of the security-critical path.
Logging is hand-rolled to stderr (no logging crate) to keep the dependency
surface minimal.

**Request outcomes.**

- Connection-level events — read timeout, oversized selector, connection reset,
  client disconnect — never crash the server and are **not logged**: the worker
  closes the connection and continues silently. These are normal, client-induced,
  per-connection events, so logging them would both contradict the no-per-request
  rule above and hand an attacker a trivial log-flooding lever.
- Unavailable targets — missing, permission-denied, or refused by cap-std as an
  escape — all return the same short body, the literal `document not found`
  (no trailing newline), then close. They are indistinguishable to the client
  by design, so nothing leaks *why* a selector is unavailable. Nex has no status
  codes. One asymmetry: when a *readable directory* is served, the `index` lookup
  treats any open error as "no index" and falls back to a generated listing — so
  a permission-denied `index`, **or a symlinked `index` (refused by the
  no-symlink rule, §6)**, inside an otherwise-readable directory yields the
  listing, not the not-found body. The indistinguishability guarantee covers the
  top-level selector resolution, not this directory-index fallback.
- Genuine operational faults on an already-opened handle (e.g. a read that fails
  mid-stream) are rare, server-side, and not driven by remote input; they
  propagate as errors the server layer may log as operational events — carrying
  no selector or client data — rather than masquerading as a missing document.
  This is the only request-path error that may reach the log, and it is not
  attacker-triggerable per request.

**Shutdown.** Termination relies on **default signal handling** — no signal
handlers are installed, because Rust std has no portable signal API and the
dependency budget keeps `libc` to OpenBSD FFI only (§8); installing a handler to
print a goodbye line would mean either a new dependency or `libc` on every
platform, cosmetics not worth the cost. On SIGINT/SIGTERM the process exits
immediately and the OS tears down the listener and sockets; in-flight requests
(one short line each, sub-second) may be cut — acceptable for this protocol, and
it keeps the blocking thread-pool model free of both drain coordination and a
signal-handling dependency. The exit status is the signal default (130/143), not
0; this is still a clean stop for a service manager — systemd treats a stop it
initiated as success, or set `SuccessExitStatus=130 143`. A hardened sample
`systemd` unit (`DynamicUser=yes`, `ProtectSystem=strict`, `NoNewPrivileges=yes`)
ships in the README as the natural step from a terminal run to a managed service;
its stderr logs are captured by journald unchanged.

## 6. Security model (consolidated)

- **Containment:** `cap-std` `Dir`, symlink- and TOCTOU-safe; primary guarantee
  on all platforms.
- **Dotfile rejection (invariant):** reject any *normal* path component
  beginning with `.`. There is no flag to disable this. It does **not** cover
  `..` (a parent-dir component): containment of `..` rests entirely on cap-std,
  which is load-bearing here — do not weaken that dependency assuming the dotfile
  rule backs it up.
- **No symlink following (invariant):** a symlink is never followed or served,
  rather than only on the typed selector. cap-std already refuses symlinks that
  *escape* the root; this rule additionally refuses symlinks that *stay inside*
  it. It applies at **every point a path is opened or enumerated**, because each
  is a potential bypass:
  1. **Selector resolution** refuses any path component that is a symlink.
  2. **The implicit `index` open** is subject to the same rule — a symlinked
     `index` (e.g. `index -> .secret`) is treated as "no index" and falls back to
     a generated listing (§5), never followed. Without this, the hardcoded index
     *name* would not be enough: the dir-index open would follow the link and
     serve the target.
  3. **Listing generation** omits symlink entries, exactly as it omits dotfiles —
     a symlink is not servable, so listing it would only produce a misleading
     dead link and leak the link's existence.

  Rationale: the dotfile rule above inspects only the typed selector's
  components, but cap-std follows in-root symlinks, so without no-follow a non-dot
  link such as `public -> .secret` or `index -> .git/config` would serve an
  otherwise-blocked target — silently bypassing dotfile rejection. With no-follow
  applied uniformly, "dotfiles are unreachable" is a true structural property,
  not a selector-syntax filter. This is the most-secure option and also the more
  minimal of the ways to close the gap (one uniform "never follow or list" rule
  vs. resolve-then-inspect-the-target). Cost: in-root symlinks are not a usable
  feature — but the design never offered them (it only ever promised symlink
  *escape* refusal). *Implementation note:* the mechanisms (a per-component
  `symlink_metadata` check / no-follow open for resolution and the index open;
  `entry.file_type()?.is_symlink()` for listing) must be confirmed against the
  cap-std API; do not assume a specific call exists.
- **Fixed safety constants:** the `1024`-byte selector bound, the `index` index
  name, and the `5s` read timeout are compile-time constants, not flags, so the
  request bound, the index lookup, and the slowloris defense cannot be
  reconfigured into a weakness (an oversized allocation, an index name that
  escapes the root or resolves to a dotfile, or a stretched read deadline).
- **Secure default bind:** loopback; public exposure is explicit.
- **Resource bounds:** a constant `5s` read deadline and a configurable
  write deadline (validated `1..=300`, since a per-write stall timeout left
  unbounded would let a stalled reader pin a worker), a constant `1024`-byte
  selector bound, and a concurrency cap validated to `1..=1024` with kernel-backlog
  refusal beyond it — none of these defenses can be disabled or set to an
  exhausting value.
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
- symlink *inside* the tree refused too (no-follow): a non-dot link to a dotfile
  target (`public -> .secret`) does not serve the dotfile, and an in-root link to
  an ordinary target is also refused;
- symlinked index treated as missing: a directory whose `index` is a symlink
  (`index -> .secret`) does not serve the target and falls back to a listing;
- symlink entries omitted from generated listings;
- dotfile rejection;
- `index` served when present;
- listing format (`=> ` links, trailing slashes on directories, dotfiles
  omitted, sorted);
- selector-length bound enforced;
- slow-client read-timeout (integration test with a stalling socket);
- default loopback bind;
- config validation: a non-directory `--root` is rejected; `--write-timeout`
  outside `1..=300` is rejected (the defense can be neither disabled nor stretched
  into a worker-pinning DoS); and `--max-conns` outside `1..=1024` is rejected
  (the concurrency bound cannot be disabled or set to an exhausting value).

Approach: the reference Go server's behavior was first captured as
characterization tests, then the unsafe "legacy" cases (dotfile served, `../`
escape, symlink escape) were inverted into buffetcar's secure contract. The
landed tests live in `tests/buffetcar_contract.rs` (the `containment.rs` /
`protocol.rs` split in §9 remains the target) and cover file/index/listing
serving, binary preservation, dotfile rejection, balanced `..` allowed, `../`
escape rejected, and symlink-escape rejected. The selector-length bound,
slow-client timeout, and loopback-bind tests arrive with the server layer.

CI currently runs the gate (fmt, clippy `-D warnings`, test) on
**ubuntu-latest and macos-latest**, plus a `cargo-deny` job. The OpenBSD
`sandbox` path is `#[cfg]`-gated; its CI compile-check and manual OpenBSD
exercise land with `sandbox.rs`.

## 8. Dependencies

Deliberately small: `clap` (CLI), `cap-std` (containment), `libc` (OpenBSD FFI
only). No async runtime, no logging crate. As of 2026-06-06 only `cap-std` is
pulled in; `clap` and `libc` arrive with the config and sandbox modules.

The crate is dual-licensed **MIT OR Apache-2.0** (`LICENSE-MIT`,
`LICENSE-APACHE`), the Rust-ecosystem default.

The toolchain is already in place (not deferred to after v1): a `Makefile`
mirroring `make check` (`cargo fmt --check`, `clippy -D warnings`,
`cargo test`), dependabot, and the Conventional Commits hook. The supply-chain
gate is **`cargo-deny`** (chosen over `cargo-audit`, which it supersedes):
`deny.toml` enforces RustSec advisories, an allow-listed set of permissive
licenses, a wildcard-dependency ban, and crates.io-only sources, run in CI via
`cargo-deny-action` and locally via `make deny`. Duplicate crate versions are
set to `warn` (reported, not failed), not denied.

## 9. Project layout

```
Cargo.toml
src/
  main.rs        # wire-up: parse config, bind, sandbox, run pool
  lib.rs         # module declarations, shared types
  config.rs      # clap Config + validation
  server.rs      # listener + worker pool, accept loop
  conn.rs        # per-connection: timeouts, read selector, dispatch
  resolve.rs     # cap-std containment, selector → file/dir, dotfile + no-symlink policy
  listing.rs     # directory listing generation, index lookup
  sandbox.rs     # cfg(openbsd) pledge/unveil; no-op elsewhere
tests/
  containment.rs # traversal, symlink, dotfile
  protocol.rs    # selector bounds, index, listing, timeouts, bind
```
