//! `sort-simple-yaml` (`sort_simple_yaml.py`; 09 §4.B). One epoch v3.0.0…v6.0.0.
//!
//! The file is read in text mode (`r+`, the locale encoding — UTF-8 here, 09 §10 Q6 —, universal newlines; an
//! undecodable file is the `UnicodeDecodeError` crash with CPython's chunk-relative positions) and rewritten in
//! text mode (`\r\n` on Windows) after ``Fixing file `…` `` is printed. Python opens `r+`; the native reads first
//! and writes only when the order changes (D-CRASH-23).
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name, rewrite};
use crate::pch::p1::util::{read_for_update, text_read_lines, text_write_bytes};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "sort-simple-yaml";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to fix"))];

/// `sort_simple_yaml.main`'s parser.
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
pub(crate) struct SortSimpleYaml;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: SortSimpleYaml = SortSimpleYaml;

impl NativeHook for SortSimpleYaml {
    fn id(&self) -> &'static str {
        "sort-simple-yaml"
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
            let before = match read_for_update(cx, name) {
                Ok(b) => b,
                Err(e) => return FileStep::Crash(e),
            };
            let text = match text_read_lines(&before) {
                Ok(t) => t,
                Err(e) => return FileStep::Crash(e),
            };
            let Some(new) = kernels::sort_simple_yaml(&text) else {
                return FileStep::Unchanged;
            };
            print_name(out, b"Fixing file `", name, b"`");
            let bytes = text_write_bytes(&new, cx.windows);
            // A file that is exactly `\n` is "rewritten" with the same bytes: exit 1, nothing modified.
            if bytes.as_bytes() == before.as_slice() {
                return FileStep::Failed;
            }
            if let Err(e) = rewrite(cx, name, bytes.as_bytes()) {
                return FileStep::Crash(e);
            }
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        framework::finish(outcome, out)
    }
}
