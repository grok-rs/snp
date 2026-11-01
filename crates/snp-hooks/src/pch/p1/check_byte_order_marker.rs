//! The real `check-byte-order-marker` of v3.0.0…v5.0.0 (`check_byte_order_marker.py`; 09 §4.H). From v6.0.0 the id
//! is the removed-hook stub (`pch::stubs`), which forwards epoch 1 here.
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name, read_head};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-byte-order-marker";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check"))];

/// `check_byte_order_marker.main`'s parser.
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
pub(crate) struct CheckByteOrderMarker;

/// The pre-commit-hooks native (epoch 1).
pub(crate) static UPSTREAM: CheckByteOrderMarker = CheckByteOrderMarker;

impl NativeHook for CheckByteOrderMarker {
    fn id(&self) -> &'static str {
        "check-byte-order-marker"
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
        let mut step = |name: &OsStr, out: &mut PyOut| match read_head(cx, name, 3) {
            Ok(head) if kernels::has_bom(&head) => {
                print_name(out, b"", name, b": Has a byte-order marker");
                FileStep::Failed
            }
            Ok(_) => FileStep::Unchanged,
            Err(e) => FileStep::Crash(e),
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
