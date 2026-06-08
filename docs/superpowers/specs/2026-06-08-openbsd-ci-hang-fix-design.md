# Design Specification: OpenBSD CI VM Hang Resolution

**Date:** 2026-06-08  
**Status:** active design; pending user review  
**Working name:** openbsd-ci-hang-fix  

This document specifies the design for resolving the indefinite CI test hang observed on OpenBSD target platforms when running the `handles_many_concurrent_clients` test. The solution consists of hardening the server's Unix accept loop using `pollfd` revents checking to prevent CPU starvation on spurious wakeups.

---

## 1. Context & Root Cause Analysis

On the `feat/graceful-shutdown` branch, signal blocking and a non-blocking `poll`-based accept loop were introduced to support clean termination. While tests pass on Linux and macOS, the suite on OpenBSD hangs during the unit test phase on `handles_many_concurrent_clients`.

### Distinction from PR #25
This hang is distinct from the previous OpenBSD CI issue addressed in PR #25 (where tests failed because the VM ran as root, which violated the server's refusal to run as root). That issue was resolved by dropping privileges to the unprivileged `ci` user.

### Proposed Root Cause: Spurious Poll Busy-Loop
The observed hang is hypothesized to be caused by a tight CPU-consuming busy-loop in the server's accept loop:
* The current accept loop calls `libc::poll` with an infinite timeout (`-1`), but does not inspect the returned `revents` mask.
* If `poll` returns on a non-`POLLIN` event, the code immediately invokes `listener.accept()`.
* `listener.accept()` returns `ErrorKind::WouldBlock` (or another non-fatal socket error), and the loop continues, immediately re-invoking `libc::poll`.
* Because the underlying event status of the socket remains unchanged, `poll` returns immediately again. Under a single-core VM scheduler, this creates a 100% CPU spin that starves all client and worker threads, causing a CI timeout that manifests as a hang.

*Note: The exact bit triggering this loop in the OpenBSD environment is unconfirmed. Therefore, this change is framed as a best-effort hardening of the accept loop against spurious and persistent socket events.*

---

## 2. Hardening the Accept Loop (`src/server.rs`)

We will update the Unix `poll` block in `serve` to inspect the returned `revents` mask before invoking `accept()`.

### Event Handling Strategy
* **Fatal Socket Failures (`POLLERR` / `POLLHUP` / `POLLNVAL`)**: Any event indicating socket invalidity (`POLLNVAL`), socket-level error (`POLLERR`), or a listener hangup (`POLLHUP`) will be treated as fatal. Since a listening socket does not experience normal client-side hangups, any `POLLHUP` on the listener fd represents a fatal state transition (e.g. shutdown by the process or kernel). Treating these as fatal prevents a persistent non-`POLLIN` event from causing an infinite `continue` busy-loop.
* **Transient Non-`POLLIN` Events**: If `poll` returns with none of the fatal bits but without `POLLIN` set (a spurious wake), the loop will `continue`. Because transient events do not persist on the descriptor, a single `continue` is safe and will not spin.
* **Observability**: Currently, errors bubbled up from the loop are dropped silently in `run` (let `_ =`). The logging behavior for these fatal events will be addressed as part of the structured logging work in Issue #28.

### Implementation Snippet
```rust
            let res = unsafe { libc::poll(&mut pollfd, 1, -1) };
            if res < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }

            // Fatal error or hangup on the listening socket descriptor itself.
            // Treating POLLHUP/POLLERR as fatal prevents infinite busy-looping on persistent bits.
            if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "fatal listener socket error or hangup",
                ));
            }

            // Only proceed to accept if the socket has pending incoming connections
            if pollfd.revents & libc::POLLIN == 0 {
                continue;
            }
```

---

## 3. Verification Plan

* **Local Verification (Single-Core Pinning)**: On Linux, run the unit test suite pinned to a single CPU core (e.g., `taskset -c 0 cargo test`) to verify that the accept loop handles high parallel concurrency without spinning or starvation.
* **CI Verification**: Commit and push to the feature branch to trigger the OpenBSD CI run. Verify that the `fmt · clippy · test (openbsd)` job (which runs as a non-required check) completes successfully, confirming the hang is gone.
