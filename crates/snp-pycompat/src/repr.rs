//! Python `repr()` and `str(exception)` texts for the value shapes pre-commit formats into messages
//! (01 §2.6, 13 X.2, 15 §A4.7; `Objects/unicodeobject.c` `unicode_repr`, `Objects/listobject.c`,
//! `Objects/tupleobject.c`, `Objects/dictobject.c`, `Objects/exceptions.c`, `Lib/subprocess.py`).

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io;

use crate::chars::{PyChar, is_printable};
use crate::messages;

pub use crate::float::repr as repr_float;

/// Appends `repr()` of a Python string given as characters (`unicode_repr`): `'` unless the text
/// holds `'` and no `"`; `\\`, the quote, `\t \n \r` escaped; other C0 and DEL as `\xNN`;
/// non-printable non-ASCII as `\xNN`/`\uNNNN`/`\UNNNNNNNN` (lowercase hex); lone surrogates as
/// `\udXXX`.
fn write_quoted<I: Iterator<Item = PyChar> + Clone>(out: &mut String, chars: I) {
    write_quoted_upto(out, chars, usize::MAX);
}

/// [`write_quoted`] that stops after `max_chars` characters of output (`%.NR`), so a huge input
/// costs O(`max_chars`) beyond the quote scan.
fn write_quoted_upto<I: Iterator<Item = PyChar> + Clone>(
    out: &mut String,
    chars: I,
    max_chars: usize,
) {
    if max_chars == 0 {
        return;
    }
    let start = out.len();
    // characters written so far (escapes are ASCII: bytes == characters)
    let mut written: usize = 0;
    let (mut sq, mut dq) = (false, false);
    for c in chars.clone() {
        match c {
            PyChar::Scalar('\'') => sq = true,
            PyChar::Scalar('"') => dq = true,
            _ => {}
        }
    }
    let quote = if sq && !dq { '"' } else { '\'' };
    out.push(quote);
    written = written.saturating_add(1);
    for c in chars {
        if written >= max_chars {
            break;
        }
        let before = out.len();
        match c {
            PyChar::Scalar(c) if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            PyChar::Scalar('\t') => out.push_str("\\t"),
            PyChar::Scalar('\n') => out.push_str("\\n"),
            PyChar::Scalar('\r') => out.push_str("\\r"),
            PyChar::Scalar(c) if c < ' ' || c == '\x7f' => {
                write!(out, "\\x{:02x}", u32::from(c)).unwrap_or_default();
            }
            PyChar::Scalar(c) if c.is_ascii() || is_printable(c) => out.push(c),
            PyChar::Scalar(c) => {
                let cp = u32::from(c);
                if cp <= 0xff {
                    write!(out, "\\x{cp:02x}")
                } else if cp <= 0xffff {
                    write!(out, "\\u{cp:04x}")
                } else {
                    write!(out, "\\U{cp:08x}")
                }
                .unwrap_or_default();
            }
            PyChar::Surrogate(u) => {
                write!(out, "\\u{u:04x}").unwrap_or_default();
            }
        }
        written = written.saturating_add(out.get(before..).map_or(0, |t| t.chars().count()));
    }
    if written < max_chars {
        out.push(quote);
    } else if written > max_chars {
        // only an escape (all ASCII, at most 9 characters) can overshoot: cut inside it
        for _ in max_chars..written {
            if out.len() <= start {
                break;
            }
            out.pop();
        }
    }
}

/// Appends `repr(s)` to `out` (`unicode_repr`).
pub fn write_repr_str(out: &mut String, s: &str) {
    write_quoted(out, s.chars().map(PyChar::Scalar));
}

/// `repr(s)` for a `str` (`{x!r}` in pre-commit and cfgv messages; 01 §2.6).
#[must_use]
pub fn repr_str(s: &str) -> String {
    let mut out = String::new();
    out.try_reserve(s.len().saturating_add(2))
        .unwrap_or_default();
    write_repr_str(&mut out, s);
    out
}

/// `repr(os.fsdecode(b))` (POSIX `surrogateescape`: undecodable bytes print as `\udcNN`).
#[must_use]
pub fn repr_fsbytes(b: &[u8]) -> String {
    let mut out = String::new();
    out.try_reserve(b.len().saturating_add(2))
        .unwrap_or_default();
    write_quoted(&mut out, crate::codec::fsdecode_chars(b));
    out
}

