//! The `dart` language backend (§6.2; `pre_commit/languages/dart.py`).
//!
//! `install_environment` (`dart.py:38-97`) creates `<env>/bin` (it must not exist), then for the repository and for
//! every `additional_dependencies` package runs `_install_dir`: read `pubspec.yaml`, `dart pub get` in the package
//! directory with `PUB_CACHE` pointing to a temporary directory, and `dart compile exe --output <env>/bin/<name>
//! <pkg>/bin/<name>.dart` for every **key** of the pubspec's `executables` mapping (alias targets are ignored). A
//! dependency `name[:version]` is first fetched with `dart pub cache add name [--version version]` into its own
//! temporary `PUB_CACHE`; the first directory of that cache (top-down walk) holding a `pubspec.yaml` is copied to a
//! fresh temporary `pkg` directory and installed from there.
//!
//! snp differences (no register IDs yet; the integration lead allocates them, see the WP-25 report):
//! * executable names must be single path components, checked before the first compile (a hostile pubspec cannot
//!   write outside `<env>/bin`, 04b §5.7; pre-commit joins `x/../../y` into the path);
//! * the pubspec is read with a strict YAML-subset reader ([`pubspec_executables`]) instead of PyYAML: every input it
//!   accepts is read as PyYAML 6.0.3 reads it (oracle vectors in `testdata/group2/pubspec`), every input PyYAML
//!   refuses is refused, and valid YAML outside the subset (tabs, anchors, tags, multi-line quoted scalars, …) is
//!   refused too (exit 3 either way). The intended end state is the workspace PyYAML-compatible loader (snp-yaml),
//!   which snp-lang's manifest does not depend on (04b §5.7 "same safe YAML stack");
//! * a `pubspec.yaml` larger than [`MAX_PUBSPEC`] is refused;
//! * a non-`str` executable (`123:`) fails like pre-commit's `TypeError` on every OS (on Windows pre-commit's
//!   `win_exe` would turn it into `123.exe`);
//! * the cache walk is sorted.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, copytree, default_version, io_err, listdir, mkdir_new, outcome,
    tempdir, win_exe,
};
use crate::common::{prefix_path, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: dart`.
#[derive(Debug)]
pub struct Dart;

/// Cap on a `pubspec.yaml` read (pubspecs are a few KiB); a larger file is refused.
pub const MAX_PUBSPEC: u64 = 1 << 20;

/// Depth bound of the `PUB_CACHE` walk.
const MAX_WALK_DEPTH: usize = 64;

/// Recursion bound of the pubspec reader (nested block and flow collections).
const MAX_NESTING: usize = 64;

/// Longest implicit (single-line) mapping key the reader accepts, in characters. LibYAML refuses a simple key whose
/// `:` is more than 1024 characters away; staying below that keeps every accepted key one LibYAML accepts too.
const MAX_KEY_CHARS: usize = 1000;

/// What `pubspec['executables']` is, as far as `_install_dir` cares (each variant is what pre-commit's
/// `for executable in pubspec_contents['executables']` loop then does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Executables {
    /// No `executables` key (pre-commit: `KeyError: 'executables'`).
    Missing,
    /// An empty document: `yaml_load` returns `None` (pre-commit: `TypeError: 'NoneType' object is not
    /// subscriptable`).
    EmptyDocument,
    /// `executables:` with a null value (pre-commit: `TypeError: 'NoneType' object is not iterable`).
    Null,
    /// A scalar that is not iterable: `bool`, `int` or `float` (pre-commit: `TypeError: '<type>' object is not
    /// iterable`).
    NotIterable(&'static str),
    /// The names iterated by the loop: the keys of a mapping (document order of first occurrence, duplicates
    /// collapsed as in a `dict`), the items of a sequence, or the characters of a string.
    Names(Vec<String>),
    /// Like [`Executables::Names`], but the loop then reaches an item that is not a `str` (its Python type name):
    /// pre-commit compiles the names before it and crashes in `os.path.join` with `TypeError`.
    NamesThenNonStr(Vec<String>, &'static str),
}

/// A `pubspec.yaml` that is not valid YAML, or that the subset reader does not understand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubspecError(pub String);

impl std::fmt::Display for PubspecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A node of the document, reduced to what the reader needs.
#[derive(Debug)]
enum Node {
    /// A null scalar (empty, `~`, `null`, …).
    Null,
    /// A `str` scalar (quoted, or plain resolving to `str`).
    Str(String),
    /// A plain scalar resolving to another type (`bool`, `int`, `float`).
    Other(&'static str),
    /// A block scalar (`|`, `>`): a `str` whose value the reader does not compute.
    Opaque,
    /// A sequence.
    Seq(Vec<Self>),
    /// A mapping, entries in document order (duplicates kept).
    Map(Vec<(Self, Self)>),
}

/// One line of the document: the number of leading spaces and the rest.
#[derive(Debug)]
struct SrcLine {
    /// 1-based line number in the file.
    no: usize,
    /// Leading spaces.
    indent: usize,
    /// The characters after the indentation.
    body: Vec<char>,
}

impl SrcLine {
    /// Empty or spaces only.
    fn is_blank(&self) -> bool {
        self.body.is_empty()
    }

    /// Blank, or a comment.
    fn is_skippable(&self) -> bool {
        self.body.first().is_none_or(|c| *c == '#')
    }
}

/// The error of a construct at a line.
fn bad(line: Option<&SrcLine>, what: &str) -> PubspecError {
    match line {
        Some(l) => PubspecError(format!("pubspec.yaml line {}: {what}", l.no)),
        None => PubspecError(format!("pubspec.yaml: {what}")),
    }
}

/// The characters that cannot start a plain scalar in the reader's subset (YAML indicators; `-`, `?` and `:` followed
/// by a non-blank would be allowed by YAML, but are refused here).
const INDICATORS: &[char] = &[
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
];

/// The `bool` words of PyYAML's YAML 1.1 resolver.
const BOOL_WORDS: &[&str] = &[
    "yes", "Yes", "YES", "no", "No", "NO", "true", "True", "TRUE", "false", "False", "FALSE", "on",
    "On", "ON", "off", "Off", "OFF",
];

/// `[0-9]` run starting at `i`; returns the end.
fn digits_end(s: &[u8], i: usize, ok: impl Fn(u8) -> bool) -> usize {
    let mut j = i;
    while s.get(j).is_some_and(|b| ok(*b)) {
        j = j.saturating_add(1);
    }
    j
}

/// `(?::[0-5]?[0-9])+` exactly covering `s[i..end]` (each group is `:` then one digit, or two digits the first of
/// which is `0`-`5`).
fn sexagesimal_tail(s: &[u8], mut i: usize, end: usize) -> bool {
    let mut groups = 0_usize;
    while i < end {
        if s.get(i) != Some(&b':') {
            return false;
        }
        let Some(d1) = s
            .get(i.saturating_add(1))
            .copied()
            .filter(u8::is_ascii_digit)
        else {
            return false;
        };
        if s.get(i.saturating_add(2)).is_some_and(u8::is_ascii_digit) {
            if d1 > b'5' {
                return false;
            }
            i = i.saturating_add(3);
        } else {
            i = i.saturating_add(2);
        }
        groups = groups.saturating_add(1);
    }
    groups > 0 && i == end
}

/// Whether `s` matches PyYAML's implicit `int` resolver. `Err` when the constructor would then raise (`0b_`, `0x_`).
fn resolves_int(s: &str) -> Result<bool, ()> {
    let b = s.as_bytes();
    let start = usize::from(matches!(b.first(), Some(b'-' | b'+')));
    let rest = b.get(start..).unwrap_or_default();
    let binary: fn(u8) -> bool = |c| matches!(c, b'0' | b'1');
    let hex: fn(u8) -> bool = |c| c.is_ascii_hexdigit();
    for (prefix, digit) in [(&b"0b"[..], binary), (&b"0x"[..], hex)] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            if tail.is_empty() || !tail.iter().all(|c| *c == b'_' || digit(*c)) {
                return Ok(false);
            }
            // `int(value[2:], base)` of nothing but underscores raises `ValueError`.
            return if tail.iter().all(|c| *c == b'_') {
                Err(())
            } else {
                Ok(true)
            };
        }
    }
    match rest.first() {
        Some(b'0') => Ok(rest.len() == 1
            || rest
                .get(1..)
                .is_some_and(|t| t.iter().all(|c| matches!(c, b'0'..=b'7' | b'_')))),
        Some(b'1'..=b'9') => {
            let end = digits_end(rest, 1, |c| c.is_ascii_digit() || c == b'_');
            Ok(end == rest.len() || sexagesimal_tail(rest, end, rest.len()))
        }
        _ => Ok(false),
    }
}

/// Whether `s` matches PyYAML's implicit `float` resolver.
fn resolves_float(s: &str) -> bool {
    let b = s.as_bytes();
    let exponent_ok = |i: usize| -> bool {
        // `(?:[eE][-+][0-9]+)?$` from `i`.
        if i == b.len() {
            return true;
        }
        matches!(b.get(i), Some(b'e' | b'E'))
            && matches!(b.get(i.saturating_add(1)), Some(b'-' | b'+'))
            && {
                let d = i.saturating_add(2);
                let end = digits_end(b, d, |c| c.is_ascii_digit());
                end > d && end == b.len()
            }
    };
    if matches!(s, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    if b.first() == Some(&b'.') {
        // `\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?`
        if b.get(1).is_some_and(u8::is_ascii_digit) {
            let end = digits_end(b, 2, |c| c.is_ascii_digit() || c == b'_');
            return exponent_ok(end);
        }
    }
    let start = usize::from(matches!(b.first(), Some(b'-' | b'+')));
    let rest = b.get(start..).unwrap_or_default();
    if matches!(rest, b".inf" | b".Inf" | b".INF") {
        return true;
    }
    if !rest.first().is_some_and(u8::is_ascii_digit) {
        return false;
    }
    let int_end = digits_end(b, start.saturating_add(1), |c| {
        c.is_ascii_digit() || c == b'_'
    });
    // `[-+]?[0-9][0-9_]*\.[0-9_]*(?:[eE][-+][0-9]+)?`
    if b.get(int_end) == Some(&b'.') {
        let frac_end = digits_end(b, int_end.saturating_add(1), |c| {
            c.is_ascii_digit() || c == b'_'
        });
        if exponent_ok(frac_end) {
            return true;
        }
    }
    // `[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*`
    if let Some(dot) = b.iter().rposition(|c| *c == b'.')
        && dot > int_end
        && b.get(dot.saturating_add(1)..)
            .unwrap_or_default()
            .iter()
            .all(|c| c.is_ascii_digit() || *c == b'_')
    {
        return sexagesimal_tail(b, int_end, dot);
    }
    false
}

/// The value of a plain scalar under PyYAML's implicit resolvers (`yaml/resolver.py`). Scalars that make the
/// `SafeConstructor` raise (`=`, `<<`, `0b_`, impossible dates) are refused; so is everything that looks like a
/// timestamp (`dddd-…`), whose construction the reader does not replicate.
fn resolve_plain(s: &str, line: Option<&SrcLine>) -> Result<Node, PubspecError> {
    if matches!(s, "" | "~" | "null" | "Null" | "NULL") {
        return Ok(Node::Null);
    }
    if BOOL_WORDS.contains(&s) {
        return Ok(Node::Other("bool"));
    }
    if resolves_float(s) {
        return Ok(Node::Other("float"));
    }
    match resolves_int(s) {
        Ok(true) => return Ok(Node::Other("int")),
        Ok(false) => {}
        Err(()) => return Err(bad(line, &format!("invalid integer {s:?}"))),
    }
    let b = s.as_bytes();
    let timestamp_like = b.len() > 4
        && b.get(..4).is_some_and(|d| d.iter().all(u8::is_ascii_digit))
        && b.get(4) == Some(&b'-');
    if timestamp_like || s == "=" || s == "<<" {
        return Err(bad(line, &format!("unsupported scalar {s:?}")));
    }
    Ok(Node::Str(s.to_owned()))
}

/// A body (or the rest of one) that starts a block sequence entry: `-` followed by a blank or the end.
fn is_seq_entry(body: &[char]) -> bool {
    body.first() == Some(&'-') && body.get(1).is_none_or(|c| *c == ' ')
}

/// The index of the first `#` that starts a comment (at `from` == 0, or after a blank).
fn comment_start(body: &[char], from: usize) -> usize {
    let mut i = from;
    while let Some(c) = body.get(i) {
        if *c == '#' && (i == 0 || body.get(i.wrapping_sub(1)) == Some(&' ')) {
            return i;
        }
        i = i.saturating_add(1);
    }
    body.len()
}

/// Whether `body[from..]` holds nothing but blanks and an optional comment.
fn only_trailing_comment(body: &[char], from: usize) -> bool {
    let mut i = from;
    while body.get(i) == Some(&' ') {
        i = i.saturating_add(1);
    }
    match body.get(i) {
        None => true,
        Some('#') => i == 0 || body.get(i.wrapping_sub(1)) == Some(&' '),
        Some(_) => false,
    }
}

/// A quoted scalar starting at `body[at]` and closed on the same line: (value, index after the closing quote).
fn quoted_scalar(
    body: &[char],
    at: usize,
    line: &SrcLine,
) -> Result<(String, usize), PubspecError> {
    let unclosed = || {
        bad(
            Some(line),
            "unsupported multi-line or unterminated quoted scalar",
        )
    };
    let delimiter = body.get(at).copied();
    let mut out = String::new();
    let mut i = at.saturating_add(1);
    if delimiter == Some('\'') {
        loop {
            match body.get(i) {
                None => return Err(unclosed()),
                Some('\'') if body.get(i.saturating_add(1)) == Some(&'\'') => {
                    out.push('\'');
                    i = i.saturating_add(2);
                }
                Some('\'') => return Ok((out, i.saturating_add(1))),
                Some(c) => {
                    out.push(*c);
                    i = i.saturating_add(1);
                }
            }
        }
    }
    loop {
        match body.get(i) {
            None => return Err(unclosed()),
            Some('"') => return Ok((out, i.saturating_add(1))),
            Some('\\') => {
                let Some(escape) = body.get(i.saturating_add(1)).copied() else {
                    return Err(unclosed());
                };
                i = i.saturating_add(2);
                let simple = match escape {
                    '0' => Some('\0'),
                    'a' => Some('\u{7}'),
                    'b' => Some('\u{8}'),
                    't' => Some('\t'),
                    'n' => Some('\n'),
                    'v' => Some('\u{b}'),
                    'f' => Some('\u{c}'),
                    'r' => Some('\r'),
                    'e' => Some('\u{1b}'),
                    ' ' => Some(' '),
                    '"' => Some('"'),
                    '/' => Some('/'),
                    '\\' => Some('\\'),
                    'N' => Some('\u{85}'),
                    '_' => Some('\u{a0}'),
                    'L' => Some('\u{2028}'),
                    'P' => Some('\u{2029}'),
                    _ => None,
                };
                if let Some(c) = simple {
                    out.push(c);
                    continue;
                }
                let width = match escape {
                    'x' => 2,
                    'u' => 4,
                    'U' => 8,
                    _ => {
                        return Err(bad(
                            Some(line),
                            &format!("invalid escape \\{escape} in a double-quoted scalar"),
                        ));
                    }
                };
                let mut code = 0_u32;
                for _ in 0..width {
                    let digit = body.get(i).and_then(|h| h.to_digit(16)).ok_or_else(|| {
                        bad(Some(line), "invalid escape in a double-quoted scalar")
                    })?;
                    code = code.saturating_mul(16).saturating_add(digit);
                    i = i.saturating_add(1);
                }
                out.push(char::from_u32(code).ok_or_else(|| {
                    bad(
                        Some(line),
                        "invalid Unicode escape in a double-quoted scalar",
                    )
                })?);
            }
            Some(c) => {
                out.push(*c);
                i = i.saturating_add(1);
            }
        }
    }
}

/// An implicit key at `body[at..]`: (key, index just after its `:`), or `None` when the text is not `key:`.
fn split_key(
    body: &[char],
    at: usize,
    line: &SrcLine,
) -> Result<Option<(Node, usize)>, PubspecError> {
    let first = body.get(at).copied();
    if matches!(first, Some('\'' | '"')) {
        let Ok((key, end)) = quoted_scalar(body, at, line) else {
            return Ok(None);
        };
        let mut i = end;
        while body.get(i) == Some(&' ') {
            i = i.saturating_add(1);
        }
        if body.get(i) == Some(&':') && body.get(i.saturating_add(1)).is_none_or(|c| *c == ' ') {
            if i.saturating_sub(at) > MAX_KEY_CHARS {
                return Err(bad(Some(line), "mapping key too long"));
            }
            return Ok(Some((Node::Str(key), i.saturating_add(1))));
        }
        return Ok(None);
    }
    if first.is_some_and(|c| matches!(c, '[' | '{' | '|' | '>' | '&' | '*' | '!')) {
        return Ok(None);
    }
    let stop = comment_start(body, at);
    let mut i = at;
    while i < stop {
        if body.get(i) == Some(&':') && body.get(i.saturating_add(1)).is_none_or(|c| *c == ' ') {
            let key: String = body
                .get(at..i)
                .unwrap_or_default()
                .iter()
                .collect::<String>()
                .trim_end_matches(' ')
                .to_owned();
            if key.is_empty() {
                return Err(bad(Some(line), "unsupported empty mapping key"));
            }
            if key.starts_with(INDICATORS) {
                return Err(bad(Some(line), &format!("unsupported mapping key {key:?}")));
            }
            if i.saturating_sub(at) > MAX_KEY_CHARS {
                return Err(bad(Some(line), "mapping key too long"));
            }
            return Ok(Some((
                resolve_plain(&key, Some(line))?,
                i.saturating_add(1),
            )));
        }
        i = i.saturating_add(1);
    }
    Ok(None)
}

/// Whether a plain-scalar segment (comment already cut off) contains a mapping indicator (`: ` or a final `:`), which
/// YAML refuses inside a plain value ("mapping values are not allowed here").
fn has_value_indicator(seg: &[char]) -> bool {
    seg.iter()
        .enumerate()
        .any(|(i, c)| *c == ':' && seg.get(i.saturating_add(1)).is_none_or(|n| *n == ' '))
}

/// The YAML-subset reader over the lines of one document.
struct Reader {
    /// The document's lines.
    lines: Vec<SrcLine>,
}

impl Reader {
    /// Line `i`.
    fn line(&self, i: usize) -> Option<&SrcLine> {
        self.lines.get(i)
    }

    /// The first line at or after `i` that is neither blank nor a comment.
    fn next_content(&self, i: usize) -> Option<usize> {
        self.lines
            .get(i..)?
            .iter()
            .position(|l| !l.is_skippable())
            .map(|p| p.saturating_add(i))
    }

    /// The error of a construct at line `i`.
    fn err(&self, i: usize, what: &str) -> PubspecError {
        bad(self.line(i), what)
    }

    /// The document: a block mapping at column 0.
    fn document(&self) -> Result<Option<Node>, PubspecError> {
        let Some(first) = self.next_content(0) else {
            return Ok(None);
        };
        let line = self.line(first).ok_or_else(|| self.err(first, "no line"))?;
        if line.indent != 0 {
            return Err(self.err(first, "unsupported indented top-level mapping"));
        }
        if split_key(&line.body, 0, line)?.is_none() {
            return Err(self.err(first, "pubspec.yaml is not a block mapping"));
        }
        let (node, next) = self.block_map(first, 0, 0, 0)?;
        if let Some(extra) = self.next_content(next) {
            return Err(self.err(extra, "unexpected content after the top-level mapping"));
        }
        Ok(Some(node))
    }

    /// A block mapping whose first key starts at `body[off]` of line `li`; its keys sit at column `col`.
    fn block_map(
        &self,
        mut li: usize,
        mut off: usize,
        col: usize,
        depth: usize,
    ) -> Result<(Node, usize), PubspecError> {
        if depth > MAX_NESTING {
            return Err(self.err(li, "unsupported nesting depth"));
        }
        let mut entries = Vec::new();
        loop {
            let line = self.line(li).ok_or_else(|| self.err(li, "no line"))?;
            if is_seq_entry(line.body.get(off..).unwrap_or_default()) {
                return Err(self.err(li, "a sequence entry is not allowed in this mapping"));
            }
            let Some((key, after)) = split_key(&line.body, off, line)? else {
                return Err(self.err(li, "could not find expected ':'"));
            };
            let (value, next) = self.inline_value(li, after, col, true, depth.saturating_add(1))?;
            entries.push((key, value));
            let Some(k) = self.next_content(next) else {
                return Ok((Node::Map(entries), next));
            };
            let indent = self.line(k).map_or(0, |l| l.indent);
            match indent.cmp(&col) {
                std::cmp::Ordering::Equal => {
                    li = k;
                    off = 0;
                }
                std::cmp::Ordering::Less => return Ok((Node::Map(entries), k)),
                std::cmp::Ordering::Greater => {
                    return Err(self.err(k, "unexpected indentation"));
                }
            }
        }
    }

    /// A block sequence whose `-` entries sit at column `col`, starting at line `li`.
    fn block_seq(
        &self,
        mut li: usize,
        col: usize,
        depth: usize,
    ) -> Result<(Node, usize), PubspecError> {
        if depth > MAX_NESTING {
            return Err(self.err(li, "unsupported nesting depth"));
        }
        let mut items = Vec::new();
        loop {
            let line = self.line(li).ok_or_else(|| self.err(li, "no line"))?;
            let mut p = 1_usize;
            while line.body.get(p) == Some(&' ') {
                p = p.saturating_add(1);
            }
            let rest = line.body.get(p..).unwrap_or_default();
            let (item, next) = if rest.first().is_none_or(|c| *c == '#') {
                self.block_value(li.saturating_add(1), col, false, depth.saturating_add(1))?
            } else if is_seq_entry(rest) {
                return Err(self.err(li, "unsupported compact nested sequence"));
            } else if split_key(&line.body, p, line)?.is_some() {
                self.block_map(li, p, col.saturating_add(p), depth.saturating_add(1))?
            } else {
                self.inline_value(li, p, col, false, depth.saturating_add(1))?
            };
            items.push(item);
            let Some(k) = self.next_content(next) else {
                return Ok((Node::Seq(items), next));
            };
            let l = self.line(k).ok_or_else(|| self.err(k, "no line"))?;
            if l.indent == col && is_seq_entry(&l.body) {
                li = k;
            } else if l.indent > col {
                return Err(self.err(k, "unexpected indentation"));
            } else {
                return Ok((Node::Seq(items), k));
            }
        }
    }

    /// The value after a `key:` (or `-`) with nothing else on its line: the following more-indented node, an
    /// indentless sequence (mapping values only), or null.
    fn block_value(
        &self,
        start: usize,
        owner: usize,
        in_map: bool,
        depth: usize,
    ) -> Result<(Node, usize), PubspecError> {
        let Some(k) = self.next_content(start) else {
            return Ok((Node::Null, start));
        };
        let line = self.line(k).ok_or_else(|| self.err(k, "no line"))?;
        if line.indent > owner {
            if is_seq_entry(&line.body) {
                return self.block_seq(k, line.indent, depth);
            }
            if split_key(&line.body, 0, line)?.is_some() {
                return self.block_map(k, 0, line.indent, depth);
            }
            if matches!(line.body.first(), Some('|' | '>')) {
                return Err(self.err(k, "unsupported block scalar on its own line"));
            }
            return self.inline_value(k, 0, owner, false, depth);
        }
        if in_map && line.indent == owner && is_seq_entry(&line.body) {
            return self.block_seq(k, owner, depth);
        }
        Ok((Node::Null, start))
    }

    /// The node starting at `body[off..]` of line `li` (after `key:`, `-`, or alone on a line); continuation lines
    /// must be indented more than `owner`.
    fn inline_value(
        &self,
        li: usize,
        off: usize,
        owner: usize,
        in_map: bool,
        depth: usize,
    ) -> Result<(Node, usize), PubspecError> {
        if depth > MAX_NESTING {
            return Err(self.err(li, "unsupported nesting depth"));
        }
        let line = self.line(li).ok_or_else(|| self.err(li, "no line"))?;
        let body = &line.body;
        let mut p = off;
        while body.get(p) == Some(&' ') {
            p = p.saturating_add(1);
        }
        let Some(first) = body.get(p).copied() else {
            return self.block_value(li.saturating_add(1), owner, in_map, depth);
        };
        match first {
            '#' => self.block_value(li.saturating_add(1), owner, in_map, depth),
            '|' | '>' => {
                let mut q = p.saturating_add(1);
                if matches!(body.get(q), Some('+' | '-')) {
                    q = q.saturating_add(1);
                }
                if body.get(q).is_some_and(|c| *c != ' ') {
                    return Err(self.err(li, "unsupported block scalar header"));
                }
                if !only_trailing_comment(body, q) {
                    return Err(self.err(li, "unsupported block scalar header"));
                }
                Ok((
                    Node::Opaque,
                    self.block_scalar(li.saturating_add(1), owner)?,
                ))
            }
            '[' | '{' => {
                let mut cur = FlowCursor { li, pos: p };
                let node = self.flow_node(&mut cur, depth)?;
                let end_line = self
                    .line(cur.li)
                    .ok_or_else(|| self.err(cur.li, "no line"))?;
                if !only_trailing_comment(&end_line.body, cur.pos) {
                    return Err(self.err(cur.li, "unexpected text after a flow collection"));
                }
                Ok((node, cur.li.saturating_add(1)))
            }
            '\'' | '"' => {
                let (value, end) = quoted_scalar(body, p, line)?;
                if !only_trailing_comment(body, end) {
                    return Err(self.err(li, "unexpected text after a quoted scalar"));
                }
                Ok((Node::Str(value), li.saturating_add(1)))
            }
            c if INDICATORS.contains(&c) => {
                Err(self.err(li, &format!("unsupported value starting with {c:?}")))
            }
            _ => self.plain(li, p, owner),
        }
    }

    /// The lines of a block scalar whose header ended line `start - 1`, for a node owned by column `owner`: returns
    /// the first line after it.
    fn block_scalar(&self, start: usize, owner: usize) -> Result<usize, PubspecError> {
        let mut k = start;
        let mut leading_blank = 0_usize;
        while let Some(l) = self.line(k).filter(|l| l.is_blank()) {
            leading_blank = leading_blank.max(l.indent);
            k = k.saturating_add(1);
        }
        let Some(first) = self.line(k) else {
            return Ok(k);
        };
        let content = first.indent;
        if content <= owner {
            return Ok(k);
        }
        if leading_blank > content {
            return Err(self.err(k, "unsupported block scalar leading blank lines"));
        }
        while self
            .line(k)
            .is_some_and(|l| l.is_blank() || l.indent >= content)
        {
            k = k.saturating_add(1);
        }
        Ok(k)
    }

    /// A plain scalar starting at `body[p]` of line `li`, possibly continued on lines indented more than `owner`.
    fn plain(&self, li: usize, p: usize, owner: usize) -> Result<(Node, usize), PubspecError> {
        let line = self.line(li).ok_or_else(|| self.err(li, "no line"))?;
        let stop = comment_start(&line.body, p);
        let seg = line.body.get(p..stop).unwrap_or_default();
        if has_value_indicator(seg) {
            return Err(self.err(li, "mapping values are not allowed here"));
        }
        let mut value: String = seg
            .iter()
            .collect::<String>()
            .trim_end_matches(' ')
            .to_owned();
        let mut ended = stop < line.body.len();
        let mut next = li.saturating_add(1);
        let mut k = next;
        let mut blanks = 0_usize;
        while !ended {
            let Some(l) = self.line(k) else { break };
            if l.is_blank() {
                blanks = blanks.saturating_add(1);
                k = k.saturating_add(1);
                continue;
            }
            if l.is_skippable() || l.indent <= owner {
                break;
            }
            if l.body.first().is_some_and(|c| INDICATORS.contains(c)) {
                return Err(self.err(k, "unsupported plain scalar continuation"));
            }
            let stop = comment_start(&l.body, 0);
            let seg = l.body.get(..stop).unwrap_or_default();
            if has_value_indicator(seg) {
                return Err(self.err(k, "mapping values are not allowed here"));
            }
            if blanks == 0 {
                value.push(' ');
            }
            for _ in 0..blanks {
                value.push('\n');
            }
            blanks = 0;
            value.extend(seg.iter());
            value = value.trim_end_matches(' ').to_owned();
            ended = stop < l.body.len();
            k = k.saturating_add(1);
            next = k;
        }
        Ok((resolve_plain(&value, Some(line))?, next))
    }

    /// A flow node at the cursor.
    fn flow_node(&self, cur: &mut FlowCursor, depth: usize) -> Result<Node, PubspecError> {
        if depth > MAX_NESTING {
            return Err(self.err(cur.li, "unsupported nesting depth"));
        }
        self.flow_skip(cur)?;
        let line = self
            .line(cur.li)
            .ok_or_else(|| self.err(cur.li, "no line"))?;
        let Some(c) = line.body.get(cur.pos).copied() else {
            return Err(self.err(cur.li, "unterminated flow collection"));
        };
        match c {
            '[' => {
                cur.pos = cur.pos.saturating_add(1);
                let mut items = Vec::new();
                loop {
                    self.flow_skip(cur)?;
                    if self.flow_peek(cur) == Some(']') {
                        cur.pos = cur.pos.saturating_add(1);
                        return Ok(Node::Seq(items));
                    }
                    items.push(self.flow_node(cur, depth.saturating_add(1))?);
                    let item_line = cur.li;
                    self.flow_skip(cur)?;
                    match self.flow_peek(cur) {
                        Some(',') => cur.pos = cur.pos.saturating_add(1),
                        Some(']') => {}
                        Some(':') if cur.li == item_line => {
                            return Err(
                                self.err(cur.li, "unsupported mapping inside a flow sequence")
                            );
                        }
                        _ => return Err(self.err(cur.li, "expected ',' or ']' in a flow sequence")),
                    }
                }
            }
            '{' => {
                cur.pos = cur.pos.saturating_add(1);
                let mut entries = Vec::new();
                loop {
                    self.flow_skip(cur)?;
                    if self.flow_peek(cur) == Some('}') {
                        cur.pos = cur.pos.saturating_add(1);
                        return Ok(Node::Map(entries));
                    }
                    if matches!(self.flow_peek(cur), Some('[' | '{' | '?')) {
                        return Err(self.err(cur.li, "unsupported complex key in a flow mapping"));
                    }
                    let key = self.flow_node(cur, depth.saturating_add(1))?;
                    let key_line = cur.li;
                    self.flow_skip(cur)?;
                    let value = if self.flow_peek(cur) == Some(':') {
                        let line = self
                            .line(cur.li)
                            .ok_or_else(|| self.err(cur.li, "no line"))?;
                        if cur.li != key_line
                            || line
                                .body
                                .get(cur.pos.saturating_add(1))
                                .is_some_and(|n| *n != ' ')
                        {
                            return Err(self.err(cur.li, "unsupported ':' in a flow mapping"));
                        }
                        cur.pos = cur.pos.saturating_add(1);
                        self.flow_skip(cur)?;
                        if matches!(self.flow_peek(cur), Some(',' | '}')) {
                            Node::Null
                        } else {
                            self.flow_node(cur, depth.saturating_add(1))?
                        }
                    } else {
                        Node::Null
                    };
                    entries.push((key, value));
                    self.flow_skip(cur)?;
                    match self.flow_peek(cur) {
                        Some(',') => cur.pos = cur.pos.saturating_add(1),
                        Some('}') => {}
                        _ => return Err(self.err(cur.li, "expected ',' or '}' in a flow mapping")),
                    }
                }
            }
            '\'' | '"' => {
                let (value, end) = quoted_scalar(&line.body, cur.pos, line)?;
                cur.pos = end;
                Ok(Node::Str(value))
            }
            c if INDICATORS.contains(&c) => {
                Err(self.err(cur.li, &format!("unexpected {c:?} in a flow collection")))
            }
            _ => {
                let start = cur.pos;
                let mut i = start;
                while let Some(ch) = line.body.get(i) {
                    if matches!(ch, ',' | '[' | ']' | '{' | '}' | ':' | '?')
                        || (*ch == '#' && line.body.get(i.wrapping_sub(1)) == Some(&' '))
                    {
                        break;
                    }
                    i = i.saturating_add(1);
                }
                if line.body.get(i) == Some(&':')
                    && line
                        .body
                        .get(i.saturating_add(1))
                        .is_some_and(|n| *n != ' ')
                {
                    return Err(self.err(cur.li, "unsupported ':' in a flow scalar"));
                }
                if line.body.get(i) == Some(&'?') {
                    return Err(self.err(cur.li, "unsupported '?' in a flow scalar"));
                }
                cur.pos = i;
                let text: String = line.body.get(start..i).unwrap_or_default().iter().collect();
                resolve_plain(text.trim_end_matches(' '), Some(line))
            }
        }
    }

    /// The character at the cursor.
    fn flow_peek(&self, cur: &FlowCursor) -> Option<char> {
        self.line(cur.li)?.body.get(cur.pos).copied()
    }

    /// Skips blanks, comments and line ends inside a flow collection (LibYAML does not check indentation there).
    fn flow_skip(&self, cur: &mut FlowCursor) -> Result<(), PubspecError> {
        loop {
            let line = self
                .line(cur.li)
                .ok_or_else(|| self.err(cur.li, "unterminated flow collection"))?;
            while line.body.get(cur.pos) == Some(&' ') {
                cur.pos = cur.pos.saturating_add(1);
            }
            let at_comment = line.body.get(cur.pos) == Some(&'#')
                && (cur.pos == 0 || line.body.get(cur.pos.wrapping_sub(1)) == Some(&' '));
            if cur.pos < line.body.len() && !at_comment {
                return Ok(());
            }
            cur.li = cur.li.saturating_add(1);
            cur.pos = 0;
            if self.line(cur.li).is_none() {
                return Err(self.err(cur.li.saturating_sub(1), "unterminated flow collection"));
            }
        }
    }
}

/// The position of the flow-collection scanner.
#[derive(Debug)]
struct FlowCursor {
    /// Line index.
    li: usize,
    /// Character index in the line body.
    pos: usize,
}

/// Splits the document into lines, after the checks of LibYAML's reader (printable characters only) and the
/// document markers (one document; `%` directives and content on a `---` line are refused).
fn document_lines(text: &str) -> Result<Vec<SrcLine>, PubspecError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for c in text.chars() {
        match c {
            '\t' => return Err(bad(None, "unsupported tab character")),
            '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{feff}' => {
                return Err(bad(None, &format!("unsupported character {c:?}")));
            }
            '\n'
            | '\r'
            | ' '..='~'
            | '\u{a0}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'.. => {}
            _ => {
                return Err(bad(
                    None,
                    &format!("control character {c:?} is not allowed"),
                ));
            }
        }
    }
    // `open()` in text mode: universal newlines.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    let mut started = false;
    let mut ended = false;
    let mut explicit_start = false;
    for (index, raw) in normalized.split('\n').enumerate() {
        let no = index.saturating_add(1);
        let marker = |m: &str| {
            raw.strip_prefix(m)
                .is_some_and(|r| r.is_empty() || r.starts_with(' '))
        };
        let indent = raw.len().saturating_sub(raw.trim_start_matches(' ').len());
        let body: Vec<char> = raw.trim_start_matches(' ').chars().collect();
        let line = SrcLine { no, indent, body };
        if ended {
            if marker("...") || line.is_skippable() {
                continue;
            }
            return Err(bad(Some(&line), "expected a single document in the stream"));
        }
        if marker("---") {
            if started || explicit_start {
                return Err(bad(Some(&line), "expected a single document in the stream"));
            }
            explicit_start = true;
            let rest: Vec<char> = raw.chars().skip(3).collect();
            if !only_trailing_comment(&rest, 0) {
                return Err(bad(
                    Some(&line),
                    "unsupported content on the document start line",
                ));
            }
            continue;
        }
        if marker("...") {
            if !started && !explicit_start {
                return Err(bad(Some(&line), "document end marker without a document"));
            }
            ended = true;
            continue;
        }
        if !started && raw.starts_with('%') {
            return Err(bad(Some(&line), "unsupported directive"));
        }
        if !line.is_skippable() {
            started = true;
        }
        lines.push(line);
    }
    Ok(lines)
}

/// `yaml_load(pubspec.yaml)['executables']` as far as `_install_dir` iterates it.
///
/// The reader is a strict subset of YAML 1.1 as LibYAML/PyYAML read it: a top-level block mapping; block mappings,
/// block sequences (also indentless), flow collections (possibly over several lines), plain scalars (also
/// multi-line, resolved with PyYAML's implicit `null`/`bool`/`int`/`float` rules), single-line quoted scalars and
/// block scalars (skipped). Everything else — tabs, anchors, aliases, tags, directives, complex keys, multi-line
/// quoted scalars, timestamps, `<<` merges, documents after the first — is refused with an error, and so is every
/// input that is not valid YAML. Accepted inputs are checked against PyYAML 6.0.3 by
/// `testdata/group2/pubspec/vectors.jsonl`.
///
/// Total on arbitrary input: every outcome is a value or a [`PubspecError`].
pub fn pubspec_executables(text: &str) -> Result<Executables, PubspecError> {
    let reader = Reader {
        lines: document_lines(text)?,
    };
    let Some(Node::Map(entries)) = reader.document()? else {
        return Ok(Executables::EmptyDocument);
    };
    let mut value = None;
    for (key, v) in entries {
        if matches!(&key, Node::Str(k) if k == "executables") {
            value = Some(v);
        }
    }
    let Some(value) = value else {
        return Ok(Executables::Missing);
    };
    let unsupported = || bad(None, "unsupported block scalar as an `executables` entry");
    let mut names: Vec<String> = Vec::new();
    let non_str = |names: Vec<String>, ty| Ok(Executables::NamesThenNonStr(names, ty));
    match value {
        Node::Null => Ok(Executables::Null),
        Node::Other(ty) => Ok(Executables::NotIterable(ty)),
        Node::Opaque => Err(bad(
            None,
            "unsupported block scalar as the `executables` value",
        )),
        Node::Str(s) => Ok(Executables::Names(s.chars().map(String::from).collect())),
        Node::Map(entries) => {
            for (key, _) in entries {
                match key {
                    Node::Str(k) => {
                        if !names.contains(&k) {
                            names.push(k);
                        }
                    }
                    Node::Null => return non_str(names, "NoneType"),
                    Node::Other(ty) => return non_str(names, ty),
                    Node::Opaque | Node::Seq(_) | Node::Map(_) => return Err(unsupported()),
                }
            }
            Ok(Executables::Names(names))
        }
        Node::Seq(items) => {
            for item in items {
                match item {
                    Node::Str(k) => names.push(k),
                    Node::Null => return non_str(names, "NoneType"),
                    Node::Other(ty) => return non_str(names, ty),
                    Node::Seq(_) => return non_str(names, "list"),
                    Node::Map(_) => return non_str(names, "dict"),
                    Node::Opaque => return Err(unsupported()),
                }
            }
            Ok(Executables::Names(names))
        }
    }
}

/// A name `dart compile exe` may write as `<env>/bin/<name>`: one path component (not empty, `.`, `..`, no separator,
/// no NUL; on Windows no `:` either).
pub fn valid_executable_name(name: &str) -> bool {
    !(name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
        || (cfg!(windows) && name.contains(['\\', ':'])))
}

/// The `ValueError` of an executable name that is not a single path component.
pub fn invalid_name_message(name: &str) -> String {
    format!(
        "pubspec.yaml executable {} is not a single path component",
        snp_pycompat::repr::repr_str(name)
    )
}

/// Reads `<pkg>/pubspec.yaml` (`open(…)` + `yaml_load`). A file over [`MAX_PUBSPEC`] bytes is refused, never parsed
/// in part.
pub fn read_pubspec(pkg: &Path) -> Result<Executables, LangError> {
    use std::io::Read as _;
    let path = PathBuf::from(prefix_path(pkg, OsStr::new("pubspec.yaml")));
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|f| {
            f.take(MAX_PUBSPEC.saturating_add(1))
                .read_to_end(&mut bytes)
        })
        .map_err(io_err("open", &path))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PUBSPEC {
        return Err(LangError::Value(format!(
            "{}: pubspec.yaml is larger than {MAX_PUBSPEC} bytes",
            path.display()
        )));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        LangError::Value(format!(
            "{}: pubspec.yaml is not valid UTF-8",
            path.display()
        ))
    })?;
    pubspec_executables(&text).map_err(|e| LangError::Value(format!("{}: {e}", path.display())))
}

