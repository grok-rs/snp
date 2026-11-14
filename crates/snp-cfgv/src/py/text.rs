//! Python text-mode newline handling.

use std::borrow::Cow;

/// Applies the newline translation of Python's text mode for reading
/// (`newline=None`): every `\r\n` and every remaining lone `\r` becomes `\n`.
///
/// Nothing else is translated (`\x85`, U+2028 and friends are kept). Returns
/// the input unchanged, without allocating, when it contains no `\r`.
///
/// ```
/// assert_eq!(cfgv::py::universal_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
/// assert_eq!(cfgv::py::universal_newlines("\r\r\n"), "\n\n");
/// ```
#[must_use]
pub fn universal_newlines(s: &str) -> Cow<'_, str> {
    if !s.contains('\r') {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    out.try_reserve(s.len()).unwrap_or_default();
    let mut rest = s;
    while let Some(pos) = rest.find('\r') {
        let (before, after) = rest.split_at(pos);
        out.push_str(before);
        out.push('\n');
        // Skip the `\r` and a directly following `\n`.
        let after = after.get(1..).unwrap_or_default();
        rest = after.strip_prefix('\n').unwrap_or(after);
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation() {
        assert!(matches!(universal_newlines("a\nb"), Cow::Borrowed("a\nb")));
        assert_eq!(universal_newlines("\r"), "\n");
        assert_eq!(universal_newlines("\n\r"), "\n\n");
        assert_eq!(universal_newlines("a\r\n"), "a\n");
        assert_eq!(universal_newlines("é\r\u{85}\u{2028}"), "é\n\u{85}\u{2028}");
    }
}
