# Multi-User Nex Server Design

Date: 2026-06-06
Status: draft for review
Working name: buffetcar

This is a first-principles design for a Rust Nex server whose primary deployment
target is a multi-user Unix host. It keeps Nex small: one selector line in, one
text or binary response out, then close. The security target is stronger than
the earlier static-site design: local untrusted users may mutate content inside
the served tree while the daemon is resolving a request.

## Core Principle

Safety/security and minimalism are equal priorities. The ideal design removes
features until the safest behavior is also the simplest behavior. When a real
tension remains, prefer security.

Consequences:

- There is no config file, access log, TLS layer, virtual hosting, upload path,
  dynamic content, cache, or plugin system.
- Safety properties are invariants, not defaults. There is no flag to serve
  dotfiles, follow symlinks, increase the selector bound, change the index name,
  disable timeouts, or run as root.
- The filesystem resolver is allowed to be more complex than the protocol. In a
  multi-user threat model, check-then-open path validation is not safe enough.

## Threat Model

The server must remain safe when:

- multiple local users can create, delete, rename, chmod, and replace files in
  their publishable subtrees;
- a local user races selector resolution by swapping a regular file for a
  symlink, directory, FIFO, device node, or hardlink;
- a local user creates symlinks, hardlinks, huge directories, unreadable files,
  or malformed names inside the served root;
- remote clients send slow, oversized, invalid, or disconnected requests.

The server does not try to stop a user from intentionally publishing bytes that
the user is allowed to publish. It does prevent path traversal, symlink escape,
dotfile access, special-file access, hidden hardlink publication, mount crossing,
and publication of files that are not public by Unix mode bits.

## Nex Compliance

The wire behavior follows the Nex specification:

- listen on TCP port 1900 by default;
- read one selector line, which may be empty;
- respond with text or binary bytes, then close;
- retain no connection state;
- return documents as-is;
- treat directories as plain text maps using `=> ` links;
- generate relative links and append `/` to directory links;
- serve an `index` file when a directory has one.

Selectors are UTF-8 text. Invalid UTF-8, NUL bytes, and selectors longer than
1024 bytes are unavailable. Empty selectors resolve to the root directory.
Selectors ending in `/` express directory intent: they may resolve to a
directory, but not to a regular file. Selectors without a trailing `/` may
resolve to either a regular file or a directory. The server performs no URL
decoding.

Nex has no status codes. Missing files, rejected files, policy failures, invalid
selectors, oversized selectors, and root escapes all return the literal visitor
body `document not found` with no trailing newline. That message is intentionally
plain English but not reason-specific: remote visitors should know the selector
did not produce a document, but should not learn whether the cause was absence,
permissions, dotfile policy, symlink policy, mount crossing, or another safety
rule.

## Architecture

The daemon is a small blocking Unix server.

| Module | Responsibility |
| --- | --- |
| `cli` | Hand-parse `serve`/`check` modes and print usage/errors. |
| `config` | Validate root, listen address, worker count, and timeout bounds. |
| `server` | Bind `TcpListener`; run a fixed pool of worker threads. |
| `conn` | Apply socket timeouts, read one bounded selector, stream response. |
| `selector` | Parse/normalize selector bytes into safe path components. |
| `root` | Own the root directory fd and all fd-relative filesystem operations. |
| `listing` | Generate bounded directory listings from opened directory fds. |
| `sandbox` | OpenBSD `pledge`/`unveil`; no-op elsewhere. |

No module except startup receives the root path. After startup the program holds
a `Root` capability containing an opened root directory descriptor, and all file
access is relative to that descriptor.

The binary has two run modes:

- `buffetcar serve --root /var/nex [--listen 127.0.0.1:1900]` starts the Nex
  daemon. For convenience, omitting `serve` is equivalent to `serve`.
- `buffetcar check --root /var/nex <selector>...` runs local diagnostics for one
  or more selectors and exits without binding a socket.

## Filesystem Resolver

The resolver is the security core.

Startup opens `--root` as an absolute path with `O_DIRECTORY | O_CLOEXEC |
O_NOFOLLOW`; the final root component must not be a symlink. The root device id
is recorded with `fstat`.

Every request is resolved component by component:

1. `selector` trims one trailing CR, rejects invalid UTF-8, rejects NUL, splits
   on `/`, and classifies components before filesystem access.
2. Any original normal component beginning with `.` rejects the whole selector.
   This happens before lexical `..` handling, so a selector cannot hide a
   dotfile probe behind a later parent component.
3. Empty and `.` components are ignored. `..` above the root rejects the
   selector. Balanced `..` is allowed after lexical normalization, so relative
   Nex links such as `../nexlog/` still work once a client has resolved them
   into a selector.
4. Each remaining component is opened relative to the currently opened directory
   fd using `openat` with `O_NOFOLLOW`. Directories use `O_DIRECTORY`.
5. Every opened fd is checked with `fstat` before use.

