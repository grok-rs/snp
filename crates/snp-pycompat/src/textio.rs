//! Text-mode file I/O newline semantics and `textwrap.indent` (15 §A4.4, §A2.8; `Lib/_pyio.py`
//! `IncrementalNewlineDecoder`, `TextIOWrapper.write`, `Lib/textwrap.py`).

use std::borrow::Cow;

use crate::chars::is_space;
use crate::text::splitlines_keepends;

/// `open(f, newline=None).read()` translation (cfgv's config read, `migrate_config.py:115`): `\r\n`
/// and a lone `\r` become `\n`; nothing else is translated (`\x85`, U+2028 survive). Input without
/// `\r` is borrowed.
#[must_use]
pub fn universal_newlines(s: &str) -> Cow<'_, str> {
    if !s.contains('\r') {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    out.try_reserve(s.len()).unwrap_or_default();
    let mut parts = s.split('\r');
    out.push_str(parts.next().unwrap_or_default());
    for part in parts {
        out.push('\n');
        out.push_str(part.trim_prefix('\n'));
    }
    Cow::Owned(out)
}

/// `open(f, 'w')` on Windows (`newline=None` writes `os.linesep`; `migrate_config.py:129`,
/// `install_uninstall.py:92`): every `\n` becomes `\r\n` (an existing `\r\n` becomes `\r\r\n`).
/// Input without `\n` is borrowed.
#[must_use]
pub fn windows_text_write(s: &str) -> Cow<'_, str> {
    if !s.contains('\n') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.replace('\n', "\r\n"))
}

/// `textwrap.indent(text, prefix)` with the default predicate (`line.strip()` is truthy; 3.13's
/// `not line.isspace()` is equivalent) over `text.splitlines(True)` (`migrate_config.py:44`).
#[must_use]
pub fn textwrap_indent(text: &str, prefix: &str) -> String {
    let mut out = String::new();
    out.try_reserve(text.len()).unwrap_or_default();
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
        assert_eq!(universal_newlines("\r\r\n\r"), "\n\n\n");
        assert_eq!(universal_newlines("\r\n\n"), "\n\n");
        assert!(matches!(universal_newlines("a\nb"), Cow::Borrowed(_)));
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
