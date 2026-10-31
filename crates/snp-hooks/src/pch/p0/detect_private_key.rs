//! `detect-private-key` (`detect_private_key.py`; 09 §4.D).
//!
//! Epochs: 1 = v3.0.0…v4.0.1 (8 patterns), 2 = v4.1.0+ (10 patterns). The report is printed after every file was
//! read; a crash on a later file therefore prints nothing for earlier hits. The builtin variant is the epoch-2
//! behaviour.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use super::common::{self, epoch_in, filenames, print_name, read_all};
use super::kernels;
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "detect-private-key";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check"))];

/// `detect_private_key.main`'s parser.
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
pub(crate) struct DetectPrivateKey {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: DetectPrivateKey = DetectPrivateKey { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: DetectPrivateKey = DetectPrivateKey { builtin: true };

impl NativeHook for DetectPrivateKey {
    fn id(&self) -> &'static str {
        "detect-private-key"
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
        let files = filenames(&ns);
        let mut found: Vec<std::ffi::OsString> = Vec::new();
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, _out: &mut PyOut| match read_all(cx, name) {
            Ok(content) if kernels::has_private_key(&content, epoch) => {
                found.push(name.to_os_string());
                FileStep::Unchanged
            }
            Ok(_) => FileStep::Unchanged,
            Err(e) => FileStep::Crash(e),
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        if !outcome.crashed && !cx.cancel.is_cancelled() && !found.is_empty() {
            for name in &found {
                print_name(&mut out, b"Private key found: ", name, b"");
            }
            outcome.code = 1;
        }
        framework::finish(outcome, out)
    }
}