The final result can only be:

- a regular file fd that passed all public-file checks;
- a directory fd that passed all public-directory checks;
- unavailable.

The network path maps every unavailable result to `document not found`. The
local `check` path preserves a concise internal reason for each unavailable
selector so operators and site authors can diagnose policy failures without
weakening remote behavior.

Whole-selector path opens are forbidden. `std::fs::File::open(path)`,
`cap_std::fs::Dir::open(path)`, and string-based path joins are not used in the
request path.

## Public Content Policy

These checks run on opened file descriptors, not on pathnames:

- Regular files must be regular, world-readable (`0o004`), on the root device,
  and have link count `1`.
- Directories must be directories, world-executable (`0o001`), and on the root
  device. A directory must be world-readable (`0o004`) to generate a listing.
- Symlinks are never followed or listed.
- Hardlinked regular files are never served. Hardlinks are not a Nex feature and
  rejecting them prevents a user from publishing an inode through a misleading
  in-tree name.
- FIFOs, sockets, block devices, character devices, and other special files are
  never served or listed.
- Crossing to another device is rejected. This blocks bind mounts, FUSE mounts,
  and removable media from silently becoming part of the station.

The implicit `index` lookup uses the same resolver policy as a typed selector,
with the hardcoded component `index`. A symlinked, hardlinked, special, unreadable,
or cross-device `index` is treated as missing; the directory then falls back to a
generated listing if listing policy permits.

## Directory Listings

If a directory has a safe `index`, the index bytes are streamed as-is.

Without an index, the server generates a listing only when the opened directory
is world-readable. It enumerates names from that directory fd, then opens each
candidate entry fd-relative and applies the same descriptor checks used for
direct requests. Entries are skipped if they are dotfiles, symlinks, hardlinks,
special files, non-UTF-8 names, cross-device entries, or not public by mode bits.

Listings are sorted by displayed name and rendered as:

```text
=> file.txt
=> subdir/
```

Generated listings are bounded by hardcoded constants: at most 4096 entries and
at most 256 KiB of rendered output. If a directory exceeds either bound, it is
unavailable. Site authors can provide an `index` file for large directories.

The Go reference server's `.modified` and `.desc` marker files are not carried
over. They are dotfiles, and dotfiles are structurally unreachable.

## Networking And Resource Bounds

The server uses blocking `std::net` with a fixed worker pool. The worker count
is the concurrency cap; no async runtime or thread-pool dependency is used.

Defaults and validation:

| Flag | Default | Validation |
| --- | --- | --- |
| `--root <PATH>` | required | absolute path, existing directory, final component not symlink |
| `--listen <ADDR>` | `127.0.0.1:1900` | parses as socket address |
| `--workers <N>` | `128` | `1..=1024` |
| `--write-timeout <SECS>` | `30` | `1..=300` |

Hardcoded invariants:

- selector bound: 1024 bytes;
- read timeout: 5 seconds;
- index name: `index`;
- generated listing bounds: 4096 entries, 256 KiB;
- no root execution. If effective UID is 0, both `serve` and `check` fail.

Files are streamed in fixed-size chunks; regular files are not read into memory
as one allocation. A stalled reader can hold one worker until the bounded
write-timeout fires.

## Logging And Operator UX

Startup succeeds with a compact stderr banner:

```text
buffetcar 0.1.0
  root:     /var/nex
  listen:   127.0.0.1:1900
  workers:  128
  timeouts: read 5s, write 30s
  policy:   no dotfiles, symlinks, hardlinks, special files, or mount crossing
  sandbox:  fd-relative containment (pledge/unveil active on OpenBSD)
```

Startup failures print one actionable `error:` line to stderr and exit non-zero.
They never panic, print a backtrace, or expose internal Rust type names in normal
operation.

There is no access log and no verbosity flag. The server never records client
IPs or selectors. Connection-level events such as timeout, disconnect, oversized
selector, invalid selector, and connection reset are silent. Rare server-side
I/O faults on already-opened descriptors may be logged without selector or
client data.

Shutdown uses default signal handling. No signal dependency is added just to
print a goodbye message.

## Error Message UX

Messages are part of the security model.

Visitor-facing messages follow these rules:

- The only server-generated error body sent over Nex is `document not found`.
- The body is used for every unavailable selector, including security-policy
  rejections and malformed selectors.
- Slow-client read timeouts, disconnected clients, write timeouts, and connection
  resets receive no extra explanatory text; the server closes the connection and
  continues.
- Reason-specific visitor messages are not added, because they would turn the
  server into a policy and filesystem oracle.

Server-user messages follow different rules because they are local and explicit:

- Usage and startup errors are specific, human, and actionable.
- Messages begin with `error:` and name the failing flag or operation.
- Operator-provided paths and addresses may be quoted; remote client selectors
  and client addresses are not written to daemon logs.
