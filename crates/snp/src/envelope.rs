//! The error envelope (§10.3, 13 X.1, ADR-013): line 1 on stdout (continuing a dangling `name....` segment, since
//! nothing is inserted before it), line 2 `Check the log at …` / `Failed to write to log at …` + the log body, the
//! K+ hint on stderr (ADR-045), and the exit code of 13 X.6 (`Exit::Raw` above 255).
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use std::io::Write;
use std::path::{Path, PathBuf};

use snp_core::brand::Brand;
use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::{ExcKind, Failure, PyExc};
use snp_core::output::{Printer, PrinterOpts};

use crate::crash_log::{self, LogInfo};
use crate::session::Session;
use crate::{Exit, panic_hook};

/// `exit(code)`: 0–255 → `Exit::Code`; anything else (Windows hook-impl `legacy | run`, negative statuses) →
/// `Exit::Raw`, which `main` passes to `process::exit` after every destructor ran (14 A.7.4).
pub(crate) fn exit(code: i32) -> Exit {
    u8::try_from(code).map_or(Exit::Raw(code), Exit::Code)
}

/// The process stdout as the `Printer` sees it: any write or flush error is reported as `BrokenPipe`, so the
/// Printer latches `closed` on ENOSPC (`> /dev/full`), EIO or EAGAIN exactly as on EPIPE and the invocation fails
/// closed with 120 instead of reporting success after its output was lost. CPython's shutdown flush of `sys.stdout`
/// fails the same way (`Exception ignored ... OSError: [Errno 28] No space left on device`, exit 120; pre-commit
/// 4.6.2, oracle); snp drops the noise as for EPIPE (D-CRASH-24).
#[derive(Debug)]
pub(crate) struct FailClosedStdout<W>(pub(crate) W);

/// `e` as the `BrokenPipe` the Printer latches on (the original error is dropped: it is never shown).
fn as_broken_pipe(e: std::io::Error) -> std::io::Error {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        e
    } else {
        std::io::Error::from(std::io::ErrorKind::BrokenPipe)
    }
}

impl<W: Write> Write for FailClosedStdout<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf).map_err(as_broken_pipe)
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf).map_err(as_broken_pipe)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush().map_err(as_broken_pipe)
    }
}

/// Whether stdout was closed under us (EPIPE): the whole invocation then ends silently with 120 (X13-EH-03).
fn stdout_closed(printer: &Printer, cancel: &CancelToken) -> bool {
    printer.is_closed() || cancel.reason() == Some(CancelReason::StdoutClosed)
}

/// A command returned `code`: flush in interpreter-exit order; a closed stdout fails closed with 120, never 0
/// (X13-EH-03).
pub(crate) fn success(code: i32, printer: &Printer, cancel: &CancelToken) -> Exit {
    printer.finish();
    if stdout_closed(printer, cancel) {
        Exit::Code(120)
    } else {
        exit(code)
    }
}

/// Where the crash log goes: `<snp store>/<pre-commit.log | snp.log>`; `None` when the store location itself
/// cannot be determined (M7 refusal, no home): then no log is written and line 2 is omitted.
fn log_path(s: &Session) -> Option<(PathBuf, PathBuf)> {
    let cwd = s.cwd.as_deref()?;
    let loc = snp_store::resolve_location(&s.env, cwd).ok()?;
    let file = loc.path.join(s.brand.crash_log_name());
    Some((loc.path, file))
}

/// Opens the log for writing (the store directory is created only now, 0700): `O_NOFOLLOW`, truncated.
fn open_log(store: &Path, file: &Path) -> std::io::Result<std::fs::File> {
    if !store.is_dir() {
        snp_os::fs::ensure_private_dir(store)?;
    }
    snp_os::fs::create_truncate_nofollow(file)
}

/// Line 2 and the log body (`error_handler._log_and_exit`, `error_handler.py:30-68`), with EH-01/EH-02 fixed: the
/// log is opened before line 2 is printed, and any failure is `Failed to write to log at …` + the body on stdout,
/// never a double fault.
fn write_log(s: &Session, line1: &[u8], exc: Option<&PyExc>) {
    let Some((store, file)) = log_path(s) else {
        return;
    };
    let info = LogInfo {
        brand: s.brand,
        git_version: s.git_version.get().map(String::as_str),
        executable: &s.self_exe,
    };
    let body = crash_log::body(&info, line1, exc);
    let shown = file.as_os_str().as_encoded_bytes();
    let written = open_log(&store, &file).and_then(|mut f| {
        f.write_all(&body)?;
        f.flush()
    });
    if written.is_ok() {
        let mut l2 = b"Check the log at ".to_vec();
        l2.extend_from_slice(shown);
        s.printer.write_line(&l2);
    } else {
        let mut l2 = b"Failed to write to log at ".to_vec();
        l2.extend_from_slice(shown);
        s.printer.write_line(&l2);
        s.printer.write(&body);
    }
}

