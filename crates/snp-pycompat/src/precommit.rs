//! pre-commit 4.7.0 expressions that are pure compositions of the shims (15 §A4.5, §A4.6,
//! §A4.11). Consumers call these, never re-implement them; each is verified by vectors.

use std::ffi::{OsStr, OsString};
use std::iter::FusedIterator;

use crate::chars::is_space;
use crate::text::{
    BytesSplitLines, SplitLines, UnpackError, bytes_lstrip_set, bytes_rsplit4, bytes_splitlines,
    bytes_strip, rsplit4, splitlines, splitlines_keepends, strip,
};

/// `git.zsplit(s)` (`git.py:19-24`): `-z` output with leading/trailing NULs stripped, split on
/// NUL; interior empty records are kept; no allocation.
#[derive(Clone, Debug)]
pub struct ZSplit<'a> {
    rest: Option<&'a [u8]>,
}

impl<'a> Iterator for ZSplit<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        let rest = self.rest?;
        let Some(i) = rest.iter().position(|&b| b == 0) else {
            self.rest = None;
            return Some(rest);
        };
        self.rest = rest.get(i.saturating_add(1)..);
        rest.get(..i)
    }
}

impl FusedIterator for ZSplit<'_> {}

/// `git.zsplit(s)` (`git.py:19-24`): `s.strip('\0')`; empty → no records; else `split('\0')`.
#[must_use]
pub fn zsplit(b: &[u8]) -> ZSplit<'_> {
    let start = b.iter().take_while(|&&c| c == 0).count();
    let end = b
        .len()
        .saturating_sub(b.iter().rev().take_while(|&&c| c == 0).count());
    let t = b.get(start..end).unwrap_or_default();
    ZSplit {
        rest: (!t.is_empty()).then_some(t),
    }
}

/// `run._get_skips` (`run.py:129-131`): `{s.strip() for s in SKIP.split(',') if s.strip()}` with
/// the strip of `os.fsdecode` (PY-WS; undecodable bytes are not whitespace), deduplicated in
/// first-seen order (Python's set order is never observable).
#[must_use]
pub fn get_skips(skip: Option<&OsStr>) -> Vec<OsString> {
    let Some(skip) = skip else { return Vec::new() };
    let mut seen: std::collections::HashSet<OsString> = std::collections::HashSet::new();
    let mut out: Vec<OsString> = Vec::new();
    for item in skip_items(skip) {
        if !item.is_empty() && seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

#[cfg(unix)]
fn skip_items(skip: &OsStr) -> Vec<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    skip.as_bytes()
        .split(|&b| b == b',')
        .map(|part| OsString::from_vec(crate::text::strip_fsbytes(part).to_vec()))
        .collect()
}

#[cfg(windows)]
fn skip_items(skip: &OsStr) -> Vec<OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    // Every PY-WS character is a single BMP unit, never a surrogate.
    let ws = |u: &u16| char::from_u32(u32::from(*u)).is_some_and(is_space);
    let units: Vec<u16> = skip.encode_wide().collect();
    units
        .split(|&u| u == u16::from(b','))
        .map(|part| {
            let start = part.iter().take_while(|u| ws(u)).count();
            let end = part
                .len()
                .saturating_sub(part.iter().rev().take_while(|u| ws(u)).count())
                .max(start);
            OsString::from_wide(part.get(start..end).unwrap_or_default())
        })
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn skip_items(skip: &OsStr) -> Vec<OsString> {
    skip.to_string_lossy()
        .split(',')
        .map(|s| OsString::from(strip(s)))
        .collect()
}

/// One pre-push stdin line (`hook_impl._pre_push_ns`; pre-commit's names).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PrePushLine<'a> {
    /// `local_branch` (git's local ref).
    pub local_branch: &'a str,
    /// `local_sha`.
    pub local_sha: &'a str,
    /// `remote_branch` (git's remote ref).
    pub remote_branch: &'a str,
    /// `remote_sha`.
    pub remote_sha: &'a str,
}

/// Lazy pre-push lines: a malformed line yields `Err` only when reached (the caller stops at the
/// first namespace, exactly like pre-commit).
#[derive(Clone, Debug)]
pub struct PrePushLines<'a> {
    lines: SplitLines<'a>,
}

