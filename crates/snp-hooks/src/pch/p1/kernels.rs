//! The byte-level cores of the P1 natives, without file system or git access: what fuzz targets and unit tests
//! drive. Each function documents the upstream source it reproduces; none panics on any input.
//!
//! Owner: WP-27.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use memchr::memchr_iter;
use memchr::memmem;
use snp_core::failure::PyExc;
use snp_pycompat::chars::PyChar;
use snp_pycompat::text::{bytes_lstrip, bytes_rstrip, bytes_strip};
use snp_pyparsers::json::{DumpOpts, PyMinor};

/// The text-mode read helpers the natives share (strict whole-file decode; chunk-relative line-read errors).
pub use super::util::{chunked_decode_error, text_read, text_read_lines};

/// `list(f)` of a binary file: lines split after each `\n` (a lone `\r` is not a break); the last line may lack
/// its `\n`.
pub fn binary_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0_usize;
    for nl in memchr_iter(b'\n', content) {
        let end = nl.saturating_add(1);
        lines.push(content.get(start..end).unwrap_or_default());
        start = end;
    }
    if start < content.len() {
        lines.push(content.get(start..).unwrap_or_default());
    }
    lines
}

/// `bytes.strip(b'\n\r')` (both ends) or `bytes.rstrip(b'\n\r')`.
fn strip_crlf(line: &[u8], both: bool) -> &[u8] {
    let is = |b: &u8| *b == b'\n' || *b == b'\r';
    let end = line
        .iter()
        .rposition(|b| !is(b))
        .map_or(0, |i| i.saturating_add(1));
    let start = if both {
        line.iter().position(|b| !is(b)).unwrap_or(end).min(end)
    } else {
        0
    };
    line.get(start..end).unwrap_or_default()
}

/// The options of [`file_contents_sort`] (the console script's flags and its epoch).
#[derive(Copy, Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "two flags and two epoch switches of one console script"
)]
pub struct SortOpts {
    /// `--ignore-case` (`key=bytes.lower`: ASCII `A-Z` only).
    pub ignore_case: bool,
    /// `--unique` (`set(lines)`).
    pub unique: bool,
    /// Epochs 1–2 (v3.0.0…v3.3.0): lines are `strip(b'\n\r')`ed at both ends (later `rstrip`).
    pub strip_both: bool,
    /// Epoch 4+ (v4.5.0): an empty result stays empty (before, the result always ends with `\n`).
    pub empty_stays_empty: bool,
}

/// `file_contents_sorter.sort_file_contents` (`file_contents_sorter.py:25-51`): `Some(new bytes)` when the sorted
/// text differs from the file.
///
/// Blank lines (ASCII-WS only) are dropped, CR/LF stripped, the rest sorted bytewise (stable; `--ignore-case`
/// sorts by the ASCII-lowered bytes) and joined with `\n`. With `--unique` Python iterates a `set` before sorting;
/// lines whose sort keys tie (only possible with both flags, v4.5.0…v5.0.0) come out in hash order there and in
/// byte order here.
pub fn file_contents_sort(content: &[u8], o: SortOpts) -> Option<Vec<u8>> {
    let mut lines: Vec<&[u8]> = binary_lines(content)
        .into_iter()
        .filter(|l| !bytes_strip(l).is_empty())
        .map(|l| strip_crlf(l, o.strip_both))
        .collect();
    if o.unique {
        let mut seen: HashSet<&[u8]> = HashSet::new();
        lines.retain(|l| seen.insert(l));
        lines.sort_unstable();
    }
    if o.ignore_case {
        lines.sort_by(|a, b| {
            a.iter()
                .map(u8::to_ascii_lowercase)
                .cmp(b.iter().map(u8::to_ascii_lowercase))
        });
    } else {
        lines.sort();
    }
    let mut after = lines.join(&b'\n');
    if !after.is_empty() || !o.empty_stays_empty {
        after.push(b'\n');
    }
    (after != content).then_some(after)
}

/// One `Requirement` of `requirements_txt_fixer.py`.
#[derive(Debug, Default)]
struct Requirement<'a> {
    value: Option<Vec<u8>>,
    comments: Vec<&'a [u8]>,
}

impl Requirement<'_> {
    /// `is_complete()`: a value that does not end in a line continuation.
    fn is_complete(&self) -> bool {
        self.value
            .as_deref()
            .is_some_and(|v| !strip_crlf(v, false).ends_with(b"\\"))
    }

    fn is_header(&self) -> bool {
        self.value.as_deref() == Some(b"\n")
    }
}

