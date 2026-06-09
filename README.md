# buffetcar

A server for Nex, the minimal [small net](https://nightfall.city/nex/in/m15o/notes/nex-and-small-net.txt) protocol.

Nex is a small protocol for publishing files over the internet ([spec](https://nightfall.city/nex/), TCP port 1900): a client sends one line naming what it wants, the server sends back the raw bytes of that file, or a generated listing if it named a directory, and closes the connection. There are no status codes, no content types, and no second request on the same connection. A listing is plain text whose links are lines beginning with `=> `.

## Try it

buffetcar serves its own source tree over Nex at `nex://nex.jonathandeamer.com/buffetcar/`. The whole protocol is one line of text, so you don't need a Nex client to look — you can speak it by hand. Ask for the source directory and you get a listing of this repo:

```
$ printf '/buffetcar/\n' | nc nex.jonathandeamer.com 1900
=> CLAUDE.md
=> CONTRIBUTING.md
=> Cargo.lock
=> Cargo.toml
...
=> docs/
=> src/
=> tests/
```

Note what isn't there: the repo's `AGENTS.md` is a symlink, so buffetcar leaves it out of the listing rather than follow it — the policy below, demonstrated live. Name a file that is served and you get its raw bytes, this very README served by the thing it describes:

```
$ printf '/buffetcar/README.md\n' | nc nex.jonathandeamer.com 1900
# buffetcar
...
```

Point a Nex client like [Lagrange](https://skyjake.fi/@lagrange) at `nex://nex.jonathandeamer.com/buffetcar/` to browse it properly.

## What buffetcar does

Point buffetcar at a directory and it serves that tree over Nex. A request for a file returns the file. A request for a directory returns its `index` file if one exists, or otherwise a listing of the directory's contents in name order. It is a single binary with no configuration files and nothing to tune for correctness.

Most of buffetcar's design is about what it refuses to do. The rules about what it will serve are fixed rather than configurable (see [What you can serve](#what-you-can-serve)). There is no option to follow symlinks, serve dotfiles, or run as root, because those are the choices that turn a file server into a way out of its own directory. The resolver opens each component of a request directly and refuses symlinks as it goes, so a request can never escape the root you gave it.

It is written in Rust on a `rustix` `openat`-based resolver; the design notes live in `docs/`.

## Install

buffetcar builds from source with a recent Rust toolchain. Clone the repo and build a release binary:

```
git clone https://github.com/jonathandeamer/buffetcar
cd buffetcar
cargo build --release
```

The binary lands at `target/release/buffetcar`. To put it on your `PATH` instead, run `cargo install --path .`.

Prebuilt binaries are coming.

## Usage

Serve a directory:

```
buffetcar serve --root /path/to/site
```

By default it listens on `127.0.0.1:1900`. Ask it for something with any TCP client, since a Nex request is just a line of text:

```
$ printf '/\n' | nc localhost 1900
=> about.txt
=> notes/
```

A bare `/` asks for the root. There is no `index` file here, so buffetcar returns a listing: `about.txt` is a file, `notes/` a subdirectory. Name a file to get its bytes:

```
$ printf 'about.txt\n' | nc localhost 1900
Hello from the small net.
```

For binding to other addresses, the worker count, and the write timeout, see `buffetcar serve --help`.

## What you can serve

buffetcar publishes only the files it can serve safely, and applies the same rules to every request. A file is served when it is world-readable and is a single name on disk rather than a hardlink to somewhere else. A directory is traversed when it is world-executable. Everything else is left out: any name beginning with a dot, symlinks, hardlinks, FIFOs and device files, and any path that crosses onto another filesystem. A request that breaks any rule gets the same `document not found` reply as one for a file that was never there, so a visitor learns nothing about what exists but is withheld.

You can check a path against these rules before publishing, without starting the server:

```
buffetcar check --root /path/to/site index about.txt notes/
```

For each selector it reports whether the file would be served, and why not when it wouldn't. Run `buffetcar check --help` for the full list of rules it verifies.

## What buffetcar is not

- A Nex client. It answers requests; it doesn't make them or browse other servers.
- Configurable into serving dotfiles, following symlinks, or running as root. Those rules are fixed.
- A gopher, gemini, or web server. It speaks Nex and nothing else.
- A way to write. It never accepts uploads or changes the files it serves.

## Contributing

Bug reports, ideas, and PRs are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Security issues go through [SECURITY.md](SECURITY.md) rather than a public issue.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option. © 2026 Jonathan Deamer.
