//! `check-executables-have-shebangs` (`check_executables_have_shebangs.py`; 09 §4.E).
//!
//! Epochs: 1 = v3.0.0…v3.0.1 (shebang check only), 2 = v3.1.0 (win32: `git ls-files --stage` without `-z`),
//! 3 = v3.2.0…v4.0.1 (win32: `-z`), 4 = v4.1.0…v4.2.0 (message gains the `git add --chmod=-x` line), 5 = v4.3.0+
//! (git path when `git config core.fileMode` prints exactly `false`). The messages go to stderr. On the git path
//! the paths are pathspecs (glob magic applies) and an unmerged path is reported once per stage (bug-compat).
//! Builtin variant: `:(literal)` pathspecs and one message per path.
//!
//! Owner: WP-26.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;

use super::common::{self, cmd_output, epoch_in, filenames, read_head};
use crate::framework::{self, FileStep, PyOut, files_positional};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-executables-have-shebangs";

static ACTIONS: [ActionSpec; 1] = [files_positional(None)];

/// `check_executables_have_shebangs.main`'s parser (`description=__doc__`).
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: Some("Check that executable text files have a shebang."),
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The native.
#[derive(Debug)]
pub(crate) struct CheckExecutablesHaveShebangs {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckExecutablesHaveShebangs =
    CheckExecutablesHaveShebangs { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: CheckExecutablesHaveShebangs =
    CheckExecutablesHaveShebangs { builtin: true };

/// `has_shebang(path)`: the first two bytes are `#!`.
fn has_shebang(cx: &HookCx<'_>, path: &OsStr) -> Result<bool, PyExc> {
    Ok(read_head(cx, path, 2)? == b"#!")
}

/// `_message(path)` (stderr).
fn message(out: &mut PyOut, path: &[u8], epoch: u16) {
    out.eprint_bytes(&super::kernels::shebang_message(path, epoch));
}

/// `OsStr` of raw bytes (a git output path).
#[cfg(unix)]
fn os_from(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt as _;
    OsStr::from_bytes(b).to_os_string()
}

/// `OsStr` of raw bytes (a git output path; git writes UTF-8 on Windows).
#[cfg(not(unix))]
fn os_from(b: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(b).into_owned())
}

/// One `git ls-files --stage` entry: `(mode, path)`.
type Entry = (String, String);

/// `git_ls_files(paths)` / `_check_git_filemode`'s listing: `-z` (epoch ≥ 3) or `splitlines` (epoch 2), every
/// record split on its single tab (`ValueError` otherwise, as Python's tuple unpacking).
fn ls_files(
    cx: &HookCx<'_>,
    paths: &[&OsStr],
    epoch: u16,
    literal: bool,
) -> Result<Vec<Entry>, PyExc> {
    let mut args: Vec<OsString> = vec!["ls-files".into()];
    if epoch >= 3 {
        args.push("-z".into());
    }
    args.push("--stage".into());
    args.push("--".into());
    for p in paths {
        if literal {
            let mut s = OsString::from(":(literal)");
            s.push(p);
            args.push(s);
        } else {
            args.push(p.to_os_string());
        }
    }
    let argv: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let outs = cmd_output(cx, &argv, Some(0))?;
    let records: Vec<&str> = if epoch >= 3 {
        let trimmed = outs.trim_matches('\0');
        if trimmed.is_empty() {
            Vec::new()
        } else {
            trimmed.split('\0').collect()
        }
    } else {
        snp_pycompat::text::splitlines(&outs).collect()
    };
    let mut entries = Vec::new();
    for rec in records {
        let mut parts = rec.split('\t');
        let (Some(meta), Some(path), None) = (parts.next(), parts.next(), parts.next()) else {
            let n = rec.split('\t').count();
            return Err(PyExc::new(
                "ValueError",
                if n > 2 {
                    "too many values to unpack (expected 2)".to_owned()
                } else {
                    format!("not enough values to unpack (expected 2, got {n})")
                },
            ));
        };
        let mode = meta.split(' ').next().unwrap_or_default().to_owned();
        entries.push((mode, path.to_owned()));
    }
    Ok(entries)
}

impl CheckExecutablesHaveShebangs {
    /// Whether the git-index path applies (`core.fileMode` epoch 5, `sys.platform == 'win32'` epochs 2-4).
    fn git_path(cx: &HookCx<'_>, epoch: u16) -> Result<bool, PyExc> {
        match epoch {
            0 | 1 => Ok(false),
            2..=4 => Ok(cx.windows),
            _ => {
                let v = cmd_output(
                    cx,
                    &[OsStr::new("config"), OsStr::new("core.fileMode")],
                    None,
                )?;
                Ok(snp_pycompat::text::strip(&v) == "false")
            }
        }
    }
}

impl NativeHook for CheckExecutablesHaveShebangs {
    fn id(&self) -> &'static str {
        "check-executables-have-shebangs"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        if self.builtin {
            Ok(())
        } else {
            epoch_in(epoch, 5)
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let epoch = if self.builtin { 5 } else { cx.epoch.0 };
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let git_path = match Self::git_path(cx, epoch) {
            Ok(g) => g,
            Err(e) => return framework::crashed(out, &e),
        };
        let mut outcome = if git_path {
            let entries = match ls_files(cx, &files, epoch, self.builtin) {
                Ok(e) => e,
                Err(e) => return framework::crashed(out, &e),
            };
            let mut seen: HashSet<String> = HashSet::new();
            let names: Vec<OsString> = entries.iter().map(|(_, p)| os_from(p.as_bytes())).collect();
            let refs: Vec<&OsStr> = names.iter().map(OsString::as_os_str).collect();
            let mut i = 0_usize;
            let builtin = self.builtin;
            let mut step = |name: &OsStr, out: &mut PyOut| {
                let Some((mode, path)) = entries.get(i) else {
                    return FileStep::Unchanged;
                };
                i = i.saturating_add(1);
                let tail = mode.get(mode.len().saturating_sub(3)..).unwrap_or(mode);
                let executable = tail.chars().any(|c| matches!(c, '1' | '3' | '5' | '7'));
                if !executable {
                    return FileStep::Unchanged;
                }
                match has_shebang(cx, name) {
                    Ok(true) => FileStep::Unchanged,
                    Ok(false) => {
                        if !builtin || !seen.contains(path) {
                            message(out, path.as_bytes(), epoch);
                        }
                        seen.insert(path.clone());
                        FileStep::Failed
                    }
                    Err(e) => FileStep::Crash(e),
                }
            };
            framework::for_each_file(cx, &refs, &mut out, &mut step)
        } else {
            let mut step = |name: &OsStr, out: &mut PyOut| match has_shebang(cx, name) {
                Ok(true) => FileStep::Unchanged,
                Ok(false) => {
                    message(out, framework::os_bytes(name), epoch);
                    FileStep::Failed
                }
                Err(e) => FileStep::Crash(e),
            };
            framework::for_each_file(cx, &files, &mut out, &mut step)
        };
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
