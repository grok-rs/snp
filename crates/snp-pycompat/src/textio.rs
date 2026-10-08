//! Text-mode file I/O newline semantics and `textwrap.indent` (15 §A4.4).

use std::borrow::Cow;

use crate::chars::is_space;
use crate::text::splitlines_keepends;

/// `open(f, newline=None).read()` translation: `\r\n` and a lone `\r` become `\n`.
#[must_use]
pub fn universal_newlines(s: &str) -> Cow<'_, str> {
    if !s.contains('\r') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `open(f, 'w')` on Windows (`newline=None` writes `os.linesep`): every `\n` becomes `\r\n`
/// (an existing `\r\n` becomes `\r\r\n`).
#[must_use]
pub fn windows_text_write(s: &str) -> Cow<'_, str> {
    if !s.contains('\n') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.replace('\n', "\r\n"))
}

/// `textwrap.indent(text, prefix)` with the default predicate (`line.strip()` is truthy) over
/// `text.splitlines(True)`.
#[must_use]
pub fn textwrap_indent(text: &str, prefix: &str) -> String {
    let mut out = String::new();
    for line in splitlines_keepends(text) {
        if !line.chars().all(is_space) {
            out.push_str(prefix);
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines() {
        assert_eq!(universal_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(windows_text_write("a\nb\r\n"), "a\r\nb\r\r\n");
        assert!(matches!(windows_text_write("x"), Cow::Borrowed(_)));
    }

    #[test]
    fn indent() {
        assert_eq!(
            textwrap_indent("- a\n\n- b\n   \n- c", "    "),
            "    - a\n\n    - b\n   \n    - c"
        );
        assert_eq!(
            textwrap_indent("- a\n\x0b- b\n", "    "),
            "    - a\n\x0b    - b\n"
        );
        assert_eq!(textwrap_indent("\u{200b}\n", "  "), "  \u{200b}\n");
    }
}
