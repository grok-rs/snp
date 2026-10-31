//! `forbid-new-submodules` (`forbid_new_submodules.py`; 09 §4.E).
//!
//! Epochs: 1 = v3.0.0…v4.0.1 (argv ignored entirely — no argparse, so not even `-h` —; `git diff --staged
//! --diff-filter=A --raw`), 2 = v4.1.0+ (`git diff --diff-filter=A --raw <--staged | FROM...TO> -- <files>`, the
//! ref range when both `PRE_COMMIT_FROM_REF` and `PRE_COMMIT_TO_REF` are in the hook's environment). The output is
//! git's non-`-z` `--raw` format, split with `str.splitlines()`: a quoted path is printed quoted. The footer is
//! printed once per partition.
//!
//! Owner: WP-27.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;

use super::kernels;
use crate::framework::{self, PyOut, files_positional};
use crate::pch::p0::common::{self, cmd_output, epoch_in, filenames};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "forbid-new-submodules";

static ACTIONS: [ActionSpec; 1] = [files_positional(None)];

/// `forbid_new_submodules.main`'s parser (v4.1.0+).
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
pub(crate) struct ForbidNewSubmodules;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: ForbidNewSubmodules = ForbidNewSubmodules;

/// The new submodules `git diff` reports, printing one line per submodule.
fn check(cx: &HookCx<'_>, files: Option<&[&OsStr]>, out: &mut PyOut) -> Result<bool, PyExc> {
    let range: OsString;
    let mut args: Vec<&OsStr> = vec![OsStr::new("diff")];
    match files {
        None => args.extend([
            OsStr::new("--staged"),
            OsStr::new("--diff-filter=A"),
            OsStr::new("--raw"),
        ]),
        Some(files) => {
            args.extend([OsStr::new("--diff-filter=A"), OsStr::new("--raw")]);
            match (
                cx.env.get("PRE_COMMIT_FROM_REF"),
                cx.env.get("PRE_COMMIT_TO_REF"),
            ) {
                (Some(from), Some(to)) => {
                    let mut r = from.to_os_string();
                    r.push("...");
                    r.push(to);
                    range = r;
                    args.push(&range);
                }
                _ => args.push(OsStr::new("--staged")),
            }
            args.push(OsStr::new("--"));
            args.extend_from_slice(files);
        }
    }
    let diff = cmd_output(cx, &args, Some(0))?;
    let mut found = false;
    for line in snp_pycompat::text::splitlines(&diff) {
        if let Some(path) = kernels::new_submodule(line)? {
            out.print(&format!("{path}: new submodule introduced"));
            found = true;
        }
    }
    Ok(found)
}

impl NativeHook for ForbidNewSubmodules {
    fn id(&self) -> &'static str {
        "forbid-new-submodules"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 2)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let mut out = PyOut::default();
        let found = if cx.epoch.0 <= 1 {
            check(cx, None, &mut out)
        } else {
            let ns = match common::parse(cx, &SPEC, PROG, argv) {
                Ok(ns) => ns,
                Err(o) => return o,
            };
            check(cx, Some(&filenames(&ns)), &mut out)
        };
        let found = match found {
            Ok(f) => f,
            Err(e) => return framework::crashed(out, &e),
        };
        if found {
            for l in kernels::SUBMODULE_FOOTER {
                out.print(l);
            }
        }
        Outcome {
            code: i32::from(found),
            output: out.finish(),
            reads_only: true,
            ..Outcome::default()
        }
    }
}
