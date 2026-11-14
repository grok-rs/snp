//! The byte-level cores of the P0 natives, without file system or git access: what fuzz targets and unit tests
//! drive. Each function documents the upstream source it reproduces; none allocates more than its output and none
//! panics on any input.
//!
//! Owner: WP-26.

use memchr::memchr_iter;

mod eof;
pub use eof::{EofAction, end_of_file, end_of_file_bytes, end_of_file_run};
mod eof_tail;
pub(crate) use eof_tail::{TailFold, block_before, fold_block};

/// `bytes.isspace()` / `bytes.strip()` whitespace (ASCII-WS): `b' \t\n\r\x0b\x0c'`.
const fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `line.rstrip(chars)`: `None` strips ASCII-WS, `Some(set)` strips the bytes of `set` (a byte set: a multibyte
/// character contributes each of its bytes, `b''` strips nothing).
fn rstrip<'a>(line: &'a [u8], chars: Option<&[u8]>) -> &'a [u8] {
    let keep = |b: &u8| match chars {
        None => !is_ascii_ws(*b),
        Some(set) => !set.contains(b),
    };
    let end = line
        .iter()
        .rposition(keep)
        .map_or(0, |i| i.saturating_add(1));
    line.get(..end).unwrap_or_default()
}

/// `trailing_whitespace_fixer._process_line` (`trailing_whitespace_fixer.py:25-41`).
fn process_line<'a>(line: &'a [u8], md: bool, chars: Option<&[u8]>, out: &mut Vec<u8>) {
    let (body, eol): (&'a [u8], &[u8]) = if let Some(b) = line.strip_suffix(b"\r\n") {
        (b, b"\r\n")
    } else if let Some(b) = line.strip_suffix(b"\n") {
        (b, b"\n")
    } else {
        (line, b"")
    };
    // `not line.isspace()` is true for an empty line (`b''.isspace()` is False).
    let is_space = !body.is_empty() && body.iter().all(|&b| is_ascii_ws(b));
    if md && !is_space && body.ends_with(b"  ") {
        let head = body.get(..body.len().saturating_sub(2)).unwrap_or_default();
        out.extend_from_slice(rstrip(head, chars));
        out.extend_from_slice(b"  ");
    } else {
        out.extend_from_slice(rstrip(body, chars));
    }
    out.extend_from_slice(eol);
}

/// `trailing_whitespace_fixer._fix_file` (`:8-22`) on the file's bytes: `Some(new bytes)` when any line changed.
///
/// Lines are `readlines()` of a binary file (split after each `\n`; a lone `\r` is not a line break). Lines are
/// copied only from the first changed one on.
pub fn trailing_whitespace(content: &[u8], md: bool, chars: Option<&[u8]>) -> Option<Vec<u8>> {
    let mut out: Option<Vec<u8>> = None;
    let mut start = 0_usize;
    let mut scratch = Vec::new();
    let mut each = |line: &[u8], line_start: usize, out: &mut Option<Vec<u8>>| {
        scratch.clear();
        process_line(line, md, chars, &mut scratch);
        match out {
            Some(o) => o.extend_from_slice(&scratch),
            None if scratch.as_slice() != line => {
                let mut o = Vec::new();
                o.extend_from_slice(content.get(..line_start).unwrap_or_default());
                o.extend_from_slice(&scratch);
                *out = Some(o);
            }
            None => {}
        }
    };
    for nl in memchr_iter(b'\n', content) {
        let end = nl.saturating_add(1);
        let line = content.get(start..end).unwrap_or_default();
        each(line, start, &mut out);
        start = end;
    }
    if start < content.len() {
        let line = content.get(start..).unwrap_or_default();
        each(line, start, &mut out);
    }
    out
}

/// `mixed-line-ending --fix` (`mixed_line_ending.py:22`): `auto`, `no`, `cr`, `crlf` or `lf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFix {
    /// `auto`.
    Auto,
    /// `no`.
    No,
    /// `cr`.
    Cr,
    /// `crlf`.
    Crlf,
    /// `lf`.
    Lf,
}

impl LineFix {
    /// The `--fix` choice.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "no" => Self::No,
            "cr" => Self::Cr,
            "crlf" => Self::Crlf,
            "lf" => Self::Lf,
            _ => return None,
        })
    }
}

/// `bytes.splitlines(keepends=True)` boundaries: each line with its ending (`\r\n`, `\r` or `\n`).
fn splitlines_keepends(content: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = content;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let cut = match memchr::memchr2(b'\r', b'\n', rest) {
            None => rest.len(),
            Some(i) => {
                let crlf =
                    rest.get(i) == Some(&b'\r') && rest.get(i.saturating_add(1)) == Some(&b'\n');
                i.saturating_add(if crlf { 2 } else { 1 })
            }
        };
        let (line, tail) = rest.split_at_checked(cut).unwrap_or((rest, &[]));
        rest = tail;
        Some(line)
    })
}

