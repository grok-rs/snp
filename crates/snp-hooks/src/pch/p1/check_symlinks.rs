//! `check-symlinks` (`check_symlinks.py`; 09 §4.E). One epoch v3.0.0…v6.0.0.
//!
//! `os.path.islink(f) and not os.path.exists(f)`: `exists` is false for any `stat` error, so a symlink loop is
//! broken too.
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-symlinks";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check"))];

/// `check_symlinks.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: Some("Checks for broken symlinks."),
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The native.
#[derive(Debug)]
pub(crate) struct CheckSymlinks;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckSymlinks = CheckSymlinks;

impl NativeHook for CheckSymlinks {
    fn id(&self) -> &'static str {
        "check-symlinks"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 1)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let path = framework::at(cx, name);
            let islink = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
            if islink && std::fs::metadata(&path).is_err() {
                print_name(out, b"", name, b": Broken symlink");
                FileStep::Failed
            } else {
                FileStep::Unchanged
            }
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
