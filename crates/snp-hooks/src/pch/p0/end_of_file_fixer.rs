//! `end-of-file-fixer` (`end_of_file_fixer.py`; 09 §4.A).
//!
//! One epoch v3.0.0…v6.0.0. Only the tail of a file is read (4 KiB blocks backwards until a byte that is not
//! `\r`/`\n`, linear time, constant memory); the file is opened for writing only when it changes, so a read-only file that needs no change
//! passes where Python's `open(filename, 'rb+')` crashes (D-CRASH-23). Builtin variant: `--check`.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write as _};
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
/// start of the file). Linear in the length of the trailing run and constant in memory: only the newest block is
/// scanned, and only the run's length and its first two bytes are kept ([`kernels::end_of_file_run`]).
fn action(f: &mut File, name: &OsStr) -> Result<EofAction, PyExc> {
    let err = |e: std::io::Error| io_exc(&e, name);
    let meta = f.metadata().map_err(err)?;
    if meta.is_dir() {
        return Err(io_exc(&std::io::Error::from_raw_os_error(eisdir()), name));
    }
    tail_action(f, meta.len()).map_err(err)
}

/// [`action`] over any seekable reader of `len` bytes.
fn tail_action<R: Read + Seek>(f: &mut R, len: u64) -> std::io::Result<EofAction> {
    let is_eol = |b: u8| b == b'\n' || b == b'\r';
    let mut block = vec![0_u8; usize::try_from(BLOCK.min(len)).unwrap_or(0)];
    // `head`: the first `min(run, 2)` bytes of the trailing run read so far.
    let mut head = [0_u8; 2];
    let mut head_len = 0_usize;
    let mut run = 0_u64;
    let mut from = len;
    while from > 0 {
        let start = from.saturating_sub(BLOCK);
        let n = usize::try_from(from.saturating_sub(start)).unwrap_or(0);
        let buf = block.get_mut(..n).unwrap_or_default();
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(buf)?;
        from = start;
        let body = buf.iter().rposition(|&b| !is_eol(b));
        let run_start = body.map_or(0, |i| i.saturating_add(1));
        let new = buf.get(run_start..).unwrap_or_default();
        run = run.saturating_add(u64::try_from(new.len()).unwrap_or(u64::MAX));
        // The bytes of this block precede the ones already seen: they become the run's first bytes.
        let mut merged = [0_u8; 2];
        let mut m = 0_usize;
        for &b in new
            .iter()
            .chain(head.get(..head_len).unwrap_or_default())
            .take(2)
        {
            if let Some(slot) = merged.get_mut(m) {
                *slot = b;
                m = m.saturating_add(1);
            }
        }
        head = merged;
        head_len = m;
        if body.is_some() {
            break;
        }
    }
    Ok(kernels::end_of_file_run(
        head.get(..head_len).unwrap_or_default(),
        run,
        len,
    ))
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{BLOCK, EofAction, kernels, tail_action};

    /// The block scan agrees with the whole-content kernel, including runs longer than a block and runs that
    /// straddle block boundaries.
    #[test]
    fn tail_scan_matches_kernel() {
        let block = usize::try_from(BLOCK).unwrap();
        let mut cases: Vec<Vec<u8>> = Vec::new();
        for body in [&b""[..], b"x", b"abc"] {
            for run in [0, 1, 2, 3, block - 1, block, block + 1, 3 * block + 2] {
                for unit in [&b"\n"[..], b"\r\n", b"\r", b"\n\r"] {
                    let mut v = body.to_vec();
                    while v.len() < body.len() + run {
                        v.extend_from_slice(unit);
                    }
                    cases.push(v);
                }
            }
        }
        for c in cases {
            let len = u64::try_from(c.len()).unwrap();
            let want = kernels::end_of_file(&c, len);
            let got = tail_action(&mut Cursor::new(&c), len).unwrap();
            assert_eq!(
                got,
                want,
                "{:?}",
                String::from_utf8_lossy(&c[..c.len().min(8)])
            );
        }
    }

    /// 64 MiB of `\n` is one linear pass (the old loop concatenated and rescanned the tail per block).
    #[test]
    fn long_eol_run_is_linear() {
        let c = vec![b'\n'; 64 << 20];
        let start = std::time::Instant::now();
        let got = tail_action(&mut Cursor::new(&c), u64::try_from(c.len()).unwrap()).unwrap();
        assert_eq!(got, EofAction::Truncate(0));
        assert!(start.elapsed() < std::time::Duration::from_secs(20));
        let mut c = c;
        c[0] = b'x';
        let got = tail_action(&mut Cursor::new(&c), u64::try_from(c.len()).unwrap()).unwrap();
        assert_eq!(got, EofAction::Truncate(2));
    }
}
