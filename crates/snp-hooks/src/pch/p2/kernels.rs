//! The byte-level cores of the P2 natives, without file system or git access: what fuzz targets and unit tests
//! drive. Each function documents the upstream source it reproduces; none panics on any input.
//!
//! Owner: WP-27.

use snp_core::failure::PyExc;
use snp_pycompat::text::{bytes_lstrip, bytes_rstrip, bytes_strip};

/// `sort_simple_yaml.sort` (`sort_simple_yaml.py:29-121`) over the file's text as `readlines()` sees it (decoded,
/// universal newlines): `Some(new text)` when the sorted lines differ from the file's `rstrip()`ed lines.
///
/// The header (leading `#` lines) stays first; the remaining blocks (runs of non-empty lines) are stably sorted
/// by their first non-comment line with one leading quote removed, joined by single empty lines. `str.rstrip()`
/// strips Python's full whitespace set (NBSP, `\x1c`…), so trailing whitespace alone never triggers a rewrite; a
/// file that is exactly `\n` is rewritten on every run (09 §0 #5).
pub fn sort_simple_yaml(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .split_inclusive('\n')
        .map(snp_pycompat::text::rstrip)
        .collect();
    let mut i = 0_usize;
    let mut new: Vec<&str> = Vec::new();
    // `parse_block(lines, header=True)`.
    while let Some(l) = lines.get(i)
        && !l.is_empty()
        && l.starts_with('#')
    {
        new.push(l);
        i = i.saturating_add(1);
    }
    // `parse_blocks(lines)`.
    let mut blocks: Vec<&[&str]> = Vec::new();
    while let Some(l) = lines.get(i) {
        if l.is_empty() {
            i = i.saturating_add(1);
            continue;
        }
        let start = i;
        while lines.get(i).is_some_and(|l| !l.is_empty()) {
            i = i.saturating_add(1);
        }
        blocks.push(lines.get(start..i).unwrap_or_default());
    }
    blocks.sort_by_key(|b| first_key(b));
    for b in blocks {
        if !new.is_empty() {
            new.push("");
        }
        new.extend_from_slice(b);
    }
    if lines == new {
        return None;
    }
    let mut out = new.join("\n");
    out.push('\n');
    Some(out)
}

/// `first_key(lines)`: the first line not starting with `#`, without one leading `'` or `"`; `''` if none.
fn first_key<'a>(block: &[&'a str]) -> &'a str {
    for line in block {
        if line.starts_with('#') {
            continue;
        }
        return line
            .strip_prefix('\'')
            .or_else(|| line.strip_prefix('"'))
            .unwrap_or(line);
    }
    ""
}

/// The regular expression check-vcs-permalinks compiles for `domain` at `epoch` (1 = v3.0.0…v3.3.0: the fixed
/// `github.com` pattern; 2 = v3.4.0: per domain, `blob/master` only; 3 = v4.0.0+: any ref that is not a 4–64
/// digit hex id). The domain is inserted unescaped, as upstream does.
pub fn permalink_pattern(domain: &str, epoch: u16) -> String {
    match epoch {
        0 | 1 => r"https://github.com/[^/ ]+/[^/ ]+/blob/master/[^# ]+#L\d+".to_owned(),
        2 => format!(r"https://{domain}/[^/ ]+/[^/ ]+/blob/master/[^# ]+#L\d+"),
        _ => format!(
            r"https://{domain}/[^/ ]+/[^/ ]+/blob/(?![a-fA-F0-9]{{4,64}}/)([^/. ]+)/[^# ]+#L\d+"
        ),
    }
}

/// One `git diff --diff-filter=A --raw` line of forbid-new-submodules: `Ok(Some(path))` for a new gitlink
/// (`split(' ')[1] == '160000'`), `Ok(None)` for any other added entry; `Err` is Python's `ValueError` (no tab)
/// or `IndexError` (no space in the metadata).
pub fn new_submodule(line: &str) -> Result<Option<&str>, PyExc> {
    let Some((meta, path)) = line.split_once('\t') else {
        return Err(PyExc::new(
            "ValueError",
            "not enough values to unpack (expected 2, got 1)",
        ));
    };
    match meta.split(' ').nth(1) {
        Some(mode) => Ok((mode == "160000").then_some(path)),
        None => Err(PyExc::new("IndexError", "list index out of range")),
    }
}

/// forbid-new-submodules' footer (after the per-submodule lines).
pub const SUBMODULE_FOOTER: [&str; 5] = [
    "",
    "This commit introduces new submodules.",
    "Did you unintentionally `git add .`?",
    "To fix: git rm {thesubmodule}  # no trailing slash",
    "Also check .gitmodules",
];

/// fix-encoding-pragma's default pragma.
pub const DEFAULT_PRAGMA: &[u8] = b"# -*- coding: utf-8 -*-";

/// `fix_encoding_pragma.has_coding(line)`.
fn has_coding(line: &[u8]) -> bool {
    if bytes_strip(line).is_empty() {
        return false;
    }
    let has = |n: &[u8]| memchr::memmem::find(line, n).is_some();
    bytes_lstrip(line).first() == Some(&b'#')
        && (has(b"unicode") || has(b"encoding") || has(b"coding:") || has(b"coding="))
}

