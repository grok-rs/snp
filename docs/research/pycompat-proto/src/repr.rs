//! Python `repr()` for the value shapes pre-commit formats into messages.
use core::fmt::Write as _;

use crate::unicode::is_printable;

/// One "Python character": a scalar value, or a lone surrogate produced by
/// `surrogateescape` decoding (POSIX `os.fsdecode`) of an undecodable byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyChar {
    Scalar(char),
    /// U+DC80..=U+DCFF (byte 0x80..=0xFF) or any other surrogate on Windows.
    Surrogate(u16),
}

fn write_quoted<I: Iterator<Item = PyChar> + Clone>(out: &mut String, chars: I) {
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
    for c in chars {
        match c {
            PyChar::Scalar(c) if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            PyChar::Scalar('\t') => out.push_str("\\t"),
            PyChar::Scalar('\n') => out.push_str("\\n"),
            PyChar::Scalar('\r') => out.push_str("\\r"),
            PyChar::Scalar(c) if c < ' ' || c == '\x7f' => {
                let _ = write!(out, "\\x{:02x}", u32::from(c));
            }
            PyChar::Scalar(c) if c.is_ascii() || is_printable(c) => out.push(c),
            PyChar::Scalar(c) => {
                let cp = u32::from(c);
                let _ = if cp <= 0xff {
                    write!(out, "\\x{cp:02x}")
                } else if cp <= 0xffff {
                    write!(out, "\\u{cp:04x}")
                } else {
                    write!(out, "\\U{cp:08x}")
                };
            }
            PyChar::Surrogate(u) => {
                let _ = write!(out, "\\u{u:04x}");
            }
        }
    }
    out.push(quote);
}

/// Append `repr(s)` to `out`.
pub fn write_repr_str(out: &mut String, s: &str) {
    write_quoted(out, s.chars().map(PyChar::Scalar));
}

/// `repr(s)`.
#[must_use]
pub fn repr_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_repr_str(&mut out, s);
    out
}

/// Iterate `bytes` as Python would see `os.fsdecode(bytes)` on POSIX
/// (UTF-8 + `surrogateescape`): each byte of an undecodable sequence becomes
/// U+DC00 + byte.
pub fn fsdecode_chars(mut b: &[u8]) -> impl Iterator<Item = PyChar> + Clone + '_ {
    let mut pieces: Vec<PyChar> = Vec::new();
    loop {
        match core::str::from_utf8(b) {
            Ok(s) => {
                pieces.extend(s.chars().map(PyChar::Scalar));
                break;
            }
            Err(e) => {
                let (ok, rest) = b.split_at(e.valid_up_to());
                if let Ok(s) = core::str::from_utf8(ok) {
                    pieces.extend(s.chars().map(PyChar::Scalar));
                }
                let bad = e.error_len().map_or(rest.len(), usize::from);
                let (bad_bytes, tail) = rest.split_at(bad.min(rest.len()));
                pieces.extend(bad_bytes.iter().map(|&x| PyChar::Surrogate(0xdc00 | u16::from(x))));
                b = tail;
            }
        }
    }
    pieces.into_iter()
}

/// `repr(os.fsdecode(b))` — for POSIX paths that may not be valid UTF-8.
#[must_use]
pub fn repr_fsbytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() + 2);
    write_quoted(&mut out, fsdecode_chars(b));
    out
}

/// The value shapes that reach `repr()`/`str()` in pre-commit and cfgv messages.
#[derive(Clone, Debug, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    Tuple(Vec<PyValue>),
    Dict(Vec<(PyValue, PyValue)>),
}

/// `repr(v)`.
#[must_use]
pub fn repr(v: &PyValue) -> String {
    let mut out = String::new();
    write_repr(&mut out, v);
    out
}

