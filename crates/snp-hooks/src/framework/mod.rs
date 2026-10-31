//! Shared machinery every native uses (WP-26 milestone M1, frozen here so WP-27 codes against it; ADR-071).
//!
//! * [`parse_args`]: the console script's `argparse.ArgumentParser().parse_args(argv)` over the FULL argv a
//!   partition receives (`args` + files, ADR-072), CPython 3.13 texts.
//! * [`PyOut`]: the bytes the console script's merged stdout/stderr pipe carries, in CPython 3.13's flush order
//!   (09 §2.3, §2.7; verified on CPython 3.13.14, see the type's documentation).
//! * [`for_each_file`]: the per-file loop with Python's crash semantics (09 §2.6): effects are committed in input
//!   order and the first uncaught exception stops the partition with exit 1.
//! * [`crash_line`]: the last line of the traceback Python would print.
//!
//! Owner: WP-26 (milestone M1).

mod spec;

use std::ffi::OsStr;
use std::path::PathBuf;

use snp_argparse::{Namespace, ParseCfg, ParserSpec};
use snp_core::env::Env;
use snp_core::failure::{Failure, PyExc, pyerr};
use snp_core::path::RepoPath;

pub use spec::{
    append, files_positional, int_opt, positional, store, store_true, with_choices, with_default,
    with_help, with_metavar,
};

use crate::{HookCx, Outcome};

/// argparse emulation of a console script: prog = console-script name, CPython 3.13 texts, `allow_abbrev`.
/// `Err` = the complete outcome of `-h` (code 0, help on stdout) or of a usage error (code 2, usage + error text).
///
/// `parser.parse_args(sys.argv[1:])` of `argparse.py` 3.13 (`snp_argparse::parse`) with `prog` as the console
/// script's `os.path.basename(sys.argv[0])`. `argv` is the partition's full argv (hook `args` followed by the
/// file names), so a file named `-x.py` is an `unrecognized arguments` error and a file named like a flag sets it
/// (ADR-072). Text written by argparse is text-mode output: `\n` becomes `\r\n` on Windows (09 §2.7).
pub fn parse_args(
    spec: &'static ParserSpec,
    prog: &str,
    argv: &[&OsStr],
    columns: usize,
    env: &Env,
) -> Result<Namespace, Outcome> {
    let owned: Vec<std::ffi::OsString> = argv.iter().map(|a| a.to_os_string()).collect();
    let cfg = ParseCfg { prog, env, columns };
    match snp_argparse::parse(spec, &owned, &cfg) {
        snp_argparse::Outcome::Parsed(ns) => Ok(ns),
        snp_argparse::Outcome::Help(text) | snp_argparse::Outcome::Version(text) => Err(Outcome {
            code: 0,
            output: text_mode(text.as_bytes()),
            ..Outcome::default()
        }),
        snp_argparse::Outcome::Usage { stderr } => Err(Outcome {
            code: 2,
            output: text_mode(stderr.as_bytes()),
            ..Outcome::default()
        }),
        // `snp_argparse::Outcome` is non-exhaustive: anything new is reported as a usage failure.
        _ => Err(Outcome {
            code: 2,
            ..Outcome::default()
        }),
    }
}

/// `parser.error(message)` of a console script after a successful parse (`argparse.py` `ArgumentParser.error`):
/// the parser's usage block plus `{prog}: error: {message}\n`, for stderr (exit 2).
///
/// Used by hooks that validate their arguments after `parse_args` (trailing-whitespace's
/// `--markdown-linebreak-ext` checks). The text is in the form [`PyOut::eprint_bytes`] expects (no Windows
/// translation applied yet).
pub fn parser_error(
    spec: &'static ParserSpec,
    prog: &str,
    columns: usize,
    env: &Env,
    message: &str,
) -> Vec<u8> {
    let cfg = ParseCfg { prog, env, columns };
    let mut text = snp_argparse::format_usage(spec, &[], &cfg).into_bytes();
    text.extend_from_slice(prog.as_bytes());
    text.extend_from_slice(b": error: ");
    text.extend_from_slice(message.as_bytes());
    text.push(b'\n');
    text
}

