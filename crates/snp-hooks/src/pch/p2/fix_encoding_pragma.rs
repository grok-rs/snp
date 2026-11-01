//! The real `fix-encoding-pragma` of v3.0.0…v5.0.0 (`fix_encoding_pragma.py`; 09 §4.H). From v6.0.0 the id is the
//! removed-hook stub (`pch::stubs`), which forwards epoch 1 here (epoch 2 is ambiguous in the data and delegated
//! there).
//!
//! Epochs: 1 = v3.0.0…v4.5.0, 2 = v4.6.0…v5.0.0 (a deprecation warning on stderr before anything else). The parser
//! is `ArgumentParser('Fixes the encoding pragma of python files')`: that string is its `prog`. The fixer is byte
//! level (`r+b`): the native reads first and writes only when the file changes (D-CRASH-23).
//!
//! Owner: WP-27.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_argparse::{ActionSpec, Conv, ParserSpec};
use snp_hooks_data::Epoch;

use super::kernels;
use crate::framework::{self, FileStep, PyOut, files_positional, store, store_true, with_help};
use crate::pch::p0::common::{epoch_in, filenames, print_name, rewrite};
use crate::pch::p1::util::{bytes_of, os_from, read_for_update};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "fix-encoding-pragma";

/// The parser's `prog` (its first positional argument).
const PARSER_PROG: &str = "Fixes the encoding pragma of python files";

/// v4.6.0+'s first line (stderr).
const DEPRECATION: &str = "warning: this hook is deprecated and will be removed in a future release because py2 \
                           is EOL. instead, use https://github.com/asottile/pyupgrade";

/// `type=_normalize_pragma`: `pragma.encode().rstrip()`; an argument that cannot be encoded (a surrogate-escaped
/// byte) is the `UnicodeEncodeError`, a `ValueError` argparse reports as `invalid _normalize_pragma value`.
fn normalize(s: &OsStr) -> Result<OsString, String> {
    let b = bytes_of(s);
    if core::str::from_utf8(b).is_err() {
        return Err(String::new());
    }
    Ok(os_from(kernels::normalize_pragma(b)))
}

static ACTIONS: [ActionSpec; 3] = [
    files_positional(Some("Filenames to fix")),
    with_help(
        ActionSpec {
            conv: Conv::Custom(normalize, "_normalize_pragma"),
            ..store(&["--pragma"], "pragma")
        },
        Some("The encoding pragma to use.  Default: # -*- coding: utf-8 -*-"),
    ),
    with_help(
        store_true(&["--remove"], "remove"),
        Some("Remove the encoding pragma (Useful in a python3-only codebase)"),
    ),
];

/// `fix_encoding_pragma.main`'s parser.
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
pub(crate) struct FixEncodingPragma;

/// The pre-commit-hooks native (epochs 1 and 2).
pub(crate) static UPSTREAM: FixEncodingPragma = FixEncodingPragma;

impl NativeHook for FixEncodingPragma {
    fn id(&self) -> &'static str {
        "fix-encoding-pragma"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 2)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let mut out = PyOut::default();
        if cx.epoch.0 >= 2 {
            out.eprint(DEPRECATION);
        }
        let ns = match framework::parse_args(&SPEC, PARSER_PROG, argv, cx.columns, cx.env) {
            Ok(ns) => ns,
            Err(mut o) => {
                let mut output = out.finish();
                output.append(&mut o.output);
                o.output = output;
                return o;
            }
        };
        let remove = ns.flag("remove");
        let pragma: Vec<u8> = ns.value("pragma").map_or_else(
            || kernels::DEFAULT_PRAGMA.to_vec(),
            |p| bytes_of(p).to_vec(),
        );
        let shown = String::from_utf8_lossy(&pragma).into_owned();
        let files = filenames(&ns);
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let content = match read_for_update(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let Some(new) = kernels::fix_encoding_pragma(&content, remove, &pragma) else {
                return FileStep::Unchanged;
            };
            if new != content
                && let Err(e) = rewrite(cx, name, &new)
            {
                return FileStep::Crash(e);
            }
            if remove {
                print_name(out, b"Removed encoding pragma from ", name, b"");
            } else {
                print_name(out, format!("Added `{shown}` to ").as_bytes(), name, b"");
            }
            if new == content {
                return FileStep::Failed;
            }
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        framework::finish(outcome, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragma_normalisation() {
        assert_eq!(
            normalize(OsStr::new("# x  \t")).unwrap(),
            OsString::from("# x")
        );
    }
}