/// `Requirement.name` (`requirements_txt_fixer.py:21-37`): `None` is the `AssertionError` of `UNTIL_SEP.match`
/// failing (a value starting with whitespace or `;`).
pub fn requirement_name(value: &[u8]) -> Option<Vec<u8>> {
    let name = value.to_ascii_lowercase();
    for egg in [&b"#egg="[..], b"&egg="] {
        if memmem::find(value, egg).is_some() {
            let at = memmem::find(&name, egg).unwrap_or(name.len());
            return Some(
                name.get(at.saturating_add(egg.len())..)
                    .unwrap_or_default()
                    .to_vec(),
            );
        }
    }
    // `UNTIL_SEP = rb'[^;\s]+'`, matched at the start (bytes `\s` = ASCII whitespace).
    let sep = name
        .iter()
        .position(|&b| b == b';' || snp_pycompat::chars::is_bytes_space(b))
        .unwrap_or(name.len());
    if sep == 0 {
        return None;
    }
    let head = name.get(..sep).unwrap_or_default();
    // `UNTIL_COMPARISON = b'={2,3}|!=|~=|>=?|<=?'`: the first position any alternative matches.
    let cut = (0..head.len()).find(|&i| {
        let c = head.get(i).copied();
        let d = head.get(i.saturating_add(1)).copied();
        matches!(c, Some(b'>' | b'<')) || (matches!(c, Some(b'=' | b'!' | b'~')) && d == Some(b'='))
    });
    Some(
        head.get(..cut.unwrap_or(head.len()))
            .unwrap_or_default()
            .to_vec(),
    )
}