/// `mixed_line_ending.fix_filename` (`:22-62`) on bytes: `(return value, Some(new bytes) when it rewrites)`.
///
/// Counts the first ending each line has in the order CR, CRLF, LF (`line.endswith`); `mixed` = more than one
/// kind present. `auto` picks the most frequent kind with ties preferring LF > CRLF > CR; an explicit target
/// rewrites when any other kind is present. A rewrite is `line.rstrip(b'\r\n') + ending` for every line,
/// including an unterminated last line.
pub fn mixed_line_ending(content: &[u8], fix: LineFix) -> (bool, Option<Vec<u8>>) {
    let (mut cr, mut crlf, mut lf) = (0_usize, 0_usize, 0_usize);
    for line in splitlines_keepends(content) {
        if line.ends_with(b"\r") {
            cr = cr.saturating_add(1);
        } else if line.ends_with(b"\r\n") {
            crlf = crlf.saturating_add(1);
        } else if line.ends_with(b"\n") {
            lf = lf.saturating_add(1);
        }
    }
    let kinds = [cr, crlf, lf].iter().filter(|&&n| n > 0).count();
    let mixed = kinds > 1;
    let ending: &[u8] = match fix {
        LineFix::No => return (mixed, None),
        LineFix::Auto if !mixed => return (false, None),
        LineFix::Auto => {
            let mut best: (&[u8], usize) = (b"\n", 0);
            for (e, n) in [(&b"\r"[..], cr), (b"\r\n", crlf), (b"\n", lf)] {
                if n >= best.1 {
                    best = (e, n);
                }
            }
            best.0
        }
        LineFix::Cr | LineFix::Crlf | LineFix::Lf => {
            let (target, others): (&[u8], usize) = match fix {
                LineFix::Cr => (b"\r", crlf.saturating_add(lf)),
                LineFix::Crlf => (b"\r\n", cr.saturating_add(lf)),
                _ => (b"\n", cr.saturating_add(crlf)),
            };
            if others == 0 {
                return (false, None);
            }
            target
        }
    };
    let mut out = Vec::new();
    for line in splitlines_keepends(content) {
        let end = line
            .iter()
            .rposition(|&b| b != b'\r' && b != b'\n')
            .map_or(0, |i| i.saturating_add(1));
        out.extend_from_slice(line.get(..end).unwrap_or_default());
        out.extend_from_slice(ending);
    }
    (true, Some(out))
}

/// The UTF-8 byte-order mark.
pub const BOM: &[u8] = b"\xef\xbb\xbf";

/// `fix_byte_order_marker.main` (`:14-25`) on bytes: `Ok(None)` without a BOM, `Ok(Some(rest))` with one, `Err`
/// = the `UnicodeDecodeError` text when the rest is not strict UTF-8 (the `utf-8-sig` text read; positions count
/// from the byte after the BOM).
pub fn fix_byte_order_marker(content: &[u8]) -> Result<Option<&[u8]>, String> {
    let Some(rest) = content.strip_prefix(BOM) else {
        return Ok(None);
    };
    snp_pycompat::codec::decode_utf8(rest).map_err(|e| e.to_string())?;
    Ok(Some(rest))
}

/// check-merge-conflict's patterns per epoch (`check_merge_conflict.py:10-16`): `=======\r\n` exists from
/// v4.2.0 (epoch 3) on.
pub fn merge_conflict_patterns(epoch: u16, diff3: bool) -> Vec<&'static [u8]> {
    let mut v: Vec<&'static [u8]> = vec![b"<<<<<<< ", b"======= "];
    if epoch >= 3 {
        v.push(b"=======\r\n");
    }
    v.push(b"=======\n");
    v.push(b">>>>>>> ");
    if diff3 {
        v.push(b"||||||| ");
    }
    v
}

/// check-merge-conflict's scan (`main`, `:39-50`): `(1-based line number, pattern)` for every line (split after
/// `\n`) that starts with a pattern, in order.
pub fn merge_conflicts<'p>(content: &[u8], patterns: &[&'p [u8]]) -> Vec<(usize, &'p [u8])> {
    let mut hits = Vec::new();
    let mut start = 0_usize;
    let mut lineno = 0_usize;
    let mut check = |line: &[u8], lineno: usize| {
        for p in patterns {
            if line.starts_with(p) {
                hits.push((lineno, *p));
            }
        }
    };
    for nl in memchr_iter(b'\n', content) {
        lineno = lineno.saturating_add(1);
        let end = nl.saturating_add(1);
        check(content.get(start..end).unwrap_or_default(), lineno);
        start = end;
    }
    if start < content.len() {
        lineno = lineno.saturating_add(1);
        check(content.get(start..).unwrap_or_default(), lineno);
    }
    hits
}

