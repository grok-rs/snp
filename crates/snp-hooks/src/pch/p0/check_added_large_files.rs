//! `check-added-large-files` (`check_added_large_files.py`; 09 §4.E).
//!
//! Epochs: 1 = v3.0.0…v3.2.0 (`(added ∩ files) − lfs`, LFS from `git lfs status --json`), 2 = v3.3.0…v4.0.1
//! (`--enforce-all`), 3 = v4.1.0+ (LFS from `git check-attr filter -z --stdin`). The staged-added list is git's
//! non-`-z` output (C-quoted, rename-aware): non-ASCII names never match (bug-compat, 09 §0 #5). Python iterates a
//! `set` (hash order, different per process); snp reports in input order. Builtin variant: `-z` plumbing (every
//! name is checked), `--include-renames` (`--diff-filter=AR`).
//!
//! Owner: WP-26.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_git::pch_facts::FactSet;
use snp_hooks_data::Epoch;
use snp_pyparsers::json::{JsonValue, PyMinor};

use super::common::{self, cmd_output, epoch_in, facts_exc, filenames};
use super::kernels;
use crate::framework::{
    self, FileStep, PyOut, files_positional, int_opt, io_exc, store_true, with_help,
};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-added-large-files";

const FILES: ActionSpec = files_positional(Some("Filenames pre-commit believes are changed."));
const ENFORCE_ALL: ActionSpec = with_help(
    store_true(&["--enforce-all"], "enforce_all"),
    Some("Enforce all files are checked, not just staged files."),
);
/// v3.x carries the help typo `Maxmimum` (fixed in v4.1.0).
const MAXKB_OLD: ActionSpec = with_help(
    int_opt(&["--maxkb"], "maxkb", 500),
    Some("Maxmimum allowable KB for added files"),
);
const MAXKB: ActionSpec = with_help(
    int_opt(&["--maxkb"], "maxkb", 500),
    Some("Maximum allowable KB for added files"),
);

static E1_ACTIONS: [ActionSpec; 2] = [FILES, MAXKB_OLD];
static E2_ACTIONS: [ActionSpec; 3] = [FILES, ENFORCE_ALL, MAXKB_OLD];
static E3_ACTIONS: [ActionSpec; 3] = [FILES, ENFORCE_ALL, MAXKB];
static BUILTIN_ACTIONS: [ActionSpec; 4] = [
    FILES,
    ENFORCE_ALL,
    MAXKB,
    with_help(
        store_true(&["--include-renames"], "include_renames"),
        Some("Also check staged renames (`--diff-filter=AR`)."),
    ),
];

/// `check_added_large_files.main`'s parser, v3.0.0…v3.2.0.
pub(crate) static SPEC_E1: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &E1_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
/// v3.3.0…v4.0.1.
pub(crate) static SPEC_E2: ParserSpec = ParserSpec {
    actions: &E2_ACTIONS,
    ..SPEC_E1
};
/// v4.1.0+.
pub(crate) static SPEC_E3: ParserSpec = ParserSpec {
    actions: &E3_ACTIONS,
    ..SPEC_E1
};
static BUILTIN_SPEC: ParserSpec = ParserSpec {
    actions: &BUILTIN_ACTIONS,
    ..SPEC_E1
};