/// `requirements_txt_fixer.fix_requirements` (`requirements_txt_fixer.py:68-142`) at `epoch` (1 = v3.1.0…v3.4.0,
/// 2 = v4.0.0…v4.5.0: indented comments, 3 = v4.6.0: duplicates removed and commented entries first, 4 = v5.0.0+:
/// `pkg_resources==0.0.0` removed too). `Ok(Some(new bytes))` when the file must be rewritten; `Err` is the
/// `AssertionError` crash of a value whose name cannot be computed, raised when `sorted()` compares it (Python only
/// compares when two or more non-header requirements remain).
pub fn requirements_fix(content: &[u8], epoch: u16) -> Result<Option<Vec<u8>>, PyExc> {
    if bytes_strip(content).is_empty() {
        return Ok(None);
    }
    let fixed_last: Vec<u8>;
    let mut lines = binary_lines(content);
    if let Some(last) = lines.last_mut()
        && !last.ends_with(b"\n")
    {
        let mut v = last.to_vec();
        v.push(b'\n');
        fixed_last = v;
        *last = &fixed_last;
    }
    let mut reqs: Vec<Requirement<'_>> = Vec::new();
    for line in lines {
        if reqs.last().is_none_or(Requirement::is_complete) {
            reqs.push(Requirement::default());
        }
        let single = reqs.len() == 1;
        let Some(r) = reqs.last_mut() else {
            continue;
        };
        let blank = bytes_strip(line).is_empty();
        let comment = if epoch >= 2 {
            bytes_lstrip(line).starts_with(b"#")
        } else {
            line.starts_with(b"#")
        };
        if single && blank {
            if r.comments.first().is_some_and(|c| c.starts_with(b"#")) {
                r.value = Some(b"\n".to_vec());
            } else {
                r.comments.push(line);
            }
        } else if comment || blank {
            r.comments.push(line);
        } else {
            match &mut r.value {
                Some(v) => v.extend_from_slice(line),
                None => r.value = Some(line.to_vec()),
            }
        }
    }
    let rest = if reqs.last().is_some_and(|r| r.value.is_none()) {
        reqs.pop().map(|r| r.comments).unwrap_or_default()
    } else {
        Vec::new()
    };
    reqs.retain(|r| {
        let v = r.value.as_deref();
        v != Some(b"pkg-resources==0.0.0\n") && (epoch < 4 || v != Some(b"pkg_resources==0.0.0\n"))
    });
    // `sorted(requirements)` with `Requirement.__lt__`: the header first, then by name; from v4.6.0 entries with
    // comments before those without on equal names. `__lt__` with the header never computes a name, so names are
    // needed (and may fail) only when two non-header requirements must be compared.
    let non_header = reqs.iter().filter(|r| !r.is_header()).count();
    let mut keyed: Vec<(bool, Vec<u8>, bool, Requirement<'_>)> = Vec::new();
    for r in reqs {
        let header = r.is_header();
        let name = if header || non_header < 2 {
            Vec::new()
        } else {
            match requirement_name(r.value.as_deref().unwrap_or_default()) {
                Some(n) => n,
                None => return Err(PyExc::new("AssertionError", Vec::new())),
            }
        };
        let no_comments = epoch >= 3 && r.comments.is_empty();
        keyed.push((!header, name, no_comments, r));
    }
    keyed.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
    let mut after: Vec<u8> = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    for (_, _, _, r) in &keyed {
        for c in &r.comments {
            after.extend_from_slice(c);
        }
        let value = r.value.as_deref().unwrap_or_default();
        if epoch < 3 || prev.as_deref() != Some(value) {
            after.extend_from_slice(value);
            prev = Some(value.to_vec());
        }
    }
    for c in rest {
        after.extend_from_slice(c);
    }
    Ok((after != content).then_some(after))
}

/// The text `pretty_format_json._get_pretty_format(contents, …)` returns, as code points with its lone surrogates
/// kept (Rust strings cannot hold them): what Python later encodes (the diff, or the autofix write) and fails on.
/// Rendered exactly through the `ensure_ascii=True` encoder, which escapes every surrogate and so keeps keys,
/// duplicate merging and order intact, with an all-space indent of the same length (the real indent, read back
/// from a probe rendering, is put back after each newline); the escapes `ensure_ascii=False` would not emit (code
/// points from U+007F up, astral pairs joined) are then decoded. `None` when the contents do not render (decode
/// error, `TypeError`, resource caps) or the rendering is not the expected shape.
pub fn pretty_json_text(contents: &str, opts: &DumpOpts, minor: PyMinor) -> Option<Vec<u32>> {
    let probe_opts = DumpOpts {
        indent: opts.indent.clone(),
        ensure_ascii: true,
        sort_keys: false,
        top_keys: Vec::new(),
    };
    // `[[0]]` renders as `[\n I [\n I I 0\n I ]\n]\n`: ten fixed characters and four copies of the indent `I`.
    let probe: Vec<char> = snp_pyparsers::json::pretty_format("[[0]]", &probe_opts, minor)
        .ok()?
        .chars()
        .collect();
    let extra = probe.len().checked_sub(10)?;
    if extra % 4 != 0 {
        return None;
    }
    let k = extra / 4;
    let indent = probe.get(2..k.checked_add(2)?)?;
    let mut spaces = String::new();
    spaces.try_reserve_exact(k).ok()?;
    spaces.extend(std::iter::repeat_n(' ', k));
    let spaced = DumpOpts {
        indent: spaces,
        ensure_ascii: true,
        ..opts.clone()
    };
    let ascii: Vec<char> = snp_pyparsers::json::pretty_format(contents, &spaced, minor)
        .ok()?
        .chars()
        .collect();
    let hex4 = |at: usize| -> Option<u32> {
        let digits: String = ascii.get(at..at.checked_add(4)?)?.iter().collect();
        u32::from_str_radix(&digits, 16).ok()
    };
    let mut out: Vec<u32> = Vec::new();
    out.try_reserve(ascii.len()).ok()?;
    let mut i = 0_usize;
    while let Some(&c) = ascii.get(i) {
        i = i.saturating_add(1);
        match c {
            // Only a `newline_indent` puts a raw newline (strings escape theirs); tokens never start with a space.
            '\n' => {
                out.push(0x0A);
                let mut m = 0_usize;
                while k > 0 && ascii.get(i) == Some(&' ') {
                    m = m.saturating_add(1);
                    i = i.saturating_add(1);
                }
                if let Some(levels) = m.checked_div(k) {
                    if levels.checked_mul(k) != Some(m) {
                        return None;
                    }
                    for _ in 0..levels {
                        out.extend(indent.iter().map(|&c| u32::from(c)));
                    }
                }
            }
            // Backslashes appear only in string escapes (the indent is all spaces here).
            '\\' => {
                let e = *ascii.get(i)?;
                i = i.saturating_add(1);
                if e != 'u' {
                    out.extend([0x5C, u32::from(e)]);
                    continue;
                }
                let v = hex4(i)?;
                if v < 0x7F {
                    out.extend([0x5C, 0x75]);
                    out.extend(
                        ascii
                            .get(i..i.saturating_add(4))?
                            .iter()
                            .map(|&c| u32::from(c)),
                    );
                    i = i.saturating_add(4);
                    continue;
                }
                i = i.saturating_add(4);
                let pair = (0xD800..0xDC00).contains(&v)
                    && ascii.get(i) == Some(&'\\')
                    && ascii.get(i.saturating_add(1)) == Some(&'u');
                match pair.then(|| hex4(i.saturating_add(2))).flatten() {
                    Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                        let hi = v.saturating_sub(0xD800) << 10;
                        out.push(
                            0x1_0000_u32
                                .saturating_add(hi)
                                .saturating_add(lo.saturating_sub(0xDC00)),
                        );
                        i = i.saturating_add(6);
                    }
                    _ => out.push(v),
                }
            }
            _ => out.push(u32::from(c)),
        }
    }
    Some(out)
}

