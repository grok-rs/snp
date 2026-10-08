//! pre-commit 4.6.2 expressions that are pure compositions of the shims.
use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::codec::{Utf8DecodeError, decode_utf8};
use crate::text::{bytes_lstrip_set, bytes_splitlines, bytes_strip, rsplit_whitespace, splitlines, strip};

/// `git.zsplit(s)` (`git.py:19-24`): `s.strip('\0')`, then split on NUL.
#[must_use]
pub fn zsplit(s: &str) -> Vec<&str> {
    let t = s.trim_matches('\0');
    if t.is_empty() { Vec::new() } else { t.split('\0').collect() }
}

/// `run._get_skips` (`run.py:129-131`): `{s.strip() for s in SKIP.split(',') if s.strip()}`.
#[must_use]
pub fn get_skips(skip: &str) -> BTreeSet<&str> {
    skip.split(',').map(strip).filter(|s| !s.is_empty()).collect()
}

/// One parsed pre-push stdin line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrePushLine<'a> {
    pub local_branch: &'a str,
    pub local_sha: &'a str,
    pub remote_branch: &'a str,
    pub remote_sha: &'a str,
}

/// `ValueError` from the 4-way tuple unpack in `hook_impl.py:130`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnpackError {
    pub got: usize,
}

impl core::fmt::Display for UnpackError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "not enough values to unpack (expected 4, got {})", self.got)
    }
}
impl std::error::Error for UnpackError {}

/// `hook_impl.py:128-130`: decoding is eager (any invalid byte anywhere is a
/// crash before the first line is looked at); unpacking is lazy (a malformed
/// line only crashes if no earlier line returned a namespace).
pub fn pre_push_lines(
    stdin: &[u8],
) -> Result<impl Iterator<Item = Result<PrePushLine<'_>, UnpackError>>, Utf8DecodeError> {
    let text = decode_utf8(stdin)?;
    Ok(splitlines(text, false).into_iter().map(|line| {
        match rsplit_whitespace(line, Some(3)).as_slice() {
            &[local_branch, local_sha, remote_branch, remote_sha] => {
                Ok(PrePushLine { local_branch, local_sha, remote_branch, remote_sha })
            }
            parts => Err(UnpackError { got: parts.len() }),
        }
    }))
}

/// `git.parse_merge_msg_for_conflicts` (`git.py:104-111`), eager.
pub fn merge_msg_conflicts(merge_msg: &[u8]) -> Result<Vec<String>, Utf8DecodeError> {
    bytes_splitlines(merge_msg, false)
        .into_iter()
        .filter(|l| l.starts_with(b"\t") || l.starts_with(b"#\t"))
        .map(|l| decode_utf8(bytes_strip(bytes_lstrip_set(l, b"#"))).map(str::to_owned))
        .collect()
}

/// Python `str`-pattern `\s` as an explicit class for the `regex` crate.
pub const PY_STR_SPACE_CLASS: &str =
    r"\t\n\x0B\x0C\r\x1C-\x20\x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

/// `autoupdate.REV_LINE_RE` (`autoupdate.py:116`) with Python `\s` semantics.
/// Python's `$` (no MULTILINE) matches at the end *or before a final `\n`*;
/// the `regex` crate has no lookahead, so `$` becomes `\n?\z` (the match span
/// is never used, only the groups).
pub static REV_LINE_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    let s = PY_STR_SPACE_CLASS;
    Regex::new(&format!(r#"\A([{s}]+)rev:([{s}]*)(['"]?)([^{s}#]+)(.*)(\r?\n)\n?\z"#)).ok()
});

/// Groups 1..=6 of `REV_LINE_RE.match(line)`.
#[must_use]
pub fn rev_line_match(line: &str) -> Option<[&str; 6]> {
    let re = REV_LINE_RE.as_ref()?;
    let c = re.captures(line)?;
    let g = |i| c.get(i).map_or("", |m| m.as_str());
    Some([g(1), g(2), g(3), g(4), g(5), g(6)])
}

