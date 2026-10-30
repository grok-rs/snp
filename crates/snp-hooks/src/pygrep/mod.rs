//! Native `pygrep` (`languages/pygrep.py`; 04a §8.4/§12): compiled once per hook per run (01 Addendum §A.4.5).
//!
//! pre-commit runs `python -m pre_commit.languages.pygrep *args entry *files` once per xargs partition (serial,
//! unshuffled). Natively each partition's full argv is parsed with `pygrep.py`'s argparse spec (prog `pygrep.py`,
//! CPython 3.13 texts, abbreviations), so a file named `-i` sets the flag and one named `-x.py` is
//! `unrecognized arguments` (exit 2) exactly as in Python (ADR-072). The pattern is a **bytes** regex
//! (`args.pattern.encode()`, `snp_pyre::Mode::Bytes`: ASCII classes and folding), `-i` is `re.IGNORECASE` and
//! `--multiline` is `re.MULTILINE | re.DOTALL`; the four per-file procedures of 04a §12.4 are reproduced byte for
//! byte. Output is written through `output.write`/`write_line_b` (flushed at once), so it is in program order; an
//! unreadable file ends the partition with the traceback's last line (`FileNotFoundError: [Errno 2] No such file
//! or directory: 'x'`), exit 1. An invalid pattern fails the hook once with Python's last line (`re.error: …`,
//! `OverflowError: …`), not once per partition (D-MISC-05).
//!
//! Owner: WP-26.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use memchr::memchr_iter;
use snp_argparse::{ActionSpec, Namespace, ParserSpec};
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_pyre::{Haystack, Mode, Options, PyRegex, RegexCache};

use crate::framework::{self, positional, store_true, with_help};

/// `prog` of `python -m pre_commit.languages.pygrep` (CPython ≤ 3.13: the module file's basename).
const PROG: &str = "pygrep.py";

static ACTIONS: [ActionSpec; 5] = [
    store_true(&["-i", "--ignore-case"], "ignore_case"),
    store_true(&["--multiline"], "multiline"),
    store_true(&["--negate"], "negate"),
    with_help(
        positional("pattern", snp_argparse::Nargs::One),
        Some("python regex pattern."),
    ),
    positional("filenames", snp_argparse::Nargs::ZeroOrMore),
];

/// `pygrep.main`'s parser (`pre_commit/languages/pygrep.py:104-118`).
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: Some(
        "grep-like finder using python regexes.  Unlike grep, this tool returns nonzero when it finds a \
         match and zero otherwise.  The idea here being that matches are \"problems\".",
    ),
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The parts of a parse that select the regex.
#[derive(Clone, PartialEq, Eq)]
struct Key {
    pattern: String,
    ignore_case: bool,
    multiline: bool,
}

impl Key {
    fn options(&self) -> Options {
        Options {
            mode: Mode::Bytes,
            haystack: if self.multiline {
                Haystack::Blob
            } else {
                Haystack::Line
            },
            ignore_case: self.ignore_case,
            multiline_dotall: self.multiline,
        }
    }
}

/// Parsed `args + [entry]` and the compiled `PyRegex` for that parse (manual `Debug`: shows the source and flags).
pub struct Compiled {
    key: Key,
    regex: Arc<PyRegex>,
    /// argparse's columns for the partitions' usage errors.
    columns: usize,
}

impl fmt::Debug for Compiled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compiled")
            .field("pattern", &self.key.pattern)
            .field("ignore_case", &self.key.ignore_case)
            .field("multiline", &self.key.multiline)
            .finish_non_exhaustive()
    }
}

/// An empty environment: pygrep's parser reads no environment defaults.
fn no_env() -> Env {
    Env::from_pairs(Vec::<(OsString, OsString)>::new())
}

/// Parses `argv` like `pygrep.py`: `Err((code, output))` for `-h` (0) and usage errors (2).
fn parse(argv: &[&OsStr], columns: usize) -> Result<Namespace, (i32, Vec<u8>)> {
    framework::parse_args(&SPEC, PROG, argv, columns, &no_env()).map_err(|o| (o.code, o.output))
}

