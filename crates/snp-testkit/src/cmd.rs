//! A process invocation with the hermetic env.
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-18"
)]

use std::ffi::OsStr;

use snp_proc as _;

/// A process invocation with the hermetic env.
#[derive(Debug)]
pub struct Cmd {
    /// W0 placeholder; WP-18 stores the program, argv, env, cwd, stdin and the tty flag.
    _priv: (),
}

impl Cmd {
    /// Appends one argument.
    #[must_use]
    pub fn arg(self, a: impl AsRef<OsStr>) -> Self {
        todo!()
    }

    /// Appends arguments.
    #[must_use]
    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(self, a: I) -> Self {
        todo!()
    }

    /// Sets a variable.
    #[must_use]
    pub fn env(self, k: &str, v: impl AsRef<OsStr>) -> Self {
        todo!()
    }

    /// Removes a variable.
    #[must_use]
    pub fn env_remove(self, k: &str) -> Self {
        todo!()
    }

    /// stdin bytes (default: empty).
    #[must_use]
    pub fn stdin(self, bytes: &[u8]) -> Self {
        todo!()
    }

    /// stdout/stderr on a pty (posix).
    #[must_use]
    pub fn tty(self) -> Self {
        todo!()
    }

    /// Runs to completion.
    pub fn output(self) -> Output {
        todo!()
    }
}

/// Exit code and output of a [`Cmd`].
#[derive(Debug)]
pub struct Output {
    /// Exit code (signal deaths as `128 + n` on Unix).
    pub code: i32,
    /// stdout bytes.
    pub stdout: Vec<u8>,
    /// stderr bytes.
    pub stderr: Vec<u8>,
}
