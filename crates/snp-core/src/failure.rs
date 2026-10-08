//! The failure envelope and Python-style exception texts (13 X.1–X.2, X.6; ARCHITECTURE §10).

use std::borrow::Cow;
use std::ffi::OsStr;

use crate::cancel::CancelReason;

/// Which line-1 header an exception gets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExcKind {
    /// `FatalError` and subclasses: `An error has occurred: …`, exit 1.
    Fatal,
    /// Anything else: `An unexpected error has occurred: …`, exit 3.
    Unexpected,
}

/// A Python-style exception as pre-commit's `error_handler` prints it (13 X.1).
#[derive(Debug, Clone)]
pub struct PyExc {
    /// The unqualified class: `CalledProcessError`, `FileNotFoundError`, `FatalError`, …
    pub ty: Cow<'static, str>,
    /// `bytes(exc)` / `str(exc).encode()`; may span lines.
    pub text: Vec<u8>,
    /// A K+ recovery line, printed on stderr after line 2 (13 X.3, ADR-045).
    pub hint: Option<String>,
    /// Crash log only: the error source chain.
    pub chain: Vec<String>,
    /// Crash log only: the Rust backtrace of a `PanicException`.
    pub backtrace: Option<String>,
}

impl PyExc {
    /// An exception of class `ty` with text `text`.
    pub fn new(ty: impl Into<Cow<'static, str>>, text: impl Into<Vec<u8>>) -> Self {
        Self {
            ty: ty.into(),
            text: text.into(),
            hint: None,
            chain: Vec::new(),
            backtrace: None,
        }
    }

    /// Adds the K+ recovery hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// A caught panic: class `PanicException`, the payload as text, the location in the chain.
    pub fn panic(payload: &str, location: Option<&str>, backtrace: Option<String>) -> Self {
        let mut exc = Self::new("PanicException", payload);
        if let Some(loc) = location {
            exc.chain.push(format!("panicked at {loc}"));
        }
        exc.backtrace = backtrace;
        exc
    }
}

/// Every way a command can end other than success.
#[derive(Debug)]
#[non_exhaustive]
pub enum Failure {
    /// Line 1 `An error has occurred: {ty}: {text}` (Fatal, exit 1) or
    /// `An unexpected error has occurred: {ty}: {text}` (Unexpected, exit 3); crash log.
    Exc {
        /// Header and exit code.
        kind: ExcKind,
        /// The exception.
        exc: PyExc,
    },
    /// Interrupt: banner + exit 130; Terminate/Hangup: no banner, die by the signal (143/129).
    Interrupted(CancelReason),
    /// stdout closed (EPIPE): exit 120, silent (X13-EH-03).
    StdoutClosed,
    /// `raise SystemExit(msg)`: `stderr` (text mode) and the exit code.
    SystemExit {
        /// The exit status.
        code: i32,
        /// What Python writes to stderr.
        stderr: Vec<u8>,
    },
    /// An argparse usage error: `stderr`, exit 2.
    Usage {
        /// The usage + error text.
        stderr: String,
    },
    /// A silent exit with this code (hazmat child status, hook-impl `legacy | run`).
    Exit(i32),
}

impl Failure {
    /// `FatalError`-family exception (exit 1).
    pub fn fatal(ty: &'static str, text: impl Into<Vec<u8>>) -> Self {
        Self::Exc {
            kind: ExcKind::Fatal,
            exc: PyExc::new(ty, text),
        }
    }

    /// Any other exception (exit 3).
    pub fn unexpected(ty: impl Into<Cow<'static, str>>, text: impl Into<Vec<u8>>) -> Self {
        Self::Exc {
            kind: ExcKind::Unexpected,
            exc: PyExc::new(ty, text),
        }
    }

    /// An `OSError`-family exception for an I/O error on `f1` (and `f2`), exit 3 (13 X.2.1).
    pub fn from_io(e: &std::io::Error, f1: Option<&OsStr>, f2: Option<&OsStr>) -> Self {
        Self::unexpected(pyerr::os_error_class(e), pyerr::os_error_text(e, f1, f2))
    }

    /// The process exit status (13 X.6): 1 / 3 / 130 / 143 / 129 / 120 / code / 2 / n.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Exc {
                kind: ExcKind::Fatal,
                ..
            } => 1,
            Self::Exc {
                kind: ExcKind::Unexpected,
                ..
            }
            | Self::Interrupted(CancelReason::Panic) => 3,
            Self::Interrupted(CancelReason::Interrupt) => 130,
            Self::Interrupted(CancelReason::Terminate) => 143,
            Self::Interrupted(CancelReason::Hangup) => 129,
            Self::Interrupted(CancelReason::StdoutClosed) | Self::StdoutClosed => 120,
            Self::SystemExit { code, .. } => *code,
            Self::Usage { .. } => 2,
            Self::Exit(n) => *n,
        }
    }

    /// Whether the envelope writes the crash log (exceptions only).
    pub const fn writes_crash_log(&self) -> bool {
        matches!(self, Self::Exc { .. })
    }
}

/// `Result` with a [`Failure`].
pub type SnpResult<T> = Result<T, Failure>;

