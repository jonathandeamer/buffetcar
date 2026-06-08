# OpenBSD CI Hang Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the server's Unix accept loop in `src/server.rs` by checking `libc::poll` `revents` to prevent infinite CPU spin/starvation hangs on OpenBSD target platforms.

**Architecture:** Update the Unix `libc::poll` loop to inspect the returned `revents` mask. Treat `POLLERR`, `POLLHUP`, and `POLLNVAL` on the listening fd as fatal, and ignore non-`POLLIN` events (by continue-ing the loop) to prevent spurious event busy-loops.

**Tech Stack:** Rust, `libc`

---

## File Structure

- Modify: [src/server.rs](file:///Users/jonathan/nex-server/src/server.rs)
  - Inspect `pollfd.revents` after calling `libc::poll`.
  - Bubble up fatal errors/hangups.
  - Skip non-`POLLIN` readiness events.

---

### Task 1: Harden the Unix Accept Loop in `src/server.rs`

**Files:**
- Modify: [src/server.rs](file:///Users/jonathan/nex-server/src/server.rs)

- [ ] **Step 1: Implement `revents` checking**

Modify `src/server.rs` around line 114 to check `pollfd.revents`.

Target code in [src/server.rs](file:///Users/jonathan/nex-server/src/server.rs#L114-L122):
```rust
            let res = unsafe { libc::poll(&mut pollfd, 1, -1) };
            if res < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }

            match listener.accept() {
```

Replacement code:
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

            match listener.accept() {
```

- [ ] **Step 2: Run local checks to verify correctness**

Run: `make check`
Expected: PASS

- [ ] **Step 3: Commit the changes**

Run:
```bash
git add src/server.rs
git commit -m "fix(server): check poll revents to prevent busy-loops on OpenBSD"
```
