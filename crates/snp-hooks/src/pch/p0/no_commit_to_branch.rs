//! `no-commit-to-branch` (`no_commit_to_branch.py`; 09 §4.E).
//!
//! Epochs: 1 = v3.0.0…v3.4.0 (default protected branch `master`), 2 = v4.0.0+ (`master`, `main`). No output;
//! exit 1 on a protected branch. The parser has no positional, so any file name is `unrecognized arguments`
//! (exit 2). Patterns are Python `re.match` (a prefix match) compiled lazily, and only when the branch is not in
//! the protected set: an invalid pattern crashes only when reached. Python iterates `frozenset`s (hash order);
//! snp tries patterns in argv order.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;
use snp_pyre::{Options, PyReError, PyRegex};

use super::common::{self, epoch_in, facts_exc, parse_static};
use crate::framework::{self, PyOut, append, with_help};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "no-commit-to-branch";

static ACTIONS: [ActionSpec; 2] = [
    with_help(
        append(&["-b", "--branch"], "branch"),
        Some("branch to disallow commits to, may be specified multiple times"),
    ),
    with_help(
        append(&["-p", "--pattern"], "pattern"),
        Some(
            "regex pattern for branch name to disallow commits to, may be specified multiple \
             times",
        ),
    ),
];

/// `no_commit_to_branch.main`'s parser.
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
pub(crate) struct NoCommitToBranch {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: NoCommitToBranch = NoCommitToBranch { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: NoCommitToBranch = NoCommitToBranch { builtin: true };

/// `re.compile(p)` as a `str` pattern matched against a branch name.
fn compile(p: &str) -> Result<PyRegex, PyReError> {
    PyRegex::compile(p, Options::CONFIG)
}

/// Why `is_on_branch` stopped without an answer.
enum Stop {
    /// An uncaught Python exception (an invalid pattern, a git failure).
    Exc(PyExc),
    /// D-SEC-10: a pattern exceeded the backtracking limit (CPython would hang, so the commit never goes through:
    /// the hook fails closed, exit 1).
    Backtrack(Vec<u8>),
}

/// `is_on_branch(protected, patterns)` (`:12-24`). Each pattern compiled emits its `FutureWarning`s into `warn`
/// (stderr, merged into the hook output) unless `warnings` is off (`SNP_PYTHON_WARNINGS=0`).
fn is_on_branch(
    cx: &HookCx<'_>,
    protected: &[&str],
    patterns: &[&str],
    warn: &mut Vec<u8>,
) -> Result<bool, Stop> {
    let Some(branch) = cx
        .facts
        .current_branch()
        .map_err(|f| Stop::Exc(facts_exc(f, false)))?
    else {
        return Ok(false);
    };
    if protected.contains(&branch.as_str()) {
        return Ok(true);
    }
    let warnings = framework::python_warnings(cx.env);
    for p in patterns {
        let re = compile(p).map_err(|e| Stop::Exc(framework::re_exc(&e)))?;
        if warnings {
            warn.extend_from_slice(&framework::regex_warnings(&re));
        }
        match re.match_prefix(&branch) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(_) => {
                return Err(Stop::Backtrack(framework::backtrack_line(
                    p,
                    OsStr::new(&branch),
                )));
            }
        }
    }
    Ok(false)
}

impl NativeHook for NoCommitToBranch {
    fn id(&self) -> &'static str {
        "no-commit-to-branch"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    /// Delegates when a `--pattern` is valid Python but beyond snp's regex engines.
    fn preflight(&self, epoch: Epoch, args: &[Arc<str>]) -> Result<(), String> {
        if !self.builtin {
            epoch_in(epoch, 2)?;
        }
        let Some(ns) = parse_static(&SPEC, PROG, args) else {
            return Ok(());
        };
        for p in ns.list("pattern") {
            if let Some(p) = p.to_str()
                && let Err(PyReError::Unsupported(why)) = compile(p)
            {
                return Err(format!("--pattern {p:?}: {why}"));
            }
        }
        Ok(())
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = if self.builtin { 2 } else { cx.epoch.0 };
        let given: Vec<String> = ns
            .list("branch")
            .iter()
            .map(|b| b.to_string_lossy().into_owned())
            .collect();
        let protected: Vec<&str> = if given.is_empty() {
            if epoch >= 2 {
                vec!["master", "main"]
            } else {
                vec!["master"]
            }
        } else {
            given.iter().map(String::as_str).collect()
        };
        let pats: Vec<String> = ns
            .list("pattern")
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut patterns: Vec<&str> = Vec::new();
        for p in &pats {
            if !patterns.contains(&p.as_str()) {
                patterns.push(p);
            }
        }
        let mut warn = Vec::new();
        let result = is_on_branch(cx, &protected, &patterns, &mut warn);
        let mut out = PyOut::default();
        out.eprint_bytes(&warn);
        let mut o = match result {
            Ok(on) => Outcome {
                code: i32::from(on),
                output: out.finish(),
                ..Outcome::default()
            },
            Err(Stop::Exc(e)) => framework::crashed(out, &e),
            Err(Stop::Backtrack(line)) => {
                out.eprint_bytes(&line);
                Outcome {
                    code: 1,
                    output: out.finish(),
                    crashed: true,
                    ..Outcome::default()
                }
            }
        };
        o.reads_only = true;
        o
    }
}
