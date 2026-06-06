# Security policy

## Supported versions

buffetcar is pre-1.0; only the latest release (and `main`) receives security fixes.

## Reporting a vulnerability

Please report security issues **privately** — not as a public issue or pull request.

- Preferred: GitHub [private vulnerability reporting](https://github.com/jonathandeamer/buffetcar/security/advisories/new) (the "Report a vulnerability" button on the repository's Security tab).
- Fallback: email `jonathandeamer@gmail.com`.

There is no formal SLA, but you can expect an acknowledgement within a few days. Credible reports will be investigated and fixed, and I am happy to credit you in the published advisory.

## Threat model

buffetcar's primary threat is an **untrusted Nex selector from an anonymous TCP client**. A client connects on port 1900, sends one line, and receives a response. The containment story is:

- **`cap-std`** makes every file open relative to the served root and structurally refuses `../` traversal and symlink escapes with no TOCTOU window — the OS kernel enforces this, not application logic.
- **No symlinks are followed** anywhere in resolution, so an in-root symlink cannot be used to bypass dotfile rejection or escape the root.
- **Dotfiles are an invariant**, not a default: there is no flag to enable serving them.
- **Request size is bounded** at a hardcoded constant; there is no way to send an unbounded selector.
- **Read timeout is hardcoded** at 5 s; a slow-writing client cannot hold a worker indefinitely.
- **`--max-conns`** caps the worker pool; connections beyond the cap queue in the kernel backlog and are then refused — no unbounded thread or memory growth.

On OpenBSD, `pledge`/`unveil` add a second OS-level wall around the live process.

### In scope

- Path traversal or symlink escape out of the served root
- Dotfile bypass (any path that causes a dotfile to be served)
- Selector parsing that causes unbounded memory allocation or hang (slowloris, amplification)
- A crafted selector that wedges or panics the server
- Worker exhaustion via connection or resource starvation beyond the configured cap
- Violations of the `cap-std` or `pledge`/`unveil` containment

### Out of scope

- Absence of TLS — the Nex protocol has none by design; buffetcar makes no claim about transport confidentiality
- Rate limiting beyond `--max-conns` — firewall policy is the right layer
- Issues that require pre-existing code execution on the server
- Issues only affecting a forked or modified build