impl<'a> Iterator for PrePushLines<'a> {
    type Item = Result<PrePushLine<'a>, UnpackError>;

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.lines.next()?;
        Some(
            rsplit4(line).map(
                |[local_branch, local_sha, remote_branch, remote_sha]| PrePushLine {
                    local_branch,
                    local_sha,
                    remote_branch,
                    remote_sha,
                },
            ),
        )
    }
}

impl FusedIterator for PrePushLines<'_> {}

/// The bytes variant of [`PrePushLines`] (non-UTF-8 stdin, D-CRASH-11): `bytes.splitlines()` and
/// `bytes.rsplit(maxsplit=3)` (B-WS).
#[derive(Clone, Debug)]
pub struct PrePushLinesBytes<'a> {
    lines: BytesSplitLines<'a>,
}

impl<'a> Iterator for PrePushLinesBytes<'a> {
    type Item = Result<[&'a [u8]; 4], UnpackError>;

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.lines.next()?;
        Some(bytes_rsplit4(line))
    }
}

impl FusedIterator for PrePushLinesBytes<'_> {}

/// `stdin.decode().splitlines()` + `line.rsplit(maxsplit=3)` (`hook_impl.py:128-130`); the caller
/// decoded strictly first.
#[must_use]
pub fn pre_push_lines(stdin: &str) -> PrePushLines<'_> {
    PrePushLines {
        lines: splitlines(stdin),
    }
}

/// [`pre_push_lines`] over raw bytes (non-UTF-8 stdin, D-CRASH-11).
#[must_use]
pub fn pre_push_lines_bytes(stdin: &[u8]) -> PrePushLinesBytes<'_> {
    PrePushLinesBytes {
        lines: bytes_splitlines(stdin),
    }
}

/// `git._parse_merge_msg_for_conflicts` (`git.py:104-111`) on bytes: lines starting with `b'\t'`
/// or `b'#\t'`, `lstrip(b'#')`, then `bytes.strip()`; never decodes (D-CRASH-12).
#[must_use]
pub fn merge_msg_conflicts(merge_msg: &[u8]) -> Vec<&[u8]> {
    bytes_splitlines(merge_msg)
        .filter(|l| l.starts_with(b"\t") || l.starts_with(b"#\t"))
        .map(|l| bytes_strip(bytes_lstrip_set(l, b"#")))
        .collect()
}

