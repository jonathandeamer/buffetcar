//! Platform sandbox hooks.
//!
//! There is no supported platform sandbox in this build: the banner reports
//! "platform sandbox unavailable" and containment is provided entirely by the
//! fd-relative, no-follow resolver in `root`. OpenBSD `pledge`/`unveil` is out
//! of scope until the resolver supports execute-only traversal on OpenBSD.

/// Apply any available platform sandbox. Currently a deliberate no-op; this is
/// the single attachment point for a future `pledge`/`unveil` or seccomp layer.
pub(crate) fn apply() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_is_callable() {
        apply();
    }
}