/// Python exception texts used in failure envelopes (13 X.2).
pub mod pyerr {
    use std::ffi::{OsStr, OsString};
    use std::io::{self, ErrorKind};

    /// The Python `OSError` subclass for an I/O error (CPython's errno map, 13 X.2.1):
    /// `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, … or `OSError`.
    pub fn os_error_class(e: &io::Error) -> &'static str {
        match e.kind() {
            ErrorKind::NotFound => return "FileNotFoundError",
            ErrorKind::PermissionDenied => return "PermissionError",
            ErrorKind::AlreadyExists => return "FileExistsError",
            ErrorKind::IsADirectory => return "IsADirectoryError",
            ErrorKind::NotADirectory => return "NotADirectoryError",
            ErrorKind::BrokenPipe => return "BrokenPipeError",
            ErrorKind::ConnectionRefused => return "ConnectionRefusedError",
            ErrorKind::ConnectionReset => return "ConnectionResetError",
            ErrorKind::ConnectionAborted => return "ConnectionAbortedError",
            ErrorKind::TimedOut => return "TimeoutError",
            ErrorKind::WouldBlock => return "BlockingIOError",
            ErrorKind::Interrupted => return "InterruptedError",
            _ => {}
        }
        match e.raw_os_error() {
            Some(code) if cfg!(unix) => errno_class(code),
            _ => "OSError",
        }
    }

    /// The errno classes `io::ErrorKind` does not distinguish (ESRCH, ECHILD, EALREADY,
    /// EINPROGRESS, ESHUTDOWN); numbers per platform.
    const fn errno_class(code: i32) -> &'static str {
        const ESRCH: i32 = 3;
        const ECHILD: i32 = 10;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const MORE: [(i32, &str); 3] = [
            (114, "BlockingIOError"),
            (115, "BlockingIOError"),
            (108, "BrokenPipeError"),
        ];
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const MORE: [(i32, &str); 3] = [
            (37, "BlockingIOError"),
            (36, "BlockingIOError"),
            (58, "BrokenPipeError"),
        ];
        match code {
            ESRCH => "ProcessLookupError",
            ECHILD => "ChildProcessError",
            c if c == MORE[0].0 => MORE[0].1,
            c if c == MORE[1].0 => MORE[1].1,
            c if c == MORE[2].0 => MORE[2].1,
            _ => "OSError",
        }
    }

    /// `str(OSError)`: `[Errno 2] No such file or directory: 'x'` (POSIX) or
    /// `[WinError N] …` (Windows), with ` -> 'f2'` for two filenames (13 X.2.1).
    pub fn os_error_text(e: &io::Error, f1: Option<&OsStr>, f2: Option<&OsStr>) -> String {
        snp_pycompat::repr::oserror_from_io(e, f1, f2)
    }

    /// `repr(tuple(argv))`, each element as Python holds it (`surrogateescape` on POSIX).
    fn repr_argv(argv: &[OsString]) -> String {
        let mut out = String::from("(");
        for (i, a) in argv.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&snp_pycompat::repr::repr_os(a));
        }
        if argv.len() == 1 {
            out.push(',');
        }
        out.push(')');
        out
    }

    /// `_indent_or_none` of `CalledProcessError.__bytes__`.
    fn indent_or_none(out: &mut Vec<u8>, part: Option<&[u8]>) {
        match part {
            Some(p) if !p.is_empty() => {
                let mut ind = b"\n    ".to_vec();
                for &b in p {
                    ind.push(b);
                    if b == b'\n' {
                        ind.extend_from_slice(b"    ");
                    }
                }
                out.extend_from_slice(snp_pycompat::text::bytes_rstrip(&ind));
            }
            _ => out.extend_from_slice(b" (none)"),
        }
    }

    /// `bytes(pre_commit.util.CalledProcessError(rc, argv, stdout, stderr))` (format A,
    /// `util.py:49-78`, 13 X.2.2):
    /// `command: (…)\nreturn code: N\nstdout:<IND>\nstderr:<IND>`.
    pub fn called_process_a(
        argv: &[OsString],
        rc: i64,
        stdout: Option<&[u8]>,
        stderr: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut out = format!("command: {}\nreturn code: {rc}\n", repr_argv(argv)).into_bytes();
        out.extend_from_slice(b"stdout:");
        indent_or_none(&mut out, stdout);
        out.extend_from_slice(b"\nstderr:");
        indent_or_none(&mut out, stderr);
        out
    }

    /// `str(subprocess.CalledProcessError(rc, argv))` (format B; `hook_impl.py` rev-list calls).
    pub fn called_process_b(argv: &[OsString], rc: i64) -> String {
        snp_pycompat::repr::stdlib_called_process_error_str(rc, &repr_argv(argv))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io;

    use super::*;

    fn argv(xs: &[&str]) -> Vec<OsString> {
        xs.iter().map(OsString::from).collect()
    }

    #[test]
    fn exit_code_table() {
        assert_eq!(Failure::fatal("FatalError", "x").exit_code(), 1);
        assert_eq!(Failure::unexpected("ValueError", "x").exit_code(), 3);
        assert_eq!(
            Failure::Interrupted(CancelReason::Interrupt).exit_code(),
            130
        );
        assert_eq!(
            Failure::Interrupted(CancelReason::Terminate).exit_code(),
            143
        );
        assert_eq!(Failure::Interrupted(CancelReason::Hangup).exit_code(), 129);
        assert_eq!(
            Failure::Interrupted(CancelReason::StdoutClosed).exit_code(),
            120
        );
        assert_eq!(Failure::Interrupted(CancelReason::Panic).exit_code(), 3);
        assert_eq!(Failure::StdoutClosed.exit_code(), 120);
        assert_eq!(
            Failure::SystemExit {
                code: 1,
                stderr: Vec::new()
            }
            .exit_code(),
            1
        );
        assert_eq!(
            Failure::Usage {
                stderr: String::new()
            }
            .exit_code(),
            2
        );
        assert_eq!(Failure::Exit(7).exit_code(), 7);
        assert!(Failure::fatal("FatalError", "x").writes_crash_log());
        assert!(Failure::unexpected("OSError", "x").writes_crash_log());
        assert!(!Failure::StdoutClosed.writes_crash_log() && !Failure::Exit(1).writes_crash_log());
    }

    #[test]
    fn exceptions() {
        let e = PyExc::new("FatalError", "boom").with_hint("try this");
        assert_eq!(
            (e.ty.as_ref(), e.text.as_slice(), e.hint.as_deref()),
            ("FatalError", &b"boom"[..], Some("try this"))
        );
        let p = PyExc::panic(
            "index out of bounds",
            Some("src/x.rs:1:2"),
            Some("bt".into()),
        );
        assert_eq!(p.ty, "PanicException");
        assert_eq!(p.text, b"index out of bounds");
        assert_eq!(p.chain, vec!["panicked at src/x.rs:1:2".to_owned()]);
        assert_eq!(p.backtrace.as_deref(), Some("bt"));
    }

    // T10-UTL-01: util_test.py:18-37
    #[test]
    fn t10_utl_01_called_process_error_str() {
        assert_eq!(
            pyerr::called_process_a(&argv(&["exe"]), 1, Some(b"output\n"), Some(b"errors\n")),
            b"command: ('exe',)\nreturn code: 1\nstdout:\n    output\nstderr:\n    errors"
        );
        assert_eq!(
            pyerr::called_process_a(&argv(&["exe"]), 1, Some(b""), Some(b"")),
            b"command: ('exe',)\nreturn code: 1\nstdout: (none)\nstderr: (none)"
        );
    }

    #[test]
    fn called_process_formats() {
        // format A (13 X.2.2): inner blank lines become four spaces, trailing newline stripped
        assert_eq!(
            pyerr::called_process_a(&argv(&["/usr/bin/git", "write-tree"]), 128, Some(b"a\n\nb\n"), None),
            b"command: ('/usr/bin/git', 'write-tree')\nreturn code: 128\nstdout:\n    a\n    \n    b\nstderr: (none)"
        );
        assert_eq!(
            pyerr::called_process_a(&argv(&["git"]), -9, None, Some(b"fatal: x")),
            b"command: ('git',)\nreturn code: -9\nstdout: (none)\nstderr:\n    fatal: x"
        );
        // format B (stdlib)
        assert_eq!(
            pyerr::called_process_b(&argv(&["git", "rev-parse", "x^"]), 1),
            "Command '('git', 'rev-parse', 'x^')' returned non-zero exit status 1."
        );
        if cfg!(target_os = "linux") {
            assert_eq!(
                pyerr::called_process_b(&argv(&["git", "x"]), -9),
                "Command '('git', 'x')' died with <Signals.SIGKILL: 9>."
            );
        }
    }

    #[test]
    fn os_errors() {
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(pyerr::os_error_class(&nf), "FileNotFoundError");
        assert_eq!(pyerr::os_error_class(&io::Error::other("x")), "OSError");
        #[cfg(unix)]
        {
            let e = io::Error::from_raw_os_error(2);
            assert_eq!(
                pyerr::os_error_text(&e, Some(OsStr::new("./x.sh")), None),
                "[Errno 2] No such file or directory: './x.sh'"
            );
            let f = Failure::from_io(
                &io::Error::from_raw_os_error(13),
                Some(OsStr::new("a")),
                Some(OsStr::new("b")),
            );
            match f {
                Failure::Exc {
                    kind: ExcKind::Unexpected,
                    exc,
                } => {
                    assert_eq!(exc.ty, "PermissionError");
                    assert_eq!(exc.text, b"[Errno 13] Permission denied: 'a' -> 'b'");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(
                pyerr::os_error_class(&io::Error::from_raw_os_error(3)),
                "ProcessLookupError"
            );
            assert_eq!(
                pyerr::os_error_class(&io::Error::from_raw_os_error(10)),
                "ChildProcessError"
            );
            assert_eq!(
                pyerr::os_error_class(&io::Error::from_raw_os_error(21)),
                "IsADirectoryError"
            );
            assert_eq!(
                pyerr::os_error_class(&io::Error::from_raw_os_error(28)),
                "OSError"
            );
        }
    }
}
