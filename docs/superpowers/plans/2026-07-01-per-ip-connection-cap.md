# Per-IP Concurrent-Connection Cap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an always-on, configurable per-source-IP concurrent-connection cap that silently drops excess connections before they consume a worker.

**Architecture:** A focused `PerIpLimiter` owns a mutex-protected `HashMap<IpAddr, u32>` and returns RAII permits. The single accept loop acquires a permit before dispatching `(TcpStream, ConnPermit)` over the existing zero-capacity channel; the worker holds the permit until connection handling ends. CLI validation derives a safe default from `workers` and allows `workers + 1` as the neutralizing maximum because the accept loop can hold one accepted connection while all workers are busy. Worker count, cap, and timeouts travel together in a private `ServeSettings` value so `serve` retains a focused signature that passes Clippy's argument-count lint.

**Tech Stack:** Rust standard library (`Arc`, `Mutex`, `HashMap`, `TcpStream`, `sync_channel`), existing hand-written CLI/config modules, Rust unit and socket integration tests.

**Design reference:** `docs/superpowers/specs/2026-07-01-per-ip-connection-cap-design.md`

---

## File Structure

- Create: `src/limiter.rs`
  - Own per-IP accounting and RAII permit release.
- Modify: `src/lib.rs`
  - Register the private limiter module.
- Modify: `src/cli.rs`
  - Parse `--max-conns-per-ip` and document it in usage/help output.
- Modify: `src/config.rs`
  - Resolve `workers` first, derive the default cap, validate `1..=(workers + 1)`, and expose the resolved `u32` in `ServeConfig`.
- Modify: `src/server.rs`
  - Acquire permits in both accept-loop variants, carry permits through the rendezvous channel, and release them in workers.
- Modify: `tests/check_contract.rs`
  - Assert the public help output includes the new flag and its worker-relative bounds/default.
- Modify: `SECURITY.md`
  - Record the in-app concurrent cap while keeping connection-rate limiting scoped to the firewall.
- Modify: `CLAUDE.md`
  - Add the limiter to the module map and mark the in-app half of issue #29 complete.

Implementation constraints: keep `serve_selector` unchanged; do not change signal-handler behavior, OpenBSD `pledge`/`unveil` promises, or request-path file access. The limiter is memory-only and introduces no new sandbox requirements or selector-resolution paths.

### Task 1: Implement the RAII Per-IP Limiter

**Files:**
- Create: `src/limiter.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Register the module and write failing unit tests**

Add `mod limiter;` beside the other private modules in `src/lib.rs`.

Create `src/limiter.rs` with imports and tests only:

```rust
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4() -> IpAddr {
        Ipv4Addr::LOCALHOST.into()
    }

    #[test]
    fn acquires_up_to_cap_and_refuses_the_next_permit() {
        let limiter = Arc::new(PerIpLimiter::new(2));

        let first = limiter.try_acquire(v4()).expect("first permit");
        let second = limiter.try_acquire(v4()).expect("second permit");
        assert!(limiter.try_acquire(v4()).is_none());

        drop((first, second));
    }

    #[test]
    fn dropping_a_permit_frees_exactly_one_slot() {
        let limiter = Arc::new(PerIpLimiter::new(2));
        let first = limiter.try_acquire(v4()).expect("first permit");
        let _second = limiter.try_acquire(v4()).expect("second permit");

        drop(first);
        let _replacement = limiter.try_acquire(v4()).expect("replacement permit");
        assert!(limiter.try_acquire(v4()).is_none());
    }

    #[test]
    fn accounts_for_distinct_ips_independently() {
        let limiter = Arc::new(PerIpLimiter::new(1));
        let ipv4 = v4();
        let ipv6 = IpAddr::V6(Ipv6Addr::LOCALHOST);

        let _v4_permit = limiter.try_acquire(ipv4).expect("IPv4 permit");
        let _v6_permit = limiter.try_acquire(ipv6).expect("IPv6 permit");
        assert!(limiter.try_acquire(ipv4).is_none());
        assert!(limiter.try_acquire(ipv6).is_none());
    }

    #[test]
    fn removes_an_ip_after_its_last_permit_drops() {
        let limiter = Arc::new(PerIpLimiter::new(2));
        let first = limiter.try_acquire(v4()).expect("first permit");
        let second = limiter.try_acquire(v4()).expect("second permit");

        drop(first);
        assert_eq!(limiter.inner.lock().unwrap().get(&v4()), Some(&1));
        drop(second);
        assert!(!limiter.inner.lock().unwrap().contains_key(&v4()));
    }
}
```

- [ ] **Step 2: Run the limiter tests to verify they fail**

Run: `cargo test limiter::tests`

Expected: compilation fails because `PerIpLimiter` is not defined.

- [ ] **Step 3: Implement the limiter and permit**

Insert this implementation before the test module in `src/limiter.rs`:

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
    pub(crate) fn new(cap: u32) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            cap,
        }
    }

    pub(crate) fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnPermit> {
        let mut counts = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        let count = counts.entry(ip).or_insert(0);
        if *count >= self.cap {
            return None;
        }
        *count += 1;
        drop(counts);

        Some(ConnPermit {
            limiter: Arc::clone(self),
            ip,
        })
    }
}

impl Drop for ConnPermit {
    fn drop(&mut self) {
        let mut counts = self
            .limiter
            .inner
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let Some(count) = counts.get_mut(&self.ip) else {
            debug_assert!(false, "permit must have a matching limiter entry");
            return;
        };
        *count -= 1;
        if *count == 0 {
            counts.remove(&self.ip);
        }
    }
}
```

