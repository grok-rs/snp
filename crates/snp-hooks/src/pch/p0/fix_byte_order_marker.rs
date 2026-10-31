//! `fix-byte-order-marker` (`fix_byte_order_marker.py`; 09 §4.A).
//!
//! One epoch v3.3.0…v6.0.0 (the hook does not exist before). A 3-byte probe; the rest is read only when a BOM is
//! present and validated as strict UTF-8 (Python reads it with `encoding='utf-8-sig'`): an invalid rest is
//! Python's `UnicodeDecodeError` crash. Builtin variant: byte-level (no validation, no crash) and `--check`.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;

use super::common::{self, epoch_in, filenames, print_name, read_all, read_head, rewrite};
use super::kernels::{self, BOM};
use crate::framework::{self, FileStep, PyOut, files_positional, store_true, with_help};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "fix-byte-order-marker";

static UPSTREAM_ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check"))];
static BUILTIN_ACTIONS: [ActionSpec; 2] = [
    files_positional(Some("Filenames to check")),
    with_help(
        store_true(&["--check"], "check"),
        Some("Report the files that would be fixed without changing them."),
    ),
];

/// `fix_byte_order_marker.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &UPSTREAM_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
static BUILTIN_SPEC: ParserSpec = ParserSpec {
    actions: &BUILTIN_ACTIONS,
    ..SPEC
};

/// The native.
#[derive(Debug)]
pub(crate) struct FixByteOrderMarker {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: FixByteOrderMarker = FixByteOrderMarker { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: FixByteOrderMarker = FixByteOrderMarker { builtin: true };

impl NativeHook for FixByteOrderMarker {
    fn id(&self) -> &'static str {
        "fix-byte-order-marker"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        if self.builtin {
            Ok(())
        } else {
            epoch_in(epoch, 1)
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let spec = if self.builtin { &BUILTIN_SPEC } else { &SPEC };
        let ns = match common::parse(cx, spec, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let check = self.builtin && ns.flag("check");
        let builtin = self.builtin;
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            match read_head(cx, name, 3) {
                Ok(head) if head == BOM => {}
                Ok(_) => return FileStep::Unchanged,
                Err(e) => return FileStep::Crash(e),
            }
            let content = match read_all(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let rest = if builtin {
                content.get(BOM.len()..).unwrap_or_default()
            } else {
                match kernels::fix_byte_order_marker(&content) {
                    Ok(Some(rest)) => rest,
                    // The file changed between the probe and the read.
                    Ok(None) => return FileStep::Unchanged,
                    Err(msg) => return FileStep::Crash(PyExc::new("UnicodeDecodeError", msg)),
                }
            };
            if check {
                print_name(out, b"", name, b": would remove byte-order marker");
                return FileStep::Failed;
            }
            if let Err(e) = rewrite(cx, name, rest) {
                return FileStep::Crash(e);
            }
            print_name(out, b"", name, b": removed byte-order marker");
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = check;
        framework::finish(outcome, out)
    }
}