/// `_install_dir(prefix_p, pub_cache)` (`dart.py:48-66`).
fn install_dir(
    cx: &InstallCtx<'_>,
    pkg: &Path,
    pub_cache: &Path,
    bin_dir: &Path,
) -> Result<(), LangError> {
    let host = cx.cx.host;
    let env = with_pub_cache(host.base_env, pub_cache);
    let executables = read_pubspec(pkg)?;
    let argv: Vec<OsString> = vec!["dart".into(), "pub".into(), "get".into()];
    setup_cmd(host.runner, &argv, &env, pkg)?;
    // The loop of `_install_dir`; every failure below is one of pre-commit's `TypeError`/`KeyError` crashes (exit 3).
    let (names, non_str) = match executables {
        Executables::Names(n) => (n, None),
        Executables::NamesThenNonStr(n, ty) => (n, Some(ty)),
        Executables::Missing => {
            return Err(LangError::Value(format!(
                "{}: pubspec.yaml has no `executables` key",
                pkg.display()
            )));
        }
        Executables::EmptyDocument => {
            return Err(LangError::Value(
                "'NoneType' object is not subscriptable".to_owned(),
            ));
        }
        Executables::Null => {
            return Err(LangError::Value(
                "'NoneType' object is not iterable".to_owned(),
            ));
        }
        Executables::NotIterable(ty) => {
            return Err(LangError::Value(format!("'{ty}' object is not iterable")));
        }
    };
    if let Some(bad) = names.iter().find(|n| !valid_executable_name(n)) {
        return Err(LangError::Value(invalid_name_message(bad)));
    }
    for name in names {
        let output = bin_dir.join(win_exe(&name));
        let source = prefix_path(pkg, OsStr::new(&format!("bin/{name}.dart")));
        let argv: Vec<OsString> = vec![
            "dart".into(),
            "compile".into(),
            "exe".into(),
            "--output".into(),
            output.into_os_string(),
            source,
        ];
        setup_cmd(host.runner, &argv, &env, pkg)?;
    }
    if let Some(ty) = non_str {
        return Err(LangError::Value(format!(
            "join() argument must be str, bytes, or os.PathLike object, not '{ty}'"
        )));
    }
    Ok(())
}