- [ ] **Step 4: Run the limiter tests to verify they pass**

Run: `cargo test limiter::tests`

Expected: 4 limiter tests pass.

- [ ] **Step 5: Commit the limiter unit**

```bash
git add src/lib.rs src/limiter.rs
git commit -m "feat(limiter): add per-IP connection permits"
```

### Task 2: Add the CLI Flag and Help Text

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/config.rs` (mechanical updates to existing `ServeArgs` test literals only)
- Modify: `tests/check_contract.rs`

- [ ] **Step 1: Extend the parser and help tests first**

Update `cli::tests::explicit_serve_accepts_all_serve_flags` to include:

```rust
                "--max-conns-per-ip",
                "8",
```

between the existing `--workers` and `--write-timeout` arguments, and add this expected field to its `ServeArgs` value:

```rust
                max_conns_per_ip: Some("8".to_string()),
```

Add the public help assertions in `tests/check_contract.rs::help_screen_triggers_and_content`:

```rust
    assert!(out_serve_str.contains("--max-conns-per-ip <N>"));
    assert!(out_serve_str.contains("between 1 and workers + 1"));
    assert!(out_serve_str.contains("default: max(1, workers / 8)"));
```

Add this assertion to `tests/check_contract.rs::error_output_includes_help_hint` so the compact error usage is covered too:

```rust
    assert!(err_str.contains("[--max-conns-per-ip <N>]"));
```

- [ ] **Step 2: Run the focused tests to verify they fail**

Run:

```bash
cargo test cli::tests::explicit_serve_accepts_all_serve_flags
cargo test --test check_contract help_screen_triggers_and_content
cargo test --test check_contract error_output_includes_help_hint
```

Expected: compilation fails because `ServeArgs` has no `max_conns_per_ip` field; after that field exists but before help is updated, the help contract fails because the flag is absent.

- [ ] **Step 3: Add parsing and help output**

Add the new field to `ServeArgs`:

```rust
    pub(crate) max_conns_per_ip: Option<String>,
```

Add this parser arm immediately after `--workers`:

```rust
            "--max-conns-per-ip" => {
                i += 1;
                parsed.max_conns_per_ip =
                    Some(take_utf8_value(args, i, "--max-conns-per-ip")?);
            }
```

Update `USAGE` and the serve-help usage line to include:

```text
[--max-conns-per-ip <N>]
```

Add this help entry after `--workers`:

```rust
    {max_conns_per_ip_flag} {n_val}
                            Per-IP concurrent-connection cap (between 1 and workers + 1;
                            default: max(1, workers / 8)). Excess connections are dropped.
```

Add the matching format argument:

```rust
        max_conns_per_ip_flag = styler.green("--max-conns-per-ip"),
```

Add `max_conns_per_ip: None,` to every pre-existing `ServeArgs` literal in `src/cli.rs` and `src/config.rs`. Keep the explicit parser test's value as `Some("8".to_string())`.

- [ ] **Step 4: Run the CLI and help tests**

Run:

```bash
cargo test cli::tests
cargo test --test check_contract help_screen_triggers_and_content
cargo test --test check_contract error_output_includes_help_hint
```

Expected: all CLI unit tests pass and both help/usage contract tests pass.

- [ ] **Step 5: Commit the CLI surface**

```bash
git add src/cli.rs src/config.rs tests/check_contract.rs
git commit -m "feat(cli): add per-IP connection cap flag"
```

### Task 3: Resolve and Validate the Worker-Dependent Configuration

**Files:**
- Modify: `src/config.rs`
- Modify: `src/server.rs` (mechanical `ServeConfig` test-literal update only)

- [ ] **Step 1: Write failing default, override, and range tests**

Extend `config::tests::validates_serve_defaults` so its expected `ServeConfig` includes:

```rust
                max_conns_per_ip: 16,