/// The regex key of a parse; `Err` = `args.pattern.encode()` failing (a non-UTF-8 argument holds surrogates).
fn key(ns: &Namespace) -> Result<Key, Vec<u8>> {
    let raw = ns.value("pattern").unwrap_or_default();
    let Some(pattern) = raw.to_str() else {
        return Err(
            b"UnicodeEncodeError: 'utf-8' codec can't encode characters: surrogates not allowed\n"
                .to_vec(),
        );
    };
    Ok(Key {
        pattern: pattern.to_owned(),
        ignore_case: ns.flag("ignore_case"),
        multiline: ns.flag("multiline"),
    })
}

/// `re.compile(pattern.encode(), flags)` failing: the traceback's last line (CPython 3.13: `re.PatternError: …`,
/// `OverflowError: …`, `ValueError: …`, `RecursionError: …`).
fn compile_error(e: &snp_pyre::PyReError) -> Vec<u8> {
    framework::crash_line(&framework::re_exc(e))
}

/// `Err((code, output))` = argparse error text (exit 2) or `re.error: …` line (exit 1) — reported once for the hook.
///
/// `-h` among the args is `Err((0, help))`: the hook prints the help and passes.
pub fn prepare(args: &[Arc<str>], entry: &str, columns: usize) -> Result<Compiled, (i32, Vec<u8>)> {
    let mut argv: Vec<&OsStr> = args.iter().map(|a| OsStr::new(&**a)).collect();
    argv.push(OsStr::new(entry));
    let ns = parse(&argv, columns)?;
    let key = key(&ns).map_err(|line| (1, line))?;
    let regex =
        PyRegex::compile(&key.pattern, key.options()).map_err(|e| (1, compile_error(&e)))?;
    Ok(Compiled {
        key,
        regex: Arc::new(regex),
        columns,
    })
}

/// `argv` = `args + [entry] + files` of the partition, re-parsed exactly like `pygrep.py` (a file named like a flag
/// changes the parse, ADR-072); the pattern is recompiled through `cache` only when the parse differs from `c`.
///
/// `root` is the hook's cwd (relative file names are opened under it). `cancel` is checked between files; a
/// cancelled partition returns what it produced so far (the runner reports `Interrupted`).
pub fn run_partition(
    c: &Compiled,
    argv: &[&OsStr],
    root: &Path,
    cache: &RegexCache,
    cancel: &CancelToken,
) -> crate::Outcome {
    let failed = |code: i32, output: Vec<u8>| crate::Outcome {
        code,
        output,
        reads_only: true,
        ..crate::Outcome::default()
    };
    let ns = match parse(argv, c.columns) {
        Ok(ns) => ns,
        Err((code, output)) => return failed(code, output),
    };
    let key = match key(&ns) {
        Ok(k) => k,
        Err(line) => return failed(1, line),
    };
    let regex = if key == c.key {
        Arc::clone(&c.regex)
    } else {
        match cache.get(&key.pattern, key.options()) {
            Ok(r) => r,
            Err(e) => return failed(1, compile_error(&e)),
        }
    };
    let negate = ns.flag("negate");
    let files: Vec<&OsStr> = ns
        .list("filenames")
        .iter()
        .map(OsString::as_os_str)
        .collect();
    let mut out = Vec::new();
    let mut code = 0;
    for name in files {
        if cancel.is_cancelled() {
            break;
        }
        let content = match std::fs::read(root.join(name)) {
            Ok(b) => b,
            Err(e) => {
                out.extend_from_slice(&framework::crash_line(&framework::io_exc(&e, name)));
                return crate::Outcome {
                    code: 1,
                    output: out,
                    crashed: true,
                    reads_only: true,
                    ..crate::Outcome::default()
                };
            }
        };
        let fname = framework::os_bytes(name);
        let r = match (key.multiline, negate) {
            (false, false) => by_line(&regex, fname, &content, &mut out),
            (false, true) => by_line_negated(&regex, fname, &content, &mut out),
            (true, false) => at_once(&regex, fname, &content, &mut out),
            (true, true) => at_once_negated(&regex, fname, &content, &mut out),
        };
        let Ok(rc) = r else {
            out.extend_from_slice(
                b"snp: pygrep: regular expression backtracking limit exceeded in ",
            );
            out.extend_from_slice(fname);
            out.push(b'\n');
            return crate::Outcome {
                code: 1,
                output: out,
                crashed: true,
                reads_only: true,
                ..crate::Outcome::default()
            };
        };
        code |= rc;
    }
    crate::Outcome {
        code,
        output: out,
        reads_only: true,
        ..crate::Outcome::default()
    }
}