/// `autoupdate.REV_LINE_RE.match(line).groups()` (`autoupdate.py:116`) in its regex-free normative
/// form (15 §A4.6): g1 = the leading PY-WS run (non-empty), literal `rev:`, g2 = the PY-WS run
/// after it, g3 = a quote only when a non-space non-`#` character follows it, g4 = the maximal
/// `[^\s#]+` run (non-empty), g5 = everything up to the first `\n`, after which only end of string
/// or exactly one more `\n` may follow (Python `$`); g6 is always `"\n"`.
#[must_use]
pub fn rev_line_match(line: &str) -> Option<[&str; 6]> {
    let after_ws = line.trim_start_matches(is_space);
    let g1 = line.get(..line.len().saturating_sub(after_ws.len()))?;
    if g1.is_empty() {
        return None;
    }
    let after_rev = after_ws.strip_prefix("rev:")?;
    let after_g2 = after_rev.trim_start_matches(is_space);
    let g2 = after_rev.get(..after_rev.len().saturating_sub(after_g2.len()))?;
    let value_char = |c: char| !is_space(c) && c != '#';
    let mut chars = after_g2.chars();
    let g3_len = match (chars.next(), chars.next()) {
        (Some(q @ ('\'' | '"')), Some(n)) if value_char(n) => q.len_utf8(),
        _ => 0,
    };
    let (g3, after_g3) = after_g2.split_at_checked(g3_len)?;
    let after_g4 = after_g3.trim_start_matches(value_char);
    let g4 = after_g3.get(..after_g3.len().saturating_sub(after_g4.len()))?;
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

/// `autoupdate._write_new_config` line assembly (`autoupdate.py:146-156`); `new_rev` is the
/// rendered YAML scalar (`yaml_dump({'rev': rev}, default_style=g3).split(':', 1)[1].strip()`).
/// The comment is `  # frozen: <tag>` when freezing, dropped when `g5.strip()` starts with
/// `# frozen:`, else `g5` kept. `None` when the line is not a rev line.
///
/// Exactly pre-commit's bytes: because g6 is always `"\n"`, a CRLF line whose comment is replaced
/// loses its `\r` (15 §A5 D3). snp's autoupdate re-appends it (DEV-PY-1, D-DATA-10, WP-21); this
/// composite stays the parity reference.
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

/// `migrate_config._migrate_map` header split (`migrate_config.py:21-34`): the longest prefix of
/// `splitlines(True)` lines that start with `#` or `---` or are PY-WS only (an all-NBSP or `\x1f`
/// line counts as blank, a ZWSP line does not), and the rest.
#[must_use]
pub fn migrate_header_split(contents: &str) -> (&str, &str) {
    let n = splitlines_keepends(contents)
        .take_while(|l| l.starts_with('#') || l.starts_with("---") || strip(l).is_empty())
        .fold(0_usize, |n, l| n.saturating_add(l.len()));
    contents.split_at_checked(n).unwrap_or((contents, ""))
}

/// `re.search(r'[/\\]shims[/\\]', found)` on code units.
fn in_shims_dir<T: Copy>(
    found: &[T],
    is_sep: impl Fn(T) -> bool,
    eq: impl Fn(T, u8) -> bool,
) -> bool {
    (0..found.len()).any(|i| {
        let at = |k: usize| found.get(i.saturating_add(k)).copied();
        at(0).is_some_and(&is_sep)
            && b"shims"
                .iter()
                .enumerate()
                .all(|(k, &c)| at(k.saturating_add(1)).is_some_and(|u| eq(u, c)))
            && at(6).is_some_and(&is_sep)
    })
}

/// The pure part of `lang_base.exe_exists` with POSIX `os.path` (15 §A4.11 V-EXE).
#[cfg_attr(
    all(windows, not(test)),
    expect(dead_code, reason = "the posix flavour serves Unix and the tests")
)]
pub(crate) fn exe_exists_posix(found: &[u8], home: &[u8]) -> bool {
    let shims = in_shims_dir(found, |b| b == b'/' || b == b'\\', |b, c| b == c);
    let home_is_root = crate::posixpath::dirname(home) == home;
    let common = crate::posixpath::commonpath(&[found, home]).ok();
    !shims && (home_is_root || common.as_deref() != Some(home))
}

/// The pure part of `lang_base.exe_exists` with Windows `os.path` (string comparison, so a
/// case-different home still counts as "outside home").
#[cfg_attr(
    all(not(windows), not(test)),
    expect(dead_code, reason = "the nt flavour serves Windows and the tests")
)]
pub(crate) fn exe_exists_nt<U: crate::ntpath::Unit>(found: &[U], home: &[U]) -> bool {
    let shims = in_shims_dir(
        found,
        |u: U| matches!(u.to_ascii(), Some(b'/' | b'\\')),
        |u: U, c| u.to_ascii() == Some(c),
    );
    let home_is_root = crate::ntpath::dirname(home) == home;
    let common = crate::ntpath::commonpath(&[found, home]).ok();
    !shims && (home_is_root || common.as_deref() != Some(home))
}