/// The character position at which Python's encode of the pretty text fails on its first lone surrogate: in
/// `get_diff(contents, pretty, file).encode()` (`windows_write` `None`), or in `open(f, 'w').write(pretty)`
/// (`Some(windows)`: on Windows each `\n` before it was written as `\r\n` first). `None` when the pretty text holds
/// no lone surrogate (or does not render, see [`pretty_json_text`]).
///
/// The diff is computed with each lone surrogate mapped injectively onto a block of private-use code points that
/// occurs nowhere in the contents, the pretty text or the file name, so line equality is exactly Python's.
pub fn pretty_json_crash_position(
    contents: &str,
    opts: &DumpOpts,
    minor: PyMinor,
    file: &str,
    windows_write: Option<bool>,
) -> Option<usize> {
    let text = pretty_json_text(contents, opts, minor)?;
    let is_surrogate = |c: u32| (0xD800..0xE000).contains(&c);
    let first = text.iter().position(|&c| is_surrogate(c))?;
    if let Some(windows) = windows_write {
        let newlines = if windows {
            text.get(..first)?.iter().filter(|&&c| c == 0x0A).count()
        } else {
            0
        };
        return first.checked_add(newlines);
    }
    let in_block = |base: u32, c: u32| (base..base.saturating_add(0x800)).contains(&c);
    let base = (0x1E0_u32..0x220).map(|block| block << 11).find(|&base| {
        !text.iter().any(|&c| in_block(base, c))
            && !contents
                .chars()
                .chain(file.chars())
                .any(|c| in_block(base, u32::from(c)))
    })?;
    let dst: String = text
        .iter()
        .map(|&c| {
            if is_surrogate(c) {
                char::from_u32(base.saturating_add(c.saturating_sub(0xD800)))
            } else {
                char::from_u32(c)
            }
        })
        .collect::<Option<String>>()?;
    let src: Vec<&str> = snp_pycompat::text::splitlines_keepends(contents).collect();
    let dst: Vec<&str> = snp_pycompat::text::splitlines_keepends(&dst).collect();
    let diff = snp_pyparsers::difflib::unified_diff(&src, &dst, file, file);
    diff.chars().position(|c| in_block(base, u32::from(c)))
}

/// `str.lower()` of a Python string held as its file-system bytes (UTF-8 with `surrogateescape`): valid text is
/// lowercased with CPython's data (UCD 15.1.0 / 15.0.0, the generated `lower_table`, never the toolchain's newer Unicode
/// tables, so letters that gained a lowercase form after 15.1 stay as they are, as in Python), CPython's final-sigma
/// rule (`handle_capital_sigma`) included; undecodable bytes (lone surrogates in Python, neither cased nor
/// case-ignorable, so they end a sigma context scan exactly as a chunk boundary does) stay as they are.
pub fn py_lower(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0_u8; 4];
    for chunk in s.utf8_chunks() {
        let valid = chunk.valid();
        for (i, c) in valid.char_indices() {
            let lower = match c {
                '\u{130}' => {
                    out.extend_from_slice("i\u{307}".as_bytes());
                    continue;
                }
                '\u{3A3}' => {
                    let before = valid.get(..i).unwrap_or_default();
                    let after = valid
                        .get(i.saturating_add(c.len_utf8())..)
                        .unwrap_or_default();
                    if final_sigma(before, after) {
                        '\u{3C2}'
                    } else {
                        '\u{3C3}'
                    }
                }
                _ => py_lower_char(c),
            };
            out.extend_from_slice(lower.encode_utf8(&mut buf).as_bytes());
        }
        out.extend_from_slice(chunk.invalid());
    }
    out
}

/// CPython `handle_capital_sigma`: final when the nearest non-case-ignorable character before is cased and the
/// nearest one after (if any) is not. Each scan stops at the next capital sigma (cased), so the total work over a
/// string stays linear.
fn final_sigma(before: &str, after: &str) -> bool {
    let first = |mut it: std::str::Chars<'_>, rev: bool| {
        let mut next = || if rev { it.next_back() } else { it.next() };
        loop {
            match next() {
                Some(c) if sigma_class(c) == 1 => {}
                other => return other,
            }
        }
    };
    let cased = |c: char| sigma_class(c) == 2;
    first(before.chars(), true).is_some_and(cased)
        && !first(after.chars(), false).is_some_and(cased)
}

/// The [`super::lower_table::SIGMA_CONTEXT`] class of `c` (0 = neither cased nor case-ignorable).
fn sigma_class(c: char) -> u8 {
    let cp = u32::from(c);
    let table = super::lower_table::SIGMA_CONTEXT;
    let i = table.partition_point(|&(first, _, _)| first <= cp);
    match i.checked_sub(1).and_then(|i| table.get(i)) {
        Some(&(_, last, class)) if cp <= last => class,
        _ => 0,
    }
}

/// CPython's one-code-point lowercase mapping of `c` (identity when [`super::lower_table::LOWER`] has no row).
fn py_lower_char(c: char) -> char {
    let cp = u32::from(c);
    super::lower_table::LOWER
        .binary_search_by_key(&cp, |&(k, _)| k)
        .ok()
        .and_then(|i| super::lower_table::LOWER.get(i))
        .and_then(|&(_, l)| char::from_u32(l))
        .unwrap_or(c)
}

