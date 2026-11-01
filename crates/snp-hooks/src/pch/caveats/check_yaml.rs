//! `check-yaml` (`check_yaml.py`; 09 §4.C, §9.5) over snp-pyparsers' ruamel.yaml ≥ 0.19 port
//! (`yamlcheck::Checker`), NATIVE-WITH-CAVEATS behind its parity gate.
//!
//! Epochs: 1 = v3.0.0…v4.4.0 (`--unsafe` help typo `contstructs`), 2 = v4.5.0+. Modes: default `yaml.load` (one
//! document), `-m`/`--multi` `load_all`, `--unsafe` `parse` only. One `Checker` serves the whole partition, as the
//! hook's module-level `YAML` object does (a `%YAML 1.1` directive and a failed construction leak into the next
//! file). A `YAMLError` is printed (`str(exc)`, marks name the file) and the hook goes on; any other exception
//! (`UnicodeDecodeError`, a constructor `ValueError`, `RecursionError`, …) ends the partition. ruamel's warnings
//! reach stderr as Python shows them, with the module path relative to `site-packages` (the hook's virtualenv
//! path is not reproduced; message class, gate-documented).
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;
use snp_pyparsers::CheckError;
use snp_pyparsers::json::PyMinor;
use snp_pyparsers::yamlcheck::{Checker, Mode};

use crate::framework::{self, FileStep, PyOut, files_positional, store_true, with_help};
use crate::pch::p0::common::{self, epoch_in, filenames, read_all};
use crate::pch::p1::util::str_of;
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-yaml";

/// The `site-packages` prefix of rendered ruamel warnings.
pub(crate) const SITE_PACKAGES: &str = "site-packages";

const MULTI: ActionSpec = store_true(&["-m", "--multi", "--allow-multiple-documents"], "multi");
const FILES: ActionSpec = files_positional(Some("Filenames to check."));

static E1_ACTIONS: [ActionSpec; 3] = [
    MULTI,
    with_help(
        store_true(&["--unsafe"], "unsafe"),
        Some(
            "Instead of loading the files, simply parse them for syntax.  A syntax-only check enables extensions \
             and unsafe contstructs which would otherwise be forbidden.  Using this option removes all \
             guarantees of portability to other yaml implementations.  Implies --allow-multiple-documents",
        ),
    ),
    FILES,
];
static E2_ACTIONS: [ActionSpec; 3] = [
    MULTI,
    with_help(
        store_true(&["--unsafe"], "unsafe"),
        Some(
            "Instead of loading the files, simply parse them for syntax.  A syntax-only check enables extensions \
             and unsafe constructs which would otherwise be forbidden.  Using this option removes all \
             guarantees of portability to other yaml implementations.  Implies --allow-multiple-documents",
        ),
    ),
    FILES,
];

/// The parser of v3.0.0…v4.4.0.
pub(crate) static SPEC_E1: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &E1_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
/// v4.5.0+.
pub(crate) static SPEC_E2: ParserSpec = ParserSpec {
    actions: &E2_ACTIONS,
    ..SPEC_E1
};

/// The parser at `epoch`.
pub(crate) fn spec(epoch: u16) -> &'static ParserSpec {
    if epoch <= 1 { &SPEC_E1 } else { &SPEC_E2 }
}

/// `LOAD_FNS[Key(multi, unsafe)]`.
pub(crate) const fn mode(multi: bool, unsafe_: bool) -> Mode {
    if unsafe_ {
        Mode::Unsafe
    } else if multi {
        Mode::AllowMultipleDocuments
    } else {
        Mode::Safe
    }
}

/// The native.
#[derive(Debug)]
pub(crate) struct CheckYaml;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckYaml = CheckYaml;

impl NativeHook for CheckYaml {
    fn id(&self) -> &'static str {
        "check-yaml"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 2)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, spec(cx.epoch.0), PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let mut checker = Checker::new(mode(ns.flag("multi"), ns.flag("unsafe")), PyMinor::P313);
        let warnings = framework::python_warnings(cx.env);
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let bytes = match read_all(cx, name) {
                Ok(b) => b,
                Err(e) => return FileStep::Crash(e),
            };
            let result = checker.check_file(&bytes, &str_of(name));
            for w in checker.take_warnings() {
                if warnings {
                    out.eprint_bytes(w.render(SITE_PACKAGES).as_bytes());
                }
            }
            match result {
                Ok(()) => FileStep::Unchanged,
                Err(CheckError::Reported(msg)) => {
                    out.print(&msg);
                    FileStep::Failed
                }
                Err(CheckError::Crash(e)) => FileStep::Crash(e),
                Err(other) => {
                    out.print(&other.into_message());
                    FileStep::Failed
                }
            }
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
