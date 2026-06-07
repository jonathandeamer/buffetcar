//! Parse and lexically normalize a Nex selector into safe path components.
//!
//! This module performs no filesystem access. It produces a list of *normal*
//! components (never empty, `.`, or `..`) plus directory intent. Lexical `..`
//! balancing is sound only because the resolver in `root` never follows a
//! symlink, so the lexical parent of a component is always its physical parent.

/// Hardcoded selector byte bound (spec: "Networking And Resource Bounds").
const MAX_SELECTOR_BYTES: usize = 1024;

/// A normalized request: normal path components in order, plus whether a
/// trailing slash expressed directory intent (which forbids a regular-file
/// resolution).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) components: Vec<String>,
    pub(crate) dir_only: bool,
}

/// Parse a selector into a `Request`, or `None` when it is unavailable by policy:
/// over the byte bound, containing a NUL or CR, naming a dotfile component, or
/// escaping above the root via unbalanced `..`.
pub(crate) fn parse(selector: &str) -> Option<Request> {
    // Strip exactly one trailing CR before the length check so that a 1024-byte
    // path sent with Windows CRLF line endings (1025 wire bytes) is accepted.
    let selector = selector.strip_suffix('\r').unwrap_or(selector);
    if selector.len() > MAX_SELECTOR_BYTES {
        return None;
    }
    // Reject NUL and any remaining CR (e.g. double-CR from a misbehaving client).
    if selector.contains('\0') || selector.contains('\r') {
        return None;
    }

    let dir_only = selector.ends_with('/');
    let mut components = Vec::new();
    for raw in selector.split('/') {
        match raw {
            "" | "." => continue,
            ".." => {
                components.pop()?;
            }
            name => {
                if name.starts_with('.') {
                    return None;
                }
                components.push(name.to_owned());
            }
        }
    }

    Some(Request {
        components,
        dir_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(components: &[&str], dir_only: bool) -> Option<Request> {
        Some(Request {
            components: components.iter().map(|s| s.to_string()).collect(),
            dir_only,
        })
    }

    #[test]
    fn empty_and_slash_resolve_to_root_directory() {
        assert_eq!(parse(""), req(&[], false));
        assert_eq!(parse("/"), req(&[], true));
    }

    #[test]
    fn trims_leading_and_interior_empty_components() {
        assert_eq!(parse("/plain.txt"), req(&["plain.txt"], false));
        assert_eq!(parse("a//b"), req(&["a", "b"], false));
    }

    #[test]
    fn trailing_slash_sets_directory_intent() {
        assert_eq!(parse("docs/"), req(&["docs"], true));
        assert_eq!(parse("docs"), req(&["docs"], false));
    }

    #[test]
    fn balances_parent_components_and_rejects_escape() {
        assert_eq!(parse("a/b/../c.txt"), req(&["a", "c.txt"], false));
        assert_eq!(parse("../outside"), None);
        assert_eq!(parse("a/../../escape"), None);
    }

    #[test]
    fn rejects_dotfile_components_before_parent_cancellation() {
        assert_eq!(parse(".secret"), None);
        assert_eq!(parse("listing/.hidden"), None);
        assert_eq!(parse(".secret/../public"), None);
    }

    #[test]
    fn rejects_nul_and_oversized_selectors() {
        assert_eq!(parse("a\0b"), None);
        let oversized = "a".repeat(1025);
        assert_eq!(parse(&oversized), None);
        let at_limit = "a".repeat(1024);
        assert_eq!(parse(&at_limit), req(&[&at_limit], false));
        // 1024-byte path + trailing CR = 1025 wire bytes: CR is stripped first,
        // leaving 1024 bytes at the limit → accepted.
        let at_limit_cr = format!("{}\r", "a".repeat(1024));
        assert_eq!(parse(&at_limit_cr), req(&[&"a".repeat(1024)], false));
    }

    #[test]
    fn tolerates_one_trailing_carriage_return() {
        assert_eq!(parse("plain.txt\r"), req(&["plain.txt"], false));
        // double-CR: stripping one leaves an embedded CR → rejected
        assert_eq!(parse("plain.txt\r\r"), None);
    }
}