/// Append `repr(v)` to `out`.
pub fn write_repr(out: &mut String, v: &PyValue) {
    match v {
        PyValue::None => out.push_str("None"),
        PyValue::Bool(true) => out.push_str("True"),
        PyValue::Bool(false) => out.push_str("False"),
        PyValue::Int(i) => {
            let _ = write!(out, "{i}");
        }
        PyValue::Float(f) => out.push_str(&repr_float(*f)),
        PyValue::Str(s) => write_repr_str(out, s),
        PyValue::List(items) => {
            out.push('[');
            write_items(out, items);
            out.push(']');
        }
        PyValue::Tuple(items) => {
            out.push('(');
            write_items(out, items);
            if items.len() == 1 {
                out.push(',');
            }
            out.push(')');
        }
        PyValue::Dict(items) => {
            out.push('{');
            for (i, (k, v)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_repr(out, k);
                out.push_str(": ");
                write_repr(out, v);
            }
            out.push('}');
        }
    }
}

fn write_items(out: &mut String, items: &[PyValue]) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_repr(out, item);
    }
}

/// `repr(tuple_of_str)` without building a `PyValue` (hot path: argv in errors).
#[must_use]
pub fn repr_str_tuple<S: AsRef<str>>(items: &[S]) -> String {
    let mut out = String::from("(");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_repr_str(&mut out, s.as_ref());
    }
    if items.len() == 1 {
        out.push(',');
    }
    out.push(')');
    out
}

/// `repr(list_of_str)`.
#[must_use]
pub fn repr_str_list<S: AsRef<str>>(items: &[S]) -> String {
    let mut out = String::from("[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_repr_str(&mut out, s.as_ref());
    }
    out.push(']');
    out
}

/// `repr(float)`: shortest round-trip digits; positional when
/// `-4 < decpt <= 16`, otherwise `d[.ddd]e±XX`.
#[must_use]
pub fn repr_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let sci = format!("{:e}", f.abs()); // shortest digits, e.g. "1.5e-7"
    let (mant, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let neg = f.is_sign_negative();
    let decpt = exp + 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        let (first, rest) = digits.split_at(1.min(digits.len()));
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let e = decpt - 1;
        let _ = write!(out, "e{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs());
    } else if decpt <= 0 {
        out.push_str("0.");
        out.extend(core::iter::repeat_n('0', decpt.unsigned_abs() as usize));
        out.push_str(&digits);
    } else {
        let d = decpt.unsigned_abs() as usize;
        if d >= digits.len() {
            out.push_str(&digits);
            out.extend(core::iter::repeat_n('0', d - digits.len()));
            out.push_str(".0");
        } else {
            let (a, b) = digits.split_at(d);
            out.push_str(a);
            out.push('.');
            out.push_str(b);
        }
    }
    out
}

/// `str(OSError(errno, strerror, filename[, None, filename2]))`.
#[must_use]
pub fn oserror_str(errno: i32, strerror: &str, filename: Option<&str>, filename2: Option<&str>) -> String {
    let mut out = format!("[Errno {errno}] {strerror}");
    if let Some(f) = filename {
        out.push_str(": ");
        write_repr_str(&mut out, f);
        if let Some(f2) = filename2 {
            out.push_str(" -> ");
            write_repr_str(&mut out, f2);
        }
    }
    out
}

/// `str(subprocess.CalledProcessError(rc, cmd))` (stdlib, raised by
/// `subprocess.check_output` in `hook_impl.py`). `cmd_str` is `str(cmd)`,
/// i.e. the tuple/list repr.
#[must_use]
pub fn stdlib_called_process_error_str(returncode: i32, cmd_str: &str) -> String {
    if returncode < 0 {
        let sig = returncode.unsigned_abs();
        match signal_name(sig) {
            Some(name) => format!("Command '{cmd_str}' died with <Signals.{name}: {sig}>."),
            None => format!("Command '{cmd_str}' died with unknown signal {sig}."),
        }
    } else {
        format!("Command '{cmd_str}' returned non-zero exit status {returncode}.")
    }
}

/// Linux signal names as Python's `signal.Signals` spells them (subset).
#[must_use]
pub const fn signal_name(sig: u32) -> Option<&'static str> {
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
        _ => return None,
    })
}
