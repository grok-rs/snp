//! Shebang parsing: `identify.parse_shebang` with a Python `shlex.split`
//! port, `/usr/bin/env [-S]` stripping and the `nix-shell` special case.

use core::fmt;
use std::io::{self, BufRead, Read};

use crate::text::read_full;

/// Upper bound on the bytes [`parse_shebang`] consumes (the `#!` line plus
/// any `nix-shell` continuation lines).
///
/// Python reads the first line without limit. A shebang line longer than
/// this is reported as "no shebang" instead of being read into memory; a
/// `nix-shell` continuation line that would cross the limit ends the
/// continuation. Real shebangs are far shorter (Linux executes at most 256
/// bytes of it).
pub const MAX_SHEBANG_BYTES: usize = 64 * 1024;

/// Size of the first read once the caller-provided head is consumed (enough
/// for any real shebang line).
const FIRST_CHUNK: usize = 512;

/// Size of later reads (only for pathologically long lines).
const CHUNK: usize = 8 * 1024;

/// Error from [`shlex_split`], mirroring the `ValueError` messages of
/// Python's `shlex`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ShlexError {
    /// A `'` or `"` quote was not closed (`No closing quotation`).
    NoClosingQuotation,
    /// The input ended with a lone backslash (`No escaped character`).
    NoEscapedCharacter,
}

impl fmt::Display for ShlexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoClosingQuotation => "No closing quotation",
            Self::NoEscapedCharacter => "No escaped character",
        })
    }
}

impl std::error::Error for ShlexError {}

/// Python's `shlex.split(s)` (POSIX mode, `whitespace_split=True`, no
/// comment characters).
///
/// Whitespace is `" \t\r\n"` only (vertical tab and form feed are ordinary
/// characters). Single quotes are literal; inside double quotes a backslash
/// escapes only `"` and `\`; outside quotes a backslash escapes any
/// character. `#` is an ordinary character.
///
/// # Errors
///
/// [`ShlexError`] for an unterminated quote or a trailing backslash, like
/// Python's `ValueError`.
///
/// ```
/// use file_identify::shlex_split;
///
/// assert_eq!(shlex_split(r#""/usr/bin/my python" -x 'a b'"#)?, ["/usr/bin/my python", "-x", "a b"]);
/// assert_eq!(shlex_split("/bin/sh #foo")?, ["/bin/sh", "#foo"]);
/// assert!(shlex_split("unterminated \"q").is_err());
/// # Ok::<(), file_identify::ShlexError>(())
/// ```
pub fn shlex_split(s: &str) -> Result<Vec<String>, ShlexError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        /// Between tokens.
        Whitespace,
        /// Inside an unquoted word.
        Word,
        /// Inside a quote of the given kind.
        Quote(char),
        /// After a backslash; `true` when inside double quotes.
        Escape { in_double: bool },
    }
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    let mut out = Vec::new();
    let mut token = String::new();
    // Python's `quoted`: an empty quoted string still yields a token.
    let mut quoted = false;
    let mut state = State::Whitespace;
    for c in s.chars() {
        state = match state {
            State::Whitespace | State::Word if is_ws(c) => {
                if state == State::Word && (!token.is_empty() || quoted) {
                    out.push(core::mem::take(&mut token));
                    quoted = false;
                }
                State::Whitespace
            }
            State::Whitespace | State::Word => match c {
                '\\' => State::Escape { in_double: false },
                '\'' | '"' => State::Quote(c),
                _ => {
                    token.push(c);
                    State::Word
                }
            },
            State::Quote(q) => {
                quoted = true;
                if c == q {
                    State::Word
                } else if q == '"' && c == '\\' {
                    State::Escape { in_double: true }
                } else {
                    token.push(c);
                    state
                }
            }
            State::Escape { in_double } => {
                if in_double && c != '"' && c != '\\' {
                    token.push('\\');
                }
                token.push(c);
                if in_double {
                    State::Quote('"')
                } else {
                    State::Word
                }
            }
        };
    }
    match state {
        State::Quote(_) => Err(ShlexError::NoClosingQuotation),
        State::Escape { .. } => Err(ShlexError::NoEscapedCharacter),
        State::Whitespace | State::Word => {
            if !token.is_empty() || quoted {
                out.push(token);
            }
            Ok(out)
        }
    }
}

/// Python's whitespace for `str.split()` / `str.strip()` restricted to ASCII.
const fn is_py_ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1f)
}