/// `repr()` of a path or argument as Python holds it: Unix → [`repr_fsbytes`]; Windows → the
/// UTF-16 string with lone surrogates as `\udXXX`.
#[must_use]
pub fn repr_os(s: &OsStr) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        repr_fsbytes(s.as_bytes())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let chars = char::decode_utf16(s.encode_wide()).map(|r| match r {
            Ok(c) => PyChar::Scalar(c),
            Err(e) => PyChar::Surrogate(e.unpaired_surrogate()),
        });
        let mut out = String::new();
        write_quoted(&mut out, chars);
        out
    }
    #[cfg(not(any(unix, windows)))]
    {
        repr_str(&s.to_string_lossy())
    }
}

fn write_seq<S: AsRef<str>>(out: &mut String, xs: &[S]) {
    for (i, s) in xs.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_repr_str(out, s.as_ref());
    }
}

/// `repr(tuple_of_str)`, including the trailing comma of a 1-tuple (`('git',)`; the argv of
/// `CalledProcessError`, `util.py:71`).
#[must_use]
pub fn repr_str_tuple<S: AsRef<str>>(xs: &[S]) -> String {
    let mut out = String::from("(");
    write_seq(&mut out, xs);
    if xs.len() == 1 {
        out.push(',');
    }
    out.push(')');
    out
}

/// `repr(list_of_str)` (`hook_impl.py:189-210` argument lists, `lang_base.py:107-111`).
#[must_use]
pub fn repr_str_list<S: AsRef<str>>(xs: &[S]) -> String {
    let mut out = String::from("[");
    write_seq(&mut out, xs);
    out.push(']');
    out
}

/// A Python `int` (YAML ints may exceed `i128`; such values keep their decimal text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PyInt {
    /// A value that fits `i128`.
    Small(i128),
    /// Decimal digits (optionally `-`-prefixed) of a larger value, printed verbatim.
    Decimal(Box<str>),
}

/// The value shapes pre-commit formats with `repr()`/`str()` (cfgv's own values are rendered by
/// cfgv itself).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PyValue {
    /// `None`.
    None,
    /// `True` / `False`.
    Bool(bool),
    /// `int`.
    Int(PyInt),
    /// `float`.
    Float(f64),
    /// `str`.
    Str(String),
    /// `list`.
    List(Vec<Self>),
    /// `tuple`.
    Tuple(Vec<Self>),
    /// `dict` in insertion order.
    Dict(Vec<(Self, Self)>),
}

/// `repr(v)`.
#[must_use]
pub fn repr(v: &PyValue) -> String {
    let mut out = String::new();
    write_repr(&mut out, v);
    out
}

/// One step of [`write_repr`]'s explicit stack.
enum Step<'a> {
    /// A value still to render.
    Value(&'a PyValue),
    /// Literal punctuation.
    Text(&'static str),
}

/// Appends `repr(v)` to `out`: `[a, b]`, `(a,)`, `{k: v}` in insertion order, `None`/`True`/
/// `False`, decimal ints, [`repr_float`] floats. Iterative: arbitrarily deep nesting needs no
/// thread stack (Python's `repr` would raise `RecursionError`).
pub fn write_repr(out: &mut String, v: &PyValue) {
    let mut stack = vec![Step::Value(v)];
    while let Some(step) = stack.pop() {
        let v = match step {
            Step::Text(t) => {
                out.push_str(t);
                continue;
            }
            Step::Value(v) => v,
        };
        match v {
            PyValue::None => out.push_str("None"),
            PyValue::Bool(true) => out.push_str("True"),
            PyValue::Bool(false) => out.push_str("False"),
            PyValue::Int(PyInt::Small(i)) => {
                write!(out, "{i}").unwrap_or_default();
            }
            PyValue::Int(PyInt::Decimal(d)) => out.push_str(d),
            PyValue::Float(f) => out.push_str(&repr_float(*f)),
            PyValue::Str(s) => write_repr_str(out, s),
            PyValue::List(items) => {
                out.push('[');
                stack.push(Step::Text("]"));
                push_items(&mut stack, items);
            }
            PyValue::Tuple(items) => {
                out.push('(');
                stack.push(Step::Text(if items.len() == 1 { ",)" } else { ")" }));
                push_items(&mut stack, items);
            }
            PyValue::Dict(items) => {
                out.push('{');
                stack.push(Step::Text("}"));
                for (i, (k, v)) in items.iter().enumerate().rev() {
                    stack.push(Step::Value(v));
                    stack.push(Step::Text(": "));
                    stack.push(Step::Value(k));
                    if i > 0 {
                        stack.push(Step::Text(", "));
                    }
                }
            }
        }
    }
}

/// Schedules `items` separated by `", "` (pushed in reverse: the stack pops them in order).
fn push_items<'a>(stack: &mut Vec<Step<'a>>, items: &'a [PyValue]) {
    for (i, item) in items.iter().enumerate().rev() {
        stack.push(Step::Value(item));
        if i > 0 {
            stack.push(Step::Text(", "));
        }
    }
}