/// Python `str` ordering (by code point) of two file-system strings (`surrogateescape`: an undecodable byte is
/// the code point U+DC00 + byte).
pub fn py_str_cmp(a: &[u8], b: &[u8]) -> Ordering {
    let cp = |c: PyChar| match c {
        PyChar::Scalar(c) => u32::from(c),
        PyChar::Surrogate(s) => u32::from(s),
    };
    snp_pycompat::codec::fsdecode_chars(a)
        .map(cp)
        .cmp(snp_pycompat::codec::fsdecode_chars(b).map(cp))
}

/// `check_case_conflict.parents(file)` at `epoch` (2: `os.path.dirname` until empty; 3: `split('/')` prefixes,
/// the empty one included).
fn parents(file: &[u8], epoch: u16, windows: bool, out: &mut Vec<Vec<u8>>) {
    if epoch >= 3 {
        for i in memchr_iter(b'/', file)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            out.push(file.get(..i).unwrap_or_default().to_vec());
        }
        return;
    }
    let mut cur = file.to_vec();
    loop {
        let up = if windows {
            snp_pycompat::ntpath::dirname(&cur)
        } else {
            snp_pycompat::posixpath::dirname(&cur).to_vec()
        };
        // Python loops forever on a root (`dirname('/') == '/'`); stop at the fixpoint instead.
        if up.is_empty() || up == cur {
            if !up.is_empty() {
                out.push(up);
            }
            return;
        }
        out.push(up.clone());
        cur = up;
    }
}

/// `check_case_conflict.find_conflicting_filenames` (`check_case_conflict.py:28-56`) over the hook's inputs:
/// `repo` = `git ls-files` lines, `files` = the argv file names, `added` = `added_files()`. Returns the names to
/// report, sorted as Python sorts `str`. Epoch 1 (v3.0.0…v3.4.0) has no parent directories.
pub fn case_conflicts(
    repo: &[&[u8]],
    files: &[&[u8]],
    added: &[&[u8]],
    epoch: u16,
    windows: bool,
) -> Vec<Vec<u8>> {
    let with_parents = |names: &mut BTreeSet<Vec<u8>>| {
        if epoch >= 2 {
            let mut extra = Vec::new();
            for n in names.iter() {
                parents(n, epoch, windows, &mut extra);
            }
            names.extend(extra);
        }
    };
    let mut repo_files: BTreeSet<Vec<u8>> = repo.iter().map(|r| r.to_vec()).collect();
    with_parents(&mut repo_files);
    let mut relevant: BTreeSet<Vec<u8>> = files.iter().chain(added).map(|r| r.to_vec()).collect();
    with_parents(&mut relevant);
    repo_files.retain(|r| !relevant.contains(r));
    let repo_lower: BTreeSet<Vec<u8>> = repo_files.iter().map(|r| py_lower(r)).collect();
    let mut counts: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for r in &relevant {
        let c = counts.entry(py_lower(r)).or_insert(0);
        *c = c.saturating_add(1);
    }
    let conflicts: BTreeSet<&Vec<u8>> = counts
        .iter()
        .filter(|(l, n)| **n > 1 || repo_lower.contains(*l))
        .map(|(l, _)| l)
        .collect();
    if conflicts.is_empty() {
        return Vec::new();
    }
    let mut names: Vec<Vec<u8>> = repo_files
        .iter()
        .chain(relevant.iter())
        .filter(|x| conflicts.contains(&py_lower(x)))
        .cloned()
        .collect();
    names.sort_by(|a, b| py_str_cmp(a, b));
    names
}

/// check-shebang-scripts-are-executable's message (`_message`, stderr) at `epoch` (1 = v4.0.0…v4.2.0, 2 =
/// v4.3.0…v5.0.0: `git add --chmod=+x` line, 3 = v6.0.0: typo `If it not` fixed). The text ends with the
/// message's own `\n` plus `print`'s.
pub fn shebang_scripts_message(path: &[u8], epoch: u16) -> Vec<u8> {
    let q = snp_pycompat::shlex::quote_bytes(path);
    let mut m = path.to_vec();
    m.extend_from_slice(b": has a shebang but is not marked executable!\n");
    m.extend_from_slice(b"  If it is supposed to be executable, try: `chmod +x ");
    m.extend_from_slice(&q);
    m.extend_from_slice(b"`\n");
    if epoch >= 2 {
        m.extend_from_slice(b"  If on Windows, you may also need to: `git add --chmod=+x ");
        m.extend_from_slice(&q);
        m.extend_from_slice(b"`\n");
    }
    if epoch >= 3 {
        m.extend_from_slice(
            b"  If it is not supposed to be executable, double-check its shebang is wanted.\n\n",
        );
    } else {
        m.extend_from_slice(
            b"  If it not supposed to be executable, double-check its shebang is wanted.\n\n",
        );
    }
    m
}