/// identify's `_shebang_split`: `shlex.split`, falling back to `str.split()`.
fn shebang_split(line: &str) -> Vec<String> {
    if !line.contains(['\'', '"', '\\']) {
        // Without quotes or escapes shlex splits on its whitespace only.
        return line
            .split([' ', '\t', '\r', '\n'])
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
    }
    shlex_split(line).unwrap_or_else(|_| {
        line.split(|c: char| u8::try_from(c).is_ok_and(is_py_ascii_space))
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// The line as `&str` if every byte is in Python's `string.printable`
/// (ASCII letters, digits, punctuation and `" \t\n\r\x0b\x0c"`).
///
/// identify decodes the line as UTF-8 and then requires every character to
/// be printable; since printable characters are all ASCII the two checks
/// collapse into this one.
fn printable(line: &[u8]) -> Option<&str> {
    if line
        .iter()
        .all(|&b| matches!(b, 0x20..=0x7e | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        core::str::from_utf8(line).ok()
    } else {
        None
    }
}

/// Python `str.strip()` for an ASCII string.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| u8::try_from(c).is_ok_and(is_py_ascii_space))
}

/// Read one line (up to and including `\n`, or to end of input) into `out`,
/// consuming at most `limit` bytes. Returns `false` if the line is longer
/// than `limit`.
fn read_line_limited<R: BufRead + ?Sized>(
    reader: &mut R,
    out: &mut Vec<u8>,
    limit: usize,
) -> io::Result<bool> {
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(true);
        }
        let room = limit.saturating_sub(out.len());
        if room == 0 {
            // The limit is reached and more data follows on this line.
            return Ok(false);
        }
        let window = available.get(..room).unwrap_or(available);
        if let Some(i) = window.iter().position(|&b| b == b'\n') {
            out.extend_from_slice(window.get(..=i).unwrap_or(window));
            reader.consume(i.saturating_add(1));
            return Ok(true);
        }
        let n = window.len();
        out.extend_from_slice(window);
        reader.consume(n);
    }
}

/// Read one line like [`read_line_limited`] and pass it to `f`. Returns
/// `None` if the line is longer than `limit`, else `f`'s result and the
/// number of bytes consumed. Lines already in the reader's buffer are not
/// copied.
fn with_line<R: BufRead + ?Sized, T>(
    reader: &mut R,
    limit: usize,
    f: impl FnOnce(&[u8]) -> T,
) -> io::Result<Option<(T, usize)>> {
    let available = loop {
        match reader.fill_buf() {
            Ok(buf) => break buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    };
    let window = available.get(..limit).unwrap_or(available);
    if let Some(i) = window.iter().position(|&b| b == b'\n') {
        let out = f(window.get(..=i).unwrap_or(window));
        // `i < window.len()`: no overflow.
        let used = i.saturating_add(1);
        reader.consume(used);
        return Ok(Some((out, used)));
    }
    let mut line = Vec::new();
    if !read_line_limited(reader, &mut line, limit)? {
        return Ok(None);
    }
    let used = line.len();
    Ok(Some((f(&line), used)))
}

/// identify's per-line processing: printable check, `strip()`, split.
fn tokenize_line(line: &[u8]) -> Option<Vec<String>> {
    printable(line).map(|text| shebang_split(py_strip(text)))
}

/// Reads first from an in-memory `head`, then from `rest` through a buffer
/// that is only allocated if `head` is exhausted.
pub(crate) struct Source<'a, R> {
    head: &'a [u8],
    rest: R,
    buf: Vec<u8>,
    pos: usize,
}

impl<'a, R: Read> Source<'a, R> {
    pub(crate) fn new(head: &'a [u8], rest: R) -> Self {
        Self {
            head,
            rest,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl<R: Read> Read for Source<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(buf.len());
        for (o, a) in buf.iter_mut().zip(available) {
            *o = *a;
        }
        self.consume(n);
        Ok(n)
    }
}

impl<R: Read> BufRead for Source<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if !self.head.is_empty() {
            return Ok(self.head);
        }
        if self.pos >= self.buf.len() {
            let size = if self.buf.capacity() == 0 {
                FIRST_CHUNK
            } else {
                CHUNK
            };
            self.buf.resize(size, 0);
            let n = loop {
                match self.rest.read(&mut self.buf) {
                    Ok(n) => break n.min(size),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        self.buf.clear();
                        self.pos = 0;
                        return Err(e);
                    }
                }
            };
            self.buf.truncate(n);
            self.pos = 0;
        }
        Ok(self.buf.get(self.pos..).unwrap_or_default())
    }

    fn consume(&mut self, amount: usize) {
        if self.head.is_empty() {
            self.pos = self.pos.saturating_add(amount).min(self.buf.len());
        } else {
            self.head = self.head.get(amount..).unwrap_or_default();
        }
    }
}

