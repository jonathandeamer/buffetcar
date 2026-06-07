# Buffetcar

`buffetcar` is a hardened, single-binary **Nex** smallnet protocol server written in Rust.

## Publishing Rules

`buffetcar` enforces strict security invariants on all served files and directories. You can verify your files using the `buffetcar check` command.

To be servable by `buffetcar`:
- **Regular files** must be regular files, world-readable (`0o004` or otherwise readable by the daemon user), on the same device as the root directory, and have a hardlink count of `1`.
- **Directories** must be world-executable (`0o001` or otherwise executable by the daemon user) to allow descent/traversal.
- **Directory listings** will be generated only if the directory is also world-readable (`0o004` or readable by the daemon user).
- **Symlinks, hardlinks (count > 1), dotfiles (starting with `.`), and special files** (FIFOs, sockets, block/character devices) are never served or traversed, and are skipped in directory listings.
- **Mount crossings** (paths crossing to other filesystems/devices) are rejected.

### OpenBSD Directory Traversal Behavior

On Linux, macOS, FreeBSD, and NetBSD, `buffetcar` uses execute-only directory descriptors (`O_PATH` or `O_SEARCH`) to traverse directories. This allows the server to descend through directories that are world-executable (`--x`) but not world-readable (`---`).

OpenBSD does not support execute-only directory open flags. On OpenBSD, `buffetcar` falls back to opening directories `O_RDONLY` during traversal. Consequently, **every directory in the served tree on OpenBSD must be readable (`r-x`) by the daemon process for traversal to succeed.** An execute-only directory tree that is traversable on other platforms will return `document not found` on OpenBSD.

## Sandboxing

On OpenBSD, `buffetcar` applies the `pledge(2)` and `unveil(2)` system calls at startup to sandbox the network daemon:
- **`unveil`** restricts the process's filesystem view exclusively to the configured `--root` directory (read-only) and immediately locks the configuration.
- **`pledge`** restricts system calls to `"stdio rpath inet"`, allowing only standard I/O/threading, read-only file access under the unveiled root, and basic networking (bind/listen/accept).
