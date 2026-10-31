//! Every user-visible text snp-files produces (ARCHITECTURE §1.3 "Messages").
//!
//! The only one is snp's regex backtrack-limit abort (D-SEC-10), identical in both brands (pre-commit has no such
//! limit: its `re` runs unbounded). Unreadable files reuse pre-commit's own `OSError` text (X13-CLS-02).
//!
//! Owner: WP-22.

/// The `ty` of the backtrack-limit failure: line 1 reads `An error has occurred: snp: regular expression …`
/// (ARCHITECTURE §3.3 item 5, D-SEC-10).
pub(crate) const BACKTRACK_TY: &str = "snp";

/// `regular expression {pattern!r} exceeded the backtracking limit on {path!r}`.
pub(crate) fn backtrack_limit(pattern: &str, path_repr: &str) -> String {
    format!(
        "regular expression {} exceeded the backtracking limit on {path_repr}",
        snp_pycompat::repr::repr_str(pattern)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden text (identical in the compat and the snp brand).
    #[test]
    fn goldens() {
        assert_eq!(BACKTRACK_TY, "snp");
        assert_eq!(
            backtrack_limit("(?:(?=a)a+)+(?<!a)b", "'aaaab'"),
            "regular expression '(?:(?=a)a+)+(?<!a)b' exceeded the backtracking limit on 'aaaab'"
        );
        assert_eq!(
            backtrack_limit("it's", "'x'"),
            "regular expression \"it's\" exceeded the backtracking limit on 'x'"
        );
    }
}