/// The records of `git ls-files -z --stage` output (decoded) as `check_executables_have_shebangs.git_ls_files`
/// reads them: `zsplit`, then `metadata, filename = out.split('\t')` and `mode, _, _ = metadata.split()`, giving
/// `(mode, path)`. The entries before a malformed record come back with Python's unpacking `ValueError` for it.
pub fn ls_files_records(outs: &str) -> (Vec<(String, String)>, Option<PyExc>) {
    let mut entries = Vec::new();
    let trimmed = outs.trim_matches('\0');
    if trimmed.is_empty() {
        return (entries, None);
    }
    for rec in trimmed.split('\0') {
        let tabs = rec.split('\t').count();
        let mut parts = rec.split('\t');
        let (Some(meta), Some(path), None) = (parts.next(), parts.next(), parts.next()) else {
            return (entries, Some(unpack_error(tabs, 2)));
        };
        let fields = snp_pycompat::text::split_whitespace(meta, None).collect::<Vec<&str>>();
        let [mode, _, _] = fields.as_slice() else {
            return (entries, Some(unpack_error(fields.len(), 3)));
        };
        entries.push(((*mode).to_owned(), path.to_owned()));
    }
    (entries, None)
}

/// Python's tuple-unpacking `ValueError` for `got` items into `expected` names.
fn unpack_error(got: usize, expected: usize) -> PyExc {
    PyExc::new(
        "ValueError",
        if got > expected {
            format!("too many values to unpack (expected {expected})")
        } else {
            format!("not enough values to unpack (expected {expected}, got {got})")
        },
    )
}

/// `any(b in EXECUTABLE_VALUES for b in mode[-3:])` (`check_executables_have_shebangs.py`): an executable bit in
/// the last three octal digits of an index mode.
pub fn mode_is_executable(mode: &str) -> bool {
    let tail = mode
        .char_indices()
        .rev()
        .nth(2)
        .map_or(mode, |(i, _)| mode.get(i..).unwrap_or(mode));
    tail.chars().any(|c| matches!(c, '1' | '3' | '5' | '7'))
}

/// One `1 ` record of `git status --porcelain=v2 -z` that may be a destroyed symlink (`destroyed_symlinks.py:16-
/// 66`): HEAD mode `120000`, index mode neither `120000` nor `000000`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkCandidate {
    /// `' '.join(path_splitted)`.
    pub path: String,
    /// `hash_HEAD`.
    pub hash_head: String,
    /// `hash_index`.
    pub hash_index: String,
}

/// The records of `git status --porcelain=v2 -z -- <files>` (decoded) that `find_destroyed_symlinks` examines,
/// in order. A `1`-record with fewer than eight space-separated fields (a rename's original path that starts with
/// `1 `) is Python's unpacking `ValueError`, returned after the candidates before it.
pub fn destroyed_symlink_candidates(status: &str) -> (Vec<SymlinkCandidate>, Option<PyExc>) {
    let mut out = Vec::new();
    let trimmed = status.trim_matches('\0');
    if trimmed.is_empty() {
        return (out, None);
    }
    for line in trimmed.split('\0') {
        let f: Vec<&str> = line.split(' ').collect();
        if f.first() != Some(&"1") {
            continue;
        }
        let (Some(mode_head), Some(mode_index), Some(hash_head), Some(hash_index)) =
            (f.get(3), f.get(4), f.get(6), f.get(7))
        else {
            return (
                out,
                Some(PyExc::new(
                    "ValueError",
                    format!(
                        "not enough values to unpack (expected at least 8, got {})",
                        f.len()
                    ),
                )),
            );
        };
        if *mode_head == "120000" && *mode_index != "120000" && *mode_index != "000000" {
            out.push(SymlinkCandidate {
                path: f.get(8..).unwrap_or_default().join(" "),
                hash_head: (*hash_head).to_owned(),
                hash_index: (*hash_index).to_owned(),
            });
        }
    }
    (out, None)
}

/// `head_content == index_content` after `bytes.rstrip()` (the content test of `find_destroyed_symlinks`).
pub fn same_after_rstrip(a: &[u8], b: &[u8]) -> bool {
    bytes_rstrip(a) == bytes_rstrip(b)
}

/// destroyed-symlinks' report (`main`, `destroyed_symlinks.py:69-90`), the lines `print` writes.
pub fn destroyed_symlinks_report(paths: &[String]) -> Vec<String> {
    let mut lines = vec!["Destroyed symlinks:".to_owned()];
    lines.extend(paths.iter().map(|p| format!("- {p}")));
    lines.push("You should unstage affected files:".to_owned());
    lines.push(format!(
        "\tgit reset HEAD -- {}",
        snp_pycompat::shlex::join(paths)
    ));
    lines.push(
        "And retry commit. As a long term solution you may try to explicitly tell git that your environment \
         does not support symlinks:"
            .to_owned(),
    );
    lines.push("\tgit config core.symlinks false".to_owned());
    lines
}