/// Lines of a binary file iterated by Python: split after each `\n` (the last one may lack it).
fn lines(content: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut start = 0_usize;
    let mut ends = memchr_iter(b'\n', content);
    std::iter::from_fn(move || {
        let end = match ends.next() {
            Some(nl) => nl.saturating_add(1),
            None if start < content.len() => content.len(),
            None => return None,
        };
        let line = content.get(start..end).unwrap_or_default();
        start = end;
        Some(line)
    })
}

/// `regex.search(h)`: the span, `Err` past the backtracking limit.
fn search(regex: &PyRegex, h: &[u8]) -> Result<Option<(usize, usize)>, ()> {
    regex.search_bytes(h).map_err(|_| ())
}

/// `_process_filename_by_line` (`pygrep.py:22-30`).
fn by_line(regex: &PyRegex, fname: &[u8], content: &[u8], out: &mut Vec<u8>) -> Result<i32, ()> {
    let mut rc = 0;
    for (i, line) in lines(content).enumerate() {
        if search(regex, line)?.is_some() {
            rc = 1;
            out.extend_from_slice(fname);
            out.extend_from_slice(format!(":{}:", i.saturating_add(1)).as_bytes());
            let end = line
                .iter()
                .rposition(|&b| b != b'\r' && b != b'\n')
                .map_or(0, |p| p.saturating_add(1));
            out.extend_from_slice(line.get(..end).unwrap_or_default());
            out.push(b'\n');
        }
    }
    Ok(rc)
}

/// `_process_filename_by_line_negated` (`:50-60`).
fn by_line_negated(
    regex: &PyRegex,
    fname: &[u8],
    content: &[u8],
    out: &mut Vec<u8>,
) -> Result<i32, ()> {
    for line in lines(content) {
        if search(regex, line)?.is_some() {
            return Ok(0);
        }
    }
    out.extend_from_slice(fname);
    out.push(b'\n');
    Ok(1)
}

/// `_process_filename_at_once` (`:33-47`).
fn at_once(regex: &PyRegex, fname: &[u8], content: &[u8], out: &mut Vec<u8>) -> Result<i32, ()> {
    let Some((start, end)) = search(regex, content)? else {
        return Ok(0);
    };
    let before = content.get(..start).unwrap_or_default();
    let line_no = memchr_iter(b'\n', before).count();
    out.extend_from_slice(fname);
    out.extend_from_slice(format!(":{}:", line_no.saturating_add(1)).as_bytes());
    let first = content
        .split(|&b| b == b'\n')
        .nth(line_no)
        .unwrap_or_default();
    let matched = content.get(start..end).unwrap_or_default();
    let mut parts = matched.split(|&b| b == b'\n');
    let _first_part = parts.next();
    out.extend_from_slice(first);
    for p in parts {
        out.push(b'\n');
        out.extend_from_slice(p);
    }
    out.push(b'\n');
    Ok(1)
}

/// `_process_filename_at_once_negated` (`:63-74`).
fn at_once_negated(
    regex: &PyRegex,
    fname: &[u8],
    content: &[u8],
    out: &mut Vec<u8>,
) -> Result<i32, ()> {
    if search(regex, content)?.is_some() {
        return Ok(0);
    }
    out.extend_from_slice(fname);
    out.push(b'\n');
    Ok(1)
}

#[cfg(test)]
mod tests;