- Messages say what failed and, where useful, what to do next.
- Normal operation never prints a panic or backtrace.

Examples:

```text
error: --root is required
error: --root '/var/nex': not an absolute path
error: --root '/var/nex': not a directory
error: --root '/var/nex': final path component is a symlink
error: refusing to run as root; run buffetcar as an unprivileged service user
error: invalid --listen 'localhost:1900': expected an IP socket address
error: could not bind 127.0.0.1:1900: address already in use
error: --workers '0': expected a value from 1 to 1024
error: --write-timeout '999': expected a value from 1 to 300 seconds
```

## Local Diagnostics

Strict policy needs a local explanation path. `buffetcar check` uses the same
selector parser, root fd, resolver, index lookup, and listing eligibility logic
as the daemon, but prints one local result line per selector:

```text
ok: users/alice/index: regular file, public
ok: users/alice/nexlog/: directory, public listing
reject: users/alice/.secret: dotfile component
reject: users/alice/link: symlink
reject: users/alice/shared.txt: hardlink count 2
reject: users/alice/private.txt: not world-readable
reject: users/alice/nexlog/: directory is not world-executable
reject: users/alice/media: crosses filesystem boundary
```

`check` never contacts the network, never requires elevated privileges, and
never runs with weaker rules than `serve`. It is a local author/operator tool,
not a remote introspection feature. Result lines go to stdout. Usage and startup
errors go to stderr. Exit status is `0` when every selector is servable, `1`
when any selector is rejected, and `2` for usage/startup errors such as a
missing `--root`.

The README should present the publishing rules in the same terms as `check`:
regular files are `0644` or otherwise world-readable, directories are `0755` or
otherwise world-executable, listing directories are world-readable, and symlinks,
hardlinks, dotfiles, special files, and mount crossings are not part of the
servable Nex tree.

## Dependencies

Use a deliberately small Unix dependency set:

- `rustix` for safe wrappers around fd-relative Unix filesystem APIs;
- target-specific `libc` only for OpenBSD `pledge`/`unveil` FFI.

Do not use `cap-std` as the primary containment primitive in this design. It is
good for static-site containment, but the multi-user invariant is clearer when
the request path explicitly opens one component at a time with `O_NOFOLLOW` and
checks the resulting fd.

Do not use `clap`; the CLI surface is small enough to parse by hand and test.
This removes a large dependency tree from a network daemon.

Continue to use `cargo-deny`, dual MIT OR Apache-2.0 licensing, CI on Linux and
macOS, and OpenBSD compile/manual testing for the sandbox path.

## Testing

Contract tests cover Nex behavior:

- empty selector and `/` serve root directory behavior;
- files and binary files are streamed unchanged;
- directories serve safe `index` files;
- generated listings use `=> ` links and trailing `/` on directories;
- directory links are relative;
- directory listings omit dotfiles and non-UTF-8 names;
- trailing slash on a regular file is unavailable;
- missing/rejected selectors return `document not found`.

Containment and multi-user tests cover:

- `../` above root is rejected;
- balanced `..` remains usable;
- dotfile components are rejected before filesystem access;
- final and intermediate symlinks are rejected;
- symlinked `index` falls back to listing;
- symlink entries are omitted from listings;
- hardlinks to files outside the root are rejected;
- files with link count greater than one are rejected;
- FIFOs and device/special files are rejected or skipped;
- non-world-readable files are rejected even when the daemon user can read them;
- non-world-executable directories are rejected;
- non-world-readable directories do not generate listings;
- cross-device entries are rejected where test infrastructure can create them;
- a stress test repeatedly swaps a name between safe file, symlink, FIFO, and
  directory while requests run, and asserts that outside or special content is
  never served.

Server tests cover:

- selector length cap;
- invalid UTF-8 and NUL handling;
- read timeout for slow clients;
- write timeout for stalled readers;
- worker cap under many concurrent clients;
- loopback default bind;
- config validation;
- human startup/usage errors for invalid root, root execution, invalid listen
  address, bind failure, invalid worker count, and invalid write timeout;
- refusal to run as root, when testable without privileges.

Diagnostic tests cover:

- `check` returns `0` and prints `ok:` for servable files and directories;
- `check` returns `1` and prints stable `reject:` reasons for dotfiles,
  symlinks, hardlinks, special files, private modes, mount crossing, trailing
  slash mismatch, and oversized listings;
- `check` returns `2` for usage and startup errors;
- `check` and `serve` use the same resolver decisions for the same selector
  fixtures.
- visitor-facing protocol tests assert unavailable selectors all return exactly
  `document not found`, without reason-specific bodies.

## Non-Goals

- TLS, because Nex has no TLS by design.
- Authentication, authorization, uploads, CGI, reverse proxying, virtual hosts,
  metrics, hot reload, per-user quotas, rate limiting, or content-type mapping.
- Following symlinks or serving hardlinks.
- Supporting Windows as a multi-user hosting target in v1.
