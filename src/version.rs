//! Version string formatting.
//!
//! The build script sets `GIT_DESCRIBE` from `git describe --tags --dirty --always`.
//! This module parses that output into the human-friendly version line printed
//! by `--version`, `version`, the help header, and the startup banner.

/// The version line printed by `--version` / `version`, and embedded in the
/// help header and startup banner.
///
/// Format examples:
/// - At tag:           `buffetcar 0.1.0`
/// - At tag, dirty:    `buffetcar 0.1.0 (dirty)`
/// - After tag:        `buffetcar 0.1.0 (3 commits after v0.1.0, gabcdef1)`
/// - After tag, dirty: `buffetcar 0.1.0 (3 commits after v0.1.0, gabcdef1, dirty)`
/// - No git / no tags: `buffetcar 0.1.0`  (CARGO_PKG_VERSION fallback)
pub(crate) fn version_line() -> &'static str {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| {
        let cargo_version = env!("CARGO_PKG_VERSION");
        let git_describe = option_env!("GIT_DESCRIBE").unwrap_or("");
        format_version("buffetcar", cargo_version, git_describe)
    })
}

/// Pure formatting function, testable without build-script side effects.
fn format_version(name: &str, cargo_version: &str, git_describe: &str) -> String {
    if git_describe.is_empty() {
        return format!("{name} {cargo_version}");
    }

    // `git describe --tags --dirty --always` output shapes:
    //   "v0.1.0"                        — exactly at tag
    //   "v0.1.0-dirty"                  — at tag, dirty
    //   "v0.1.0-3-gabcdef1"             — 3 commits after tag
    //   "v0.1.0-3-gabcdef1-dirty"       — 3 commits after tag, dirty
    //   "abcdef1"                        — no tags, just a hash
    //   "abcdef1-dirty"                  — no tags, dirty

    let dirty = git_describe.ends_with("-dirty");
    let base = if dirty {
        git_describe.trim_end_matches("-dirty")
    } else {
        git_describe
    };

    // Try to parse as tag with optional commit-count + hash.
    // A tag starts with 'v', e.g. "v0.1.0" or "v0.1.0-3-gabcdef1".
    if let Some(rest) = base.strip_prefix('v') {
        // Check for "version-count-ghash" pattern.
        if let Some((tag_version, count_and_hash)) = split_describe(rest) {
            let _ = tag_version; // The tag version — we use cargo_version for the primary number.
            let (count, hash) = count_and_hash;
            // Reconstruct the tag name from the parsed version.
            let tag = format!("v{tag_version}");
            return if dirty {
                format!("{name} {cargo_version} ({count} commits after {tag}, {hash}, dirty)")
            } else {
                format!("{name} {cargo_version} ({count} commits after {tag}, {hash})")
            };
        }
        // Exact tag match (possibly dirty).
        return if dirty {
            format!("{name} {cargo_version} (dirty)")
        } else {
            format!("{name} {cargo_version}")
        };
    }

    // No tag — bare hash (with or without dirty).
    // Fall back to cargo version.
    if dirty {
        format!("{name} {cargo_version} (dirty)")
    } else {
        format!("{name} {cargo_version}")
    }
}

/// Split a post-tag describe string like "0.1.0-3-gabcdef1" into
/// ("0.1.0", ("3", "gabcdef1")).
///
/// Returns `None` for a plain version like "0.1.0".
fn split_describe(rest: &str) -> Option<(&str, (&str, &str))> {
    // Find the hash suffix: last segment starting with 'g'.
    let hash_start = rest.rfind("-g")?;
    let hash = &rest[hash_start + 1..]; // "gabcdef1"

    let before_hash = &rest[..hash_start];
    // Find the count: last numeric segment before the hash.
    let count_start = before_hash.rfind('-')?;
    let count = &before_hash[count_start + 1..];
    let tag_version = &before_hash[..count_start];

    // Validate: count should be numeric.
    if count.parse::<u32>().is_err() {
        return None;
    }

    Some((tag_version, (count, hash)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_tag_clean() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "v0.1.0"),
            "buffetcar 0.1.0"
        );
    }

    #[test]
    fn at_tag_dirty() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "v0.1.0-dirty"),
            "buffetcar 0.1.0 (dirty)"
        );
    }

    #[test]
    fn commits_after_tag_clean() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "v0.1.0-3-gabcdef1"),
            "buffetcar 0.1.0 (3 commits after v0.1.0, gabcdef1)"
        );
    }

    #[test]
    fn commits_after_tag_dirty() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "v0.1.0-3-gabcdef1-dirty"),
            "buffetcar 0.1.0 (3 commits after v0.1.0, gabcdef1, dirty)"
        );
    }

    #[test]
    fn no_git_describe_falls_back_to_cargo_version() {
        assert_eq!(format_version("buffetcar", "0.2.0", ""), "buffetcar 0.2.0");
    }

    #[test]
    fn bare_hash_no_tags() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "abcdef1"),
            "buffetcar 0.1.0"
        );
    }

    #[test]
    fn bare_hash_dirty_no_tags() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "abcdef1-dirty"),
            "buffetcar 0.1.0 (dirty)"
        );
    }

    #[test]
    fn single_commit_after_tag() {
        assert_eq!(
            format_version("buffetcar", "0.1.0", "v0.1.0-1-g1234567"),
            "buffetcar 0.1.0 (1 commits after v0.1.0, g1234567)"
        );
    }

    #[test]
    fn version_line_is_consistent() {
        // version_line() should return the same pointer on repeated calls.
        let a = version_line();
        let b = version_line();
        assert!(std::ptr::eq(a, b));
    }
}
