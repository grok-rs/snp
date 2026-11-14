//! `check-case-conflict` (`check_case_conflict.py`; 09 §4.E), bug-compatible.
//!
//! Epochs: 1 = v3.0.0…v3.4.0 (files only), 2 = v4.0.0…v4.0.1 (parent directories via `os.path.dirname`), 3 =
//! v4.1.0+ (parents via `split('/')`; differs from 2 only on Windows). The repository side is `git ls-files` and
//! the staged-added side `git diff --staged --name-only --diff-filter=A`, both non-`-z`: with the default
//! `core.quotePath` a non-ASCII path is C-quoted and never conflicts (09 §0 #5). Every partition sees all staged
//! files, so a conflict among them is reported once per partition, as in Python.
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use super::util::bytes_of;
use crate::framework::{self, PyOut, files_positional};
use crate::pch::p0::common::{self, epoch_in, facts_exc, filenames};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-case-conflict";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some(
    "Filenames pre-commit believes are changed.",
))];

/// `check_case_conflict.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The [`snp_git::pch_facts::PchFacts::tracked_derived`] tag of the repository index at `epoch` / `windows`.
fn index_tag(epoch: u16, windows: bool) -> u32 {
    u32::from(epoch).saturating_mul(2).saturating_add(u32::from(windows))
}

/// The native.
#[derive(Debug)]
pub(crate) struct CheckCaseConflict;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckCaseConflict = CheckCaseConflict;

impl NativeHook for CheckCaseConflict {
    fn id(&self) -> &'static str {
        "check-case-conflict"
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
        let out = PyOut::default();
        let repo = match cx.facts.tracked_files() {
            Ok(r) => r,
            Err(f) => return framework::crashed(out, &facts_exc(f, false)),
        };
        let added = match cx.facts.added_files() {
            Ok(a) => a,
            Err(f) => return framework::crashed(out, &facts_exc(f, false)),
        };
        let files: Vec<&[u8]> = filenames(&ns).into_iter().map(bytes_of).collect();
        let added: Vec<&[u8]> = added.iter().map(Vec::as_slice).collect();
        let (epoch, windows) = (cx.epoch.0, cx.windows);
        // the repository side is built once per `git ls-files` answer, outside any partition (ADR-102)
        let index = cx
            .facts
            .tracked_derived(&repo, index_tag(epoch, windows), |r| {
                kernels::RepoCaseIndex::new(r, epoch, windows)
            });
        let index = index.as_deref().and_then(Option::as_ref);
        let names = kernels::case_conflicts_indexed(index, &repo, &files, &added, epoch, windows);
        let mut out = out;
        for n in &names {
            let mut line = b"Case-insensitivity conflict found: ".to_vec();
            line.extend_from_slice(n);
            out.print_bytes_text(&line);
        }
        Outcome {
            code: i32::from(!names.is_empty()),
            output: out.finish(),
            reads_only: true,
            ..Outcome::default()
        }
    }
}