/// `lang_base.exe_exists` (`lang_base.py:63-83`) after `find_executable` returned `found` and
/// `os.path.expanduser('~')` returned `home`: not in a `/shims/` directory, and either the home
/// is a root (`dirname(home) == home`) or `commonpath((found, home)) != home` (a `ValueError`
/// counts as "outside home"). The `os.path` flavour follows the target OS.
#[must_use]
pub fn exe_exists_pure(found: &OsStr, home: &OsStr) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        exe_exists_posix(found.as_bytes(), home.as_bytes())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let f: Vec<u16> = found.encode_wide().collect();
        let h: Vec<u16> = home.encode_wide().collect();
        exe_exists_nt(&f, &h)
    }
    #[cfg(not(any(unix, windows)))]
    {
        exe_exists_posix(
            found.to_string_lossy().as_bytes(),
            home.to_string_lossy().as_bytes(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O1: &str = "1111111111111111111111111111111111111111";
    const O2: &str = "2222222222222222222222222222222222222222";
    const Z40: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn zsplit_vectors() {
        let z = |b: &[u8]| zsplit(b).map(<[u8]>::to_vec).collect::<Vec<_>>();
        assert_eq!(z(b"").len(), 0);
        assert_eq!(z(b"\0").len(), 0);
        assert_eq!(z(b"\0\0").len(), 0);
        assert_eq!(z(b"a"), vec![b"a".to_vec()]);
        assert_eq!(z(b"a\0"), vec![b"a".to_vec()]);
        assert_eq!(z(b"a\0b\0"), vec![b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(z(b"\0a\0"), vec![b"a".to_vec()]);
        assert_eq!(
            z(b"a\0\0b\0"),
            vec![b"a".to_vec(), Vec::new(), b"b".to_vec()]
        );
        assert_eq!(z(b" a \0 b\0"), vec![b" a ".to_vec(), b" b".to_vec()]);
    }

    #[test]
    fn skip_vectors() {
        let sk = |s: &str| {
            let mut v: Vec<String> = get_skips(Some(OsStr::new(s)))
                .into_iter()
                .map(|o| o.into_string().unwrap())
                .collect();
            v.sort();
            v
        };
        let empty: Vec<String> = Vec::new();
        assert_eq!(sk(""), empty);
        assert_eq!(sk(" "), empty);
        assert_eq!(sk(","), empty);
        assert_eq!(sk(" a , b "), vec!["a", "b"]);
        assert_eq!(sk("a,,b"), vec!["a", "b"]);
        assert_eq!(sk("a,a"), vec!["a"]);
        assert_eq!(sk("a b,c"), vec!["a b", "c"]);
        assert_eq!(sk("a\u{a0},b"), vec!["a", "b"]);
        assert_eq!(sk("\u{a0}"), empty);
        assert_eq!(sk("a\x1f"), vec!["a"]);
        assert_eq!(sk("a\u{200b}"), vec!["a\u{200b}"]);
        assert_eq!(sk("a\tb"), vec!["a\tb"]);
        assert_eq!(
            sk("flake8, mypy ,\tblack\n"),
            vec!["black", "flake8", "mypy"]
        );
        assert_eq!(get_skips(None).len(), 0);
        let order: Vec<OsString> = get_skips(Some(OsStr::new("b,a,b")));
        assert_eq!(order, vec![OsString::from("b"), OsString::from("a")]);
    }

    fn pp(s: &str) -> Vec<Result<[String; 4], String>> {
        pre_push_lines(s)
            .map(|r| {
                r.map(|l| {
                    [l.local_branch, l.local_sha, l.remote_branch, l.remote_sha].map(|x| {
                        x.replace(O1, "<O1>")
                            .replace(O2, "<O2>")
                            .replace(Z40, "<Z40>")
                    })
                })
                .map_err(|e| e.to_string())
            })
            .collect()
    }

    #[test]
    fn pre_push_vectors() {
        let ok = |a: &str, b: &str, c: &str, d: &str| {
            Ok([a.to_owned(), b.to_owned(), c.to_owned(), d.to_owned()])
        };
        assert_eq!(
            pp(&format!("refs/heads/main {O1} refs/heads/main {O2}\r\n")),
            vec![ok("refs/heads/main", "<O1>", "refs/heads/main", "<O2>")]
        );
        assert_eq!(
            pp(&format!("HEAD@{{1 day ago}} {O1} refs/heads/x {Z40}\n")),
            vec![ok("HEAD@{1 day ago}", "<O1>", "refs/heads/x", "<Z40>")]
        );
        assert_eq!(pp("").len(), 0);
        assert_eq!(
            pp("\n"),
            vec![Err(
                "not enough values to unpack (expected 4, got 0)".to_owned()
            )]
        );
        assert_eq!(
            pp(&format!(
                "refs/heads/a\u{2028}b {O1} refs/heads/a\u{2028}b {Z40}\n"
            ))[0],
            Err("not enough values to unpack (expected 4, got 1)".to_owned())
        );
        assert_eq!(
            pp(&format!(
                "refs/heads/a\u{a0}b {O1} refs/heads/a\u{a0}b {Z40}\n"
            )),
            vec![ok("refs/heads/a\u{a0}b <O1>", "refs/heads/a", "b", "<Z40>")]
        );
        assert_eq!(
            pp(&format!("  refs/heads/x {O1} refs/heads/x {Z40}  \n")),
            vec![ok("  refs/heads/x", "<O1>", "refs/heads/x", "<Z40>")]
        );
        assert_eq!(
            pp(&format!(
                "refs/heads/x {O1} refs/heads/x {Z40}\x0crefs/heads/y {O2} refs/heads/y {Z40}\n"
            ))
            .len(),
            2
        );
        let b: Vec<_> = pre_push_lines_bytes(b"refs/heads/caf\xe9 o1 refs/heads/x z\n").collect();
        assert_eq!(
            b,
            vec![Ok([
                &b"refs/heads/caf\xe9"[..],
                b"o1",
                b"refs/heads/x",
                b"z"
            ])]
        );
    }

    #[test]
    fn merge_msg_vectors() {
        let mm = |b: &[u8]| {
            merge_msg_conflicts(b)
                .into_iter()
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            mm(b"Merge branch 'foo'\n\n# Conflicts:\n#\tconflict_file\n#\tother\n"),
            vec![b"conflict_file".to_vec(), b"other".to_vec()]
        );
        assert_eq!(
            mm(b"Merge\r\n\r\n# Conflicts:\r\n#\ta.txt\r\n#\tb c.txt \r\n"),
            vec![b"a.txt".to_vec(), b"b c.txt".to_vec()]
        );
        assert_eq!(
            mm(b"#\t  spaced  \n##\tdouble\n# \tnot\n\t\x0bvt\x0b\n#\tx\x0cy\n"),
            vec![b"spaced".to_vec(), b"vt".to_vec(), b"x\x0cy".to_vec()]
        );
        assert_eq!(mm(b"#\tcaf\xe9\n"), vec![b"caf\xe9".to_vec()]);
        assert_eq!(mm(b"#\tfile\rwith-cr\n"), vec![b"file".to_vec()]);
        assert_eq!(mm(b"").len(), 0);
    }

    #[test]
    fn rev_line_vectors() {
        let g = |l: &str| rev_line_match(l).map(|g| g.map(str::to_owned));
        let some = |a: [&str; 6]| Some(a.map(str::to_owned));
        assert_eq!(
            g("    rev: v1.0.0\n"),
            some(["    ", " ", "", "v1.0.0", "", "\n"])
        );
        assert_eq!(
            g("    rev: v1.0.0\r\n"),
            some(["    ", " ", "", "v1.0.0", "\r", "\n"])
        );
        assert_eq!(g("    rev: v1.0.0"), None);
        assert_eq!(
            g("    rev: 'v1.0.0'\n"),
            some(["    ", " ", "'", "v1.0.0'", "", "\n"])
        );
        assert_eq!(g("-   rev: v1\n"), None);
        assert_eq!(g("rev: v1\n"), None);
        assert_eq!(g("    rev:v1\n"), some(["    ", "", "", "v1", "", "\n"]));
        assert_eq!(
            g("    rev: abc  # frozen: v1\r\n"),
            some(["    ", " ", "", "abc", "  # frozen: v1\r", "\n"])
        );
        assert_eq!(g("    rev: #x\n"), None);
        assert_eq!(g("    rev: \n"), None);
        assert_eq!(
            g("\u{1f} rev: v1\n"),
            some(["\u{1f} ", " ", "", "v1", "", "\n"])
        );
        assert_eq!(
            g("    rev: v1\u{3000}x\n"),
            some(["    ", " ", "", "v1", "\u{3000}x", "\n"])
        );
        assert_eq!(
            g("    rev: v1\n\n"),
            some(["    ", " ", "", "v1", "", "\n"])
        );
        assert_eq!(g("    rev: v1\u{85}"), None);
        assert_eq!(
            g("    rev: \"v1'\n"),
            some(["    ", " ", "\"", "v1'", "", "\n"])
        );
        assert_eq!(
            g("    rev: [v1]\n"),
            some(["    ", " ", "", "[v1]", "", "\n"])
        );
        assert_eq!(g("    rev: v1\r"), None);
    }

    #[test]
    fn rev_line_rewrite_vectors() {
        let a40: String = std::iter::repeat_n('a', 40).collect();
        let b40: String = std::iter::repeat_n('b', 40).collect();
        let rw = |l: &str, n: &str, f: Option<&str>| rev_line_rewrite(l, n, f).unwrap();
        assert_eq!(rw("    rev: v1.0.0\n", "v2.0.0", None), "    rev: v2.0.0\n");
        assert_eq!(
            rw("    rev: v1.0.0\r\n", "v2.0.0", None),
            "    rev: v2.0.0\r\n"
        );
        assert_eq!(
            rw("    rev: v1.0.0\r\n", &a40, Some("v2.0.0")),
            format!("    rev: {a40}  # frozen: v2.0.0\n")
        );
        assert_eq!(
            rw(&format!("    rev: {a40}  # frozen: v1\r\n"), "v2", None),
            "    rev: v2\n"
        );
        assert_eq!(
            rw("    rev: v1  # keep me\n", "v2", None),
            "    rev: v2  # keep me\n"
        );
        assert_eq!(
            rw("    rev: v1  # keep me\r\n", &b40, Some("v2")),
            format!("    rev: {b40}  # frozen: v2\n")
        );
        assert_eq!(rw("    rev: 'v1'\n", "'v2'", None), "    rev: 'v2'\n");
        assert_eq!(
            rw("    rev: v1 #frozen: x\n", "v2", None),
            "    rev: v2 #frozen: x\n"
        );
        assert_eq!(
            rw("    rev: v1\u{a0}# frozen: x\n", "v2", None),
            "    rev: v2\n"
        );
        assert_eq!(rev_line_rewrite("x\n", "v2", None), None);
    }

    #[test]
    fn migrate_header_vectors() {
        assert_eq!(
            migrate_header_split("-   repo: r\n    hooks: []\n"),
            ("", "-   repo: r\n    hooks: []\n")
        );
        assert_eq!(
            migrate_header_split("# comment\n---\n\n-   repo: r\n"),
            ("# comment\n---\n\n", "-   repo: r\n")
        );
        assert_eq!(
            migrate_header_split("\u{a0}\n-   repo: r\n"),
            ("\u{a0}\n", "-   repo: r\n")
        );
        assert_eq!(
            migrate_header_split("\u{200b}\n-   repo: r\n"),
            ("", "\u{200b}\n-   repo: r\n")
        );
        assert_eq!(
            migrate_header_split("#c\x0c-   repo: r\n"),
            ("#c\x0c", "-   repo: r\n")
        );
        assert_eq!(
            migrate_header_split("# only comments\n"),
            ("# only comments\n", "")
        );
        assert_eq!(migrate_header_split(""), ("", ""));
    }

    #[test]
    fn exe_exists_vectors() {
        let px = |f: &str, h: &str| exe_exists_posix(f.as_bytes(), h.as_bytes());
        assert!(px("/usr/bin/node", "/home/u"));
        assert!(!px("/home/u/.nvm/versions/node/v20/bin/node", "/home/u"));
        assert!(!px("/home/u/.pyenv/shims/python", "/home/u"));
        assert!(!px("/opt/x/shims/ruby", "/home/u"));
        assert!(px("/usr/bin/node", "/"));
        assert!(px("/home/u/bin/node", "/"));
        assert!(px("bin/node", "/home/u"));
        assert!(px("/home/u/bin/node", "/home/u/"));
        assert!(px("/home/u2/node", "/home/u"));
        assert!(px("/home/u/node", ""));
        let nt = |f: &str, h: &str| exe_exists_nt(f.as_bytes(), h.as_bytes());
        assert!(nt("C:\\Program Files\\nodejs\\node.EXE", "C:\\Users\\u"));
        assert!(!nt(
            "C:\\Users\\u\\AppData\\Roaming\\npm\\node.EXE",
            "C:\\Users\\u"
        ));
        assert!(!nt("c:\\users\\U\\scoop\\shims\\node.EXE", "C:\\Users\\u"));
        assert!(nt("C:\\users\\u\\x\\node.EXE", "C:\\Users\\u"));
        assert!(nt("D:\\tools\\node.EXE", "C:\\Users\\u"));
        assert!(!nt("C:\\Users\\u/bin\\node.EXE", "C:\\Users\\u"));
        assert!(nt("C:\\x\\node.EXE", "C:\\"));
    }
}
