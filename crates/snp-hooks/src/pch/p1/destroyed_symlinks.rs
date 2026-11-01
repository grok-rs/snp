//! `destroyed-symlinks` (`destroyed_symlinks.py`; 09 §4.E). One epoch v3.4.0…v6.0.0.
//!
//! `git status --porcelain=v2 -z -- <files>` (pathspecs, as in Python) lists the changed entries; a `1 ` record
//! whose HEAD mode is `120000` and whose index mode is a regular file is destroyed when the blob ids are equal, or
//! when the index blob is at most two bytes larger and both contents agree after `bytes.rstrip()` (`git cat-file
//! -s` / `-p`, the same calls Python makes). The report lists the paths and a `git reset` hint.
//!
//! Owner: WP-27.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_core::failure::{PyExc, pyerr};
use snp_hooks_data::Epoch;

use super::kernels::{self, SymlinkCandidate};
use crate::framework::{self, PyOut, files_positional};
use crate::pch::p0::common::{self, cmd_output, epoch_in, facts_exc, filenames};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "destroyed-symlinks";

static ACTIONS: [ActionSpec; 1] = [files_positional(Some("Filenames to check."))];

/// `destroyed_symlinks.main`'s parser.
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
pub(crate) struct DestroyedSymlinks;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: DestroyedSymlinks = DestroyedSymlinks;

/// `int(cmd_output('git', 'cat-file', '-s', sha).strip())`.
fn blob_size(cx: &HookCx<'_>, sha: &str) -> Result<i128, PyExc> {
    let out = cmd_output(
        cx,
        &[OsStr::new("cat-file"), OsStr::new("-s"), OsStr::new(sha)],
        Some(0),
    )?;
    let text = snp_pycompat::text::strip(&out);
    snp_pycompat::int::parse_int(text)
        .map_err(|_| PyExc::new("ValueError", snp_pycompat::repr::int_literal_error(text)))
}

/// `subprocess.check_output(('git', 'cat-file', '-p', sha))`: stdout bytes; git's stderr is inherited (it reaches
/// the hook output) and a non-zero exit is the stdlib `CalledProcessError`.
fn blob_content(cx: &HookCx<'_>, sha: &str, out: &mut PyOut) -> Result<Vec<u8>, PyExc> {
    let args = [OsStr::new("cat-file"), OsStr::new("-p"), OsStr::new(sha)];
    let c = cx.facts.raw(&args).map_err(|f| facts_exc(f, true))?;
    out.eprint_bytes(&c.stderr);
    if c.status.0 != 0 {
        let mut argv = vec![OsString::from("git")];
        argv.extend(args.iter().map(|a| a.to_os_string()));
        return Err(PyExc::new(
            "subprocess.CalledProcessError",
            pyerr::called_process_b(&argv, c.status.0),
        ));
    }
    Ok(c.stdout)
}

/// Whether `c` is a destroyed symlink.
fn destroyed(cx: &HookCx<'_>, c: &SymlinkCandidate, out: &mut PyOut) -> Result<bool, PyExc> {
    if c.hash_head == c.hash_index {
        return Ok(true);
    }
    let size_index = blob_size(cx, &c.hash_index)?;
    let size_head = blob_size(cx, &c.hash_head)?;
    if size_index > size_head.saturating_add(2) {
        return Ok(false);
    }
    let head = blob_content(cx, &c.hash_head, out)?;
    let index = blob_content(cx, &c.hash_index, out)?;
    Ok(kernels::same_after_rstrip(&head, &index))
}

/// `find_destroyed_symlinks(files)`.
fn find(cx: &HookCx<'_>, files: &[&OsStr], out: &mut PyOut) -> Result<Vec<String>, PyExc> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("status"),
        OsStr::new("--porcelain=v2"),
        OsStr::new("-z"),
        OsStr::new("--"),
    ];
    args.extend_from_slice(files);
    let status = cmd_output(cx, &args, Some(0))?;
    let (candidates, pending) = kernels::destroyed_symlink_candidates(&status);
    let mut found = Vec::new();
    for c in &candidates {
        if cx.cancel.is_cancelled() {
            break;
        }
        if destroyed(cx, c, out)? {
            found.push(c.path.clone());
        }
    }
    match pending {
        Some(e) => Err(e),
        None => Ok(found),
    }
}

impl NativeHook for DestroyedSymlinks {
    fn id(&self) -> &'static str {
        "destroyed-symlinks"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 1)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let files = filenames(&ns);
        let mut out = PyOut::default();
        let found = match find(cx, &files, &mut out) {
            Ok(f) => f,
            Err(e) => return framework::crashed(out, &e),
        };
        if !found.is_empty() {
            for line in kernels::destroyed_symlinks_report(&found) {
                out.print(&line);
            }
        }
        Outcome {
            code: i32::from(!found.is_empty()),
            output: out.finish(),
            reads_only: true,
            ..Outcome::default()
        }
    }
}
