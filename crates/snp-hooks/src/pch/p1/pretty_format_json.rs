//! `pretty-format-json` (`pretty_format_json.py`; 09 §4.C) over snp-pyparsers' CPython `json` decoder/encoder and
//! `difflib` ports (CPython 3.13).
//!
//! Epochs: 0 = v3.0.0…v3.4.0 (DELEGATE: the diff went through text-mode `print`), 1 = v4.0.0…v4.6.0 (the first
//! invalid file ends the run with exit 1), 2 = v5.0.0+ (later files are still checked). The file is read in text
//! mode (strict UTF-8, universal newlines: a CRLF file that is otherwise pretty passes); without `--autofix` the
//! unified diff goes to `sys.stdout.buffer` and therefore ahead of the `print`ed messages ([`PyOut`]); with it the
//! file is rewritten in text mode (`\r\n` on Windows). A lone surrogate kept by `--no-ensure-ascii` is the
//! `UnicodeEncodeError` crash at the point Python encodes (the diff, or the write after `Fixing file …`, which
//! leaves the file empty).
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, DefaultValue, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;
use snp_pyparsers::CheckError;
use snp_pyparsers::json::{DumpOpts, PyMinor};

use super::kernels;
use super::util::{bytes_of, str_of, text_read, text_write_bytes};
use crate::framework::{
    self, FileStep, PyOut, files_positional, store, store_true, with_default, with_help,
    with_metavar,
};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name, read_all, rewrite};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "pretty-format-json";

/// The emulated CPython (09 §10 Q2).
const MINOR: PyMinor = PyMinor::P313;

static ACTIONS: [ActionSpec; 6] = [
    with_help(
        store_true(&["--autofix"], "autofix"),
        Some("Automatically fixes encountered not-pretty-formatted files"),
    ),
    with_help(
        with_default(store(&["--indent"], "indent"), DefaultValue::Str("2")),
        Some(
            "The number of indent spaces or a string to be used as delimiter for indentation level e.g. 4 or \
             \"\t\" (Default: 2)",
        ),
    ),
    with_help(
        store_true(&["--no-ensure-ascii"], "no_ensure_ascii"),
        Some("Do NOT convert non-ASCII characters to Unicode escape sequences (\\uXXXX)"),
    ),
    with_help(
        store_true(&["--no-sort-keys"], "no_sort_keys"),
        Some("Keep JSON nodes in the same order"),
    ),
    with_help(
        with_metavar(store(&["--top-keys"], "top_keys"), "TOP_KEYS"),
        Some("Ordered list of keys to keep at the top of JSON hashes"),
    ),
    files_positional(Some("Filenames to fix")),
];

/// `pretty_format_json.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The native.
#[derive(Debug)]
pub(crate) struct PrettyFormatJson;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: PrettyFormatJson = PrettyFormatJson;

/// Shifts the character positions of a `UnicodeEncodeError` text (`… in position N: …` or `… in position N-M:
/// …`) by `delta`.
fn shift_positions(text: &[u8], delta: usize) -> Vec<u8> {
    let s = String::from_utf8_lossy(text);
    let marker = " in position ";
    let Some(at) = s.find(marker) else {
        return text.to_vec();
    };
    let head = at.saturating_add(marker.len());
    let tail = s.get(head..).unwrap_or_default();
    let end = tail.find(':').unwrap_or(tail.len());
    let shifted: Vec<String> = tail
        .get(..end)
        .unwrap_or_default()
        .split('-')
        .map(|n| {
            n.parse::<usize>()
                .map_or_else(|_| n.to_owned(), |v| v.saturating_add(delta).to_string())
        })
        .collect();
    let mut out = s.get(..head).unwrap_or_default().to_owned();
    out.push_str(&shifted.join("-"));
    out.push_str(tail.get(end..).unwrap_or_default());
    out.into_bytes()
}