/// detect-private-key's byte strings (`detect_private_key.py:6-17`): the first 8 up to v4.0.1 (epoch 1), all 10
/// from v4.1.0 (epoch 2).
pub const PRIVATE_KEY_PATTERNS: [&[u8]; 10] = [
    b"BEGIN RSA PRIVATE KEY",
    b"BEGIN DSA PRIVATE KEY",
    b"BEGIN EC PRIVATE KEY",
    b"BEGIN OPENSSH PRIVATE KEY",
    b"BEGIN PRIVATE KEY",
    b"PuTTY-User-Key-File-2",
    b"BEGIN SSH2 ENCRYPTED PRIVATE KEY",
    b"BEGIN PGP PRIVATE KEY BLOCK",
    b"BEGIN ENCRYPTED PRIVATE KEY",
    b"BEGIN OpenVPN Static key V1",
];

/// `any(line in content for line in BLACKLIST)` for the epoch's pattern set.
pub fn has_private_key(content: &[u8], epoch: u16) -> bool {
    let n = if epoch >= 2 { 10 } else { 8 };
    PRIVATE_KEY_PATTERNS
        .iter()
        .take(n)
        .any(|p| memchr::memmem::find(content, p).is_some())
}

/// `math.ceil(st_size / 1024)` (exact for every size below 2^53, and `u64` beyond).
pub const fn size_kb(size: u64) -> u64 {
    size.div_ceil(1024)
}

/// `os.path.splitext(path.lower())[1]` of check-free extension matching (trailing-whitespace): the extension of a
/// lowercased path, Python `splitext` rules (leading dots of the name are not an extension; `foo.` → `.`).
pub fn splitext_ext(path: &[u8]) -> &[u8] {
    if cfg!(windows) {
        snp_pycompat::ntpath::splitext(path).1
    } else {
        snp_pycompat::posixpath::splitext(path).1
    }
}

/// The argparse parse of a P0 console script at `epoch` (`None` for a script that is not a P0 native): what the
/// native's `run` starts with (ADR-072), without a repository.
pub fn parse_console_script(
    script: &str,
    epoch: u16,
    argv: &[&std::ffi::OsStr],
    env: &snp_core::env::Env,
) -> Option<Result<snp_argparse::Namespace, crate::Outcome>> {
    use super::{
        check_added_large_files as cal, check_executables_have_shebangs, check_json,
        check_merge_conflict, detect_private_key, end_of_file_fixer, fix_byte_order_marker,
        mixed_line_ending, no_commit_to_branch, trailing_whitespace,
    };
    let spec: &'static snp_argparse::ParserSpec = match script {
        "trailing-whitespace-fixer" => &trailing_whitespace::SPEC,
        "end-of-file-fixer" => &end_of_file_fixer::SPEC,
        "mixed-line-ending" => &mixed_line_ending::SPEC,
        "fix-byte-order-marker" => &fix_byte_order_marker::SPEC,
        "check-merge-conflict" => &check_merge_conflict::SPEC,
        "detect-private-key" => &detect_private_key::SPEC,
        "check-added-large-files" => match epoch {
            0 | 1 => &cal::SPEC_E1,
            2 => &cal::SPEC_E2,
            _ => &cal::SPEC_E3,
        },
        "no-commit-to-branch" => &no_commit_to_branch::SPEC,
        "check-json" => &check_json::SPEC,
        "check-executables-have-shebangs" => &check_executables_have_shebangs::SPEC,
        _ => return None,
    };
    Some(crate::framework::parse_args(spec, script, argv, 80, env))
}

/// check-executables-have-shebangs' message for `path` (stderr, `_message`, `:63-73`): the `git add --chmod=-x`
/// line exists from v4.1.0 (epoch 4) on; `shlex.quote` with POSIX quoting on every platform.
pub fn shebang_message(path: &[u8], epoch: u16) -> Vec<u8> {
    let q = snp_pycompat::shlex::quote_bytes(path);
    let mut m = path.to_vec();
    m.extend_from_slice(b": marked executable but has no (or invalid) shebang!\n");
    m.extend_from_slice(b"  If it isn't supposed to be executable, try: `chmod -x ");
    m.extend_from_slice(&q);
    m.extend_from_slice(b"`\n");
    if epoch >= 4 {
        m.extend_from_slice(b"  If on Windows, you may also need to: `git add --chmod=-x ");
        m.extend_from_slice(&q);
        m.extend_from_slice(b"`\n");
    }
    m.extend_from_slice(b"  If it is supposed to be executable, double-check its shebang.\n");
    m
}

#[cfg(test)]
mod tests;
