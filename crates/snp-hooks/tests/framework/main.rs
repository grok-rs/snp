//! Milestone M1: the frozen `snp_hooks::framework` API (ARCHITECTURE §1.4.19, ADR-071, ADR-072) — argparse over
//! `args + files`, Python's flush order, the per-file loop's crash and cancellation semantics, and the committed
//! Appendix A goldens of every console script of every supported tag.
//!
//! Owner: WP-26.
#![cfg(test)]

mod support;

use std::ffi::OsStr;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::cancel::CancelReason;
use snp_core::failure::PyExc;
use snp_core::path::RepoPath;
use snp_hooks::framework::{self, FileStep, PyOut, files_positional, store_true};
use snp_hooks::{Epoch, Outcome};
use support::{Ctx, Sandbox, latin1, show};

static ACTIONS: [ActionSpec; 3] = [
    store_true(&["-i", "--ignore-case"], "ignore_case"),
    store_true(&["--multiline"], "multiline"),
    files_positional(None),
];
static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

fn parse(argv: &[&str]) -> Result<snp_argparse::Namespace, Outcome> {
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    framework::parse_args(&SPEC, "some-hook", &argv, 80, &env)
}

const USAGE: &str = "usage: some-hook [-h] [-i] [--multiline] [filenames ...]\n";

/// ADR-072: the argv is `args + files`, parsed like the console script: a file named `-x.py` is an unrecognized
/// argument (exit 2, usage on stderr), one named `-i` sets the flag, `--` ends the options, abbreviations work.
#[test]
fn parse_args_over_args_and_files() {
    support::link_all();
    let e = parse(&["a.py", "-x.py"]).unwrap_err();
    assert_eq!(e.code, 2);
    assert_eq!(
        show(&e.output),
        format!("{USAGE}some-hook: error: unrecognized arguments: -x.py\n")
    );
    let ns = parse(&["a.py", "-i"]).unwrap();
    assert!(ns.flag("ignore_case"));
    assert_eq!(ns.list("filenames"), ["a.py"]);
    let ns = parse(&["--", "-i"]).unwrap();
    assert!(!ns.flag("ignore_case"));
    assert_eq!(ns.list("filenames"), ["-i"]);
    let ns = parse(&["--multi", "b"]).unwrap();
    assert!(ns.flag("multiline"));
    let e = parse(&["--multiline=1"]).unwrap_err();
    assert_eq!(
        show(&e.output),
        format!("{USAGE}some-hook: error: argument --multiline: ignored explicit argument '1'\n")
    );
    let h = parse(&["a", "-h", "--bogus"]).unwrap_err();
    assert_eq!(h.code, 0);
    assert!(show(&h.output).starts_with(USAGE));
    assert!(show(&h.output).contains("\noptions:\n  -h, --help"));
}

/// `parser.error(msg)` after a successful parse (trailing-whitespace's extension checks).
#[test]
fn parser_error_text() {
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    assert_eq!(
        show(&framework::parser_error(
            &SPEC,
            "some-hook",
            80,
            &env,
            "bad"
        )),
        format!("{USAGE}some-hook: error: bad\n")
    );
}

/// Interleaved `print` / `print(file=sys.stderr)` / `sys.stdout.buffer.write` on one pipe (CPython 3.13):
/// stderr is written through, `buffer.write` overtakes pending `print` text, stdout is flushed at exit.
#[cfg(not(windows))]
#[test]
fn pyout_flush_order() {
    let mut o = PyOut::default();
    o.print("a");
    o.eprint("b");
    o.write_bytes(b"c\n");
    o.print("d");
    o.eprint("e");
    assert_eq!(o.finish(), b"b\ne\nc\na\nd\n");
    // Beyond 8 KiB of pending text, `print` reaches the pipe before later stderr lines.
    let mut o = PyOut::default();
    let big: String = std::iter::repeat_n('x', 9000).collect();
    o.print(&big);
    o.eprint("after");
    let out = o.finish();
    // `print(x)` writes `x` then `\n`: the newline is still pending when stderr is written.
    assert_eq!(out, format!("{big}after\n\n").into_bytes());
    // `sys.stdout.flush()` (check-vcs-permalinks) keeps program order.
    let mut o = PyOut::default();
    o.print("1");
    o.flush();
    o.write_bytes(b"2\n");
    o.flush();
    o.eprint("3");
    assert_eq!(o.finish(), b"1\n2\n3\n");
}