/// `f'{name=}'` with a `str` value (`hazmat.py:51`): `prefix='sub/'`.
#[must_use]
pub fn fstring_eq(name: &str, v: &str) -> String {
    let mut out = String::from(name);
    out.push('=');
    write_repr_str(&mut out, v);
    out
}

/// `str(OSError(errno, strerror[, filename[, None, filename2]]))` (`OSError_str`):
/// `[Errno N] strerror: 'f1' -> 'f2'`; `f2` is ignored without `f1`.
#[must_use]
pub fn oserror_str(errno: i32, strerror: &str, f1: Option<&OsStr>, f2: Option<&OsStr>) -> String {
    let mut out = format!("[Errno {errno}] {strerror}");
    push_filenames(&mut out, f1, f2);
    out
}

fn push_filenames(out: &mut String, f1: Option<&OsStr>, f2: Option<&OsStr>) {
    if let Some(f) = f1 {
        out.push_str(": ");
        out.push_str(&repr_os(f));
        if let Some(f2) = f2 {
            out.push_str(" -> ");
            out.push_str(&repr_os(f2));
        }
    }
}

/// The C library's message for an OS error code (Rust's text minus ` (os error N)`, which on
/// POSIX is `strerror(N)` like `os.strerror`); on Windows CPython strips trailing whitespace and
/// dots of the `FormatMessageW` text (`PyErr_SetExcFromWindowsErrWithFilenameObjects`).
fn os_message(code: i32) -> String {
    let full = io::Error::from_raw_os_error(code).to_string();
    let suffix = format!(" (os error {code})");
    let msg = full.trim_suffix(&suffix);
    if cfg!(windows) {
        msg.trim_end_matches(|c: char| c == '.' || c <= ' ')
            .to_owned()
    } else {
        msg.to_owned()
    }
}

/// `str(OSError)` for an `io::Error`: `[Errno N] strerror…` (POSIX) or `[WinError N] text…`
/// (Windows), with the filenames of [`oserror_str`] (13 X.2.1). An error without an OS code
/// renders as its own message.
///
/// On Windows this is the form of a failed Win32 call (`os.stat`, `os.rename`, `os.listdir`, …:
/// `PyErr_SetExcFromWindowsErr`). CRT-backed calls — the builtin `open()` / `io.FileIO` and
/// `os.open` (`_wopen`) — keep `[Errno N] <msvcrt strerror>` (13 X.2.1), e.g. `[Errno 13]
/// Permission denied: 'x'` where this function gives `[WinError 5] Access is denied: 'x'`
/// (Rust's `File::open` reports the Win32 code). A caller emulating such a call renders it with
/// [`oserror_str`] and the CRT `errno` / `strerror` instead.
#[must_use]
pub fn oserror_from_io(e: &io::Error, f1: Option<&OsStr>, f2: Option<&OsStr>) -> String {
    let Some(code) = e.raw_os_error() else {
        return e.to_string();
    };
    let msg = os_message(code);
    if cfg!(windows) {
        let mut out = format!("[WinError {code}] {msg}");
        push_filenames(&mut out, f1, f2);
        out
    } else {
        oserror_str(code, &msg, f1, f2)
    }
}

/// The CRT's `errno` and `strerror` for a Win32 error code (`_dosmaperr`, whose default is `EINVAL`).
const fn crt_error(win32: i32) -> (i32, &'static str) {
    match win32 {
        // ERROR_FILE_NOT_FOUND, _PATH_NOT_FOUND, _INVALID_DRIVE, _NO_MORE_FILES, _BAD_NETPATH, _BAD_NET_NAME,
        // _BAD_PATHNAME and names over the length limit
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => (2, "No such file or directory"),
        4 => (24, "Too many open files"),
        // ERROR_ACCESS_DENIED, _SHARING_VIOLATION, _LOCK_VIOLATION
        5 | 32 | 33 => (13, "Permission denied"),
        6 => (9, "Bad file descriptor"),
        8 | 14 => (12, "Not enough space"),
        // ERROR_FILE_EXISTS, _ALREADY_EXISTS
        80 | 183 => (17, "File exists"),
        _ => (22, "Invalid argument"),
    }
}