/// `{**os.environ, 'PUB_CACHE': pub_cache}`.
fn with_pub_cache(env: &Env, pub_cache: &Path) -> Env {
    env.patched(&EnvPatch::default().set("PUB_CACHE", pub_cache.as_os_str()))
}

/// `os.walk(top)` (top-down, not following directory symlinks, sorted): the first directory directly containing a
/// non-directory entry named `pubspec.yaml`.
fn find_pubspec_dir(top: &Path, depth: usize) -> Result<Option<PathBuf>, LangError> {
    if depth > MAX_WALK_DEPTH {
        return Ok(None);
    }
    let names = listdir(top)?;
    let mut subdirs = Vec::new();
    let mut found = false;
    for name in names {
        let p = top.join(&name);
        let is_dir = std::fs::metadata(&p).is_ok_and(|m| m.is_dir());
        if is_dir {
            let is_link = std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
            if !is_link {
                subdirs.push(p);
            }
        } else if name == OsStr::new("pubspec.yaml") {
            found = true;
        }
    }
    if found {
        return Ok(Some(top.to_path_buf()));
    }
    for d in subdirs {
        if let Some(hit) = find_pubspec_dir(&d, depth.saturating_add(1))? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

impl Language for Dart {
    fn id(&self) -> LanguageId {
        LanguageId::Dart
    }

    /// `assert_version_default('dart', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Dart)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`dart.py:38-97`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Dart, cx)?;
        let host = cx.cx.host;
        let bin_dir = cx.cx.env_dir.join("bin");
        mkdir_new(&bin_dir)?;
        {
            let tmp = tempdir(host.base_env)?;
            install_dir(cx, cx.cx.prefix, tmp.path(), &bin_dir)?;
        }
        for dep_s in cx.deps {
            let dep_tmp = tempdir(host.base_env)?;
            let (dep, _, version) = snp_pycompat::text::partition(dep_s, ":");
            let mut argv: Vec<OsString> = vec![
                "dart".into(),
                "pub".into(),
                "cache".into(),
                "add".into(),
                dep.into(),
            ];
            if !version.is_empty() {
                argv.push("--version".into());
                argv.push(version.into());
            }
            let env = with_pub_cache(host.base_env, dep_tmp.path());
            setup_cmd(host.runner, &argv, &env, cx.cx.prefix)?;
            let Some(root) = find_pubspec_dir(dep_tmp.path(), 0)? else {
                return Err(LangError::Assertion(format!(
                    "could not find pubspec.yaml for {dep_s}"
                )));
            };
            let copied = tempdir(host.base_env)?;
            let pkg = copied.path().join("pkg");
            copytree(&root, &pkg)?;
            install_dir(cx, &pkg, dep_tmp.path(), &bin_dir)?;
        }
        Ok(outcome("dart"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)`: `<env>/bin` first on `PATH` (no `PUB_CACHE` at run time).
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(EnvPatch::default().prepend_path("PATH", [cx.env_dir.join("bin")]))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