/// `TextIOWrapper` newline translation of a text-mode write: `\n` → `os.linesep` (`\r\n` on Windows only), appended
/// to `dst` (no intermediate allocation on the hot `print` path).
fn push_text(dst: &mut Vec<u8>, b: &[u8]) {
    if cfg!(windows) {
        dst.reserve(b.len());
        for &c in b {
            if c == b'\n' {
                dst.push(b'\r');
            }
            dst.push(c);
        }
    } else {
        dst.extend_from_slice(b);
    }
}

/// [`push_text`] into a new buffer (argparse texts, once per parse).
fn text_mode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    push_text(&mut out, b);
    out
}

/// `TextIOWrapper._CHUNK_SIZE` (8192): pending text is pushed to the buffer once it reaches this size.
const TEXT_CHUNK: usize = 8192;

/// The `BufferedWriter` size of `sys.stdout` on a pipe: `st_blksize` of the pipe (4096 on Linux and macOS);
/// `io.DEFAULT_BUFFER_SIZE` (8192) on Windows, where pipes report no block size.
const BUFFER_SIZE: usize = if cfg!(windows) { 8192 } else { 4096 };

/// The console script's stdout/stderr when both go to one pipe: `print()` is block-buffered text (8192 bytes),
/// `sys.stdout.buffer.write` and stderr are written through; the merged bytes follow Python's flush order. Text
/// writes use CRLF on Windows where upstream writes in text mode (09 §2.7).
///
/// The model is CPython 3.13's `io` stack with `stdout` and `stderr` both on one pipe (pre-commit's
/// `stderr=STDOUT`), measured on CPython 3.13.14:
///
/// * `sys.stdout` is a `TextIOWrapper` (pending text, flushed into the buffer when the pending size reaches 8192
///   bytes) over a `BufferedWriter` of 4096 bytes (8192 on Windows) (a write that does not fit flushes the buffer first;
///   a write at least as large as the buffer goes straight to the pipe).
/// * `sys.stdout.buffer.write` goes into the `BufferedWriter` **ahead of** the pending text (pretty-format-json's
///   diffs come before its messages).
/// * `sys.stderr` is line-buffered: every `print(…, file=sys.stderr)` reaches the pipe at once, ahead of stdout
///   text that is still buffered.
/// * At exit stdout is flushed (pending text after the buffered bytes). A console script that dies of an uncaught
///   exception flushes stdout **before** its traceback (`_PyRun_SimpleFileObject` calls `flush_io()` before
///   `PyErr_Print()`); see [`PyOut::traceback`].
#[derive(Debug, Default)]
pub struct PyOut {
    /// `TextIOWrapper` pending bytes (already encoded and newline-translated).
    pending: Vec<u8>,
    /// `BufferedWriter` contents.
    buffer: Vec<u8>,
    /// Bytes that reached the pipe, in order.
    out: Vec<u8>,
}

impl PyOut {
    /// `print(s)`.
    pub fn print(&mut self, s: &str) {
        self.text_write(s.as_bytes());
        self.text_write(b"\n");
    }

    /// Text-layer write of bytes (`print` of an already-encoded line): `bytes` then `\n`, like [`PyOut::print`].
    ///
    /// For messages that embed file names as raw bytes (a Unix file name need not be UTF-8; 09 §9.2 item 1).
    pub fn print_bytes_text(&mut self, b: &[u8]) {
        self.text_write(b);
        self.text_write(b"\n");
    }

    /// `sys.stdout.write(s)` without a newline (text layer).
    pub fn write_text(&mut self, b: &[u8]) {
        self.text_write(b);
    }

    /// `sys.stdout.buffer.write(b)`: into the `BufferedWriter`, ahead of any pending text (CPython does not flush
    /// the text layer first).
    pub fn write_bytes(&mut self, b: &[u8]) {
        self.buffer_write(b);
    }

    /// `sys.stdout.flush()`: pending text, then the buffer, reach the pipe.
    pub fn flush(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        self.buffer_write(&pending);
        let buffered = std::mem::take(&mut self.buffer);
        self.out.extend_from_slice(&buffered);
    }

    /// `print(s, file=sys.stderr)`.
    pub fn eprint(&mut self, s: &str) {
        self.eprint_bytes(s.as_bytes());
        self.eprint_bytes(b"\n");
    }