/// `error_handler()` (`error_handler.py:71-81`): line 1 (byte layer, `output.write_line_b`), line 2 and the log,
/// the K+ hint on stderr, the exit code of 13 X.6. Infallible: the Printer never panics or returns an error.
pub(crate) fn report(failure: Failure, s: &Session) -> Exit {
    let printer = &s.printer;
    if stdout_closed(printer, &s.cancel) {
        printer.finish();
        return Exit::Code(120);
    }
    let failure = match failure {
        // A worker thread panicked and the main thread saw only the cancellation.
        Failure::Interrupted(CancelReason::Panic) => panic_hook::take_failure(),
        // EPIPE while the command was running.
        Failure::Interrupted(CancelReason::StdoutClosed) => Failure::StdoutClosed,
        other => other,
    };
    match &failure {
        Failure::Exc { kind, exc } => {
            let head: &[u8] = match kind {
                ExcKind::Fatal => b"An error has occurred: ",
                ExcKind::Unexpected => b"An unexpected error has occurred: ",
            };
            let mut line = head.to_vec();
            line.extend_from_slice(exc.ty.as_bytes());
            line.extend_from_slice(b": ");
            line.extend_from_slice(&exc.text);
            printer.write_line(&line);
            write_log(s, &line, Some(exc));
            if let Some(hint) = &exc.hint {
                printer.py_eprint(hint);
            }
        }
        Failure::Interrupted(CancelReason::Interrupt) => {
            let line = b"Interrupted (^C): KeyboardInterrupt: ";
            printer.write_line(line);
            write_log(s, line, None);
        }
        // `raise SystemExit(msg)`: the interpreter prints `str(msg)` through the stderr text layer (CRLF in Windows
        // text mode)
        Failure::SystemExit { stderr, .. } => match std::str::from_utf8(stderr)
            .ok()
            .and_then(|t| t.strip_suffix('\n'))
        {
            Some(text) => printer.py_eprint(text),
            None => printer.write_stderr(stderr),
        },
        Failure::Usage { stderr } => match stderr.strip_suffix('\n') {
            // argparse writes through the stderr text layer (CRLF in Windows text mode).
            Some(text) => printer.py_eprint(text),
            None => printer.write_stderr(stderr.as_bytes()),
        },
        // Silent: Terminate/Hangup (die by the signal after the restore), StdoutClosed, Exit(n).
        _ => {}
    }
    printer.finish();
    if stdout_closed(printer, &s.cancel) {
        return Exit::Code(120);
    }
    exit(failure.exit_code())
}

