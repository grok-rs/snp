//! Shared machinery every native uses (WP-26 milestone M1, frozen here so WP-27 codes against it; ADR-071).
//!
//! Owner: WP-26 (milestone M1).
#![expect(clippy::todo, reason = "W0 stub, removed by WP-26")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-26")]

use std::ffi::OsStr;

use snp_argparse::{Namespace, ParserSpec};
use snp_core::env::Env;
use snp_core::failure::PyExc;
use snp_core::path::RepoPath;

use crate::{HookCx, Outcome};

/// argparse emulation of a console script: prog = console-script name, CPython 3.13 texts, `allow_abbrev`.
/// `Err` = the complete outcome of `-h` (code 0, help on stdout) or of a usage error (code 2, usage + error text).
pub fn parse_args(
    spec: &'static ParserSpec,
    prog: &str,
    argv: &[&OsStr],
    columns: usize,
    env: &Env,
) -> Result<Namespace, Outcome> {
    todo!()
}

/// The console script's stdout/stderr when both go to one pipe: `print()` is block-buffered text (8192 bytes),
/// `sys.stdout.buffer.write` and stderr are written through; the merged bytes follow Python's flush order. Text
/// writes use CRLF on Windows where upstream writes in text mode (09 §2.7).
#[derive(Debug, Default)]
pub struct PyOut {
    /// W0 placeholder; WP-26 stores the pending text buffer and the merged output.
    _priv: (),
}

impl PyOut {
    /// `print(s)`.
    pub fn print(&mut self, s: &str) {
        todo!()
    }

    /// Text-layer write of bytes (`print` of an already-encoded line).
    pub fn print_bytes_text(&mut self, b: &[u8]) {
        todo!()
    }

    /// `sys.stdout.buffer.write(b)` (flushes pending text first).
    pub fn write_bytes(&mut self, b: &[u8]) {
        todo!()
    }

    /// `print(s, file=sys.stderr)`.
    pub fn eprint(&mut self, s: &str) {
        todo!()
    }

    /// The merged output at interpreter exit.
    pub fn finish(self) -> Vec<u8> {
        todo!()
    }
}

/// What one file step did.
#[derive(Debug)]
#[non_exhaustive]
pub enum FileStep {
    /// Nothing to report.
    Unchanged,
    /// The file was rewritten.
    Changed(RepoPath),
    /// rc 1, keep going.
    Failed,
    /// An uncaught Python exception: stops the partition.
    Crash(PyExc),
}

/// Iterates `files` in order; commits effects in order; the first `Crash` writes `crash_line` and stops the
/// partition (exit 1); `cx.cancel` is checked between files.
pub fn for_each_file(
    cx: &HookCx<'_>,
    files: &[&OsStr],
    out: &mut PyOut,
    f: &mut dyn FnMut(&OsStr, &mut PyOut) -> FileStep,
) -> Outcome {
    todo!()
}

/// Python's final traceback line `Type: text\n` (with the module-qualified name where Python prints one).
pub fn crash_line(exc: &PyExc) -> Vec<u8> {
    todo!()
}