/// Regex-free equivalent of `REV_LINE_RE.match(line)` (normative form).
///
/// Derivation: g1 is the whole leading whitespace run (`r` is not
/// whitespace), g2 the whole run after `rev:`, g3 a quote only if a
/// non-space non-`#` char follows it (else Python backtracks to `''`), g4
/// the maximal `[^\s#]+` run, g5 everything up to the first `\n` (`.*` is
/// greedy and swallows a `\r`), g6 is therefore always `"\n"`; Python's `$`
/// tolerates one extra trailing `\n`.
#[must_use]
pub fn rev_line_match_manual(line: &str) -> Option<[&str; 6]> {
    use crate::unicode::is_space;
    let after_ws = line.trim_start_matches(is_space);
    let g1 = line.get(..line.len() - after_ws.len())?;
    if g1.is_empty() {
        return None;
    }
    let after_rev = after_ws.strip_prefix("rev:")?;
    let after_g2 = after_rev.trim_start_matches(is_space);
    let g2 = after_rev.get(..after_rev.len() - after_g2.len())?;
    let value_char = |c: char| !is_space(c) && c != '#';
    let mut chars = after_g2.chars();
    let g3_len = match (chars.next(), chars.next()) {
        (Some(q @ ('\'' | '"')), Some(n)) if value_char(n) => q.len_utf8(),
        _ => 0,
    };
    let (g3, after_g3) = after_g2.split_at_checked(g3_len)?;
    let after_g4 = after_g3.trim_start_matches(value_char);
    let g4 = after_g3.get(..after_g3.len() - after_g4.len())?;
    if g4.is_empty() {
        return None;
    }
    let nl = after_g4.find('\n')?;
    let (g5, tail) = after_g4.split_at_checked(nl)?;
    if tail != "\n" && tail != "\n\n" {
        return None;
    }
    Some([g1, g2, g3, g4, g5, "\n"])
}

/// `autoupdate._write_new_config` line assembly (`autoupdate.py:146-156`);
/// `new_rev` is the already-rendered YAML scalar.
#[must_use]
pub fn rev_line_rewrite(line: &str, new_rev: &str, frozen: Option<&str>) -> Option<String> {
    let [g1, g2, _g3, _g4, g5, g6] = rev_line_match(line)?;
    let comment = match frozen {
        Some(f) => format!("  # frozen: {f}"),
        None if strip(g5).starts_with("# frozen:") => String::new(),
        None => g5.to_owned(),
    };
    Some(format!("{g1}rev:{g2}{new_rev}{comment}{g6}"))
}

/// `migrate_config._migrate_map` header split (`migrate_config.py:21-34`).
#[must_use]
pub fn migrate_header_split(contents: &str) -> (&str, &str) {
    let lines = splitlines(contents, true);
    let n = lines
        .iter()
        .take_while(|l| l.starts_with('#') || l.starts_with("---") || strip(l).is_empty())
        .map(|l| l.len())
        .sum::<usize>();
    contents.split_at_checked(n).unwrap_or((contents, ""))
}

/// Which flavor of `os.path` to emulate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Posix,
    Nt,
}

/// The pure part of `lang_base.exe_exists` (`lang_base.py:63-83`) after
/// `find_executable` returned `found` and `expanduser('~')` returned `home`.
#[must_use]
pub fn exe_exists_pure(flavor: Flavor, found: &str, home: &str) -> bool {
    let shims = found.as_bytes().windows(7).any(|w| {
        matches!(w.first(), Some(b'/' | b'\\'))
            && w.get(1..6) == Some(b"shims")
            && matches!(w.get(6), Some(b'/' | b'\\'))
    });
    let (common, home_is_root) = match flavor {
        Flavor::Posix => (
            crate::posixpath::commonpath(&[found.as_bytes(), home.as_bytes()]).ok().map(|v| String::from_utf8_lossy(&v).into_owned()),
            crate::posixpath::dirname(home.as_bytes()) == home.as_bytes(),
        ),
        Flavor::Nt => (crate::ntpath::commonpath(&[found, home]).ok(), crate::ntpath::dirname(home) == home),
    };
    !shims && (home_is_root || common.as_deref() != Some(home))
}
