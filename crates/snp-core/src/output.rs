//! The only writer of user-facing stdout/stderr (ARCHITECTURE §3.9, ADR-028, ADR-029).
//!
//! pre-commit writes through two CPython layers: `output.write*` and the logging handler write
//! bytes to `sys.stdout.buffer` and flush; `print()` goes through the text layer
//! (`TextIOWrapper`), which keeps up to 8192 pending bytes when stdout is not a TTY (line
//! buffering on a TTY). Piped output therefore reorders (01 §8.2), and [`Printer`] reproduces it
//! byte for byte: [`Printer::py_print`] text is held in a pending buffer, moved into the byte layer
//! only when the text layer would flush, and leaves the process with the next byte write or at
//! [`Printer::finish`].

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::brand::Brand;
use crate::cancel::{CancelReason, CancelToken};
use crate::fault::{self, Point};
use crate::sync;

/// `--color` / `PRE_COMMIT_COLOR` values (`color.py:COLOR_CHOICES`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorSetting {
    /// `auto`: stderr is a terminal and `TERM != dumb` (decided by snp-os).
    Auto,
    /// `always`.
    Always,
    /// `never`.
    Never,
}

impl ColorSetting {
    /// Exact, case-sensitive parse of `auto` / `always` / `never`.
    pub fn parse(v: &OsStr) -> Option<Self> {
        match v.to_str()? {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// pre-commit's colours (`color.py:59-64`, 02 §4).
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Color {
    /// `\x1b[41m` (red background): `Failed`, `[ERROR]`.
    Red,
    /// `\x1b[42m`: `Passed`.
    Green,
    /// `\x1b[43;30m`: `Skipped` (SKIP), `[WARNING]`.
    Yellow,
    /// `\x1b[46;30m`: `Skipped` (no files).
    Turquoise,
    /// `\x1b[2m`: detail lines.
    Subtle,
}

impl Color {
    const fn code(self) -> &'static str {
        match self {
            Self::Red => "\x1b[41m",
            Self::Green => "\x1b[42m",
            Self::Yellow => "\x1b[43;30m",
            Self::Turquoise => "\x1b[46;30m",
            Self::Subtle => "\x1b[2m",
        }
    }
}

/// `color.NORMAL`.
const NORMAL: &str = "\x1b[m";

/// Log levels of pre-commit's logging handler (`logging_handler.py`).
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Level {
    /// `[INFO]` (colour `''`, but the reset is still appended: `[INFO]\x1b[m`).
    Info,
    /// `[WARNING]` (yellow).
    Warning,
    /// `[ERROR]` (red).
    Error,
}

impl Level {
    const fn label(self) -> &'static str {
        match self {
            Self::Info => "[INFO]",
            Self::Warning => "[WARNING]",
            Self::Error => "[ERROR]",
        }
    }

    const fn color(self) -> &'static str {
        match self {
            Self::Info => "",
            Self::Warning => Color::Yellow.code(),
            Self::Error => Color::Red.code(),
        }
    }
}

/// How the [`Printer`] writes.
#[derive(Clone, Debug)]
pub struct PrinterOpts {
    /// Product name in snp-only texts.
    pub brand: Brand,
    /// The colour decision (`use_color`).
    pub color: bool,
    /// Whether stdout is a TTY: line-buffered `print()` instead of the 8192-byte pending buffer.
    pub stdout_is_tty: bool,
    /// CRLF for `py_print`/`py_eprint` (Python text-mode stdio; true on Windows, ADR-029).
    pub windows_text_mode: bool,
    /// Cancelled with `StdoutClosed` when stdout returns EPIPE.
    pub cancel: CancelToken,
}

/// `TextIOWrapper._CHUNK_SIZE`.
const TEXT_CHUNK: usize = 8192;
/// `io.DEFAULT_BUFFER_SIZE` of `sys.stdout.buffer` (a `BufferedWriter`).
const BYTE_BUFFER: usize = 8192;

struct State {
    stdout: Box<dyn Write + Send>,
    stderr: Box<dyn Write + Send>,
    /// Text-layer bytes not yet handed to the byte layer (encoded, CRLF applied).
    pending_text: Vec<u8>,
    /// Byte-layer bytes not yet written to the file descriptor.
    byte_buf: Vec<u8>,
}

struct Shared {
    opts: PrinterOpts,
    closed: AtomicBool,
    state: Mutex<State>,
}

