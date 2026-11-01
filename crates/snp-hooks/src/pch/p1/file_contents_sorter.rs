//! `file-contents-sorter` (`file_contents_sorter.py`; 09 §4.B).
//!
//! Epochs: 1 = v3.0.0…v3.2.0 (no options; lines `strip(b'\n\r')`), 2 = v3.3.0 (`--ignore-case`), 3 = v3.4.0…v4.4.0
//! (`--unique`; lines `rstrip(b'\n\r')`), 4 = v4.5.0…v5.0.0 (an empty result stays empty), 5 = v6.0.0
//! (`--ignore-case` and `--unique` mutually exclusive). Python opens every file `rb+`; the native reads first and
//! writes only when the sorted text differs (D-CRASH-23).
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, Kind, Nargs, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use super::util::read_for_update;
use crate::framework::{self, FileStep, PyOut, positional, store, store_true, with_help};
use crate::pch::p0::common::{self, epoch_in, filenames, print_name, rewrite};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "file-contents-sorter";

const FILES: ActionSpec = with_help(
    positional("filenames", Nargs::OneOrMore),
    Some("Files to sort"),
);
const IGNORE_CASE: ActionSpec = with_help(
    ActionSpec {
        kind: Kind::StoreConst("bytes.lower"),
        nargs: Nargs::Zero,
        ..store(&["--ignore-case"], "ignore_case")
    },
    Some("fold lower case to upper case characters"),
);
const UNIQUE: ActionSpec = with_help(
    store_true(&["--unique"], "unique"),
    Some("ensure each line is unique"),
);

static E1_ACTIONS: [ActionSpec; 1] = [FILES];
static E2_ACTIONS: [ActionSpec; 2] = [FILES, IGNORE_CASE];
static E3_ACTIONS: [ActionSpec; 3] = [FILES, IGNORE_CASE, UNIQUE];
static E5_ACTIONS: [ActionSpec; 3] = [
    FILES,
    ActionSpec {
        mutex_group: Some(0),
        ..IGNORE_CASE
    },
    ActionSpec {
        mutex_group: Some(0),
        ..UNIQUE
    },
];

/// The parser of v3.0.0…v3.2.0.
pub(crate) static SPEC_E1: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &E1_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
/// v3.3.0.
pub(crate) static SPEC_E2: ParserSpec = ParserSpec {
    actions: &E2_ACTIONS,
    ..SPEC_E1
};
/// v3.4.0…v5.0.0.
pub(crate) static SPEC_E3: ParserSpec = ParserSpec {
    actions: &E3_ACTIONS,
    ..SPEC_E1
};
/// v6.0.0 (mutually exclusive group).
pub(crate) static SPEC_E5: ParserSpec = ParserSpec {
    actions: &E5_ACTIONS,
    ..SPEC_E1
};

/// The parser at `epoch`.
pub(crate) fn spec(epoch: u16) -> &'static ParserSpec {
    match epoch {
        0 | 1 => &SPEC_E1,
        2 => &SPEC_E2,
        3 | 4 => &SPEC_E3,
        _ => &SPEC_E5,
    }
}

/// The native.
#[derive(Debug)]
pub(crate) struct FileContentsSorter;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: FileContentsSorter = FileContentsSorter;

impl NativeHook for FileContentsSorter {
    fn id(&self) -> &'static str {
        "file-contents-sorter"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 5)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let epoch = cx.epoch.0;
        let ns = match common::parse(cx, spec(epoch), PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let opts = kernels::SortOpts {
            ignore_case: ns.value("ignore_case").is_some(),
            unique: ns.flag("unique"),
            strip_both: epoch <= 2,
            empty_stays_empty: epoch >= 4,
        };
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let content = match read_for_update(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let Some(sorted) = kernels::file_contents_sort(&content, opts) else {
                return FileStep::Unchanged;
            };
            if let Err(e) = rewrite(cx, name, &sorted) {
                return FileStep::Crash(e);
            }
            print_name(out, b"Sorting ", name, b"");
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        framework::finish(outcome, out)
    }
}
