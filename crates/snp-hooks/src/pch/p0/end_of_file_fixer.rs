//! `end-of-file-fixer` (`end_of_file_fixer.py`; 09 §4.A).
//!
//! One epoch v3.0.0…v6.0.0. Only the tail of a file is read (4 KiB blocks backwards until a byte that is not
//! `\r`/`\n`); the file is opened for writing only when it changes, so a read-only file that needs no change
//! passes where Python's `open(filename, 'rb+')` crashes (D-CRASH-23). Builtin variant: `--check`.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;

use super::common::{self, epoch_in, filenames, print_name};
use super::kernels::{self, EofAction};
use crate::framework::{self, FileStep, PyOut, files_positional, io_exc, store_true, with_help};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "end-of-file-fixer";

static UPSTREAM_ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to fix"))];
static BUILTIN_ACTIONS: [ActionSpec; 2] = [
    files_positional(Some("Filenames to fix")),
    with_help(
        store_true(&["--check"], "check"),
        Some("Report the files that would be fixed without changing them."),
    ),
];

/// `end_of_file_fixer.main`'s parser.
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
pub(crate) struct EndOfFileFixer {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: EndOfFileFixer = EndOfFileFixer { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: EndOfFileFixer = EndOfFileFixer { builtin: true };

/// Tail block size.
const BLOCK: u64 = 4096;

/// The action for `name`: reads backwards from the end in [`BLOCK`]s until a byte that is not `\r`/`\n` (or the
/// start of the file) is in the tail.
fn action(f: &mut File, name: &OsStr) -> Result<EofAction, PyExc> {
    let err = |e: std::io::Error| io_exc(&e, name);
    let len = f.metadata().map_err(err)?.len();
    if f.metadata().map_err(err)?.is_dir() {
        return Err(io_exc(&std::io::Error::from_raw_os_error(eisdir()), name));
    }
    let mut tail: Vec<u8> = Vec::new();
    let mut from = len;
    loop {
        let start = from.saturating_sub(BLOCK);
        let n = usize::try_from(from.saturating_sub(start)).unwrap_or(0);
        let mut block = vec![0_u8; n];
        f.seek(SeekFrom::Start(start)).map_err(err)?;
        f.read_exact(&mut block).map_err(err)?;
        block.extend_from_slice(&tail);
        tail = block;
        from = start;
        let has_body = tail.iter().any(|&b| b != b'\n' && b != b'\r');
        if has_body || from == 0 {
            break;
        }
    }
    Ok(kernels::end_of_file(&tail, len))
}

/// `EISDIR`.
const fn eisdir() -> i32 {
    if cfg!(windows) { 13 } else { 21 }
}

/// Applies `act` in place (append `\n` or truncate).
fn apply(cx: &HookCx<'_>, name: &OsStr, act: EofAction) -> Result<(), PyExc> {
    let err = |e: std::io::Error| io_exc(&e, name);
    let path = framework::at(cx, name);
    match act {
        EofAction::Keep => Ok(()),
        EofAction::Append => {
            let mut f = OpenOptions::new().append(true).open(&path).map_err(err)?;
            f.write_all(b"\n").map_err(err)
        }
        EofAction::Truncate(n) => {
            let f = OpenOptions::new().write(true).open(&path).map_err(err)?;
            f.set_len(n).map_err(err)
        }
    }
}

impl NativeHook for EndOfFileFixer {
    fn id(&self) -> &'static str {
        "end-of-file-fixer"
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
        let check = self.builtin && ns.flag("check");
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let act = File::open(framework::at(cx, name))
                .map_err(|e| io_exc(&e, name))
                .and_then(|mut f| action(&mut f, name));
            let act = match act {
                Ok(EofAction::Keep) => return FileStep::Unchanged,
                Ok(a) => a,
                Err(e) => return FileStep::Crash(e),
            };
            if check {
                print_name(out, b"Would fix ", name, b"");
                return FileStep::Failed;
            }
            if let Err(e) = apply(cx, name, act) {
                return FileStep::Crash(e);
            }
            print_name(out, b"Fixing ", name, b"");
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = check;
        framework::finish(outcome, out)
    }
}