/// `str(OSError)` of the builtin `open(name)`: [`oserror_from_io`] on POSIX, the CRT's `[Errno N] strerror` on
/// Windows, where `open()` goes through `_wopen` while Rust's `File::open` reports the Win32 code.
#[must_use]
pub fn oserror_from_open(e: &io::Error, name: &OsStr) -> String {
    match e.raw_os_error() {
        Some(code) if cfg!(windows) => {
            let (errno, text) = crt_error(code);
            oserror_str(errno, text, Some(name), None)
        }
        _ => oserror_from_io(e, Some(name), None),
    }
}

/// `str(subprocess.CalledProcessError(rc, cmd))` (stdlib `Lib/subprocess.py`; raised by
/// `subprocess.check_output` in `hook_impl.py:143-165`). `cmd_str` is `str(cmd)` (the tuple/list
/// repr). A negative `rc` is a signal: `died with <Signals.SIGKILL: 9>.` or `died with unknown
/// signal N.`.
#[must_use]
pub fn stdlib_called_process_error_str(rc: i64, cmd_str: &str) -> String {
    if rc < 0 {
        let sig = rc.unsigned_abs();
        match u32::try_from(sig).ok().and_then(signal_name) {
            Some(name) => messages::cpe_died_with(cmd_str, name, sig),
            None => messages::cpe_unknown_signal(cmd_str, sig),
        }
    } else {
        messages::cpe_exit_status(cmd_str, rc)
    }
}

/// The name Python's `signal.Signals` gives signal number `sig` on this platform (`Lib/signal.py`
/// `_IntEnum._convert_`, which sorts by value then name, so the alphabetically first alias wins:
/// `SIGABRT` over `SIGIOT`, `SIGCHLD` over `SIGCLD`, `SIGIO` over `SIGPOLL`). `None` where
/// `signal.Signals(sig)` raises `ValueError`.
#[must_use]
pub const fn signal_name(sig: u32) -> Option<&'static str> {
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        Some(match sig {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            4 => "SIGILL",
            5 => "SIGTRAP",
            6 => "SIGABRT",
            7 => "SIGEMT",
            8 => "SIGFPE",
            9 => "SIGKILL",
            10 => "SIGBUS",
            11 => "SIGSEGV",
            12 => "SIGSYS",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            16 => "SIGURG",
            17 => "SIGSTOP",
            18 => "SIGTSTP",
            19 => "SIGCONT",
            20 => "SIGCHLD",
            21 => "SIGTTIN",
            22 => "SIGTTOU",
            23 => "SIGIO",
            24 => "SIGXCPU",
            25 => "SIGXFSZ",
            26 => "SIGVTALRM",
            27 => "SIGPROF",
            28 => "SIGWINCH",
            29 => "SIGINFO",
            30 => "SIGUSR1",
            31 => "SIGUSR2",
            _ => return None,
        })
    }
    #[cfg(windows)]
    {
        // `signal.py` also converts the `CTRL_*` constants of the Windows `signal` module.
        Some(match sig {
            0 => "CTRL_C_EVENT",
            1 => "CTRL_BREAK_EVENT",
            2 => "SIGINT",
            4 => "SIGILL",
            8 => "SIGFPE",
            11 => "SIGSEGV",
            15 => "SIGTERM",
            21 => "SIGBREAK",
            22 => "SIGABRT",
            _ => return None,
        })
    }
    #[cfg(not(any(
        windows,
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )))]
    {
        Some(match sig {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            4 => "SIGILL",
            5 => "SIGTRAP",
            6 => "SIGABRT",
            7 => "SIGBUS",
            8 => "SIGFPE",
            9 => "SIGKILL",
            10 => "SIGUSR1",
            11 => "SIGSEGV",
            12 => "SIGUSR2",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            16 => "SIGSTKFLT",
            17 => "SIGCHLD",
            18 => "SIGCONT",
            19 => "SIGSTOP",
            20 => "SIGTSTP",
            21 => "SIGTTIN",
            22 => "SIGTTOU",
            23 => "SIGURG",
            24 => "SIGXCPU",
            25 => "SIGXFSZ",
            26 => "SIGVTALRM",
            27 => "SIGPROF",
            28 => "SIGWINCH",
            29 => "SIGIO",
            30 => "SIGPWR",
            31 => "SIGSYS",
            34 => "SIGRTMIN",
            64 => "SIGRTMAX",
            _ => return None,
        })
    }
}

