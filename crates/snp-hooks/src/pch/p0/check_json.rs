//! `check-json` (`check_json.py`; 09 §4.C) over snp-pyparsers' CPython `json` port (CPython 3.13 messages).
//!
//! Epochs: 1 = v3.0.0…v3.4.0 (plain `json.load`, duplicate keys accepted), 2 = v4.0.0+ (`Duplicate key: …` via
//! `object_pairs_hook`). A `ValueError` is reported as `{f}: Failed to json decode ({exc})`; nesting beyond
//! CPython's recursion limit is the `RecursionError` crash. Builtin variant: the epoch-2 checks, and a document
//! too deep for CPython is reported (`maximum nesting depth exceeded`) instead of ending the partition (09 §7.2).
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;
use snp_pyparsers::CheckError;
use snp_pyparsers::json::PyMinor;

use super::common::{self, epoch_in, filenames, print_name, read_all};
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-json";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check."))];

/// `check_json.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The emulated CPython (09 §10 Q2: the minor pre-commit picks today).
const MINOR: PyMinor = PyMinor::P313;

/// The native.
#[derive(Debug)]
pub(crate) struct CheckJson {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckJson = CheckJson { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: CheckJson = CheckJson { builtin: true };

impl NativeHook for CheckJson {
    fn id(&self) -> &'static str {
        "check-json"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        if self.builtin {
            Ok(())
        } else {
            epoch_in(epoch, 2)
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = if self.builtin { 2 } else { cx.epoch.0 };
        let builtin = self.builtin;
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let content = match read_all(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let checked = if epoch <= 1 {
                snp_pyparsers::json::check_v3(&content, MINOR)
            } else {
                snp_pyparsers::json::check_ex(&content, MINOR)
            };
            let msg = match checked {
                Ok(()) => return FileStep::Unchanged,
                Err(CheckError::Reported(msg)) => msg,
                Err(CheckError::Crash(exc)) if builtin && exc.ty == "RecursionError" => {
                    "maximum nesting depth exceeded".to_owned()
                }
                Err(CheckError::Crash(exc)) => return FileStep::Crash(exc),
                Err(other) => other.into_message(),
            };
            let mut suffix = b": Failed to json decode (".to_vec();
            suffix.extend_from_slice(msg.as_bytes());
            suffix.push(b')');
            print_name(out, b"", name, &suffix);
            FileStep::Failed
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
