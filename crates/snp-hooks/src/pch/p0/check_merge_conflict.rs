//! `check-merge-conflict` (`check_merge_conflict.py`; 09 §4.D).
//!
//! Epochs: 1 = v3.0.0…v4.0.1 (literal `.git` directory, message `Merge conflict string "{pattern}" found in
//! {f}:{n}` with the unstripped pattern), 2 = v4.1.0 (`git rev-parse --git-dir`, old message), 3 = v4.2.0+
//! (`=======\r\n` pattern, `{f}:{n}: Merge conflict string '{marker}' found`). The merge state is always computed
//! first (a crash outside a repository even with `--assume-in-merge`); cherry-pick and revert conflicts are not
//! merges (bug-compat). Builtin variant: also `CHERRY_PICK_HEAD`/`REVERT_HEAD`, and `--diff3` adds `||||||| `.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_git::pch_facts::FactSet;
use snp_hooks_data::Epoch;

use super::common::{self, cmd_output, epoch_in, filenames, py_rstrip, read_all};
use super::kernels;
use crate::framework::{self, FileStep, PyOut, files_positional, store_true, with_help};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-merge-conflict";

static UPSTREAM_ACTIONS: [ActionSpec; 2] = [
    files_positional(None),
    store_true(&["--assume-in-merge"], "assume_in_merge"),
];
static BUILTIN_ACTIONS: [ActionSpec; 3] = [
    files_positional(None),
    store_true(&["--assume-in-merge"], "assume_in_merge"),
    with_help(
        store_true(&["--diff3"], "diff3"),
        Some("Also report diff3 base markers (`||||||| `)."),
    ),
];

/// `check_merge_conflict.main`'s parser.
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
pub(crate) struct CheckMergeConflict {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckMergeConflict = CheckMergeConflict { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: CheckMergeConflict = CheckMergeConflict { builtin: true };

/// `is_in_merge()` (`:19-29`; epoch 1 uses the literal `.git`).
fn is_in_merge(cx: &HookCx<'_>, epoch: u16, builtin: bool) -> Result<bool, PyExc> {
    let git_dir: PathBuf = if epoch <= 1 && !builtin {
        cx.root.join(".git")
    } else {
        let out = cmd_output(
            cx,
            &[OsStr::new("rev-parse"), OsStr::new("--git-dir")],
            Some(0),
        )?;
        cx.root.join(py_rstrip(&out))
    };
    let exists = |name: &str| git_dir.join(name).exists();
    let mut heads = exists("MERGE_HEAD") || exists("rebase-apply") || exists("rebase-merge");
    if builtin {
        heads = heads || exists("CHERRY_PICK_HEAD") || exists("REVERT_HEAD");
    }
    Ok(exists("MERGE_MSG") && heads)
}

/// The report line for a hit.
fn message(name: &OsStr, lineno: usize, pattern: &[u8], epoch: u16) -> Vec<u8> {
    let mut line = Vec::new();
    if epoch >= 3 {
        line.extend_from_slice(framework::os_bytes(name));
        line.extend_from_slice(format!(":{lineno}: Merge conflict string ").as_bytes());
        let marker = String::from_utf8_lossy(snp_pycompat::text::bytes_strip(pattern)).into_owned();
        line.extend_from_slice(snp_pycompat::repr::repr_str(&marker).as_bytes());
        line.extend_from_slice(b" found");
    } else {
        line.extend_from_slice(b"Merge conflict string \"");
        line.extend_from_slice(pattern);
        line.extend_from_slice(b"\" found in ");
        line.extend_from_slice(framework::os_bytes(name));
        line.extend_from_slice(format!(":{lineno}").as_bytes());
    }
    line
}

impl NativeHook for CheckMergeConflict {
    fn id(&self) -> &'static str {
        "check-merge-conflict"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        if self.builtin {
            Ok(())
        } else {
            epoch_in(epoch, 3)
        }
    }

    /// `rev-parse --git-dir` (`is_in_merge`), except at epoch 1 (the literal `.git`).
    fn facts(&self, _args: &[Arc<str>], epoch: Epoch) -> FactSet {
        if self.builtin || epoch.0 >= 2 {
            FactSet::GIT_DIR
        } else {
            FactSet::NONE
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let spec = if self.builtin { &BUILTIN_SPEC } else { &SPEC };
        let ns = match common::parse(cx, spec, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = if self.builtin { 3 } else { cx.epoch.0 };
        let out = PyOut::default();
        let in_merge = match is_in_merge(cx, epoch, self.builtin) {
            Ok(m) => m,
            Err(e) => return framework::crashed(out, &e),
        };
        let reads_only = Outcome {
            reads_only: true,
            ..Outcome::default()
        };
        if !in_merge && !ns.flag("assume_in_merge") {
            return reads_only;
        }
        let patterns = kernels::merge_conflict_patterns(epoch, self.builtin && ns.flag("diff3"));
        let files = filenames(&ns);
        let mut out = out;
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let content = match read_all(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let hits = kernels::merge_conflicts(&content, &patterns);
            for (lineno, pattern) in &hits {
                out.print_bytes_text(&message(name, *lineno, pattern, epoch));
            }
            if hits.is_empty() {
                FileStep::Unchanged
            } else {
                FileStep::Failed
            }
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