/// `for_each_file`: effects in input order; the first crash writes the traceback's last line after the flushed
/// stdout and stops the partition with exit 1; cancellation stops before the next file.
#[test]
fn for_each_file_crash_and_cancel() {
    let ctx = Ctx::new(Sandbox::new(true));
    let cx = ctx.cx(Epoch(1));
    let files = [
        OsStr::new("a"),
        OsStr::new("b"),
        OsStr::new("c"),
        OsStr::new("d"),
    ];
    let mut seen = Vec::new();
    let mut out = PyOut::default();
    let mut step = |f: &OsStr, out: &mut PyOut| {
        seen.push(f.to_string_lossy().into_owned());
        match f.to_str().unwrap() {
            "a" => {
                out.print("Fixing a");
                FileStep::Changed(RepoPath::new(b"a").unwrap())
            }
            "b" => FileStep::Failed,
            "c" => FileStep::Crash(PyExc::new("UnicodeDecodeError", "boom")),
            _ => FileStep::Unchanged,
        }
    };
    let o = framework::for_each_file(&cx, &files, &mut out, &mut step);
    assert_eq!(seen, ["a", "b", "c"]);
    assert_eq!((o.code, o.crashed), (1, true));
    assert_eq!(o.modified, [RepoPath::new(b"a").unwrap()]);
    assert_eq!(o.output.len(), 0);
    let o = framework::finish(o, out);
    assert_eq!(show(&o.output), "Fixing a\nUnicodeDecodeError: boom\n");

    ctx.cancel().cancel(CancelReason::Interrupt);
    let mut calls = 0;
    let mut out = PyOut::default();
    let o = framework::for_each_file(&cx, &files, &mut out, &mut |_, _| {
        calls += 1;
        FileStep::Failed
    });
    assert_eq!((calls, o.code), (0, 0));
}

/// `crash_line`: builtins bare, other classes module-qualified, a bare class for an empty text.
#[test]
fn crash_lines() {
    assert_eq!(
        framework::crash_line(&PyExc::new(
            "FileNotFoundError",
            "[Errno 2] No such file or directory: 'x'"
        )),
        b"FileNotFoundError: [Errno 2] No such file or directory: 'x'\n"
    );
    assert_eq!(
        framework::crash_line(&PyExc::new("JSONDecodeError", "Expecting value")),
        b"json.decoder.JSONDecodeError: Expecting value\n"
    );
    assert_eq!(
        framework::crash_line(&PyExc::new("KeyboardInterrupt", "")),
        b"KeyboardInterrupt\n"
    );
}

/// The regenerated Appendix A (09; `cargo xtask pch-goldens help`) is committed for every supported tag and every
/// console script of it (P0, P1, P2 and caveat hooks; WP-27 reads them): `-h` exits 0 and an unknown option is the
/// usage error, exit 2.
#[test]
fn appendix_a_goldens_committed() {
    let tags = [
        "v3.0.0", "v3.0.1", "v3.1.0", "v3.2.0", "v3.3.0", "v3.4.0", "v4.0.0", "v4.0.1", "v4.1.0",
        "v4.2.0", "v4.3.0", "v4.4.0", "v4.5.0", "v4.6.0", "v5.0.0", "v6.0.0",
    ];
    for tag in tags {
        let g = support::json(&format!("goldens/appendix_a/{tag}.json"));
        assert_eq!(g["version"].as_str().unwrap(), tag.trim_start_matches('v'));
        assert!(g["python"].as_str().unwrap().starts_with("3.13."));
        let scripts = g["scripts"].as_object().unwrap();
        assert!(scripts.len() >= 28, "{tag}: {} scripts", scripts.len());
        for p0 in [
            "trailing-whitespace-fixer",
            "end-of-file-fixer",
            "mixed-line-ending",
            "check-merge-conflict",
            "detect-private-key",
            "check-added-large-files",
            "no-commit-to-branch",
            "check-json",
            "check-executables-have-shebangs",
            "check-yaml",
            "pretty-format-json",
        ] {
            let s = &scripts[p0];
            assert_eq!(s["help"]["code"].as_i64(), Some(0), "{tag} {p0}");
            assert!(latin1(s["help"]["out"].as_str().unwrap()).starts_with(b"usage: "));
            assert_eq!(s["usage"]["code"].as_i64(), Some(2), "{tag} {p0}");
        }
    }
}
