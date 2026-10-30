//! `shlex.quote`, `shlex.join` and `shlex.split` (`posix=True`, `whitespace_split=True`,
//! `commenters=''`) (15 §A4.8; `Lib/shlex.py`).

use std::borrow::Cow;
use std::fmt;

use crate::messages;

/// `ValueError` raised by `shlex.split` (`shlex.read_token`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShlexError {
    /// EOF inside quotes: `No closing quotation`.
    NoClosingQuotation,
    /// EOF directly after a backslash (also inside `"…"`): `No escaped character`.
    NoEscapedCharacter,
}

impl ShlexError {
    /// The Python exception class: `ValueError`.
    #[must_use]
    pub const fn py_type(&self) -> &'static str {
        messages::VALUE_ERROR
    }
}

impl fmt::Display for ShlexError {
    /// `str(exc)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoClosingQuotation => messages::NO_CLOSING_QUOTATION,
            Self::NoEscapedCharacter => messages::NO_ESCAPED_CHARACTER,
        })
    }
}

impl std::error::Error for ShlexError {}

/// `shlex._find_unsafe` complement: ASCII `[A-Za-z0-9_@%+=:,./-]` (`re.ASCII` `\w`).
const fn is_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
        )
}

/// `shlex.quote(s)` (`clientlib.py:106,355`, `install_uninstall.py:105-107`): `''` for the empty
/// string; `s` itself when every character is safe; else `'…'` with each `'` as `'"'"'`.
/// Non-ASCII letters are unsafe (`'é'` → `"'é'"`).
#[must_use]
pub fn quote(s: &str) -> Cow<'_, str> {
    if s.is_empty() {
        return Cow::Borrowed("''");
    }
    if s.bytes().all(is_safe) {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    out.try_reserve(s.len().saturating_add(2))
        .unwrap_or_default();
    out.push('\'');
    for (i, part) in s.split('\'').enumerate() {
        if i > 0 {
            out.push_str("'\"'\"'");
        }
        out.push_str(part);
    }
    out.push('\'');
    Cow::Owned(out)
}

/// `shlex.quote(os.fsdecode(b))` re-encoded with `surrogateescape` (hook scripts on Unix):
/// non-ASCII and undecodable bytes are unsafe and therefore quoted, never altered.
#[must_use]
pub fn quote_bytes(b: &[u8]) -> Cow<'_, [u8]> {
    if b.is_empty() {
        return Cow::Borrowed(b"''");
    }
    if b.iter().all(|&c| is_safe(c)) {
        return Cow::Borrowed(b);
    }
    let mut out = vec![b'\''];
    for &c in b {
        if c == b'\'' {
            out.extend_from_slice(b"'\"'\"'");
        } else {
            out.push(c);
        }
    }
    out.push(b'\'');
    Cow::Owned(out)
}

/// `shlex.join(args)` = `' '.join(quote(a) for a in args)` (`install_uninstall.py:107`).
#[must_use]
pub fn join<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> String {
    let mut out = String::new();
    for (i, a) in args.into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&quote(a.as_ref()));
    }
    out
}

/// SHLEX-WS: `shlex.whitespace` = `' \t\r\n'`.
const fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Ws,
    Word,
    Quote(char),
    Escape(Esc),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Esc {
    Word,
    InDouble,
}

/// `shlex.split(s)` (`lang_base.py:175` `hook_cmd`, `languages/r.py:183`): the `read_token` state
/// machine with SHLEX-WS separators (`' \t\r\n'` only); `'…'` is literal; inside `"…"` a backslash
/// escapes only `"` and `\`; outside quotes it escapes any character (a newline too); `''` and
/// `""` produce an empty token; `#` is ordinary (`commenters=''`).
pub fn split(s: &str) -> Result<Vec<String>, ShlexError> {
    let mut out = Vec::new();
    let mut tok = String::new();
    let mut quoted = false;
    let mut st = St::Ws;
    for c in s.chars() {
        match st {
            St::Ws => {
                if is_ws(c) {
                    // separators between tokens
                } else if c == '\\' {
                    st = St::Escape(Esc::Word);
                } else if c == '\'' || c == '"' {
                    st = St::Quote(c);
                } else {
                    tok.push(c);
                    st = St::Word;
                }
            }
            St::Word => {
                if is_ws(c) {
                    st = St::Ws;
                    if !tok.is_empty() || quoted {
                        out.push(core::mem::take(&mut tok));
                    }
                    quoted = false;
                } else if c == '\'' || c == '"' {
                    st = St::Quote(c);
                } else if c == '\\' {
                    st = St::Escape(Esc::Word);
                } else {
                    tok.push(c);
                }
            }
            St::Quote(q) => {
                quoted = true;
                if c == q {
                    st = St::Word;
                } else if q == '"' && c == '\\' {
                    st = St::Escape(Esc::InDouble);
                } else {
                    tok.push(c);
                }
            }
            St::Escape(esc) => {
                if esc == Esc::InDouble && c != '\\' && c != '"' {
                    tok.push('\\');
                }
                tok.push(c);
                st = match esc {
                    Esc::Word => St::Word,
                    Esc::InDouble => St::Quote('"'),
                };
            }
        }
    }
    match st {
        St::Quote(_) => return Err(ShlexError::NoClosingQuotation),
        St::Escape(_) => return Err(ShlexError::NoEscapedCharacter),
        St::Ws | St::Word => {}
    }
    if !tok.is_empty() || quoted {
        out.push(tok);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a-b_c.d/e:f,g@h%i+j=k"), "a-b_c.d/e:f,g@h%i+j=k");
        assert_eq!(quote("é"), "'é'");
        assert_eq!(quote("it's"), "'it'\"'\"'s'");
        assert_eq!(join(["a b", "c"]), "'a b' c");
        assert_eq!(&*quote_bytes(b"a\xffb"), b"'a\xffb'");
        assert_eq!(&*quote_bytes(b"x'"), b"'x'\"'\"''");
        assert_eq!(&*quote_bytes(b"plain"), b"plain");
    }

    #[test]
    fn splitting() {
        assert_eq!(
            split("a 'b c' \"d\\\"e\" f\\ g").unwrap(),
            vec!["a", "b c", "d\"e", "f g"]
        );
        assert_eq!(split("\"a\\$b\"").unwrap(), vec!["a\\$b"]);
        assert_eq!(split("'' \"\"").unwrap(), vec!["", ""]);
        assert_eq!(split("a#b c").unwrap(), vec!["a#b", "c"]);
        assert_eq!(split("a\\\nb").unwrap(), vec!["a\nb"]);
        assert_eq!(split("'abc"), Err(ShlexError::NoClosingQuotation));
        assert_eq!(split("abc\\"), Err(ShlexError::NoEscapedCharacter));
        assert_eq!(split("\"a\\"), Err(ShlexError::NoEscapedCharacter));
        assert_eq!(
            ShlexError::NoClosingQuotation.to_string(),
            "No closing quotation"
        );
    }

    #[test]
    fn round_trip() {
        for s in ["", "a b", "it's", "x\"y", "\\", " lead", "tab\there", "é"] {
            assert_eq!(split(&quote(s)).unwrap(), vec![s.to_owned()], "{s:?}");
        }
    }
}