```

In `config::tests::validates_serve_overrides`, set:

```rust
                max_conns_per_ip: Some("2".to_string()),
```

and expect:

```rust
                max_conns_per_ip: 2,
```

Add these tests:

```rust
    #[test]
    fn derives_max_conns_per_ip_default_from_workers() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: Some("4".to_string()),
                max_conns_per_ip: None,
                write_timeout: None,
            }),
            1000,
        )
        .expect("valid serve config");

        let RunMode::Serve(config) = mode else {
            panic!("expected serve config");
        };
        assert_eq!(config.max_conns_per_ip, 1);
    }

    #[test]
    fn validates_max_conns_per_ip_against_workers_plus_one() {
        let site = TempSite::new();
        let mode = validate_with_euid(
            Command::Serve(ServeArgs {
                root: Some(site.path().to_path_buf()),
                listen: None,
                workers: Some("1024".to_string()),
                max_conns_per_ip: Some("1025".to_string()),
                write_timeout: None,
            }),
            1000,
        )
        .expect("workers + 1 is the neutralizing maximum");

        let RunMode::Serve(config) = mode else {
            panic!("expected serve config");
        };
        assert_eq!(config.max_conns_per_ip, 1025);
    }

    #[test]
    fn rejects_max_conns_per_ip_outside_worker_dependent_range() {
        let site = TempSite::new();
        for value in ["0", "6"] {
            let err = validate_with_euid(
                Command::Serve(ServeArgs {
                    root: Some(site.path().to_path_buf()),
                    listen: None,
                    workers: Some("4".to_string()),
                    max_conns_per_ip: Some(value.to_string()),
                    write_timeout: None,
                }),
                1000,
            )
            .unwrap_err();
            assert_eq!(
                err.message(),
                format!("--max-conns-per-ip '{value}': expected a value from 1 to 5")
            );
        }
    }
```

- [ ] **Step 2: Run the config tests to verify they fail**

Run: `cargo test config::tests`

Expected: compilation fails because `ServeConfig` has no `max_conns_per_ip` field.

- [ ] **Step 3: Implement dependent resolution and validation**

Add this field to `ServeConfig`:

```rust
    pub(crate) max_conns_per_ip: u32,
```

Replace `validate_serve` with:

```rust
fn validate_serve(args: ServeArgs) -> Result<ServeConfig, CliError> {
    let root = validate_root(args.root)?;
    let listen = validate_listen(args.listen)?;
    let workers = validate_workers(args.workers)?;
    let max_conns_per_ip = validate_max_conns_per_ip(args.max_conns_per_ip, workers)?;
    let write_timeout = Duration::from_secs(validate_write_timeout(args.write_timeout)?);

    Ok(ServeConfig {
        root,
        listen,
        workers,
        max_conns_per_ip,
        write_timeout,
    })
}
```

Add this validator after `validate_workers`:

```rust
fn validate_max_conns_per_ip(raw: Option<String>, workers: usize) -> Result<u32, CliError> {
    let default = u32::try_from((workers / 8).max(1)).expect("validated workers fit in u32");
    let max = u32::try_from(workers + 1).expect("validated workers + 1 fit in u32");
    parse_range("--max-conns-per-ip", raw, default, 1, max, "")
}
```

Add `max_conns_per_ip: 2,` to the `ServeConfig` literal in `server::tests::run_reports_bind_conflict`; this is `workers + 1` for its one-worker pool.

- [ ] **Step 4: Run the config tests and compile all targets**

Run: `cargo test config::tests && cargo test --all-targets --no-run`

Expected: config tests pass and every test target compiles.

- [ ] **Step 5: Commit resolved configuration**

```bash
git add src/config.rs src/server.rs
git commit -m "feat(config): validate per-IP cap against workers"
```

### Task 4: Enforce the Cap in the Accept Loop

**Files:**
- Modify: `src/server.rs`

- [ ] **Step 1: Write the failing server integration test**

Add `use std::time::Instant;` to the server test module, then add:

```rust
    #[test]
    fn per_ip_cap_refuses_excess_and_releases_permits() {
        let site = TempSite::new();
        site.write("a.txt", b"hi\n");
        let root = Root::open(site.path()).expect("open root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            let _ = serve(
                listener,
                root,
                serve_settings(4, 2),
                &NEVER_SHUTDOWN,
                -1,
            );
        });

        let mut first = TcpStream::connect(addr).expect("first held connection");
        let _second = TcpStream::connect(addr).expect("second held connection");

        let mut refused = TcpStream::connect(addr).expect("excess connection");
        refused
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set refused read timeout");
        let mut refused_body = Vec::new();
        refused
            .read_to_end(&mut refused_body)
            .expect("excess connection should close");
        assert!(refused_body.is_empty(), "refusal must write no response body");

        first.write_all(b"a.txt\n").expect("finish first request");
        first.shutdown(Shutdown::Write).expect("shutdown first write");
        let mut first_body = Vec::new();
        first.read_to_end(&mut first_body).expect("read first response");
        assert_eq!(first_body, b"hi\n");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let response = request(addr, b"a.txt\n");
            if response == b"hi\n" {
                break;
            }
            assert!(response.is_empty(), "retry must be served or silently refused");
            assert!(Instant::now() < deadline, "released permit was not reusable");
            thread::yield_now();
        }
    }
