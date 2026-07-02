# In-app per-IP concurrent-connection cap

**Date:** 2026-07-01
**Issue:** [#29 — Abuse resistance: per-IP connection/rate limits](https://github.com/jonathandeamer/buffetcar/issues/29)
**Status:** implemented

## Problem

Concurrency is capped globally at `--workers` (default 128) via the rendezvous
channel, and per-connection timeouts bound a single client to ~35 s of
worker-hold. But there is no **per-IP** cap: one source IP can open all 128
connections and starve every other visitor — a low-effort slowloris-style
denial of service.

The host-firewall layer (`contrib/buffetcar.nftables`, per-source `ct count` on
tcp/1900) already closes this at the network layer for the production
deployment. This design adds the **in-app** counterpart, justified narrowly as
making the binary **safe-by-default when deployed without a firewall** — the
firewall is Linux-only and external to the binary, so a `buffetcar` run
anywhere else (OpenBSD, a box with no nftables) currently has no per-IP
protection at all.

## Scope

An in-app **per-IP concurrent-connection cap** — the resource-cap sibling of the
global `--workers` cap. One source IP may hold at most *N* simultaneous
in-flight connections; excess connections are dropped before they occupy a
worker.

Explicitly **out of scope**: rate limiting (connections per unit time). That is
the firewall layer's job, as `SECURITY.md` states and the 2026-06-06 design
scopes out. This change is a concurrent *resource* cap, not a *rate* limit.

## Configuration

Follows the established pattern of `--workers` / `--write-timeout`: an
operational tuning flag with a safe default, always enforced, no disable switch.

- **Flag:** `--max-conns-per-ip <N>`, validated to `1..=(workers + 1)`, where
  the upper bound is the worker pool's true maximum concurrent occupancy (see
  next bullet). The bound is resolved *after* `workers`, so it tracks the
  configured pool size (default 128 workers → `1..=129`; max 1024 workers →
  `1..=1025`).
- **Default:** derived from the resolved worker count as `max(1, workers / 8)`.
  At the default 128 workers this is **16** — coincidentally the same per-IP cap
  the nftables ruleset uses, giving one coherent number across both layers. It
  scales: 1024 workers → default 128.
- **Always enforced; no disable sentinel.** The "no flag to disable safety"
  principle is honoured, and no escape hatch is needed as a *special* mechanism:
  setting `--max-conns-per-ip` to its maximum, **`workers + 1`**, makes the
  per-IP cap unreachable, naturally neutralising it for deployments that need to
  (e.g. behind a reverse proxy — see Limitations). The `+ 1` is load-bearing:
  with the zero-buffer rendezvous channel the accept loop can hold **one**
  already-accepted connection (parked in `tx.send`) while all `workers` workers
  are busy, so a single IP can hold at most `workers + 1` permits at once.
  Capping at `workers` would still drop that extra connection — the one the
  backlog would otherwise hold — so `workers + 1` is the smallest value that is
  provably a no-op. This falls out of the plain validated integer with no magic
  value.

### Why a flag rather than a hardcoded constant

The project hardcodes the *read* timeout (a slow reader is pure abuse, no
legitimate tuning) but makes the *write* timeout a flag (legitimate client
variance). A per-IP cap has genuine legitimate variance — the reverse-proxy
deployment wants a different value — so by the project's own classification it
is *operational*, like `--write-timeout`, and gets a flag with a safe default.

## Design

### New module `src/limiter.rs`

A focused unit whose only job is per-IP connection accounting.

```rust
pub(crate) struct PerIpLimiter {
    inner: Mutex<HashMap<IpAddr, u32>>,
    cap: u32,
}

pub(crate) struct ConnPermit {
    limiter: Arc<PerIpLimiter>,
    ip: IpAddr,
}

impl PerIpLimiter {
    pub(crate) fn new(cap: u32) -> Self;

    /// Under the lock: if this IP is already at `cap`, return `None`;
    /// otherwise increment its count and return a permit.
    pub(crate) fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnPermit>;
}

impl Drop for ConnPermit {
    /// Under the lock: decrement this IP's count, removing the entry when it
    /// reaches zero so the map cannot grow unboundedly with distinct IPs.
    fn drop(&mut self);
}
```

### Lifecycle in the accept loop (`src/server.rs`)

The private `ServeSettings` gains a `max_conns_per_ip: u32` field, and
`serve(...)` constructs an `Arc<PerIpLimiter>` shared with the workers via the
channel payload. Grouping worker count, cap, and timeouts in `ServeSettings`
keeps the function below Clippy's argument-count limit.

- `accept()` already returns the peer `SocketAddr` (currently discarded as
  `(stream, _)`); take `ip = peer.ip()`.
