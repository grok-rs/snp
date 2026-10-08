//! Text-mode file I/O newline semantics and `textwrap.indent`.
use std::borrow::Cow;

use crate::text::splitlines;
use crate::unicode::is_space;

/// `open(f, newline=None).read()` translation: `\r\n` and lone `\r` become `\n`.
#[must_use]
pub fn universal_newlines(s: &str) -> Cow<'_, str> {
    if !s.contains('\r') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `open(f, 'w')` translation on Windows (`newline=None` writes `os.linesep`):
/// every `\n` becomes `\r\n` (an existing `\r\n` becomes `\r\r\n`).
#[must_use]
pub fn windows_text_write(s: &str) -> Cow<'_, str> {
    if !s.contains('\n') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.replace('\n', "\r\n"))
}

/// `textwrap.indent(text, prefix)` with the default predicate
/// (`line.strip()` truthy) over `text.splitlines(True)`.
#[must_use]
pub fn textwrap_indent(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len() + prefix.len() * 8);
    for line in splitlines(text, true) {
        if !line.chars().all(is_space) {
            out.push_str(prefix);
        }
        out.push_str(line);
    }
    out
}