```

- [ ] **Step 2: Run the server test to verify it fails**

Run: `cargo test server::tests::per_ip_cap_refuses_excess_and_releases_permits`

Expected: compilation fails because `serve_settings` and `ServeSettings` do not exist; after the settings wrapper exists but before enforcement, the excess connection waits or is served instead of closing silently.

- [ ] **Step 3: Thread the configured cap into `serve`**

Import the limiter:

```rust
use crate::limiter::{ConnPermit, PerIpLimiter};
```

Update the module-level concurrency description so it distinguishes active workers from the one accepted stream held by the sender:

```rust
//! `Arc`. A rendezvous `sync_channel` plus exactly `workers` threads caps active
//! handlers at `workers`; the single accept loop can hold one additional accepted
//! stream while blocked in `send`. There is no unbounded spawn-per-connection and
//! no async runtime.
```

Add a private settings value before `serve`:

```rust
#[derive(Clone, Copy)]
struct ServeSettings {
    workers: usize,
    max_conns_per_ip: u32,
    read_timeout: Duration,
    write_timeout: Duration,
}
```

Construct it in `run` from `config.workers`, `config.max_conns_per_ip`, the
hardcoded read timeout, and `config.write_timeout`, then pass it to `serve` in
place of those four separate arguments. Use the same wrapper in direct server
tests, with `workers + 1` as the neutralizing cap where limiting is not under
test.

Add this helper to the server test module:

```rust
fn serve_settings(workers: usize, max_conns_per_ip: u32) -> ServeSettings {
    ServeSettings {
        workers,
        max_conns_per_ip,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
    }
}
```

Inside `serve`, construct the limiter and change the channel payload:

```rust
    let limiter = Arc::new(PerIpLimiter::new(settings.max_conns_per_ip));
    // The zero-capacity channel allows at most `workers` handled connections
    // plus one accepted connection parked in `send`.
    let (tx, rx) = mpsc::sync_channel::<(TcpStream, ConnPermit)>(0);
```

- [ ] **Step 4: Acquire before dispatch in both accept-loop variants**

Replace the Unix accept success arm with:

```rust
                Ok((stream, peer)) => {
                    let Some(permit) = limiter.try_acquire(peer.ip()) else {
                        continue;
                    };
                    stream.set_nonblocking(false)?;
                    if tx.send((stream, permit)).is_err() {
                        break; // all workers have gone away
                    }
                }
```

Replace the non-Unix incoming success arm with:

```rust
                Ok(stream) => {
                    let Ok(peer) = stream.peer_addr() else {
                        continue;
                    };
                    let Some(permit) = limiter.try_acquire(peer.ip()) else {
                        continue;
                    };
                    if tx.send((stream, permit)).is_err() {
                        break; // all workers have gone away
                    }
                }
