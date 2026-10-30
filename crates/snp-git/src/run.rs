//! pre-commit's process helpers over the `GitBackend` seam: `cmd_output_b` (checked/unchecked), `xargs` (serial
//! partitions, rc aggregated) and the two `CalledProcessError` renderings (`util.py:49-120`, `xargs.py`, 13 X.2.2).
//!
//! Owner: WP-18.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::path::Path;

use snp_core::env::Env;
use snp_core::failure::{Failure, pyerr};
use snp_proc::spawn::{Group, PyStatus, SpawnError, Stdin};
use snp_proc::xargs;

use crate::GitBackend;

/// `(returncode, stdout, stderr)` of `cmd_output_b`; `stderr` is `None` when the spawn itself failed.
#[derive(Debug)]
pub(crate) struct Out {
    pub(crate) status: i64,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Option<Vec<u8>>,
}

impl Out {
    /// `_oserror_to_output` (`util.py:85-86`): returncode 1, `str(e)` + `\n` on stdout, no stderr.
    pub(crate) fn from_spawn_error(e: &SpawnError) -> Self {
        let mut text = e.python_text();
        while text.ends_with('\n') {
            text.pop();
        }
        text.push('\n');
        Self {
            status: 1,
            stdout: text.into_bytes(),
            stderr: None,
        }
    }

    /// `bytes(CalledProcessError(rc, argv, stdout, stderr))` as an exception (exit 3, 13 X.2.2 format A).
    pub(crate) fn called_process_error(&self, argv: &[OsString]) -> Failure {
        Failure::unexpected(
            "CalledProcessError",
            pyerr::called_process_a(
                argv,
                self.status,
                Some(&self.stdout),
                self.stderr.as_deref(),
            ),
        )
    }
}

/// `cmd_output_b(*argv, check=False)` with stdin `/dev/null` (ADR-019: snp's own git calls never read stdin).
pub(crate) fn capture(
    backend: &dyn GitBackend,
    argv: &[OsString],
    env: &Env,
    cwd: &Path,
    group: Group,
) -> Out {
    match backend.capture(argv, env, cwd, Stdin::Null, group) {
        Ok(c) => Out {
            status: c.status.0,
            stdout: c.stdout,
            stderr: Some(c.stderr),
        },
        Err(e) => Out::from_spawn_error(&e),
    }
}

/// `cmd_output_b(*argv)` (`check=True`): a non-zero returncode raises `CalledProcessError` (exit 3).
pub(crate) fn checked(
    backend: &dyn GitBackend,
    argv: &[OsString],
    env: &Env,
    cwd: &Path,
    group: Group,
) -> Result<Out, Failure> {
    let out = capture(backend, argv, env, cwd, group);
    if out.status == 0 {
        Ok(out)
    } else {
        Err(out.called_process_error(argv))
    }
}

/// `xargs(cmd, files, **kwargs)` with the default `target_concurrency=1`: serial partitions by command length,
/// returncode = the largest absolute partition returncode (`xargs.py:120-160`). Zero files run nothing (every
/// pre-commit caller guards the call with `if files:`).
pub(crate) fn xargs_serial(
    backend: &dyn GitBackend,
    cmd: &[OsString],
    files: &[&OsStr],
    env: &Env,
    cwd: &Path,
    group: Group,
) -> Result<i64, Failure> {
    if files.is_empty() {
        return Ok(0);
    }
    let codes: Vec<PyStatus> = partitions(cmd, files, env)?
        .iter()
        .map(|argv| PyStatus(capture(backend, argv, env, cwd, group).status))
        .collect();
    Ok(xargs::aggregate(codes).0)
}

/// `xargs`-partitioned like [`xargs_serial`], every partition checked: the first failing partition's
/// `CalledProcessError` (snp-only callers that must not ignore failures: `stage_fixed`'s `add`, try-repo's shadow).
pub(crate) fn xargs_checked(
    backend: &dyn GitBackend,
    cmd: &[OsString],
    files: &[&OsStr],
    env: &Env,
    cwd: &Path,
    group: Group,
) -> Result<(), Failure> {
    if files.is_empty() {
        return Ok(());
    }
    for argv in partitions(cmd, files, env)? {
        checked(backend, &argv, env, cwd, group)?;
    }
    Ok(())
}

/// The full argv of every `xargs` partition (serial, `target_concurrency=1`).
fn partitions(
    cmd: &[OsString],
    files: &[&OsStr],
    env: &Env,
) -> Result<Vec<Vec<OsString>>, Failure> {
    let max_len = xargs::platform_max_length(env.python_environ_size());
    let parts = xargs::partition(cmd, files, NonZeroUsize::MIN, max_len).map_err(|e| {
        // `raise ArgumentTooLongError(arg)` (`xargs.py:105-106`): str(exc) is the argument.
        Failure::unexpected("ArgumentTooLongError", e.0.to_string_lossy().into_owned())
    })?;
    Ok(parts
        .into_iter()
        .map(|range| {
            let mut argv = cmd.to_vec();
            argv.extend(
                files
                    .get(range)
                    .unwrap_or_default()
                    .iter()
                    .map(|f| f.to_os_string()),
            );
            argv
        })
        .collect())
}

/// `argv` with the first element replaced by a bare `git`: the argv pre-commit prints where it builds the
/// `CalledProcessError` itself or calls `subprocess` directly (stash `diff-index`, hook-impl `rev-list`).
pub(crate) fn bare_git_argv(args: &[OsString]) -> Vec<OsString> {
    let mut v = vec![OsString::from("git")];
    v.extend_from_slice(args);
    v
}

/// Bytes → `OsString` (Unix: the bytes; elsewhere lossy UTF-8).
pub(crate) fn os_from_bytes(b: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(b.to_vec())
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// `OsString` from parts (`f'{a}...{b}'`).
pub(crate) fn concat(parts: &[&OsStr]) -> OsString {
    let mut out = OsString::new();
    for p in parts {
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn spawn_errors_render_like_oserror_to_output() {
        let e = SpawnError {
            program: OsString::from("/usr/bin/git"),
            io: io::Error::from_raw_os_error(2),
        };
        let out = Out::from_spawn_error(&e);
        assert_eq!(out.status, 1);
        assert!(out.stderr.is_none());
        let argv = [OsString::from("/usr/bin/git"), OsString::from("write-tree")];
        let Failure::Exc { exc, .. } = out.called_process_error(&argv) else {
            panic!("not an exception")
        };
        assert_eq!(exc.ty, "CalledProcessError");
        if cfg!(unix) {
            assert_eq!(
                String::from_utf8(exc.text).unwrap(),
                "command: ('/usr/bin/git', 'write-tree')\nreturn code: 1\nstdout:\n    [Errno 2] No such file \
                 or directory: '/usr/bin/git'\nstderr: (none)"
            );
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(
            bare_git_argv(&[OsString::from("rev-list")]),
            vec![OsString::from("git"), OsString::from("rev-list")]
        );
        assert_eq!(
            concat(&[OsStr::new("a"), OsStr::new("..."), OsStr::new("b")]),
            OsString::from("a...b")
        );
        assert_eq!(os_from_bytes(b"x y"), OsString::from("x y"));
    }
}
