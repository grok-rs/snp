//! Backend errors and their mapping to the failure envelope (04b §1.8, 13 X.1).
//!
//! Owner: WP-16.

use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;

use snp_core::cancel::CancelReason;
use snp_core::failure::{Failure, PyExc, pyerr};
use snp_net::NetError;

/// Every failure of a backend.
#[derive(Debug)]
#[non_exhaustive]
pub enum LangError {
    /// `FatalError`.
    Fatal(String),
    /// `AssertionError` (`assert_version_default`, `assert_no_additional_deps`).
    Assertion(String),
    /// `ValueError`.
    Value(String),
    /// `CalledProcessError` (format A).
    CalledProcess {
        /// The normalised argv.
        cmd: Vec<OsString>,
        /// Return code.
        rc: i64,
        /// stdout.
        stdout: Vec<u8>,
        /// stderr.
        stderr: Vec<u8>,
    },
    /// `OSError` of a filesystem operation.
    Io {
        /// The operation.
        op: &'static str,
        /// The path.
        path: PathBuf,
        /// The error.
        err: io::Error,
    },
    /// A download failure.
    Net(NetError),
    /// The cancel token fired.
    Interrupted,
}

impl LangError {
    /// The Python exception class pre-commit would show for this error (`error_handler.py:75-81` prints
    /// `{type(e).__name__}: {e}`): `FatalError`, `AssertionError`, `ValueError`, `CalledProcessError`, the `OSError`
    /// subclass of an I/O error (`FileNotFoundError`, `PermissionError`, …), `KeyboardInterrupt`; downloads (which
    /// pre-commit performs with `urllib`) are `HTTPError`/`URLError`, and snp-only download failures (offline mode,
    /// checksum, size cap, unsafe archive entry, insecure URL) `NetError`.
    pub fn py_type(&self) -> Cow<'static, str> {
        match self {
            Self::Fatal(_) => "FatalError".into(),
            Self::Assertion(_) => "AssertionError".into(),
            Self::Value(_) => "ValueError".into(),
            Self::CalledProcess { .. } => "CalledProcessError".into(),
            Self::Io { err, .. } | Self::Net(NetError::Io(err)) => {
                pyerr::os_error_class(err).into()
            }
            Self::Net(NetError::Cancelled) | Self::Interrupted => "KeyboardInterrupt".into(),
            Self::Net(NetError::Http { .. }) => "HTTPError".into(),
            Self::Net(NetError::Transport { .. }) => "URLError".into(),
            Self::Net(_) => "NetError".into(),
        }
    }
}

/// `str(exc)`: the text after `{type}: ` in pre-commit's first error line (format A bytes are decoded lossily).
impl fmt::Display for LangError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fatal(m) | Self::Assertion(m) | Self::Value(m) => f.write_str(m),
            Self::CalledProcess {
                cmd,
                rc,
                stdout,
                stderr,
            } => f.write_str(&String::from_utf8_lossy(&pyerr::called_process_a(
                cmd,
                *rc,
                Some(stdout),
                Some(stderr),
            ))),
            Self::Io { path, err, .. } => {
                f.write_str(&pyerr::os_error_text(err, Some(path.as_os_str()), None))
            }
            Self::Net(e) => write!(f, "{e}"),
            Self::Interrupted => Ok(()),
        }
    }
}

impl std::error::Error for LangError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { err, .. } => Some(err),
            Self::Net(e) => Some(e),
            _ => None,
        }
    }
}

impl From<NetError> for LangError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::Cancelled => Self::Interrupted,
            other => Self::Net(other),
        }
    }
}

/// Python type + exit code (1 Fatal, 130 Interrupted, else 3).
///
/// `CalledProcessError` keeps format A as raw bytes (non-UTF-8 tool output never crashes the envelope, T10-XRG-21);
/// an I/O error is the `OSError` family with its path (`[Errno 2] No such file or directory: '…'`), the operation
/// going to the crash-log chain; an interruption is `Interrupted (^C)`, exit 130.
impl From<LangError> for Failure {
    fn from(e: LangError) -> Self {
        let ty = e.py_type();
        match e {
            LangError::Fatal(msg) => Self::fatal("FatalError", msg),
            LangError::Assertion(msg) | LangError::Value(msg) => Self::unexpected(ty, msg),
            LangError::CalledProcess {
                cmd,
                rc,
                stdout,
                stderr,
            } => Self::unexpected(
                ty,
                pyerr::called_process_a(&cmd, rc, Some(&stdout), Some(&stderr)),
            ),
            LangError::Io { op, path, err } => with_chain(
                Self::from_io(&err, Some(path.as_os_str()), None),
                format!("during {op}"),
            ),
            LangError::Interrupted | LangError::Net(NetError::Cancelled) => {
                Self::Interrupted(CancelReason::Interrupt)
            }
            LangError::Net(NetError::Io(err)) => Self::from_io(&err, None, None),
            LangError::Net(net) => {
                let chain = std::error::Error::source(&net).map(ToString::to_string);
                let f = Self::Exc {
                    kind: snp_core::failure::ExcKind::Unexpected,
                    exc: PyExc::new(ty, net.to_string()),
                };
                match chain {
                    Some(c) => with_chain(f, c),
                    None => f,
                }
            }
        }
    }
}

/// Appends a crash-log-only chain entry to an exception failure.
fn with_chain(f: Failure, entry: String) -> Failure {
    match f {
        Failure::Exc { kind, mut exc } => {
            exc.chain.push(entry);
            Failure::Exc { kind, exc }
        }
        other => other,
    }
}