```

In both branches, `continue` drops the excess `TcpStream` immediately and writes no bytes. On Unix, acquire before `set_nonblocking(false)`: refused connections avoid that syscall, and a socket-mode error cannot terminate the server for a connection that was going to be dropped. If the call fails after acquisition, returning via `?` drops the local RAII permit.

- [ ] **Step 5: Hold each permit for the full worker iteration**

Change `worker_loop` to receive the tuple and bind the permit for the duration of `conn::handle`:

```rust
fn worker_loop(
    rx: &Mutex<Receiver<(TcpStream, ConnPermit)>>,
    root: &Root,
    read_timeout: Duration,
    write_timeout: Duration,
) {
    crate::signal::block_signals_on_current_thread();
    loop {
        // Hold the lock only across `recv`; release it before handling so other
        // workers can pick up the next connection. Recover from a poisoned lock
        // rather than panicking, so one failed worker cannot collapse the pool.
        let (stream, _permit) = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
            Ok(item) => item,
            Err(_) => return, // channel closed: shut the worker down
        };
        // Connection-level errors are silent; the connection is simply dropped.
        // `_permit` drops at the end of this iteration, including during unwind.
        let _ = conn::handle(stream, root, read_timeout, write_timeout);
    }
}
```

- [ ] **Step 6: Update existing server tests to use the neutralizing maximum**

For every existing direct `serve` call with `workers = 4`, replace the separate worker/timeouts arguments with `serve_settings(4, 5)`. This includes `serves_files_over_loopback`, `handles_many_concurrent_clients`, `shutdown_flag_stops_accept_loop`, `in_flight_request_completes_before_shutdown`, and `signal_pipe_wakes_accept_loop_without_a_connection`.

The 16-client test must specifically use `5`, not `16`: `workers + 1` proves the limit is neutralized while the kernel backlog continues to queue the remaining clients.

- [ ] **Step 7: Run focused server and regression tests**

Run: `cargo test server::tests`

Expected: all server tests pass, including silent over-cap refusal, permit reuse, the 16-client neutralization case, and shutdown behavior.

Run:

```bash
cargo test limiter::tests
cargo test config::tests
cargo test cli::tests
```

Expected: all limiter, config, and CLI tests pass.

- [ ] **Step 8: Commit server enforcement**

```bash
git add src/server.rs
git commit -m "feat(server): enforce per-IP connection cap"
```

### Task 5: Update Security and Maintainer Documentation

**Files:**
- Modify: `SECURITY.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Document the concurrent cap and rate-limit boundary**

In `SECURITY.md`, add this threat-model bullet after `--workers`:

```markdown
- **`--max-conns-per-ip`** caps concurrent connections from one source IP; excess connections are closed silently before they consume a worker. The default is `max(1, workers / 8)` and the maximum `workers + 1` neutralizes the cap for reverse-proxy deployments.
```

Replace the stale out-of-scope rate-limit bullet with:

```markdown
- Connection-rate limiting (connections per unit time) beyond the concurrent caps — firewall policy is the right layer
```

- [ ] **Step 2: Update maintainer guidance and issue status**

In `CLAUDE.md`, add `limiter` to the implemented module list near the top.

Replace the open #29 roadmap entry with this completed entry beside the completed #27 note:

```markdown
(**[#29 — Abuse resistance: per-IP connection/rate limits](https://github.com/jonathandeamer/buffetcar/issues/29)** is **done** — the host firewall enforces per-source concurrent and new-connection-rate limits in production, while the in-app `--max-conns-per-ip` cap makes the binary safe-by-default without a firewall. The application cap is concurrent-resource accounting only; rate limiting remains at the firewall layer.)
```

Keep issue #28 as the remaining open production-readiness item and retain the human-merge requirement for this branch because it changes the threat model.

- [ ] **Step 3: Run documentation consistency checks**

Run:

```bash
rg -n "max-conns-per-ip|#29|Rate limiting|rate limiting|worker" SECURITY.md CLAUDE.md docs/superpowers/specs/2026-07-01-per-ip-connection-cap-design.md
git diff --check
```

Expected: every in-app cap reference describes `workers + 1` as the neutralizing maximum; rate limiting remains explicitly firewall-owned; `git diff --check` exits successfully.

- [ ] **Step 4: Commit documentation**

```bash
git add SECURITY.md CLAUDE.md
git commit -m "docs(security): document in-app per-IP cap"
```

### Task 6: Run the Full Gate and Prepare Human Review

**Files:**
- Verify all changed files.

- [ ] **Step 1: Run the repository gate**

Run: `make check`

Expected: formatting check, clippy with `-D warnings`, and the complete default test suite all pass.

- [ ] **Step 2: Verify scope and history**

Run:

```bash
git status --short
git diff --stat origin/main...HEAD
git log --oneline origin/main..HEAD
```

Expected: only the approved limiter, CLI/config, server, tests, and documentation files are changed; commits are conventional and focused. The plan file may remain as an additional docs change if it is intentionally included on the branch.

- [ ] **Step 3: Stop before shipping**

Do not push, open a pull request, or merge without explicit user approval. When shipping is authorized, open a PR and leave the merge to the human because this change modifies the worker-exhaustion threat model. Review CodeQL findings before that human merge, as required for code changes by `CLAUDE.md`.
