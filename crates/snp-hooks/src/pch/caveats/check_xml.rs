//! `check-xml` (`check_xml.py`; 09 §4.C) over snp-pyparsers' expat port (as `xml.sax.parse` drives it),
//! NATIVE-WITH-CAVEATS behind its parity gate. One epoch v3.0.0…v6.0.0.
//!
//! A `SAXException` is printed as `{filename}: Failed to xml parse ({exc})` (`exc` = `{filename}:{line}:{col}:
//! {message}`, expat's 0-based column in characters) and the hook goes on; pyexpat's unknown-encoding failures
//! (`ValueError: multi-byte encodings are not supported`, `LookupError`) are crashes.
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
use crate::pch::p1::util::str_of;
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-xml";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("XML filenames to check."))];

/// `check_xml.main`'s parser.
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
pub(crate) struct CheckXml;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckXml = CheckXml;

impl NativeHook for CheckXml {
    fn id(&self) -> &'static str {
        "check-xml"
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
            match snp_pyparsers::xml::check_ex(&bytes, &str_of(name), PyMinor::P313) {
                Ok(()) => FileStep::Unchanged,
                Err(CheckError::Crash(e)) => FileStep::Crash(e),
                Err(other) => {
                    let mut suffix = b": Failed to xml parse (".to_vec();
                    suffix.extend_from_slice(other.into_message().as_bytes());
                    suffix.push(b')');
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
