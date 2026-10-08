//! `shlex.quote`, `shlex.join`, `shlex.split` (posix=True, whitespace_split=True,
//! commenters='').
use std::borrow::Cow;

/// Errors raised by `shlex.split` as `ValueError(<msg>)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShlexError {
    NoClosingQuotation,
    NoEscapedCharacter,
}

impl core::fmt::Display for ShlexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NoClosingQuotation => "No closing quotation",
            Self::NoEscapedCharacter => "No escaped character",
        })
    }
}
impl std::error::Error for ShlexError {}

const fn is_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-')
}

/// `shlex.quote(s)`.
#[must_use]
pub fn quote(s: &str) -> Cow<'_, str> {
    if s.is_empty() {
        return Cow::Borrowed("''");
    }
    if s.bytes().all(is_safe) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    out.push_str(&s.replace('\'', "'\"'\"'"));
    out.push('\'');
    Cow::Owned(out)
}

/// `shlex.join(args)`.
#[must_use]
pub fn join<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    for (i, a) in args.into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&quote(a.as_ref()));
    }
    out
}

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

/// `shlex.split(s)`.
pub fn split(s: &str) -> Result<Vec<String>, ShlexError> {
    let mut out = Vec::new();
    let mut tok = String::new();
    let mut quoted = false;
    let mut st = St::Ws;
    for c in s.chars() {
        match st {
            St::Ws => {
                if is_ws(c) {
                    // nothing pending (tokens are emitted on the transition below)
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
