//! Backend errors and their mapping to the failure envelope.
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use snp_core::failure::Failure;
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

/// Python type + exit code (1 Fatal, 130 Interrupted, else 3).
impl From<LangError> for Failure {
    fn from(e: LangError) -> Self {
        todo!()
    }
}