- `limiter.try_acquire(ip)`:
  - `None` (at cap) → **drop the stream** (its `Drop` closes the fd; no bytes,
    no `document not found`, no reason) and `continue`. The connection never
    reaches a worker.
  - `Some(permit)` → send `(stream, permit)` over the rendezvous channel.
- The channel payload type changes from `TcpStream` to
  `(TcpStream, ConnPermit)`. The worker handles the connection, then the permit
  drops at the end of the loop iteration → decrement. RAII guarantees the count
  is released even if `conn::handle` returns `Err` or the worker unwinds.

### Concurrency: no TOCTOU, negligible contention

Only the **single accept-loop thread** calls `try_acquire`, so the
check-then-increment is atomic on one thread under one lock — the TOCTOU window
#29 warns about cannot occur. Workers only *drop* permits (decrement). A single
`Mutex<HashMap>` guards both; each critical section is a single map operation.
Increment is single-threaded; decrements are brief and happen only at connection
end. At 128 workers this contention is negligible.

Peak permits held for a single IP is `workers + 1`: up to `workers` connections
being handled, plus one already-accepted connection the single accept-loop
thread can be holding while parked in `tx.send` on the zero-buffer rendezvous
channel. This ceiling is why the flag's validated maximum is `workers + 1` (see
Configuration).

### Over-cap behaviour

Silent drop: the excess connection is closed with no bytes written. Serving
`document not found` was considered and rejected — it would require dispatching
to a worker, defeating the entire purpose (the point is to *not* consume a
worker). A closed connection leaks nothing, consistent with the no-leakage
contract.

## Non-impacts

- **OpenBSD sandbox:** the limiter uses only memory and already-permitted
  syscalls, so `pledge`/`unveil` promises are unchanged.
- **Signal-safety:** the limiter is never touched from a signal handler.
- **`tests/architecture.rs`:** the limiter opens no files and is not in the
  request-path module set, so the whole-path-open / selector-`.join(` guards are
  unaffected.
- **`serve_selector` wrapper:** per-call, no server/accept loop; unaffected.

## Limitations (documented, not built)

- **Reverse proxy.** Nex has no header channel (`X-Forwarded-For` has no
  equivalent), so behind a TCP reverse proxy every connection carries the
  proxy's IP and the cap would throttle all clients as one. buffetcar's
  production deployment serves Nex **directly** (peer IP is the real client), so
  this does not apply there; operators who front it with a proxy set
  `--max-conns-per-ip` to `workers + 1` (its maximum) to neutralise it. The same
  limitation applies to the firewall layer.
- **IPv6 prefix.** Keyed on the full `IpAddr`. A single IPv6 client controls a
  whole prefix (≥ /64) and could rotate addresses within it, so full-address
  keying is weak for IPv6. Production binds `0.0.0.0` (IPv4 only), so this is
  moot today; subnet-aware keying is a possible future refinement (YAGNI now).

## Testing

- **`limiter.rs` unit tests** (deterministic — the core correctness):
  - acquire up to `cap` succeeds; the next `try_acquire` for that IP returns
    `None`;
  - dropping a permit frees exactly one slot for that IP;
  - distinct IPs are accounted independently;
  - after all permits for an IP drop, the map contains no entry for it
    (zero-count eviction).
- **`server.rs` integration test:** with a small cap and enough workers, hold
  `cap` slow connections open from loopback, assert the next connection from the
  same IP is refused (closed with no body), then release one and assert a fresh
  connection is accepted.
- **Signature update:** the existing `handles_many_concurrent_clients` test
  passes the neutralising maximum `workers + 1` for `max_conns_per_ip`. Its 16
  loopback clients all share one IP, but with a 4-worker pool at most
  `workers + 1 = 5` are ever in flight at once, and `workers + 1` is provably a
  no-op (see Concurrency), so every client still succeeds.

## Files touched

- `src/limiter.rs` — new module (`PerIpLimiter`, `ConnPermit`, unit tests).
- `src/lib.rs` — `mod limiter;`.
- `src/config.rs` — `ServeConfig.max_conns_per_ip`; resolve `workers` first,
  then derive the default `max(1, workers / 8)` and validate
  `--max-conns-per-ip` against `1..=(workers + 1)`.
- `src/cli.rs` — parse `--max-conns-per-ip`; update usage/help text.
- `src/server.rs` — `ServeSettings.max_conns_per_ip`; accept-loop acquire/drop;
  channel payload type; worker carries the permit; `run` passes
  `config.max_conns_per_ip` into the settings value.
- `tests/check_contract.rs` — CLI help/usage snapshot for the new flag.
- `SECURITY.md` — note worker-exhaustion is now mitigated in-app (per-IP cap) in
  addition to the firewall.
- `CLAUDE.md` — mark both halves of #29 done; add the flag to the CLI list.

## Close-out

Closes #29. Human-merge (touches the threat model).