/// The native.
#[derive(Debug)]
pub(crate) struct CheckAddedLargeFiles {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckAddedLargeFiles = CheckAddedLargeFiles { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: CheckAddedLargeFiles = CheckAddedLargeFiles { builtin: true };

/// `lfs_files()` of epochs 1-2 (`check_added_large_files.py:14-21` at v3.x): `git lfs status --json` (a failing
/// git, e.g. without git-lfs, means no LFS files), `set(json.loads(out)['files'])`.
fn lfs_status(cx: &HookCx<'_>) -> Result<HashSet<Vec<u8>>, PyExc> {
    let args = [
        OsStr::new("lfs"),
        OsStr::new("status"),
        OsStr::new("--json"),
    ];
    let text = match cmd_output(cx, &args, Some(0)) {
        Ok(t) => t,
        Err(e) if e.ty == "pre_commit_hooks.util.CalledProcessError" => r#"{"files":{}}"#.into(),
        Err(e) => return Err(e),
    };
    let value = snp_pyparsers::json::loads(text.as_bytes(), PyMinor::P313)
        .map_err(|m| PyExc::new("json.decoder.JSONDecodeError", m))?;
    let JsonValue::Object(top) = &value else {
        return Err(PyExc::new(
            "TypeError",
            "list indices must be integers or slices, not str",
        ));
    };
    let Some((_, files)) = top.iter().rev().find(|(k, _)| k == "files") else {
        return Err(PyExc::new("KeyError", "'files'"));
    };
    let mut set = HashSet::new();
    match files {
        JsonValue::Object(entries) => {
            for (k, _) in entries {
                set.insert(k.as_bytes().to_vec());
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                if let JsonValue::Str(s) = item {
                    set.insert(s.as_bytes().to_vec());
                }
            }
        }
        _ => {
            return Err(PyExc::new("TypeError", "object is not iterable"));
        }
    }
    Ok(set)
}

/// The builtin's staged-added names: `git diff --staged --name-only -z --diff-filter=A[R]` (raw names).
fn added_z(cx: &HookCx<'_>, renames: bool) -> Result<HashSet<Vec<u8>>, PyExc> {
    let filter = if renames {
        "--diff-filter=AR"
    } else {
        "--diff-filter=A"
    };
    let args = [
        OsStr::new("diff"),
        OsStr::new("--staged"),
        OsStr::new("--name-only"),
        OsStr::new("-z"),
        OsStr::new(filter),
    ];
    let c = cx.facts.raw(&args).map_err(framework::failure_exc)?;
    Ok(snp_pycompat::precommit::zsplit(&c.stdout)
        .map(<[u8]>::to_vec)
        .collect())
}

impl CheckAddedLargeFiles {
    /// The file names to stat, in input order without repeats.
    fn selected<'f>(
        &self,
        cx: &HookCx<'_>,
        files: &[&'f OsStr],
        epoch: u16,
        enforce_all: bool,
        renames: bool,
    ) -> Result<Vec<&'f OsStr>, PyExc> {
        let mut seen = HashSet::new();
        let unique: Vec<&OsStr> = files
            .iter()
            .copied()
            .filter(|f| seen.insert(f.as_encoded_bytes()))
            .collect();
        let lfs: HashSet<Vec<u8>> = if epoch >= 3 {
            let names: Vec<&[u8]> = unique.iter().map(|f| f.as_encoded_bytes()).collect();
            cx.facts
                .lfs_files(&names)
                .map_err(|f| facts_exc(f, true))?
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        };
        let added: Option<HashSet<Vec<u8>>> = if enforce_all && epoch >= 2 {
            None
        } else if self.builtin {
            Some(added_z(cx, renames)?)
        } else {
            let lines = cx.facts.added_files().map_err(|f| facts_exc(f, false))?;
            Some(lines.iter().cloned().collect())
        };
        // Epochs 1-2 compute `lfs_files()` after `added_files()` (v3.2.0: `(added & files) - lfs`).
        let lfs = if epoch <= 2 { lfs_status(cx)? } else { lfs };
        Ok(unique
            .into_iter()
            .filter(|f| !lfs.contains(f.as_encoded_bytes()))
            .filter(|f| {
                added
                    .as_ref()
                    .is_none_or(|a| a.contains(f.as_encoded_bytes()))
            })
            .collect())
    }
}

impl NativeHook for CheckAddedLargeFiles {
    fn id(&self) -> &'static str {
        "check-added-large-files"
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

    /// The upstream variant's `added_files()`, unless `--enforce-all` (epoch >= 2; any argument argparse could take
    /// for it counts) skips it; the builtin reads the raw `-z` list itself, uncached.
    fn facts(&self, args: &[Arc<str>], epoch: Epoch) -> FactSet {
        let enforce_all = epoch.0 >= 2
            && args
                .iter()
                .any(|a| a.len() >= 3 && "--enforce-all".starts_with(&**a));
        if self.builtin || enforce_all {
            FactSet::NONE
        } else {
            FactSet::ADDED
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let epoch = if self.builtin { 3 } else { cx.epoch.0 };
        let spec = match (self.builtin, epoch) {
            (true, _) => &BUILTIN_SPEC,
            (false, 0 | 1) => &SPEC_E1,
            (false, 2) => &SPEC_E2,
            (false, _) => &SPEC_E3,
        };
        let ns = match common::parse(cx, spec, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let maxkb = ns.int("maxkb").unwrap_or(500);
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let selected = match self.selected(
            cx,
            &files,
            epoch,
            ns.flag("enforce_all"),
            ns.flag("include_renames"),
        ) {
            Ok(s) => s,
            Err(e) => return framework::crashed(out, &e),
        };
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let size = match std::fs::metadata(framework::at(cx, name)) {
                Ok(md) => md.len(),
                Err(e) => return FileStep::Crash(io_exc(&e, name)),
            };
            let kb = kernels::size_kb(size);
            if i128::from(kb) > i128::from(maxkb) {
                let mut line = framework::os_bytes(name).to_vec();
                line.extend_from_slice(format!(" ({kb} KB) exceeds {maxkb} KB.").as_bytes());
                out.print_bytes_text(&line);
                FileStep::Failed
            } else {
                FileStep::Unchanged
            }
        };
        let mut outcome = framework::for_each_file(cx, &selected, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
