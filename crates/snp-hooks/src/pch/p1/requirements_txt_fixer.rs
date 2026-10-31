//! `requirements-txt-fixer` (`requirements_txt_fixer.py`; 09 §4.B).
//!
//! Epochs: 0 = v3.0.0…v3.0.1 (DELEGATE: names cut at `==` only), 1 = v3.1.0…v3.4.0, 2 = v4.0.0…v4.5.0 (indented
//! comments), 3 = v4.6.0 (duplicates removed, commented entries first), 4 = v5.0.0+ (`pkg_resources==0.0.0`
//! removed too). A requirement whose name cannot be computed (an indented or `;`-led line) is the
//! `AssertionError` crash once two requirements must be compared. Python opens every file `rb+`; the native
//! reads first and writes only when the text changes (D-CRASH-23).
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use super::util::read_for_update;
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name, rewrite};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "requirements-txt-fixer";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to fix"))];

/// `requirements_txt_fixer.main`'s parser.
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
pub(crate) struct RequirementsTxtFixer;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: RequirementsTxtFixer = RequirementsTxtFixer;

impl NativeHook for RequirementsTxtFixer {
    fn id(&self) -> &'static str {
        "requirements-txt-fixer"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 4)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = cx.epoch.0;
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let content = match read_for_update(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let fixed = match kernels::requirements_fix(&content, epoch) {
                Ok(Some(f)) => f,
                Ok(None) => return FileStep::Unchanged,
                Err(e) => return FileStep::Crash(e),
            };
            if let Err(e) = rewrite(cx, name, &fixed) {
                return FileStep::Crash(e);
            }
            print_name(out, b"Sorting ", name, b"");
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        framework::finish(outcome, out)
    }
}