/// The only writer of user-facing stdout/stderr. Never panics, never returns an error: EPIPE on
/// stdout latches `closed` and cancels the token with `StdoutClosed` (later stdout writes are
/// dropped). Poison-proof (S14). Cheap to clone. The fault point `PrinterWrite` (feature
/// `fault-injection`) fires while the state lock is held.
#[derive(Clone)]
pub struct Printer(Arc<Shared>);

impl fmt::Debug for Printer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Printer")
            .field("brand", &self.0.opts.brand)
            .field("color", &self.0.opts.color)
            .field("stdout_is_tty", &self.0.opts.stdout_is_tty)
            .field("windows_text_mode", &self.0.opts.windows_text_mode)
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl Printer {
    /// A printer over explicit writers (tests, captured output).
    pub fn new(
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
        o: PrinterOpts,
    ) -> Self {
        Self(Arc::new(Shared {
            opts: o,
            closed: AtomicBool::new(false),
            state: Mutex::new(State {
                stdout,
                stderr,
                pending_text: Vec::new(),
                byte_buf: Vec::new(),
            }),
        }))
    }

    /// A printer over the process's stdout and stderr.
    pub fn stdio(o: PrinterOpts) -> Self {
        Self::new(Box::new(io::stdout()), Box::new(io::stderr()), o)
    }

    /// The brand texts are rendered in.
    pub fn brand(&self) -> Brand {
        self.0.opts.brand
    }

    /// The colour decision.
    pub fn color(&self) -> bool {
        self.0.opts.color
    }

    /// Runs `f` with the state lock held (poison-proof); the fault point fires inside the lock.
    fn with_state(&self, f: impl FnOnce(&Self, &mut State)) {
        let mut st = sync::lock(&self.0.state);
        fault::hit(Point::PrinterWrite);
        f(self, &mut st);
    }

    fn latch_closed(&self) {
        if !self.0.closed.swap(true, Ordering::AcqRel) {
            self.0.opts.cancel.cancel(CancelReason::StdoutClosed);
        }
    }

    /// Writes to the stdout file descriptor; EPIPE latches `closed`, other errors drop the bytes.
    fn fd_write(&self, st: &mut State, bytes: &[u8]) {
        if bytes.is_empty() || self.is_closed() {
            return;
        }
        let res = st.stdout.write_all(bytes).and_then(|()| st.stdout.flush());
        if res.is_err_and(|e| e.kind() == io::ErrorKind::BrokenPipe) {
            self.latch_closed();
        }
    }

    /// `BufferedWriter.write`: buffer what fits, else flush the buffer and buffer (or, when larger
    /// than the buffer, write through) the new data.
    fn byte_write(&self, st: &mut State, data: &[u8]) {
        if data.len() <= BYTE_BUFFER.saturating_sub(st.byte_buf.len()) {
            st.byte_buf.extend_from_slice(data);
            return;
        }
        self.byte_flush(st);
        if data.len() > BYTE_BUFFER {
            self.fd_write(st, data);
        } else {
            st.byte_buf.extend_from_slice(data);
        }
    }

    /// `BufferedWriter.flush`.
    fn byte_flush(&self, st: &mut State) {
        let buf = std::mem::take(&mut st.byte_buf);
        self.fd_write(st, &buf);
    }

    /// `TextIOWrapper._writeflush`: pending text into the byte layer (not to the fd).
    fn text_writeflush(&self, st: &mut State) {
        let pending = std::mem::take(&mut st.pending_text);
        if !pending.is_empty() {
            self.byte_write(st, &pending);
        }
    }

    /// `TextIOWrapper.write(s)` (`_io_TextIOWrapper_write_impl`).
    fn text_write(&self, st: &mut State, s: &str) {
        let needflush = self.0.opts.stdout_is_tty && (s.contains('\n') || s.contains('\r'));
        let encoded: Cow<'_, str> = if self.0.opts.windows_text_mode {
            snp_pycompat::textio::windows_text_write(s)
        } else {
            Cow::Borrowed(s)
        };
        let b = encoded.as_bytes();
        if st.pending_text.len().saturating_add(b.len()) > TEXT_CHUNK {
            self.text_writeflush(st);
        }
        st.pending_text.extend_from_slice(b);
        if st.pending_text.len() >= TEXT_CHUNK || needflush {
            self.text_writeflush(st);
        }
        if needflush {
            self.byte_flush(st);
        }
    }

    fn stderr_write(st: &mut State, bytes: &[u8]) {
        let _ignored: io::Result<()> = st.stderr.write_all(bytes).and_then(|()| st.stderr.flush());
    }

    fn text_line(&self, s: &str) -> Vec<u8> {
        let mut line = String::from(s);
        line.push('\n');
        if self.0.opts.windows_text_mode {
            snp_pycompat::textio::windows_text_write(&line)
                .into_owned()
                .into_bytes()
        } else {
            line.into_bytes()
        }
    }

    /// `output.write`: `sys.stdout.buffer.write(b)` + `flush()` (the byte layer; pending
    /// `print()` text stays pending).
    pub fn write(&self, bytes: &[u8]) {
        self.with_state(|p, st| {
            p.byte_write(st, bytes);
            p.byte_flush(st);
        });
    }

    /// `output.write_line_b(b)`: `b`, `\n`, flush.
    pub fn write_line(&self, bytes: &[u8]) {
        self.with_state(|p, st| {
            p.byte_write(st, bytes);
            p.byte_write(st, b"\n");
            p.byte_flush(st);
        });
    }

    /// The logging handler: `"{level_msg} {msg}\n"` on stdout, where `level_msg` is
    /// `[WARNING]` etc., coloured as `{COLOR}[LEVEL]\x1b[m` (`[INFO]\x1b[m` for INFO).
    pub fn log(&self, level: Level, msg: &str) {
        let mut line = String::new();
        if self.color() {
            line.push_str(level.color());
            line.push_str(level.label());
            line.push_str(NORMAL);
        } else {
            line.push_str(level.label());
        }
        line.push(' ');
        line.push_str(msg);
        self.write_line(line.as_bytes());
    }

    /// `print(s)`: `s` then `\n` through the text layer (8192-byte pending buffer when piped,
    /// line-buffered on a TTY; CRLF in Windows text mode).
    pub fn py_print(&self, s: &str) {
        self.with_state(|p, st| {
            p.text_write(st, s);
            p.text_write(st, "\n");
        });
    }

    /// `print(s, file=sys.stderr)` / `SystemExit(msg)` / argparse errors: line-buffered stderr
    /// text (CRLF in Windows text mode).
    pub fn py_eprint(&self, s: &str) {
        let line = self.text_line(s);
        self.with_state(|_, st| Self::stderr_write(st, &line));
    }

    /// Raw stderr bytes.
    pub fn write_stderr(&self, bytes: &[u8]) {
        self.with_state(|_, st| Self::stderr_write(st, bytes));
    }

    /// snp-only diagnostics: a stderr line, never stdout.
    #[expect(
        clippy::same_name_method,
        reason = "frozen API: Printer::diag and DiagSink::diag (ARCHITECTURE §1.4.1)"
    )]
    pub fn diag(&self, s: &str) {
        self.py_eprint(s);
    }

    /// `color.format_color(text, color, self.color())`: `{color}{text}\x1b[m` when colour is on.
    pub fn paint<'a>(&self, text: &'a str, c: Color) -> Cow<'a, str> {
        if self.color() {
            let mut s = String::from(c.code());
            s.push_str(text);
            s.push_str(NORMAL);
            Cow::Owned(s)
        } else {
            Cow::Borrowed(text)
        }
    }

    /// Flushes the OS-level writers before stdout is handed to an inheriting child. The modelled
    /// CPython buffers keep their content: CPython does not flush them before `subprocess.call`.
    pub fn flush(&self) {
        self.with_state(|p, st| {
            if !p.is_closed()
                && st
                    .stdout
                    .flush()
                    .is_err_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
            {
                p.latch_closed();
            }
            let _ignored: io::Result<()> = st.stderr.flush();
        });
    }

    /// Interpreter-exit flush order: pending `print()` text last, then stderr.
    pub fn finish(&self) {
        self.with_state(|p, st| {
            p.text_writeflush(st);
            p.byte_flush(st);
            let _ignored: io::Result<()> = st.stderr.flush();
        });
    }

    /// Whether stdout returned EPIPE.
    pub fn is_closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }
}