/// `snp __hazmat …` (path H): the tool's own exit status; a panic is reported on stderr (stdout is the hook's
/// output pipe) with exit 3.
pub(crate) fn hazmat_exit(outcome: Option<i32>, cancel: &CancelToken) -> Exit {
    if let Some(code) = outcome {
        return exit(code);
    }
    let printer = Printer::stdio(PrinterOpts {
        brand: Brand::Snp,
        color: false,
        stdout_is_tty: false,
        windows_text_mode: cfg!(windows),
        cancel: cancel.clone(),
    });
    let payload = panic_hook::take_payload().unwrap_or_default();
    printer.py_eprint(&format!(
        "An unexpected error has occurred: PanicException: {payload}"
    ));
    printer.finish();
    Exit::Code(3)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex, OnceLock};

    use snp_core::env::Env;

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            snp_core::sync::lock(&self.0).extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn session(store: &Path, out: &Buf, err: &Buf) -> Session {
        let cancel = CancelToken::new();
        let printer = Printer::new(
            Box::new(out.clone()),
            Box::new(err.clone()),
            PrinterOpts {
                brand: Brand::PreCommit,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: cancel.clone(),
            },
        );
        Session {
            brand: Brand::PreCommit,
            env: Env::from_pairs([("SNP_HOME", store.as_os_str())]),
            cwd: Some(store.parent().unwrap().to_path_buf()),
            arg0: None,
            printer,
            cancel,
            self_exe: PathBuf::from("/usr/bin/snp"),
            signals: OnceLock::new(),
            git_version: OnceLock::new(),
        }
    }

    /// A temp directory and its real path: the store location is realpath'd, as pre-commit's
    /// `os.path.realpath(PRE_COMMIT_HOME)` (macOS `/var` -> `/private/var`, Windows 8.3 short names).
    fn real_tempdir() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let real = snp_pycompat::ospath::realpath(d.path().as_os_str(), d.path()).unwrap();
        (d, real)
    }

    fn render(f: Failure) -> (Exit, String, String, PathBuf, tempfile::TempDir) {
        let (d, base) = real_tempdir();
        let store = base.join("store");
        let (out, err) = (Buf::default(), Buf::default());
        let s = session(&store, &out, &err);
        let exit = report(f, &s);
        let out = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let err = String::from_utf8(err.0.lock().unwrap().clone()).unwrap();
        (exit, out, err, store, d)
    }

    /// A writer whose every write fails with `kind`.
    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// ENOSPC (`> /dev/full`), EIO, EBADF: the output is lost, so the command fails closed with 120, never 0.
    #[test]
    fn any_stdout_write_error_fails_closed() {
        for kind in [
            io::ErrorKind::StorageFull,
            io::ErrorKind::Other,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::WouldBlock,
        ] {
            let cancel = CancelToken::new();
            let err = Buf::default();
            let printer = Printer::new(
                Box::new(FailClosedStdout(Failing(kind))),
                Box::new(err.clone()),
                PrinterOpts {
                    brand: Brand::PreCommit,
                    color: false,
                    stdout_is_tty: false,
                    windows_text_mode: false,
                    cancel: cancel.clone(),
                },
            );
            printer.py_print("x");
            assert_eq!(success(0, &printer, &cancel), Exit::Code(120), "{kind:?}");
            assert!(err.0.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn line_one_line_two_and_exit_codes() {
        let (e, out, _, store, _d) = render(Failure::fatal("InvalidConfigError", "bad"));
        assert_eq!(e, Exit::Code(1));
        let log = store.join("pre-commit.log");
        assert_eq!(
            out,
            format!(
                "An error has occurred: InvalidConfigError: bad\nCheck the log at {}\n",
                log.display()
            )
        );
        let body = std::fs::read_to_string(&log).unwrap();
        assert!(body.contains("An error has occurred: InvalidConfigError: bad\n"));

        let f = Failure::Exc {
            kind: ExcKind::Unexpected,
            exc: PyExc::new("CalledProcessError", "x").with_hint("try y"),
        };
        let (e, out, err, _, _d) = render(f);
        assert_eq!(e, Exit::Code(3));
        assert!(out.starts_with(
            "An unexpected error has occurred: CalledProcessError: x\nCheck the log at "
        ));
        assert_eq!(err, "try y\n");

        let (e, out, _, _, _d) = render(Failure::Interrupted(CancelReason::Interrupt));
        assert_eq!(e, Exit::Code(130));
        assert!(out.starts_with("Interrupted (^C): KeyboardInterrupt: \nCheck the log at "));

        let (e, out, _, _, _d) = render(Failure::StdoutClosed);
        assert_eq!((e, out.len()), (Exit::Code(120), 0));
        let (e, _, err, _, _d) = render(Failure::Usage {
            stderr: "usage: x\nx: error: y\n".to_owned(),
        });
        assert_eq!(
            (e, err.as_str()),
            (Exit::Code(2), "usage: x\nx: error: y\n")
        );
        let (e, _, err, _, _d) = render(Failure::SystemExit {
            code: 1,
            stderr: b"msg\n".to_vec(),
        });
        assert_eq!((e, err.as_str()), (Exit::Code(1), "msg\n"));
        assert_eq!(render(Failure::Exit(300)).0, Exit::Raw(300));
        assert_eq!(render(Failure::Exit(-1)).0, Exit::Raw(-1));
        assert_eq!(render(Failure::Interrupted(CancelReason::Terminate)).1, "");
    }

    /// X13-EH-02: a directory where the log goes → `Failed to write to log at …` + the body on stdout, exit 3.
    #[test]
    fn unwritable_log_prints_the_body() {
        let (_d, base) = real_tempdir();
        let store = base.join("store");
        std::fs::create_dir_all(store.join("pre-commit.log")).unwrap();
        let (out, err) = (Buf::default(), Buf::default());
        let s = session(&store, &out, &err);
        let e = report(Failure::unexpected("ValueError", "ohai"), &s);
        assert_eq!(e, Exit::Code(3));
        let out = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(out.starts_with(&format!(
            "An unexpected error has occurred: ValueError: ohai\nFailed to write to log at {}\n### version information\n",
            store.join("pre-commit.log").display()
        )));
        assert!(err.0.lock().unwrap().is_empty());
    }
}
