//! Native pygrep against `python -m pre_commit.languages.pygrep` (pre-commit 4.7.0, CPython 3.13): every row of
//! 04a §8.4, the §12.5 vectors and the 10 pygrep-hooks patterns from `cases.json`, recorded by
//! `cargo xtask pch-goldens pygrep` (WP-26 acceptance; X13-EXE-07; ADR-072 flag-shaped file names).
//!
//! Owner: WP-26.
#![cfg(test)]
#![cfg_attr(
    windows,
    allow(
        dead_code,
        unused_imports,
        reason = "most cases are posix-only (sh scripts, modes, signals): on Windows their shared helpers go unused"
    )
)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_core::cancel::CancelToken;
use snp_hooks::pygrep;
use snp_pyre::RegexCache;
use support::{latin1, os, show};

/// One case's outcome: `prepare` once for `args + [entry]`, then `run_partition` over the full argv (the runner's
/// sequence), in a directory holding the fixtures. The ones the case's argv names are written last: `u.bin` and
/// `U.bin` are one file on a case-insensitive file system, which then holds the content the case reads.
fn run(
    fixtures: &serde_json::Map<String, serde_json::Value>,
    case: &serde_json::Value,
) -> (i32, Vec<u8>, bool) {
    let dir = tempfile::tempdir().unwrap();
    let named: Vec<&str> = [&case["files"], &case["args"]]
        .into_iter()
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .chain(std::iter::once(&case["entry"]))
        .filter_map(serde_json::Value::as_str)
        .collect();
    let mut ordered: Vec<_> = fixtures.iter().collect();
    ordered.sort_by_key(|(p, _)| named.contains(&p.as_str()));
    for (path, content) in ordered {
        let p = dir.path().join(os(&latin1(path)));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, latin1(content.as_str().unwrap())).unwrap();
    }
    let args: Vec<Arc<str>> = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| Arc::from(a.as_str().unwrap()))
        .collect();
    let entry = case["entry"].as_str().unwrap();
    let compiled = match pygrep::prepare(&args, entry, 80) {
        Ok(c) => c,
        Err((code, out)) => return (code, out, code == 1),
    };
    let mut argv: Vec<OsString> = args.iter().map(|a| OsString::from(&**a)).collect();
    argv.push(entry.into());
    argv.extend(support::argv_of(&case["files"]));
    let refs: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();
    let cache = RegexCache::new();
    let o = pygrep::run_partition(&compiled, &refs, dir.path(), &cache, &CancelToken::new());
    assert!(o.reads_only && o.modified.is_empty());
    (o.code, o.output, o.crashed)
}

#[cfg(unix)]
#[test]
fn pygrep_goldens() {
    support::link_all();
    let cases = support::json("pygrep/cases.json");
    let goldens = support::json("pygrep/goldens.json");
    let fixtures = cases["fixtures"].as_object().unwrap();
    let mut failures = Vec::new();
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let want = &goldens["results"][name];
        let (code, out, crashed) = run(fixtures, case);
        let want_out = latin1(want["out"].as_str().unwrap());
        if out != want_out
            || i64::from(code) != want["code"].as_i64().unwrap()
            || crashed != want["crashed"].as_bool().unwrap()
        {
            failures.push(format!(
                "{name}: code {code} crashed {crashed} (want {} {})\n--- got\n{}\n--- want\n{}",
                want["code"],
                want["crashed"],
                show(&out),
                show(&want_out)
            ));
        }
    }
    assert!(cases["cases"].as_array().unwrap().len() >= 50);
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// X13-EXE-07: an entry Python rejects with something other than `re.error` fails the hook once with exit 1 and
/// the traceback's last line (`OverflowError`, `RecursionError`, `ValueError`), never exit 3.
#[test]
fn x13_exe_07_non_re_error_entries() {
    let cases: [(&str, &[u8]); 3] = [
        (
            "a{4294967295}",
            b"OverflowError: the repetition number is too large\n",
        ),
        (
            "(?a)(?L)x",
            b"ValueError: ASCII and LOCALE flags are incompatible\n",
        ),
        (
            &format!(
                "{}a{}",
                std::iter::repeat_n("(", 1000).collect::<String>(),
                std::iter::repeat_n(")", 1000).collect::<String>()
            ),
            b"RecursionError: maximum recursion depth exceeded\n",
        ),
    ];
    for (entry, line) in cases {
        let (code, out) = pygrep::prepare(&[], entry, 80).unwrap_err();
        assert_eq!(code, 1, "{entry}");
        assert_eq!(show(&out), show(line), "{entry}");
    }
}

/// ADR-072: a partition whose file names change the parse (`-i`, `--negate`) behaves like pygrep.py, and one
/// with a `-x.py` file is the usage error of that partition only.
#[test]
fn flag_shaped_file_names_change_the_parse() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"FOO\n").unwrap();
    std::fs::write(dir.path().join("--negate"), b"x\n").unwrap();
    let c = pygrep::prepare(&[], "foo", 80).unwrap();
    let cache = RegexCache::new();
    let cancel = CancelToken::new();
    let go = |argv: &[&str]| {
        let refs: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
        pygrep::run_partition(&c, &refs, dir.path(), &cache, &cancel)
    };
    // Same parse: no match (case-sensitive).
    let o = go(&["foo", "a.txt"]);
    assert_eq!((o.code, o.output.as_slice()), (0, &b""[..]));
    // A file named `-i` turns on IGNORECASE for this partition (`-i` itself is not read).
    let o = go(&["foo", "a.txt", "-i"]);
    assert_eq!((o.code, show(&o.output).as_str()), (1, "a.txt:1:FOO\n"));
    // A file named `--negate`: negated matching; a.txt has no `foo` → printed.
    let o = go(&["foo", "a.txt", "--negate"]);
    assert_eq!((o.code, show(&o.output).as_str()), (1, "a.txt\n"));
    // `-x.py` is an unrecognized option.
    let o = go(&["foo", "a.txt", "-x.py"]);
    assert_eq!(o.code, 2);
    assert!(show(&o.output).ends_with("pygrep.py: error: unrecognized arguments: -x.py\n"));
}

/// A cancelled run stops before the next file.
#[test]
fn cancellation_between_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"foo\n").unwrap();
    let c = pygrep::prepare(&[], "foo", 80).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel(snp_core::cancel::CancelReason::Interrupt);
    let o = pygrep::run_partition(
        &c,
        &[OsStr::new("foo"), OsStr::new("a.txt")],
        dir.path(),
        &RegexCache::new(),
        &cancel,
    );
    assert_eq!((o.code, o.output.len()), (0, 0));
}