/// identify's `parse_shebang` on a buffered reader positioned at the start
/// of the file.
pub(crate) fn parse_shebang_buf<R: BufRead + ?Sized>(reader: &mut R) -> io::Result<Vec<String>> {
    let mut budget = MAX_SHEBANG_BYTES;
    let mut magic = [0u8; 2];
    if read_full(reader, &mut magic)? != 2 || magic != *b"#!" {
        return Ok(Vec::new());
    }
    // MAX_SHEBANG_BYTES >= 2.
    budget = budget.saturating_sub(magic.len());
    let Some((tokens, used)) = with_line(reader, budget, tokenize_line)? else {
        return Ok(Vec::new());
    };
    budget = budget.saturating_sub(used);
    let Some(mut cmd) = tokens else {
        return Ok(Vec::new());
    };
    match cmd.as_slice() {
        [env, flag, ..] if env == "/usr/bin/env" && flag == "-S" => {
            #[expect(
                clippy::disallowed_methods,
                reason = "the slice pattern proves len >= 2"
            )]
            cmd.drain(..2);
        }
        [env, ..] if env == "/usr/bin/env" => {
            #[expect(
                clippy::disallowed_methods,
                reason = "the slice pattern proves len >= 1"
            )]
            cmd.remove(0);
        }
        _ => {}
    }
    if matches!(cmd.as_slice(), [only] if only == "nix-shell") {
        return parse_nix_shebang(reader, cmd, budget);
    }
    Ok(cmd)
}

/// identify's `_parse_nix_shebang`: every following `#!` line is tokenized
/// and the argument of its last `-i` flag replaces the command.
fn parse_nix_shebang<R: BufRead + ?Sized>(
    reader: &mut R,
    mut cmd: Vec<String>,
    mut budget: usize,
) -> io::Result<Vec<String>> {
    loop {
        if budget < 2 {
            return Ok(cmd);
        }
        let mut magic = [0u8; 2];
        if read_full(reader, &mut magic)? != 2 || magic != *b"#!" {
            return Ok(cmd);
        }
        // `budget >= 2` (checked above).
        budget = budget.saturating_sub(2);
        let Some((tokens, used)) = with_line(reader, budget, tokenize_line)? else {
            return Ok(cmd);
        };
        budget = budget.saturating_sub(used);
        let Some(tokens) = tokens else {
            return Ok(cmd);
        };
        #[expect(clippy::disallowed_methods, reason = "a constant non-zero window size")]
        let pairs = tokens.windows(2);
        for pair in pairs {
            if let [flag, arg] = pair
                && flag == "-i"
            {
                cmd = vec![arg.clone()];
            }
        }
    }
}

/// Parse a shebang from the start of `reader`, exactly like
/// `identify.parse_shebang`, returning the command tokens (empty when there
/// is no usable shebang).
///
/// * Input that does not start with `#!` is rejected after its first two
///   bytes (one read of at most 512 bytes from `reader`).
/// * The line must be printable ASCII; it is split with [`shlex_split`]
///   (falling back to whitespace splitting on a quoting error).
/// * A leading `/usr/bin/env` — or `/usr/bin/env -S` — is removed (a bare
///   `env` is kept, as in identify).
/// * `#!/usr/bin/env nix-shell` followed by `#! nix-shell -i <interp> …`
///   lines resolves to `[<interp>]`.
/// * At most [`MAX_SHEBANG_BYTES`] bytes are consumed.
///
/// Interrupted reads are retried; other read errors are returned.
///
/// # Errors
///
/// Propagates read errors from `reader`.
///
/// ```
/// use file_identify::parse_shebang;
///
/// assert_eq!(parse_shebang(&b"#!/usr/bin/env -S python3 -u\nprint()"[..])?, ["python3", "-u"]);
/// assert_eq!(parse_shebang(&b"#!\"/opt/my tools/x\" y"[..])?, ["/opt/my tools/x", "y"]);
/// let nix = b"#! /usr/bin/env nix-shell\n#! nix-shell -i bash -p jq\n";
/// assert_eq!(parse_shebang(&nix[..])?, ["bash"]);
/// assert!(parse_shebang(&b"print('no shebang')"[..])?.is_empty());
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn parse_shebang<R: Read>(reader: R) -> io::Result<Vec<String>> {
    parse_shebang_buf(&mut Source::new(&[], reader))
}