    /// `sys.stderr.write(…)` of already-encoded text (line-buffered: it reaches the pipe at once).
    pub fn eprint_bytes(&mut self, b: &[u8]) {
        push_text(&mut self.out, b);
    }

    /// An uncaught exception ends a console script: stdout is flushed, then the traceback's last line
    /// ([`crash_line`]) is written to stderr. snp prints only that last line (09 §2.6).
    pub fn traceback(&mut self, exc: &PyExc) {
        self.flush();
        self.eprint_bytes(&crash_line(exc));
    }

    /// The merged output at interpreter exit.
    pub fn finish(mut self) -> Vec<u8> {
        self.flush();
        self.out
    }

    fn text_write(&mut self, b: &[u8]) {
        push_text(&mut self.pending, b);
        if self.pending.len() >= TEXT_CHUNK {
            let pending = std::mem::take(&mut self.pending);
            self.buffer_write(&pending);
        }
    }

    fn buffer_write(&mut self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        let avail = BUFFER_SIZE.saturating_sub(self.buffer.len());
        if b.len() <= avail && b.len() < BUFFER_SIZE {
            self.buffer.extend_from_slice(b);
            return;
        }
        let buffered = std::mem::take(&mut self.buffer);
        self.out.extend_from_slice(&buffered);
        if b.len() >= BUFFER_SIZE {
            self.out.extend_from_slice(b);
        } else {
            self.buffer.extend_from_slice(b);
        }
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
///
/// The returned [`Outcome`] carries the code (`1` once any step changed, failed or crashed), the modified paths in
/// order and `crashed`; its `output` is **empty**: `out` stays the single owner of the partition's output, so a
/// hook can print after the loop (detect-private-key's report), and finishes with [`finish`]. On cancellation the
/// loop stops before the next file (the runner reports `Interrupted`).
pub fn for_each_file(
    cx: &HookCx<'_>,
    files: &[&OsStr],
    out: &mut PyOut,
    f: &mut dyn FnMut(&OsStr, &mut PyOut) -> FileStep,
) -> Outcome {
    let mut outcome = Outcome::default();
    for file in files {
        if cx.cancel.is_cancelled() {
            break;
        }
        match f(file, out) {
            FileStep::Unchanged => {}
            FileStep::Changed(p) => {
                outcome.code = 1;
                outcome.modified.push(p);
            }
            FileStep::Failed => outcome.code = 1,
            FileStep::Crash(exc) => {
                out.traceback(&exc);
                outcome.code = 1;
                outcome.crashed = true;
                break;
            }
        }
    }
    outcome
}

/// `outcome` with `out`'s merged bytes as its output (the console script exiting).
pub fn finish(mut outcome: Outcome, out: PyOut) -> Outcome {
    outcome.output = out.finish();
    outcome
}

/// The class name as the last traceback line prints it: builtins bare, other classes module-qualified.
fn qualified(ty: &str) -> &str {
    match ty {
        "ExecutableNotFoundError" => "pre_commit.parse_shebang.ExecutableNotFoundError",
        "JSONDecodeError" => "json.decoder.JSONDecodeError",
        other => other,
    }
}

/// Python's final traceback line `Type: text\n` (with the module-qualified name where Python prints one).
///
/// `traceback.format_exception_only`: `{qualname}: {str(exc)}`, or the bare name when `str(exc)` is empty. Classes
/// defined outside `builtins` print with their module (`json.decoder.JSONDecodeError`,
/// `pre_commit.parse_shebang.ExecutableNotFoundError`, `pre_commit_hooks.util.CalledProcessError`); a `ty` that
/// already contains a `.` is used as is.
pub fn crash_line(exc: &PyExc) -> Vec<u8> {
    let mut line = qualified(&exc.ty).as_bytes().to_vec();
    if !exc.text.is_empty() {
        line.extend_from_slice(b": ");
        line.extend_from_slice(&exc.text);
    }
    line.push(b'\n');
    line
}

// ----- helpers shared by the natives (not part of the frozen surface) --------------------------------------------

/// The raw bytes of an argv string as Python's `str` would print them (Unix: the file name bytes; Windows: UTF-8).
pub fn os_bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// `os.path.join(cwd, name)` for a hook whose cwd is `cx.root` (absolute names are kept).
pub fn at(cx: &HookCx<'_>, name: &OsStr) -> PathBuf {
    cx.root.join(name)
}

/// `RepoPath` of an argv file name, when it is one (relative, non-empty, no NUL).
pub fn repo_path(name: &OsStr) -> Option<RepoPath> {
    RepoPath::new(os_bytes(name))
}

/// The `OSError` subclass Python raises for `e` on `name` (`open(name, …)`): `FileNotFoundError: [Errno 2] No such
/// file or directory: 'x'`.
pub fn io_exc(e: &std::io::Error, name: &OsStr) -> PyExc {
    PyExc::new(
        pyerr::os_error_class(e),
        pyerr::os_error_text(e, Some(name), None),
    )
}

/// The exception a [`Failure`] from a git helper stands for, as the hook's traceback would end.
pub fn failure_exc(f: Failure) -> PyExc {
    match f {
        Failure::Exc { exc, .. } => exc,
        Failure::SystemExit { stderr, .. } => PyExc::new("SystemExit", stderr),
        Failure::Usage { stderr } => PyExc::new("SystemExit", stderr.into_bytes()),
        Failure::Interrupted(_) => PyExc::new("KeyboardInterrupt", Vec::new()),
        Failure::StdoutClosed => PyExc::new("BrokenPipeError", "[Errno 32] Broken pipe"),
        Failure::Exit(code) => PyExc::new("SystemExit", code.to_string()),
        _ => PyExc::new("RuntimeError", Vec::new()),
    }
}

/// A regex compile error as CPython 3.13's traceback names it: `re.error` is `re.PatternError` since 3.13 (the
/// alias `re.error` remains), `OverflowError`, `ValueError` and `RecursionError` unchanged.
pub(crate) fn re_exc(e: &snp_pyre::PyReError) -> PyExc {
    let ty = match e.py_type() {
        "re.error" => "re.PatternError",
        other => other,
    };
    PyExc::new(ty, e.to_string())
}

/// D-SEC-10: a tier-2 match of `pattern` on `subject` (a path or branch name) exceeded snp-pyre's backtracking limit
/// (CPython would hang): `snp: regular expression {pattern!r} exceeded the backtracking limit on {subject!r}\n`.
pub(crate) fn backtrack_line(pattern: &str, subject: &OsStr) -> Vec<u8> {
    format!(
        "snp: regular expression {} exceeded the backtracking limit on {}\n",
        snp_pycompat::repr::repr_str(pattern),
        snp_pycompat::repr::repr_os(subject)
    )
    .into_bytes()
}

/// The `FutureWarning`s `re.compile(pattern)` emits (`Possible nested set at position 1`), in the D-MISC-04 form,
/// one line per warning: `[WARNING] regex {pattern!r}: {msg} (Python FutureWarning)\n`. Python writes them to
/// stderr, which the hook runner merges into the hook's output; the warning's source location cannot be
/// reproduced (it names the interpreter's files), so the line is snp's (Tier B, D-MISC-05).
pub(crate) fn regex_warnings(re: &snp_pyre::PyRegex) -> Vec<u8> {
    let mut out = Vec::new();
    for w in re.warnings() {
        out.extend_from_slice(
            format!(
                "[WARNING] regex {}: {} (Python FutureWarning)\n",
                snp_pycompat::repr::repr_str(re.source()),
                w.msg
            )
            .as_bytes(),
        );
    }
    out
}

/// `true` unless `SNP_PYTHON_WARNINGS=0` (D-MISC-04) silences emulated Python warnings.
pub(crate) fn python_warnings(env: &Env) -> bool {
    env.get("SNP_PYTHON_WARNINGS").is_none_or(|v| v != "0")
}

/// The outcome of a console script that died of `exc` before touching a file.
pub fn crashed(mut out: PyOut, exc: &PyExc) -> Outcome {
    out.traceback(exc);
    Outcome {
        code: 1,
        output: out.finish(),
        crashed: true,
        ..Outcome::default()
    }
}

#[cfg(test)]
mod tests;