/// `ValueError` text of `int(literal)` (base 10, `Objects/longobject.c`):
/// `invalid literal for int() with base 10: %.200R` — the repr is cut at 200 *characters*, even in
/// the middle of an escape, and then has no closing quote.
#[must_use]
pub fn int_literal_error(literal: &str) -> String {
    let mut out = String::from(messages::INT_LITERAL_PREFIX);
    write_quoted_upto(
        &mut out,
        literal.chars().map(PyChar::Scalar),
        messages::INT_LITERAL_REPR_CHARS,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s * n` (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    #[test]
    fn str_reprs() {
        let cases: &[(&str, &str)] = &[
            ("", "''"),
            ("it's", "\"it's\""),
            ("both '\"", "'both \\'\"'"),
            ("back\\slash", "'back\\\\slash'"),
            ("tab\t", "'tab\\t'"),
            ("nul\x00", "'nul\\x00'"),
            ("del\x7f", "'del\\x7f'"),
            ("\u{80}", "'\\x80'"),
            ("\u{a0}", "'\\xa0'"),
            ("\u{ad}", "'\\xad'"),
            ("caf\u{e9}", "'café'"),
            ("\u{378}", "'\\u0378'"),
            ("\u{200b}", "'\\u200b'"),
            ("\u{2028}", "'\\u2028'"),
            ("\u{3000}", "'\\u3000'"),
            ("\u{feff}", "'\\ufeff'"),
            ("\u{fffd}", "'\u{fffd}'"),
            ("\u{1f600}", "'😀'"),
            ("\u{1fae9}", "'\\U0001fae9'"),
            ("\u{10ffff}", "'\\U0010ffff'"),
            ("x\u{85}y", "'x\\x85y'"),
            ("e\u{301}", "'e\u{301}'"),
        ];
        for (s, want) in cases {
            assert_eq!(repr_str(s), *want, "{s:?}");
        }
        assert_eq!(repr_fsbytes(b"/tmp/\xff\x80"), "'/tmp/\\udcff\\udc80'");
    }

    #[test]
    fn container_reprs() {
        assert_eq!(repr_str_tuple(&["git"]), "('git',)");
        assert_eq!(repr_str_tuple::<&str>(&[]), "()");
        assert_eq!(
            repr_str_tuple(&["/usr/bin/git", "write-tree"]),
            "('/usr/bin/git', 'write-tree')"
        );
        assert_eq!(
            repr_str_list(&["pyyaml==6.0", "it's"]),
            "['pyyaml==6.0', \"it's\"]"
        );
        let v = PyValue::List(vec![
            PyValue::Str("a".into()),
            PyValue::List(vec![
                PyValue::Str("b".into()),
                PyValue::Tuple(vec![PyValue::Str("c".into())]),
            ]),
        ]);
        assert_eq!(repr(&v), "['a', ['b', ('c',)]]");
        let v = PyValue::List(vec![
            PyValue::Bool(true),
            PyValue::Bool(false),
            PyValue::None,
        ]);
        assert_eq!(repr(&v), "[True, False, None]");
        let v = PyValue::List(vec![
            PyValue::Float(1.0),
            PyValue::Float(1e16),
            PyValue::Float(1.5e-7),
            PyValue::Float(-0.0),
        ]);
        assert_eq!(repr(&v), "[1.0, 1e+16, 1.5e-07, -0.0]");
        let v = PyValue::Dict(vec![(
            PyValue::Str("k".into()),
            PyValue::List(vec![PyValue::Str("v".into())]),
        )]);
        assert_eq!(repr(&v), "{'k': ['v']}");
        assert_eq!(
            repr(&PyValue::Int(PyInt::Decimal(
                "100000000000000000000000000000000000000000".into()
            ))),
            "100000000000000000000000000000000000000000"
        );
        assert_eq!(repr(&PyValue::Tuple(vec![PyValue::Tuple(vec![])])), "((),)");
        let v = PyValue::Dict(vec![
            (PyValue::Str("a".into()), PyValue::Tuple(vec![])),
            (
                PyValue::Int(PyInt::Small(-1)),
                PyValue::Dict(vec![(PyValue::None, PyValue::List(vec![]))]),
            ),
        ]);
        assert_eq!(repr(&v), "{'a': (), -1: {None: []}}");
    }

    #[test]
    fn deep_nesting_needs_no_stack() {
        let depth = 100_000;
        let mut v = PyValue::List(vec![]);
        for _ in 0..depth {
            v = PyValue::List(vec![v]);
        }
        let render = move || {
            let r = repr(&v);
            // drop the nested value iteratively (its recursive Drop is the caller's concern)
            let mut stack = vec![v];
            while let Some(PyValue::List(items)) = stack.pop() {
                stack.extend(items);
            }
            r
        };
        let out = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(render)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(out.len(), 2 * (depth + 1));
        assert!(out.starts_with("[[[") && out.ends_with("]]]"));
    }

    #[test]
    fn crt_errors_follow_dosmaperr() {
        assert_eq!(crt_error(2), (2, "No such file or directory"));
        assert_eq!(crt_error(3), (2, "No such file or directory"));
        assert_eq!(crt_error(5), (13, "Permission denied"));
        assert_eq!(crt_error(32), (13, "Permission denied"));
        assert_eq!(crt_error(183), (17, "File exists"));
        // ERROR_INVALID_NAME: `open('a?b')` is `[Errno 22] Invalid argument`
        assert_eq!(crt_error(123), (22, "Invalid argument"));
    }

    #[test]
    fn exception_texts() {
        assert_eq!(fstring_eq("prefix", "it's/"), "prefix=\"it's/\"");
        assert_eq!(
            oserror_str(
                2,
                "No such file or directory",
                Some(OsStr::new("a")),
                Some(OsStr::new("b"))
            ),
            "[Errno 2] No such file or directory: 'a' -> 'b'"
        );
        assert_eq!(
            oserror_str(2, "No such file or directory", None, Some(OsStr::new("b"))),
            "[Errno 2] No such file or directory"
        );
        assert_eq!(
            stdlib_called_process_error_str(1, "('git', 'rev-parse', 'x^')"),
            "Command '('git', 'rev-parse', 'x^')' returned non-zero exit status 1."
        );
        assert_eq!(
            stdlib_called_process_error_str(-99, "('git',)"),
            "Command '('git',)' died with unknown signal 99."
        );
        assert_eq!(
            stdlib_called_process_error_str(i64::MIN, "x"),
            "Command 'x' died with unknown signal 9223372036854775808."
        );
        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                stdlib_called_process_error_str(-9, "('git', 'x')"),
                "Command '('git', 'x')' died with <Signals.SIGKILL: 9>."
            );
            assert_eq!(
                oserror_from_io(
                    &io::Error::from_raw_os_error(2),
                    Some(OsStr::new("./x.sh")),
                    None
                ),
                "[Errno 2] No such file or directory: './x.sh'"
            );
        }
        assert_eq!(
            oserror_from_io(&io::Error::other("custom"), None, None),
            "custom"
        );
        assert_eq!(
            int_literal_error("it's"),
            "invalid literal for int() with base 10: \"it's\""
        );
        let long: String = std::iter::repeat_n('x', 300).collect();
        let e = int_literal_error(&long);
        assert_eq!(
            e.len(),
            "invalid literal for int() with base 10: ".len() + 200
        );
        assert!(!e.ends_with('\''));
        // `%.200R` against the full repr, for cuts at every position inside escapes and
        // multi-byte characters
        let units = [
            "a",
            "\u{e9}",
            "\u{1f600}",
            "\n",
            "\x01",
            "\u{378}",
            "\u{1fae9}",
            "'",
            "\\",
        ];
        for n in 0..240_usize {
            let lit: String = (0..n).map(|i| units[(i * 7 + n) % units.len()]).collect();
            let full: String = repr_str(&lit).chars().take(200).collect();
            assert_eq!(
                int_literal_error(&lit),
                format!("invalid literal for int() with base 10: {full}"),
                "{lit:?}"
            );
        }
        // a multi-megabyte literal renders the same 200 characters
        let huge = rep("\u{e9}\n", 4_000_000);
        let want: String = repr_str(&rep("\u{e9}\n", 200)).chars().take(200).collect();
        assert_eq!(
            int_literal_error(&huge),
            format!("invalid literal for int() with base 10: {want}")
        );
    }
}
