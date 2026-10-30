//! `mixed-line-ending` (`mixed_line_ending.py`; 09 §4.A).
//!
//! One epoch v3.0.0…v6.0.0. Builtin variant: `--check` reports what `--fix` would do without writing (fixer exit
//! semantics: 1 for every file that would change).
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, DefaultValue, ParserSpec};
use snp_hooks_data::Epoch;

use super::common::{self, epoch_in, filenames, print_name, read_all, rewrite};
use super::kernels::{self, LineFix};
use crate::framework::{
    self, FileStep, PyOut, files_positional, store, store_true, with_choices, with_default,
    with_help,
};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "mixed-line-ending";

const FIX: ActionSpec = with_help(
    with_default(
        with_choices(
            store(&["-f", "--fix"], "fix"),
            &["auto", "no", "cr", "crlf", "lf"],
        ),
        DefaultValue::Str("auto"),
    ),
    Some("Replace line ending with the specified. Default is \"auto\""),
);

static UPSTREAM_ACTIONS: [ActionSpec; 2] = [FIX, files_positional(Some("Filenames to fix"))];
static BUILTIN_ACTIONS: [ActionSpec; 3] = [
    FIX,
    with_help(
        store_true(&["--check"], "check"),
        Some("Report the files that would be fixed without changing them."),
    ),
    files_positional(Some("Filenames to fix")),
];

/// `mixed_line_ending.main`'s parser.
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
pub(crate) struct MixedLineEnding {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: MixedLineEnding = MixedLineEnding { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: MixedLineEnding = MixedLineEnding { builtin: true };

impl NativeHook for MixedLineEnding {
    fn id(&self) -> &'static str {
        "mixed-line-ending"
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
        let fix = ns
            .value("fix")
            .and_then(OsStr::to_str)
            .and_then(LineFix::parse)
            .unwrap_or(LineFix::Auto);
        let check = self.builtin && ns.flag("check");
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let content = match read_all(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let (ret, new) = kernels::mixed_line_ending(&content, fix);
            if !ret {
                return FileStep::Unchanged;
            }
            let Some(new) = new.filter(|_| !check) else {
                // `--fix=no` (or the builtin `--check`): report only.
                let msg: &[u8] = if fix == LineFix::No || check {
                    b": mixed line endings"
                } else {
                    b": fixed mixed line endings"
                };
                print_name(out, b"", name, msg);
                return FileStep::Failed;
            };
            if let Err(e) = rewrite(cx, name, &new) {
                return FileStep::Crash(e);
            }
            print_name(out, b"", name, b": fixed mixed line endings");
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = fix == LineFix::No || check;
        framework::finish(outcome, out)
    }
}