/// The first character position the exception text names (`position N`), if any.
fn first_position(text: &[u8]) -> Option<usize> {
    let s = String::from_utf8_lossy(text);
    let tail = s.get(s.find(" in position ")?.saturating_add(13)..)?;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// What a `UnicodeEncodeError` from the encoder means for the hook: the pretty text holds a lone surrogate at the
/// position `exc` names. Returns the exception as Python raises it at its encode point.
fn encode_crash(
    exc: &PyExc,
    contents: &str,
    opts: &DumpOpts,
    name: &OsStr,
    windows_write: Option<bool>,
) -> PyExc {
    // The exact pretty text (lone surrogates kept) gives the encode position; `at` is its index in that text.
    let shifted = first_position(&exc.text).and_then(|at| {
        let file = str_of(name);
        kernels::pretty_json_crash_position(contents, opts, MINOR, &file, windows_write)?
            .checked_sub(at)
    });
    let Some(delta) = shifted else {
        return exc.clone();
    };
    PyExc::new(exc.ty.clone(), shift_positions(&exc.text, delta))
}

/// `_get_pretty_format(...)`, with Python's order of failures (kept by [`snp_pyparsers::json::pretty_format`]):
/// `json.loads` (and its `object_pairs_hook`) runs before the `--indent` is applied, so a decode error or a
/// `TypeError` wins over an indent `OverflowError` / `MemoryError`.
fn pretty(contents: &str, opts: &DumpOpts) -> Result<String, CheckError> {
    snp_pyparsers::json::pretty_format(contents, opts, MINOR)
}

impl NativeHook for PrettyFormatJson {
    fn id(&self) -> &'static str {
        "pretty-format-json"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 2)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = cx.epoch.0;
        let autofix = ns.flag("autofix");
        let opts = DumpOpts {
            indent: ns
                .value("indent")
                .map_or_else(|| "2".to_owned(), |v| v.to_string_lossy().into_owned()),
            ensure_ascii: !ns.flag("no_ensure_ascii"),
            sort_keys: !ns.flag("no_sort_keys"),
            top_keys: ns
                .value("top_keys")
                .map(|v| v.to_string_lossy().split(',').map(str::to_owned).collect())
                .unwrap_or_default(),
        };
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut stopped = false;
        let mut emptied = Vec::new();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            if stopped {
                return FileStep::Unchanged;
            }
            let contents = match read_all(cx, name).and_then(|b| text_read(&b)) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let pretty = match pretty(&contents, &opts) {
                Ok(p) => p,
                Err(CheckError::Reported(_)) => {
                    print_name(
                        out,
                        b"Input File ",
                        name,
                        b" is not a valid JSON, consider using check-json",
                    );
                    // v4.x: `return 1` at the first invalid file.
                    stopped = epoch <= 1;
                    return FileStep::Failed;
                }
                Err(CheckError::Crash(e)) if e.ty == "UnicodeEncodeError" => {
                    if !autofix {
                        return FileStep::Crash(encode_crash(&e, &contents, &opts, name, None));
                    }
                    print_name(out, b"Fixing file ", name, b"");
                    // `open(f, 'w')` truncates before the write raises.
                    if let Err(w) = rewrite(cx, name, b"") {
                        return FileStep::Crash(w);
                    }
                    if let Some(p) = framework::repo_path(name) {
                        emptied.push(p);
                    }
                    return FileStep::Crash(encode_crash(
                        &e,
                        &contents,
                        &opts,
                        name,
                        Some(cx.windows),
                    ));
                }
                Err(CheckError::Crash(e)) => return FileStep::Crash(e),
                Err(other) => {
                    return FileStep::Crash(PyExc::new("RuntimeError", other.into_message()));
                }
            };
            if contents == pretty {
                return FileStep::Unchanged;
            }
            if autofix {
                print_name(out, b"Fixing file ", name, b"");
                let text = text_write_bytes(&pretty, cx.windows);
                if let Err(e) = rewrite(cx, name, text.as_bytes()) {
                    return FileStep::Crash(e);
                }
                return framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed);
            }
            let a: Vec<&str> = snp_pycompat::text::splitlines_keepends(&contents).collect();
            let b: Vec<&str> = snp_pycompat::text::splitlines_keepends(&pretty).collect();
            let f = String::from_utf8_lossy(bytes_of(name)).into_owned();
            let diff = snp_pyparsers::difflib::unified_diff(&a, &b, &f, &f);
            out.write_bytes(diff.as_bytes());
            FileStep::Failed
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.modified.extend(emptied);
        outcome.reads_only = !autofix;
        framework::finish(outcome, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_positions() {
        assert_eq!(
            shift_positions(
                b"'utf-8' codec can't encode character '\\ud800' in position 7: surrogates",
                3
            ),
            b"'utf-8' codec can't encode character '\\ud800' in position 10: surrogates"
        );
        assert_eq!(
            shift_positions(b"x in position 1-2: y", 5),
            b"x in position 6-7: y"
        );
        assert_eq!(first_position(b"c in position 12: s"), Some(12));
    }

    fn opts(indent: &str, sort_keys: bool) -> DumpOpts {
        DumpOpts {
            indent: indent.to_owned(),
            ensure_ascii: false,
            sort_keys,
            top_keys: Vec::new(),
        }
    }

    /// Keys differing only by lone surrogates, or a lone surrogate next to a literal U+FFFD, stay distinct keys
    /// (Python positions verified against the v6.0.0 oracle with the file `a.json`: 73, 90, 148).
    #[test]
    fn crash_positions_keep_surrogate_keys_apart() {
        let o = opts("2", false);
        let pos = |c: &str| kernels::pretty_json_crash_position(c, &o, MINOR, "a.json", None);
        assert_eq!(pos("{\"\u{fffd}\":1,\"\\ud800\":2}"), Some(73));
        assert_eq!(pos("{\"\u{fffd}\":1,\"x\":0,\"\\ud800\":2}"), Some(90));
        assert_eq!(
            pos(
                "{\"\u{e9}\":1,\"e\":2,\"\u{1f600}\":3,\"\u{ffff}\":4,\"\\ud800\":5,\"\\udc00\":6,\"A\":7,\"a\":8}"
            ),
            Some(148)
        );
        let text = kernels::pretty_json_text(
            "{\"\\ud800\":\"\\ud83d\\ude00\\u007f\\u0001\"}",
            &opts("\t", true),
            MINOR,
        )
        .unwrap();
        let want: Vec<u32> = "{\n\t\"\u{fffd}\": \"\u{1f600}\u{7f}\\u0001\"\n}\n"
            .chars()
            .map(|c| {
                if c == '\u{fffd}' {
                    0xD800
                } else {
                    u32::from(c)
                }
            })
            .collect();
        assert_eq!(text, want);
        assert_eq!(
            kernels::pretty_json_crash_position("[1]", &o, MINOR, "f", None),
            None
        );
    }
}
