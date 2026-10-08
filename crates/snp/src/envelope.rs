//! The minimal W0 error envelope (§10.3, 13 X.1): line 1 and the exit code of every `Failure`; the crash log,
//! line 2 (`Check the log at …`), the dangling-segment continuation and signal re-raising are WP-33's.
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use snp_core::brand::Brand;
use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::{ExcKind, Failure};
use snp_core::output::{Printer, PrinterOpts};

use crate::{Exit, panic_hook};

/// `exit(code)`: 0–255 → `Exit::Code`; anything else (Windows hook-impl `legacy | run`, negative statuses) →
/// `Exit::Raw`, which `main` passes to `process::exit` after every destructor ran (14 A.7.4).
pub(crate) fn exit(code: i32) -> Exit {
    u8::try_from(code).map_or(Exit::Raw(code), Exit::Code)
}

/// A command returned `code`: flush in interpreter-exit order; a closed stdout fails closed with 120, never 0
/// (X13-EH-03).
pub(crate) fn success(code: i32, printer: &Printer) -> Exit {
    printer.finish();
    if printer.is_closed() {
        Exit::Code(120)
    } else {
        exit(code)
    }
}

/// `error_handler._log_and_exit` reduced to line 1 (byte layer, `output.write_line_b`) and the exit code of
/// 13 X.6; the hint goes to stderr. Infallible: the Printer never panics and never returns an error.
pub(crate) fn report(failure: &Failure, printer: &Printer) -> Exit {
    match failure {
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
            if let Some(hint) = &exc.hint {
                printer.py_eprint(hint);
            }
        }
        Failure::Interrupted(CancelReason::Interrupt) => {
            printer.write_line(b"Interrupted (^C): KeyboardInterrupt: ");
        }
        Failure::SystemExit { stderr, .. } => printer.write_stderr(stderr),
        Failure::Usage { stderr } => match stderr.strip_suffix('\n') {
            // argparse writes through the stderr text layer (CRLF in Windows text mode).
            Some(text) => printer.py_eprint(text),
            None => printer.write_stderr(stderr.as_bytes()),
        },
        // Silent: Terminate/Hangup (die by the signal after the restore, WP-33/WP-15), StdoutClosed, Exit(n).
        _ => {}
    }
    printer.finish();
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
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use snp_core::failure::PyExc;

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

    fn render(f: &Failure) -> (Exit, Vec<u8>, Vec<u8>) {
        let (out, err) = (Buf::default(), Buf::default());
        let p = Printer::new(
            Box::new(out.clone()),
            Box::new(err.clone()),
            PrinterOpts {
                brand: Brand::PreCommit,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: CancelToken::new(),
            },
        );
        let exit = report(f, &p);
        let out = out.0.lock().unwrap().clone();
        let err = err.0.lock().unwrap().clone();
        (exit, out, err)
    }

    #[test]
    fn line_one_and_exit_codes() {
        let (e, out, _) = render(&Failure::fatal("InvalidConfigError", "bad"));
        assert_eq!(
            (e, out.as_slice()),
            (
                Exit::Code(1),
                &b"An error has occurred: InvalidConfigError: bad\n"[..]
            )
        );
        let f = Failure::Exc {
            kind: ExcKind::Unexpected,
            exc: PyExc::new("CalledProcessError", "x").with_hint("try y"),
        };
        let (e, out, err) = render(&f);
        assert_eq!(e, Exit::Code(3));
        assert_eq!(
            out,
            b"An unexpected error has occurred: CalledProcessError: x\n"
        );
        assert_eq!(err, b"try y\n");
        let (e, out, _) = render(&Failure::Interrupted(CancelReason::Interrupt));
        assert_eq!(
            (e, out.as_slice()),
            (
                Exit::Code(130),
                &b"Interrupted (^C): KeyboardInterrupt: \n"[..]
            )
        );
        let (e, out, _) = render(&Failure::StdoutClosed);
        assert_eq!((e, out.len()), (Exit::Code(120), 0));
        let (e, _, err) = render(&Failure::Usage {
            stderr: "usage: x\nx: error: y\n".to_owned(),
        });
        assert_eq!(
            (e, err.as_slice()),
            (Exit::Code(2), &b"usage: x\nx: error: y\n"[..])
        );
        let (e, _, err) = render(&Failure::SystemExit {
            code: 1,
            stderr: b"msg\n".to_vec(),
        });
        assert_eq!((e, err.as_slice()), (Exit::Code(1), &b"msg\n"[..]));
        assert_eq!(render(&Failure::Exit(300)).0, Exit::Raw(300));
        assert_eq!(render(&Failure::Exit(-1)).0, Exit::Raw(-1));
    }
}