/// `f.readline()` of a binary file: up to and including the next `\n`.
fn readline(b: &[u8]) -> (&[u8], &[u8]) {
    let end = memchr::memchr(b'\n', b).map_or(b.len(), |i| i.saturating_add(1));
    b.split_at_checked(end).unwrap_or((b, &[]))
}

/// `fix_encoding_pragma.fix_encoding_pragma(f, remove, expected_pragma)` (≤ v5.0.0): `Some(new bytes)` when the
/// file is rewritten (return value 1). A file whose only content is a shebang and/or a pragma becomes empty.
pub fn fix_encoding_pragma(content: &[u8], remove: bool, pragma: &[u8]) -> Option<Vec<u8>> {
    let (first, tail) = readline(content);
    let (second, rest) = readline(tail);
    let ending: &[u8] = if first.ends_with(b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let (shebang, potential, mut rest_v): (&[u8], &[u8], Vec<u8>) = if first.starts_with(b"#!") {
        (first, second, rest.to_vec())
    } else {
        (b"", first, tail.to_vec())
    };
    let crlf = |b: &u8| *b == b'\r' || *b == b'\n';
    let trimmed_end = potential
        .iter()
        .rposition(|b| !crlf(b))
        .map_or(0, |i| i.saturating_add(1));
    // True: the expected pragma; None: another coding line; False: none.
    let status: Option<bool> = if potential.get(..trimmed_end) == Some(pragma) {
        Some(true)
    } else if has_coding(potential) {
        None
    } else {
        let mut v = potential.to_vec();
        v.extend_from_slice(&rest_v);
        rest_v = v;
        Some(false)
    };
    if bytes_strip(&rest_v).is_empty() {
        return (status != Some(false) || !shebang.is_empty()).then(Vec::new);
    }
    if status == Some(!remove) {
        return None;
    }
    let mut out = shebang.to_vec();
    if !remove {
        out.extend_from_slice(pragma);
        out.extend_from_slice(ending);
    }
    out.extend_from_slice(&rest_v);
    Some(out)
}

/// `_normalize_pragma(pragma)`: `pragma.encode().rstrip()`.
pub fn normalize_pragma(p: &[u8]) -> &[u8] {
    bytes_rstrip(p)
}

/// The name-tests-test pattern at `epoch` (1 = v3.0.0…v4.2.0: `--django` or the pytest default; 2 = v4.3.0+:
/// the option the mutually exclusive group stored).
pub fn tests_pattern(django: bool) -> &'static str {
    if django {
        r"test.*\.py"
    } else {
        r".*_test\.py"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssy(t: &str) -> Option<String> {
        sort_simple_yaml(t)
    }

    #[test]
    fn sort_simple_yaml_table() {
        assert_eq!(ssy("b: 1  \n").as_deref(), None);
        assert_eq!(ssy("\n").as_deref(), Some("\n"));
        assert_eq!(ssy("\n\n").as_deref(), Some("\n"));
        assert_eq!(ssy(""), None);
        assert_eq!(
            ssy("# c\nb: 1\n\na: 2").as_deref(),
            Some("# c\n\na: 2\n\nb: 1\n")
        );
        assert_eq!(ssy("a: 2\u{a0}\nb: 1\n"), None);
        assert_eq!(
            ssy("b: 1\n\n\"a\": 2\n").as_deref(),
            Some("\"a\": 2\n\nb: 1\n")
        );
    }

    #[test]
    fn submodule_lines() {
        assert_eq!(
            new_submodule(":000000 160000 0000000 abc1234 A\tsub").unwrap(),
            Some("sub")
        );
        assert_eq!(new_submodule(":000000 100644 0 1 A\tf").unwrap(), None);
        assert!(new_submodule("x").is_err());
        assert!(new_submodule("x\ty").is_err());
    }

    #[test]
    fn encoding_pragma() {
        let p = DEFAULT_PRAGMA;
        let s = |b: Option<Vec<u8>>| b.map(|b| String::from_utf8(b).unwrap());
        assert_eq!(
            s(fix_encoding_pragma(b"x = 1\n", false, p)).as_deref(),
            Some("# -*- coding: utf-8 -*-\nx = 1\n")
        );
        assert_eq!(
            s(fix_encoding_pragma(
                b"#!/usr/bin/env python\r\nx\r\n",
                false,
                p
            ))
            .as_deref(),
            Some("#!/usr/bin/env python\r\n# -*- coding: utf-8 -*-\r\nx\r\n")
        );
        assert_eq!(
            fix_encoding_pragma(b"# -*- coding: utf-8 -*-\nx\n", false, p),
            None
        );
        assert_eq!(
            s(fix_encoding_pragma(
                b"# -*- coding: utf-8 -*-\nx\n",
                true,
                p
            ))
            .as_deref(),
            Some("x\n")
        );
        assert_eq!(
            s(fix_encoding_pragma(b"# coding: latin-1\nx\n", false, p)).as_deref(),
            Some("# -*- coding: utf-8 -*-\nx\n")
        );
        assert_eq!(
            s(fix_encoding_pragma(b"#!/bin/sh\n", false, p)).as_deref(),
            Some("")
        );
        assert_eq!(fix_encoding_pragma(b"", false, p), None);
        assert_eq!(fix_encoding_pragma(b"\n\n", true, p), None);
    }
}