/// `[WARNING] msg` lines on stdout, in order (cfgv warnings, deprecation notes).
pub trait WarnSink: Send + Sync + fmt::Debug {
    /// Emits one warning.
    fn warn(&self, msg: &str);
}

/// stderr diagnostics (regex `FutureWarning`s, snp notes).
pub trait DiagSink: Send + Sync + fmt::Debug {
    /// Emits one diagnostic line.
    fn diag(&self, msg: &str);
}

impl WarnSink for Printer {
    fn warn(&self, msg: &str) {
        self.log(Level::Warning, msg);
    }
}

impl DiagSink for Printer {
    fn diag(&self, msg: &str) {
        Self::diag(self, msg);
    }
}

/// Collects warnings and diagnostics in arrival order (tests, bare replay in meta hooks).
#[derive(Debug, Default)]
pub struct CollectWarnings(Mutex<Vec<String>>);

impl CollectWarnings {
    /// An empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes everything collected so far.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *sync::lock(&self.0))
    }
}

impl WarnSink for CollectWarnings {
    fn warn(&self, msg: &str) {
        sync::lock(&self.0).push(msg.to_owned());
    }
}

impl DiagSink for CollectWarnings {
    fn diag(&self, msg: &str) {
        sync::lock(&self.0).push(msg.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A writer whose bytes the test can read back, recording flushes as `|`.
    #[derive(Clone, Default)]
    struct Sink {
        bytes: Arc<Mutex<Vec<u8>>>,
        events: Arc<Mutex<Vec<String>>>,
        name: &'static str,
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            sync::lock(&self.bytes).extend_from_slice(buf);
            sync::lock(&self.events).push(format!(
                "{}:{}",
                self.name,
                String::from_utf8_lossy(buf)
            ));
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Sink {
        fn take(&self) -> Vec<u8> {
            std::mem::take(&mut sync::lock(&self.bytes))
        }
    }

    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn opts(color: bool, tty: bool, crlf: bool) -> PrinterOpts {
        PrinterOpts {
            brand: Brand::Snp,
            color,
            stdout_is_tty: tty,
            windows_text_mode: crlf,
            cancel: CancelToken::new(),
        }
    }

    fn printer(color: bool, tty: bool, crlf: bool) -> (Printer, Sink, Sink) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let out = Sink {
            name: "out",
            events: Arc::clone(&events),
            ..Sink::default()
        };
        let err = Sink {
            name: "err",
            events,
            ..Sink::default()
        };
        (
            Printer::new(
                Box::new(out.clone()),
                Box::new(err.clone()),
                opts(color, tty, crlf),
            ),
            out,
            err,
        )
    }

    #[test]
    fn log_line_goldens() {
        let (p, out, _) = printer(true, false, false);
        p.log(Level::Info, "Initializing environment for x.");
        p.log(Level::Warning, "w");
        p.log(Level::Error, "e");
        assert_eq!(
            out.take(),
            b"[INFO]\x1b[m Initializing environment for x.\n\x1b[43;30m[WARNING]\x1b[m w\n\x1b[41m[ERROR]\x1b[m e\n"
        );
        let (p, out, _) = printer(false, false, false);
        p.log(Level::Info, "i");
        p.log(Level::Warning, "w");
        p.log(Level::Error, "e");
        assert_eq!(out.take(), b"[INFO] i\n[WARNING] w\n[ERROR] e\n");
    }

    #[test]
    fn color_goldens() {
        let (p, _, _) = printer(true, false, false);
        assert_eq!(p.paint("Failed", Color::Red), "\x1b[41mFailed\x1b[m");
        assert_eq!(p.paint("Passed", Color::Green), "\x1b[42mPassed\x1b[m");
        assert_eq!(
            p.paint("Skipped", Color::Yellow),
            "\x1b[43;30mSkipped\x1b[m"
        );
        assert_eq!(
            p.paint("Skipped", Color::Turquoise),
            "\x1b[46;30mSkipped\x1b[m"
        );
        assert_eq!(
            p.paint("- hook id: x", Color::Subtle),
            "\x1b[2m- hook id: x\x1b[m"
        );
        let (p, _, _) = printer(false, false, false);
        assert!(matches!(
            p.paint("Passed", Color::Green),
            Cow::Borrowed("Passed")
        ));
        assert!(!p.color() && p.brand() == Brand::Snp);
    }

    #[test]
    fn color_settings() {
        assert_eq!(
            ColorSetting::parse(OsStr::new("auto")),
            Some(ColorSetting::Auto)
        );
        assert_eq!(
            ColorSetting::parse(OsStr::new("always")),
            Some(ColorSetting::Always)
        );
        assert_eq!(
            ColorSetting::parse(OsStr::new("never")),
            Some(ColorSetting::Never)
        );
        assert_eq!(ColorSetting::parse(OsStr::new("Always")), None);
        assert_eq!(ColorSetting::parse(OsStr::new("")), None);
    }

    /// 01 §8.2: piped, `print()` blocks appear after later warnings (and only at exit).
    #[test]
    fn py_print_reorders_when_piped() {
        let (p, out, _) = printer(false, false, false);
        p.py_print("==> File .pre-commit-config.yaml\n=====> bad");
        p.log(Level::Warning, "Unexpected key(s) present on local => x: y");
        p.write_line(b"after");
        assert_eq!(
            out.take(),
            b"[WARNING] Unexpected key(s) present on local => x: y\nafter\n"
        );
        p.finish();
        assert_eq!(
            out.take(),
            b"==> File .pre-commit-config.yaml\n=====> bad\n"
        );
    }

    #[test]
    fn py_print_is_line_buffered_on_a_tty() {
        let (p, out, _) = printer(false, true, false);
        p.py_print("block");
        p.log(Level::Warning, "w");
        p.finish();
        assert_eq!(out.take(), b"block\n[WARNING] w\n");
    }

    #[test]
    fn py_print_overflow_moves_text_into_the_byte_layer() {
        let (p, out, _) = printer(false, false, false);
        let big: String = std::iter::repeat_n('x', 8191).collect();
        p.py_print(&big); // 8191 + 1 = 8192 pending → handed to the byte layer
        assert_eq!(out.take(), b"", "moved, not yet flushed");
        p.write(b"!");
        let got = out.take();
        assert_eq!(got.len(), 8193);
        assert_eq!(got.last(), Some(&b'!'));
        p.py_print("tail");
        p.write(b"?");
        assert_eq!(out.take(), b"?");
        p.finish();
        assert_eq!(out.take(), b"tail\n");
    }

    #[test]
    fn crlf_only_for_the_text_layer() {
        let (p, out, err) = printer(false, true, true);
        p.py_print("a\nb");
        p.write_line(b"raw");
        p.py_eprint("e");
        p.diag("d");
        p.finish();
        assert_eq!(out.take(), b"a\r\nb\r\nraw\n");
        assert_eq!(err.take(), b"e\r\nd\r\n");
    }

    #[test]
    fn stderr_writes() {
        let (p, out, err) = printer(false, false, false);
        p.write_stderr(b"raw\xff");
        p.py_eprint("usage: snp");
        p.flush();
        assert_eq!(err.take(), b"raw\xffusage: snp\n");
        assert_eq!(out.take(), b"");
    }

    #[test]
    fn epipe_latches_closed_and_cancels() {
        let o = opts(false, false, false);
        let token = o.cancel.clone();
        let err = Sink::default();
        let p = Printer::new(Box::new(Broken), Box::new(err.clone()), o);
        p.write_line(b"x");
        assert!(p.is_closed());
        assert_eq!(token.reason(), Some(CancelReason::StdoutClosed));
        p.write(b"more");
        p.py_print("more");
        p.finish();
        p.diag("still on stderr");
        assert_eq!(err.take(), b"still on stderr\n");
        assert!(format!("{p:?}").contains("closed: true"));
    }

    #[cfg(unix)]
    #[test]
    fn epipe_on_a_real_closed_pipe() {
        let (reader, writer) = io::pipe().unwrap();
        drop(reader);
        let o = opts(false, false, false);
        let token = o.cancel.clone();
        let p = Printer::new(Box::new(writer), Box::new(io::sink()), o);
        p.write_line(b"to nobody");
        assert!(p.is_closed());
        assert!(token.is_cancelled());
    }

    #[test]
    fn sinks() {
        let c = CollectWarnings::new();
        c.warn("a");
        DiagSink::diag(&c, "b");
        assert_eq!(c.take(), vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(c.take().len(), 0);
        let (p, out, err) = printer(false, false, false);
        WarnSink::warn(&p, "careful");
        DiagSink::diag(&p, "note");
        assert_eq!(out.take(), b"[WARNING] careful\n");
        assert_eq!(err.take(), b"note\n");
    }

    #[test]
    fn printer_survives_a_poisoned_lock() {
        let (p, out, _) = printer(false, false, false);
        let p2 = p.clone();
        let res = std::thread::Builder::new()
            .spawn(move || {
                let _g = p2.0.state.lock().unwrap();
                panic!("poison the printer lock");
            })
            .unwrap()
            .join();
        assert!(res.is_err());
        assert!(p.0.state.is_poisoned());
        p.write_line(b"still works");
        assert_eq!(out.take(), b"still works\n");
    }
}
