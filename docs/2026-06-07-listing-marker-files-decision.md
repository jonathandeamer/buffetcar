# Decision: Do Not Implement Listing Marker Files (`.modified`, `.desc`, `.header`)

Date: 2026-06-07
Status: decided; reaffirms prior design
Working name: buffetcar

## Decision

buffetcar will **not** implement the Go reference server's directory-listing
marker files: `.modified` (sort entries by mtime, newest first), `.desc`
(reverse-alphabetical sort), or `.header` (prepend a file's contents to the
listing). Generated listings remain deterministic, sorted ascending by displayed
name, with no in-tree file able to change ordering or inject content.

## Status of the prior decision

This is not a new decision. The original designs already declined the two
markers that existed at the time:

- `2026-06-05-buffetcar-design.md` ("No `.desc` reverse-ordering. … `.desc` is a
  dotfile and is rejected by policy.")
- `2026-06-06-multi-user-nex-server-design.md` ("The Go reference server's
  `.modified` and `.desc` marker files are not carried over. They are dotfiles,
  and dotfiles are structurally unreachable.")

This log records a **deliberate re-evaluation** of that decision — prompted by
re-reading the reference implementation — and reaches the **same conclusion**,
now also covering `.header`, which did not exist when the original designs were
written.

## What changed since the original decision

The reference protocol library `nex-pfm` has gained a third marker since the
designs were written. Verified against a fresh clone of
`https://hg.sr.ht/~m15o/nex-pfm`:

- `.modified` and `.desc` — added in `bf1597c0fd1a`, released as **v0.1.0**
  (2023-07-23).
- `.header` — added in `a2cdebbab2d4` ("support .header files"), released as
  **v0.1.1** (2024-04-16). It reads the directory's `.header` file and prepends
  its raw contents to the generated listing.

The `nexd` reference server pins `nex-pfm v0.1.0`, so `.header` is absent from
the locally built reference and from the `nexd_contract` characterization suite.
The original designs predate it.

## Re-evaluation

### Mechanism conflict (applies to all three)

All three markers work by having the server **notice or read a dotfile** in the
directory being listed. buffetcar holds dotfiles as *structurally unreachable*:
never opened, never listed, never acted upon. Honoring any marker requires
carving a named exception into that invariant. The markers are also, in effect,
**per-directory configuration files** — in-tree control surfaces — which the
project deliberately does not have (there is no flag or file to change index
name, follow symlinks, serve dotfiles, or disable timeouts; safety properties
are invariants, not configurable defaults). Deterministic, content-independent
listing output is part of the same posture.

### Per-marker security assessment

- **`.desc` — information-neutral.** Reverse-alphabetical is a pure reordering of
  names the listing already exposes. No new information, no new input handling
  beyond "does this regular file exist." Safe on its own merits.

- **`.modified` — low-severity metadata side channel.** The Nex protocol exposes
  no metadata: it serves bytes and closes; a client cannot `stat` anything.
  mtime-ordering makes the rendered order encode the relative modification times
  of every entry — observable activity/freshness data (which file changed most
  recently, update cadence) that the protocol otherwise never reveals.

- **`.header` — content/link injection; heaviest of the three.** It emits the
  raw bytes of an in-tree file into a server-generated page, including arbitrary
  `=> ` links to *any* destination (other hosts, arbitrary selectors) and
  free-form prose — neither of which a filename-based listing entry can express.
  It is also the largest puncture of the dotfile invariant: a dotfile's *content*
  is served. A faithful port would additionally have to bound its size against
  the listing byte cap and read it under the no-follow / regular-file /
  world-readable discipline rather than the reference's symlink-following whole
  read.

### The cross-tenant nuance (why the security case alone is not decisive)

In a shared/writable directory, an attacker who can write a marker can already
create entries, control their content, and overwrite files — so the markers add
*amplification within an already-lost trust boundary*, not a new attacker or a
server-invariant breach (containment, no-follow, and no-escape all still hold).
And in buffetcar's intended layout — untrusted users each publishing under their
*own* directory — a marker only affects the owner's own listing, which is benign.
A genuinely shared-writable served directory is a misconfiguration in which
listing integrity is already conceded.

So the deciding factor is **not** a concrete new exploit. It is **auditability
and invariant simplicity**: buffetcar buys its security with absolute, one-line
properties — "every unavailable selector returns identical `document not found`,"
"dotfiles are inert and never served," "listing output is deterministic with no
in-tree control surfaces." Each marker converts one of those absolutes into a
conditional ("…except `.header`, whose bytes are emitted"; "…deterministic unless
a marker is present"). Every conditional makes the security argument harder to
state airtight. That cost is paid for the *entire* server's posture, not just for
the feature, and it outweighs the modest convenience the markers provide.

## Consequences

- Listings stay deterministic and ascending-by-name; the dotfile invariant
  remains absolute (no named exceptions).
- buffetcar diverges from `nex-pfm` v0.1.1 on three documented points. The
  `nexd_contract` suite's marker behavior remains pinned as *reference behavior
  buffetcar deliberately does not match*, not as behavior to preserve.
- If this is ever revisited, `.desc` is the only marker that is
  information-neutral and would be the least-objectionable to adopt; `.header`
  is the most consequential and should be treated as a separate, larger decision
  on its own.

## Alternative approaches explored

Following reaffirmation of the above, three designs were explored to understand
whether equivalent per-directory customisation (sort order, header text) could
be provided without the dotfile or in-tree-control-surface cost.

### Approach A — offline generation tool (pre-baked)

A `buffetcar gen` subcommand or standalone tool that users run in their
directory. It reads a config (anywhere on disk; the server never sees it),
generates a standard `index` file with entries sorted as desired and any header
prepended, and writes it to disk. The server is completely unchanged — it serves
the `index` file as it always has. All three invariants remain absolute.

Trade-off: listings do not update automatically; the user must re-run the tool
(a post-commit hook or watch script covers most deployments). Not ruled out as a
future external tool, but not part of the server.

### Approach B — out-of-tree sidecar (live)

A `--listing-meta PATH` flag pointing to a directory outside the served root
that mirrors its structure. Per-directory config files live there, not in the
served tree. All in-tree invariants hold.

Trade-off: users need write access to their own sidecar path, which the operator
must provision. A default path (e.g. `~/.config/buffetcar/listings/`) reduces
flag friction but does not provide true self-service in a multi-user deployment —
the server runs as a single service user, so `~` is that user's home, not each
content publisher's. Not implemented: operational complexity outweighs the
benefit given the `index` file alternative.

### Approach C — named non-dot in-tree config file (live, self-service)

A reserved filename (e.g. `_listing`) inside a directory. The server reads it at
listing time, never serves it in response to a selector, and never lists it.
Sort preference and optional header text are drawn from it.

This is the closest analog to the Go markers — a regular non-dot file with
explicit server-side exclusion. It preserves the dotfile invariant by letter but
explicitly relaxes "no in-tree control surfaces" to "no in-tree control surfaces
*except `_listing`*." That is one named exception rather than three dotfiles, but
it is still a conditional: predicting a listing now requires inspecting
`_listing` files in addition to directory contents, and the server acquires a new
parser in the hot path.

Header content is the heaviest part: the server injects user-controlled bytes
into a server-generated page. Even plain-text-only, a user can write content that
appears to come from the server; with `=> ` links allowed, arbitrary link
injection to external destinations becomes possible. A sort-only subset is more
defensible but still costs the invariant.

Not implemented: the only gap Approach C fills over writing an `index` file
manually is *live* listing updates without any tool invocation — a convenience
feature. Paying a permanent auditability tax on the server's security posture
for a convenience feature is inconsistent with the project's character.

### Why the existing model is sufficient

The `index` file mechanism already covers every legitimate use case. A user who
wants custom listing order or a preamble writes their `index` file; the server
serves it verbatim. The listing model stays a one-liner. A future external
generation tool (Approach A) could automate `index` maintenance without touching
the server at all.

## References

- `docs/superpowers/specs/2026-06-05-buffetcar-design.md` (original `.desc`
  decision)
- `docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md` (original
  `.modified`/`.desc` decision)
- `nex-pfm` upstream: `https://hg.sr.ht/~m15o/nex-pfm` (`handler.go`)
- `tests/nexd_contract.rs` (`nexd_reverses_directory_listings_when_desc_marker_exists`)