/// `f.read(3) == b'\xef\xbb\xbf'` (the real check-byte-order-marker, ≤ v5.0.0).
pub fn has_bom(head: &[u8]) -> bool {
    head == b"\xef\xbb\xbf"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: Option<Vec<u8>>) -> Option<String> {
        let b = b?;
        Some(String::from_utf8(b).unwrap())
    }

    #[test]
    fn sorter_table() {
        let d = SortOpts {
            empty_stays_empty: true,
            ..SortOpts::default()
        };
        assert_eq!(
            s(file_contents_sort(b"b\r\nA\n\n  \nc \na\nB", d)).as_deref(),
            Some("A\nB\na\nb\nc \n")
        );
        assert_eq!(
            s(file_contents_sort(b"a\rc\rb\r", d)).as_deref(),
            Some("a\rc\rb\n")
        );
        let ic = SortOpts {
            ignore_case: true,
            ..d
        };
        assert_eq!(
            s(file_contents_sort(b"b\nB\nA\na\n", ic)).as_deref(),
            Some("A\na\nb\nB\n")
        );
        let u = SortOpts { unique: true, ..d };
        assert_eq!(
            s(file_contents_sort(b"b\nb\r\nA\na\na \n", u)).as_deref(),
            Some("A\na\na \nb\n")
        );
        assert_eq!(file_contents_sort(b"", d), None);
        assert_eq!(s(file_contents_sort(b"\n\n", d)).as_deref(), Some(""));
        let old = SortOpts::default();
        assert_eq!(s(file_contents_sort(b"", old)).as_deref(), Some("\n"));
        let both = SortOpts {
            strip_both: true,
            ..old
        };
        assert_eq!(
            s(file_contents_sort(b"\rb\na\n", both)).as_deref(),
            Some("a\nb\n")
        );
    }

    fn req(c: &[u8], epoch: u16) -> Option<String> {
        s(requirements_fix(c, epoch).unwrap())
    }

    #[test]
    fn requirements_table() {
        let input = b"# top comment\n\nzope==1\n# about flask\nFlask>=2  # x\nflask==1\nDjango[bcrypt]~=4\n\
pkg-resources==0.0.0\nrequests @ https://x/r.zip\n-e git+https://g/x.git#egg=Alpha\nnumpy; python_version>\"3\"\n\
foo \\\n  --hash=sha256:abc\nbar\n# trailing\n";
        assert_eq!(
            req(input, 4).as_deref(),
            Some(
                "# top comment\n\n-e git+https://g/x.git#egg=Alpha\nbar\nDjango[bcrypt]~=4\n# about flask\n\
Flask>=2  # x\nflask==1\nfoo \\\n  --hash=sha256:abc\nnumpy; python_version>\"3\"\nrequests @ https://x/r.zip\n\
zope==1\n# trailing\n"
            )
        );
        assert_eq!(
            req(b"a==1\nb\na==1\n# c\nb\n", 4).as_deref(),
            Some("a==1\n# c\nb\n")
        );
        assert_eq!(req(b"\nb\na\n", 4).as_deref(), Some("a\n\nb\n"));
        assert_eq!(
            req(b"# c1\n# c2\nb\n\na\n", 4).as_deref(),
            Some("\na\n# c1\n# c2\nb\n")
        );
        assert_eq!(
            req(b"# hdr\n# hdr2\n\n# for z\nz\n\na\n# trailing\n\n", 4).as_deref(),
            Some("# hdr\n# hdr2\n\n\na\n# for z\nz\n# trailing\n\n")
        );
        assert_eq!(
            req(b"a \\\n\n  --hash=x\nB\n", 4).as_deref(),
            Some("\na \\\n  --hash=x\nB\n")
        );
        assert_eq!(
            req(b"b\r\npkg-resources==0.0.0\r\na\r\n", 4).as_deref(),
            Some("a\r\nb\r\npkg-resources==0.0.0\r\n")
        );
        assert!(requirements_fix(b"b\n  flask==1\n", 4).is_err());
        // One requirement is never compared: no crash.
        assert_eq!(requirements_fix(b"  flask==1\n", 4).unwrap(), None);
        assert_eq!(req(b"x", 4).as_deref(), Some("x\n"));
        assert_eq!(requirements_fix(b" \n\t\n", 4).unwrap(), None);
        assert_eq!(
            requirement_name(b"Django[bcrypt]~=4\n").unwrap(),
            b"django[bcrypt]"
        );
        assert_eq!(requirement_name(b"a=1\n").unwrap(), b"a=1");
        assert_eq!(requirement_name(b"a===1\n").unwrap(), b"a");
        assert_eq!(requirement_name(b";x").as_deref(), None);
    }

    #[test]
    fn case_conflict_table() {
        let repo: Vec<&[u8]> = vec![b"Dir/x", b"README"];
        let added: Vec<&[u8]> = vec![b"readme", b"dir/y", b"new/A.txt", b"new/a.txt", b"NEW/b"];
        let got: Vec<String> = case_conflicts(&repo, &[], &added, 3, false)
            .into_iter()
            .map(|b| String::from_utf8(b).unwrap())
            .collect();
        assert_eq!(
            got,
            [
                "Dir",
                "NEW",
                "README",
                "dir",
                "new",
                "new/A.txt",
                "new/a.txt",
                "readme"
            ]
        );
        assert_eq!(case_conflicts(&repo, &[b"other"], &[], 3, false).len(), 0);
        assert_eq!(py_lower("ÄΣ".as_bytes()), "äς".as_bytes());
        assert_eq!(py_str_cmp(b"\xff", "\u{e000}".as_bytes()), Ordering::Less);
    }

    #[test]
    fn destroyed_records() {
        let st = "1 M. N... 120000 100644 100644 aaa bbb my link\0? untracked\0";
        let (c, e) = destroyed_symlink_candidates(st);
        assert!(e.is_none());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "my link");
        let (_, e) = destroyed_symlink_candidates("1 x\0");
        assert!(e.is_some());
        assert!(same_after_rstrip(b"target", b"target\r\n"));
    }

    #[test]
    fn py_lower_ignores_post_15_1_case_pairs() {
        // Letters that gained a lowercase mapping after UCD 15.1: CPython 3.12/3.13 leave them unchanged.
        for c in [
            '\u{1C89}',
            '\u{A7CB}',
            '\u{A7CC}',
            '\u{A7DA}',
            '\u{A7DC}',
            '\u{10D50}',
            '\u{16EA0}',
        ] {
            let s = c.to_string();
            assert_eq!(
                py_lower(s.as_bytes()),
                s.as_bytes(),
                "U+{:04X}",
                u32::from(c)
            );
        }
        assert_eq!(
            py_lower("ABC\u{130}\u{3A3}".as_bytes()),
            "abci\u{307}\u{3C2}".as_bytes()
        );
        assert_eq!(py_lower("\u{3A3}A".as_bytes()), "\u{3C3}a".as_bytes());
        assert_eq!(
            py_lower("A\u{3A3}.TXT".as_bytes()),
            "a\u{3C3}.txt".as_bytes()
        );
        assert_eq!(py_lower("A\u{3A3}. 1".as_bytes()), "a\u{3C2}. 1".as_bytes());
        // U+0295 is cased in CPython's UCD 15.1 data (not in newer Unicode): the sigma after it is final.
        assert_eq!(
            py_lower("A\u{295}\u{3A3}".as_bytes()),
            "a\u{295}\u{3C2}".as_bytes()
        );
        assert_eq!(py_lower(b"A\xce\xa3\xff"), b"a\xcf\x82\xff");
        assert_eq!(py_lower(b"A\xffB"), b"a\xffb");
        let pair: &[&[u8]] = &["\u{1C89}.txt".as_bytes(), "\u{1C8A}.txt".as_bytes()];
        assert_eq!(
            case_conflicts(&[], pair, &[], 3, false),
            Vec::<Vec<u8>>::new()
        );
    }

    /// Every code point (alone and as `A<c>Σ` and `Σ<c>`, exercising the final-sigma context) against
    /// `str.lower()` of the pc462 oracles, when they are present (skipped on machines without them).
    #[test]
    fn lower_table_matches_the_oracle() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oracle");
        for py in ["pc462-py312/bin/python", "pc462-py313/bin/python"] {
            let exe = root.join(py);
            if !exe.exists() {
                continue;
            }
            let script = "import sys\nw=sys.stdout.buffer.write\nfor cp in range(0x110000):\n    if 0xd800<=cp<0xe000: continue\n    c=chr(cp)\n    w((' '.join(s.encode('utf-8').hex() for s in (c.lower(), ('A'+c+'\\u03a3').lower(), ('\\u03a3'+c).lower()))+'\\n').encode())\n";
            let out = std::process::Command::new(&exe)
                .args(["-I", "-c", script])
                .output()
                .unwrap();
            assert!(out.status.success());
            let mut lines = out.stdout.split(|&b| b == b'\n');
            for cp in (0_u32..0x11_0000).filter(|cp| !(0xD800..0xE000).contains(cp)) {
                let c = char::from_u32(cp).unwrap();
                let line = lines.next().unwrap();
                let text = std::str::from_utf8(line).unwrap();
                let mut parts = text.split(' ').map(|h| {
                    (0..h.len() / 2)
                        .map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2).unwrap(), 16).unwrap())
                        .collect::<Vec<u8>>()
                });
                let alone = c.to_string();
                assert_eq!(
                    py_lower(alone.as_bytes()),
                    parts.next().unwrap(),
                    "{py} U+{cp:04X}"
                );
                let ctx = format!("A{c}\u{3A3}");
                assert_eq!(
                    py_lower(ctx.as_bytes()),
                    parts.next().unwrap(),
                    "{py} A U+{cp:04X} SIGMA"
                );
                let ctx = format!("\u{3A3}{c}");
                assert_eq!(
                    py_lower(ctx.as_bytes()),
                    parts.next().unwrap(),
                    "{py} SIGMA U+{cp:04X}"
                );
            }
        }
    }
}
