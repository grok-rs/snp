//! `check-shebang-scripts-are-executable` (`check_shebang_scripts_are_executable.py`; 09 §4.E).
//!
//! Epochs (message only): 1 = v4.0.1…v4.2.0 (v4.0.0's console script names a missing `main_reverse` and is
//! delegated), 2 = v4.3.0…v5.0.0 (`git add --chmod=+x` line), 3 = v6.0.0 (`If it is not supposed`). Always the
//! git path: `git ls-files -z --stage -- <paths>` (pathspecs: glob magic applies; an unmerged path is reported once
//! per stage; untracked files are skipped). Messages go to stderr.
//!
//! Owner: WP-27.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use super::util::{git_ls_files, has_shebang, mode_is_executable, os_from};
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-shebang-scripts-are-executable";

static ACTIONS: [ActionSpec; 1] = [files_positional(None)];

/// The parser (`description=__doc__`).
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: Some("Check that text files with a shebang are executable."),
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The native.
#[derive(Debug)]
pub(crate) struct CheckShebangScriptsAreExecutable;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckShebangScriptsAreExecutable = CheckShebangScriptsAreExecutable;

impl NativeHook for CheckShebangScriptsAreExecutable {
    fn id(&self) -> &'static str {
        "check-shebang-scripts-are-executable"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 3)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = cx.epoch.0;
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let (entries, mut pending) = match git_ls_files(cx, &files) {
            Ok(e) => e,
            Err(e) => return framework::crashed(out, &e),
        };
        let mut names: Vec<OsString> = entries.iter().map(|(_, p)| os_from(p.as_bytes())).collect();
        if pending.is_some() {
            // The malformed record's turn: the generator raises there.
            names.push(OsString::new());
        }
        let refs: Vec<&OsStr> = names.iter().map(OsString::as_os_str).collect();
        let mut i = 0_usize;
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let Some((mode, path)) = entries.get(i) else {
                return pending.take().map_or(FileStep::Unchanged, FileStep::Crash);
            };
            i = i.saturating_add(1);
            if mode_is_executable(mode) {
                return FileStep::Unchanged;
            }
            match has_shebang(cx, name) {
                Ok(false) => FileStep::Unchanged,
                Ok(true) => {
                    out.eprint_bytes(&kernels::shebang_scripts_message(path.as_bytes(), epoch));
                    FileStep::Failed
                }
                Err(e) => FileStep::Crash(e),
            }
        };
        let mut outcome = framework::for_each_file(cx, &refs, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
