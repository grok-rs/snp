//! `check-toml` (`check_toml.py`; 09 §4.C) over snp-pyparsers' CPython 3.13 `tomllib` port, NATIVE-WITH-CAVEATS
//! behind its parity gate.
//!
//! Epochs: 0 = v3.0.1…v4.2.0 (DELEGATE: the `toml` PyPI package), 1 = v4.3.0+ (`tomllib`). A `TOMLDecodeError` is
//! printed as `{filename}: {exc}` and the hook goes on; an undecodable file (`UnicodeDecodeError`), nesting beyond
//! the recursion limit and over-long integers are crashes.
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;
use snp_pyparsers::CheckError;
use snp_pyparsers::json::PyMinor;

use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name, read_all};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-toml";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check."))];

/// `check_toml.main`'s parser.
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
pub(crate) struct CheckToml;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckToml = CheckToml;

impl NativeHook for CheckToml {
    fn id(&self) -> &'static str {
        "check-toml"
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
            let bytes = match read_all(cx, name) {
                Ok(b) => b,
                Err(e) => return FileStep::Crash(e),
            };
            match snp_pyparsers::toml::check_ex(&bytes, PyMinor::P313) {
                Ok(()) => FileStep::Unchanged,
                Err(CheckError::Crash(e)) => FileStep::Crash(e),
                Err(other) => {
                    let mut suffix = b": ".to_vec();
                    suffix.extend_from_slice(other.into_message().as_bytes());
                    print_name(out, b"", name, &suffix);
                    FileStep::Failed
                }
            }
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
