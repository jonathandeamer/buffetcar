# Outstanding Tests: Multi-User Race Stress Test and Deterministic Write-Timeout Test

Date: 2026-06-07

## Context

The multi-user Nex server design
(`2026-06-06-multi-user-nex-server-design.md`) is implemented, but two of its
listed test obligations are unbuilt:

- **Race stress test** (design line 461–463): "a stress test repeatedly swaps a
  name between safe file, symlink, FIFO, and directory while requests run, and
  asserts that outside or special content is never served." The containment is
  structural (fd-relative `openat` + `O_NOFOLLOW` + `fstat`), so the protection
  exists; the asserting test does not.
- **Write-timeout test** (design line 470): "write timeout for stalled
  readers." The timeout is applied in `conn::handle`
  (`stream.set_write_timeout`), but no test asserts it fires.

This spec covers building both. It does not cover the other two outstanding
items (README, OpenBSD `pledge`/`unveil`), which are not tests.

## Test 1 — Multi-User Race Stress Test

### Location

`tests/buffetcar_contract.rs`, gated `#[cfg(unix)]` (it needs FIFOs, symlinks,
and Unix modes). It drives the public `serve_selector` wrapper, matching the
other policy tests in that file. `serve_selector` opens a `Root` per call but
still resolves each selector component with `openat` + `O_NOFOLLOW` + `fstat`,
so the time-of-check/time-of-use window between component opens is exercised
exactly as in the daemon.

### Setup

- A served root (its own `TempSite`), world-traversable.
- An **outside** file containing a known `SECRET` marker, created in a *sibling*
  temp directory (same filesystem so `rename` works, but not under the served
  root). It is reachable only by following a symlink — which the resolver must
  refuse.
- A single swap name, `target`, directly under the served root.

### Mutator thread

One thread cycles `target` through four variants. Each variant is built fully
under a sibling staging path, then `rename(2)`'d onto `target` after the
previous occupant is removed. Staging-then-rename guarantees every concurrent
observation of `target` is either *absent* (mid-swap) or a *complete* variant —
never a half-constructed directory.

The four variants:

1. **Regular file** — world-readable (0644), content `SAFE\n`.
2. **Symlink** — points at the outside `SECRET` file (absolute path).
3. **FIFO** — created with `libc::mkfifo` (fast; not a `mkfifo` subprocess per
   cycle), world-readable.
4. **Directory** — 0755, containing one world-readable `child.txt`.

Removal of the current occupant before each cycle uses best-effort
`remove_file` followed by `remove_dir_all` (one succeeds for the file/symlink/
FIFO case, the other for the directory case; errors are ignored). The mutator
runs until the readers finish, then removes `target` a final time.

### Reader threads

A small fixed number of reader threads (e.g. 3), each calling
`serve_selector(root, "target")` a fixed number of times (e.g. 2000). The loop
is bounded by iteration count, not wall-clock, so total runtime is deterministic
and sub-second; race coverage comes from request volume and thread interleaving.

### Assertion — allowlist of safe outcomes

Every `serve_selector` call must return `Ok`, and the body must be **exactly one
of**:

- `SAFE\n` — the file variant was resolved.
- `=> child.txt\n` — the directory variant was listed.
- `document not found` — symlink rejected, FIFO rejected (special file), or
  `target` absent mid-swap.

Any other outcome fails the test: the `SECRET` bytes (outside content served),
FIFO content, an unexpected body, a panic, or an `Err` (no operational fault is
expected in this controlled scenario; an `Err` would be a real finding). The
allowlist inherently excludes the `SECRET` marker, satisfying the design's
"outside or special content is never served" requirement with a stronger
guarantee.

Reader threads collect failures (e.g. push the offending body into a shared
`Mutex<Vec<_>>` or return them via the join handle) and the test asserts the
collection is empty after join, so a failure reports the actual unexpected body.

## Test 2 — Deterministic Write-Timeout Test

### Location

`src/conn.rs`, in the existing `#[cfg(test)] mod tests`, beside
`read_timeout_fires_for_a_silent_client`. Gated `#[cfg(unix)]` (it uses
`libc::setsockopt`).

### Setup

- A `TempSite` containing a file `big` that is comfortably larger than any
  socket buffer (8 MiB), world-readable. `Root::open` over it.
- An ephemeral `127.0.0.1:0` `TcpListener`; the client connects to its address.
- Before the transfer, `libc::setsockopt` shrinks `SO_RCVBUF` on the client
  socket and `SO_SNDBUF` on the accepted server socket, so the kernel send/
  receive pipe fills quickly. (Belt-and-suspenders: an 8 MiB file exceeds even
  default buffers, but shrinking makes the block fast and the test cheap.)

### Drive

The client sends the selector `big\n` and then **never reads** the response. A
scoped thread runs `handle(server_stream, &root, read_timeout = 5s,
write_timeout = 200ms)`. With no reader draining the socket, the server's
`write_all` blocks once the buffers fill, and the 200 ms write timeout makes the
write return an error.

### Assertion

`handle` returns `Err`, mirroring `read_timeout_fires_for_a_silent_client`
(which asserts `is_err()`). The error kind on a write timeout is platform-
dependent (`WouldBlock`/`TimedOut`), so the test asserts `is_err()` rather than a
specific kind, with a message explaining the stalled-reader scenario.

## Shared Notes

- Both tests are `#[cfg(unix)]`.
- Both use `libc`, already a `[dependencies]` entry, so it is available to the
  integration-test crate (`tests/buffetcar_contract.rs`) and the in-crate unit
  module (`src/conn.rs`).
- Both must keep `make check` green and fast: the stress test is bounded by a
  fixed iteration count; the write-timeout test is bounded by the 200 ms write
  timeout.

## Out of Scope

- README stating publishing rules in `check`'s terms.
- OpenBSD `pledge`/`unveil` sandbox.
- Any change to production code in `src/` other than test modules.