/// [`parse_shebang`] for in-memory content (infallible).
///
/// ```
/// use file_identify::parse_shebang_bytes;
///
/// assert_eq!(parse_shebang_bytes(b"#!/bin/sh -e\n"), ["/bin/sh", "-e"]);
/// ```
#[must_use]
pub fn parse_shebang_bytes(content: &[u8]) -> Vec<String> {
    let mut reader = content;
    parse_shebang_buf(&mut reader).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Vec<String> = Vec::new();

    fn split(s: &str) -> Result<Vec<String>, ShlexError> {
        shlex_split(s)
    }

    #[test]
    fn shlex_golden() {
        // Verified against CPython 3.12 shlex.split.
        let ok: &[(&str, &[&str])] = &[
            ("/bin/sh #foo", &["/bin/sh", "#foo"]),
            (
                "/usr/bin/env python # c",
                &["/usr/bin/env", "python", "#", "c"],
            ),
            ("\"/usr/bin/my python\" -x", &["/usr/bin/my python", "-x"]),
            ("a\\ b c", &["a b", "c"]),
            ("\"a\\\\b\\q\"", &["a\\b\\q"]),
            ("x\x0by", &["x\x0by"]),
            ("x\ry", &["x", "y"]),
            ("a '' b", &["a", "", "b"]),
            ("''", &[""]),
            ("a\"b c\"d", &["ab cd"]),
            ("'a\\b'", &["a\\b"]),
            ("\"a\\\"b\"", &["a\"b"]),
            ("", &[]),
            ("   ", &[]),
            ("\\'", &["'"]),
        ];
        for (input, expected) in ok {
            let expected: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(split(input), Ok(expected), "{input:?}");
        }
        assert_eq!(
            split("unterminated \"q"),
            Err(ShlexError::NoClosingQuotation)
        );
        assert_eq!(split("'"), Err(ShlexError::NoClosingQuotation));
        assert_eq!(split("trail\\"), Err(ShlexError::NoEscapedCharacter));
        assert_eq!(split("\"x\\"), Err(ShlexError::NoEscapedCharacter));
        assert_eq!(
            ShlexError::NoClosingQuotation.to_string(),
            "No closing quotation"
        );
        assert_eq!(shebang_split("unterminated \"q"), ["unterminated", "\"q"]);
        assert_eq!(shebang_split("trail\\"), ["trail\\"]);
        assert_eq!(shebang_split("a\x0bb 'c"), ["a", "b", "'c"]);
    }

    #[test]
    fn upstream_parse_shebang_cases() {
        // identify/tests/identify_test.py::test_parse_shebang
        let cases: &[(&[u8], &[&str])] = &[
            (b"", &[]),
            (b"#!/usr/bin/python", &["/usr/bin/python"]),
            (b"#!/usr/bin/env python", &["python"]),
            (b"#! /usr/bin/python", &["/usr/bin/python"]),
            (b"#!/usr/bin/foo  python", &["/usr/bin/foo", "python"]),
            (b"#!\"/path/with spaces/x\" y", &["/path/with spaces/x", "y"]),
            (b"#!/path'with/quotes    y", &["/path'with/quotes", "y"]),
            (b"#! /path'with/quotes y ", &["/path'with/quotes", "y"]),
            (b"#! /usr/bin/env nix-shell\n#! nix-shell -i bash -p python", &["bash"]),
            (b"#! /usr/bin/env nix-shell\n#! nix-shell -i python -p coreutils", &["python"]),
            (b"#! /usr/bin/env nix-shell\n#! nix-shell -p coreutils -i python", &["python"]),
            (b"#! /usr/bin/env nix-shell\n#! nix-shell -p coreutils\n#! nix-shell -i python", &["python"]),
            (b"#! /usr/bin/env nix-shell\n#!nix-shell -p coreutils\n#!nix-shell -i python", &["python"]),
            (b"#! /usr/bin/env nix-shell\n#!\xf9\x93\x01\x42\xcd", &["nix-shell"]),
            (b"#! /usr/bin/env nix-shell\n#!\x00\x00\x00\x00", &["nix-shell"]),
            (b"#! /usr/bin/nix-shell", &["/usr/bin/nix-shell"]),
            (b"#! /usr/bin/env nix-shell", &["nix-shell"]),
            (b"#! /usr/bin/env nix-shell non-portable-argument", &["nix-shell", "non-portable-argument"]),
            (b"#! /usr/bin/env nix-shell\n#! nix-shell -i", &["nix-shell"]),
            (
                b"#!/usr/bin/env nix-shell\n#!nix-shell --argstr x \"a -i python3 p\"\n#!nix-shell -p hello\n#!nix-shell -i bash\n#!nix-shell --argstr y \"b -i runhaskell q\"",
                &["bash"],
            ),
            (b"\xf9\x93\x01\x42\xcd", &[]),
            (b"#!\xf9\x93\x01\x42\xcd", &[]),
            (b"#!\x00\x00\x00\x00", &[]),
            (b"#!/usr/bin/env -S python -u", &["python", "-u"]),
            (b"#!/usr/bin/env", &[]),
            (b"#!/usr/bin/env -S", &[]),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_shebang_bytes(input),
                *expected,
                "{:?}",
                String::from_utf8_lossy(input)
            );
            let expected: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(parse_shebang(*input).ok(), Some(expected));
        }
    }

    #[test]
    fn printable_and_strip_rules() {
        assert_eq!(
            parse_shebang_bytes(b"#!/bin/sh\x0b-x\x0c\r\n"),
            ["/bin/sh\x0b-x"]
        );
        assert_eq!(parse_shebang_bytes(b"#!/bin/sh\r\n"), ["/bin/sh"]);
        assert_eq!(parse_shebang_bytes(b"#!/bin/sh\x01\n"), NONE);
        assert_eq!(parse_shebang_bytes("#!/bin/sh é\n".as_bytes()), NONE);
        assert_eq!(parse_shebang_bytes(b"#!"), NONE);
        assert_eq!(parse_shebang_bytes(b"#"), NONE);
        assert_eq!(parse_shebang_bytes(b"#!   \n"), NONE);
        // Only the absolute /usr/bin/env is stripped.
        assert_eq!(parse_shebang_bytes(b"#!env python"), ["env", "python"]);
        assert_eq!(
            parse_shebang_bytes(b"#!/usr/bin/env -S"),
            Vec::<String>::new()
        );
        // env -S followed by nix-shell triggers the nix logic too.
        assert_eq!(
            parse_shebang_bytes(b"#!/usr/bin/env -S nix-shell\n#!x -i zsh\n"),
            ["zsh"]
        );
    }

    #[test]
    fn line_limit() {
        let mut long = b"#!/bin/sh ".to_vec();
        long.resize(MAX_SHEBANG_BYTES + 10, b'a');
        assert_eq!(parse_shebang_bytes(&long), NONE);
        // Exactly at the limit (2 + line) and terminated by end of input: parsed.
        let mut fits = b"#!/bin/sh ".to_vec();
        fits.resize(MAX_SHEBANG_BYTES, b'a');
        assert_eq!(parse_shebang_bytes(&fits).len(), 2);
        // A newline just past the limit makes the line too long.
        fits.push(b'\n');
        assert_eq!(parse_shebang_bytes(&fits), NONE);
    }

    #[test]
    fn bounded_reads_on_endless_input() {
        /// `#!` followed by an endless line of `a`.
        struct Endless {
            served: usize,
        }
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = b"#!".get(self.served + i).copied().unwrap_or(b'a');
                }
                self.served += buf.len();
                Ok(buf.len())
            }
        }
        /// The same line repeated forever.
        struct Cycle {
            data: Vec<u8>,
            pos: usize,
        }
        impl Read for Cycle {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                for b in buf.iter_mut() {
                    *b = self.data[self.pos % self.data.len()];
                    self.pos += 1;
                }
                Ok(buf.len())
            }
        }

        let mut endless = Endless { served: 0 };
        assert!(parse_shebang(&mut endless).is_ok_and(|v| v.is_empty()));
        assert!(
            endless.served <= MAX_SHEBANG_BYTES + CHUNK,
            "served {}",
            endless.served
        );

        // Endless nix-shell continuation lines stop at the budget as well.
        let mut nix = Cycle {
            data: b"#!/usr/bin/env nix-shell\n".to_vec(),
            pos: 0,
        };
        let cmd = parse_shebang(&mut nix).unwrap_or_default();
        assert_eq!(cmd, ["nix-shell"]);
        assert!(nix.pos <= MAX_SHEBANG_BYTES + CHUNK, "served {}", nix.pos);
    }

    #[test]
    fn source_reads_head_then_rest() {
        let mut src = Source::new(b"#!/usr/bin/env py", &b"thon3 -u\n#! more"[..]);
        assert_eq!(
            parse_shebang_buf(&mut src).unwrap_or_default(),
            ["python3", "-u"]
        );
        let mut rest = String::new();
        assert!(src.read_to_string(&mut rest).is_ok());
        assert_eq!(rest, "#! more");
    }

    #[test]
    fn read_errors_propagate() {
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("boom"))
            }
        }
        assert!(parse_shebang(Failing).is_err());
        let mut src = Source::new(b"#!/bin/sh", Failing);
        assert!(parse_shebang_buf(&mut src).is_err());
    }
}
