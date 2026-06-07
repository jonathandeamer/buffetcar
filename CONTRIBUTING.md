# Contributing

buffetcar is a young hobby project, and contributions, bug reports, and ideas are all welcome. The most useful report is a selector that gets served when it shouldn't, or refused when it should be served: include the file's mode and type and how you reached it, since the policy is tested against a corpus of these cases. AI-assisted code is welcome too, as long as you've read and tested it yourself. The commit's yours, so skip the AI co-author trailers.

You'll need a recent Rust toolchain. To run the same checks CI does:

```
make check    # fmt --check, clippy -D warnings, test
make deny     # cargo deny (needs: cargo install cargo-deny)
make hooks    # installs the commit-message hook (once per clone)
```

Commits follow Conventional Commits (`fix(conn): ...`, `docs: ...`), and the hook checks the subject line. Bypass it once with `git commit --no-verify` if you need to.

Open an issue for bugs or ideas, and for anything bigger start one before a PR so we can sort out the approach. Security issues go through `SECURITY.md` rather than a public issue. The reasoning behind past decisions lives in `docs/` and `CLAUDE.md`; the authoritative design is `docs/superpowers/specs/2026-06-06-multi-user-nex-server-design.md`.

## Branching

The project is trunk-based: `main` is the only long-lived branch, and it's protected. Every change lands through a short-lived branch and a PR, even maintainer changes, so CI gates `main` rather than your memory. Merging requires three green checks — `fmt · clippy · test` on Linux and macOS, plus `cargo deny` — and force-pushes and deletions are blocked, so published history stays stable. Keep local commits green with `make check` before pushing, but treat CI as the gate that actually guards `main`.
