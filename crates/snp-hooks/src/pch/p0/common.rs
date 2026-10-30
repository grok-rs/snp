//! Helpers shared by the P0 natives and their builtin variants: argument parsing in the hook's context, file I/O
//! with Python's exception texts, and `pre_commit_hooks.util.cmd_output` over [`snp_git::pch_facts::PchFacts`].
//!
//! Owner: WP-26.

use std::ffi::{OsStr, OsString};
use std::io::Read as _;

use snp_argparse::{Namespace, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;
use snp_pycompat::codec::decode_utf8;
use snp_pycompat::repr::{repr_str, repr_str_tuple};

use crate::framework::{self, PyOut, io_exc};
use crate::{HookCx, Outcome};

/// `Ok` when `epoch` is one this native reproduces (`1..=max`), else the reason the caller delegates.
pub(crate) fn epoch_in(epoch: Epoch, max: u16) -> Result<(), String> {
    if (1..=max).contains(&epoch.0) {
        Ok(())
    } else {
        Err(format!(
            "epoch {} is not reproduced natively (supported: 1..={max})",
            epoch.0
        ))
    }
}

/// [`framework::parse_args`] with the hook context's columns and environment.
pub(crate) fn parse(
    cx: &HookCx<'_>,
    spec: &'static ParserSpec,
    prog: &str,
    argv: &[&OsStr],
) -> Result<Namespace, Outcome> {
    framework::parse_args(spec, prog, argv, cx.columns, cx.env)
}

/// `parse_args` over `args` alone (preflight: `None` when argparse would reject them; that error is emulated, not
/// delegated).
pub(crate) fn parse_static(
    spec: &'static ParserSpec,
    prog: &str,
    args: &[std::sync::Arc<str>],
) -> Option<Namespace> {
    let env = snp_core::env::Env::from_pairs(Vec::<(OsString, OsString)>::new());
    let argv: Vec<&OsStr> = args.iter().map(|a| OsStr::new(&**a)).collect();
    framework::parse_args(spec, prog, &argv, 80, &env).ok()
}

/// The `filenames` positional.
pub(crate) fn filenames(ns: &Namespace) -> Vec<&OsStr> {
    ns.list("filenames")
        .iter()
        .map(OsString::as_os_str)
        .collect()
}

/// `open(name, 'rb').read()` relative to the hook's cwd.
pub(crate) fn read_all(cx: &HookCx<'_>, name: &OsStr) -> Result<Vec<u8>, PyExc> {
    std::fs::read(framework::at(cx, name)).map_err(|e| io_exc(&e, name))
}

/// `open(name, 'rb').read(n)`: at most `n` leading bytes.
pub(crate) fn read_head(cx: &HookCx<'_>, name: &OsStr, n: u64) -> Result<Vec<u8>, PyExc> {
    let f = std::fs::File::open(framework::at(cx, name)).map_err(|e| io_exc(&e, name))?;
    let mut buf = Vec::new();
    f.take(n)
        .read_to_end(&mut buf)
        .map_err(|e| io_exc(&e, name))?;
    Ok(buf)
}

/// `open(name, 'wb').write(bytes)` on the same inode (09 §2.8, `snp_os::fs::rewrite_in_place`).
pub(crate) fn rewrite(cx: &HookCx<'_>, name: &OsStr, bytes: &[u8]) -> Result<(), PyExc> {
    snp_os::fs::rewrite_in_place(&framework::at(cx, name), bytes).map_err(|e| io_exc(&e, name))
}

/// `print(prefix + name + suffix)` with the file name as raw bytes.
pub(crate) fn print_name(out: &mut PyOut, prefix: &[u8], name: &OsStr, suffix: &[u8]) {
    let mut line = prefix.to_vec();
    line.extend_from_slice(framework::os_bytes(name));
    line.extend_from_slice(suffix);
    out.print_bytes_text(&line);
}

/// `repr(bytes)`.
pub(crate) fn repr_bytes(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::from("b");
    out.push(char::from(quote));
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(char::from(c));
            }
            0x20..=0x7e => out.push(char::from(c)),
            c => {
                use std::fmt::Write as _;
                let _infallible: std::fmt::Result = write!(out, "\\x{c:02x}");
            }
        }
    }
    out.push(char::from(quote));
    out
}

/// `pre_commit_hooks.util.cmd_output('git', *args, retcode=retcode)` (`util.py:16-24`): stdout decoded as
/// strict UTF-8 (a `UnicodeDecodeError` comes first), then `retcode` checked (`None` = unchecked), raising
/// pre-commit-hooks' own `CalledProcessError(cmd, retcode, returncode, stdout, stderr)`.
pub(crate) fn cmd_output(
    cx: &HookCx<'_>,
    args: &[&OsStr],
    retcode: Option<i64>,
) -> Result<String, PyExc> {
    let c = cx.facts.raw(args).map_err(framework::failure_exc)?;
    let stdout = decode_utf8(&c.stdout)
        .map_err(|e| PyExc::new(e.py_type(), e.to_string()))?
        .to_owned();
    if let Some(want) = retcode
        && c.status.0 != want
    {
        let mut cmd: Vec<String> = vec!["git".into()];
        cmd.extend(args.iter().map(|a| a.to_string_lossy().into_owned()));
        return Err(PyExc::new(
            "pre_commit_hooks.util.CalledProcessError",
            format!(
                "({}, {want}, {}, {}, {})",
                repr_str_tuple(&cmd),
                c.status.0,
                repr_str(&stdout),
                repr_bytes(&c.stderr)
            ),
        ));
    }
    Ok(stdout)
}

/// `str.rstrip()` (PY-STR-WS).
pub(crate) fn py_rstrip(s: &str) -> &str {
    snp_pycompat::text::rstrip(s)
}

/// The class a [`snp_git::pch_facts::PchFacts`] failure prints as, module-qualified the way the hook's traceback
/// names it (`util.cmd_output` raises `pre_commit_hooks.util.CalledProcessError`; `subprocess.run(check=True)`
/// raises `subprocess.CalledProcessError`).
pub(crate) fn facts_exc(f: snp_core::failure::Failure, subprocess: bool) -> PyExc {
    let mut exc = framework::failure_exc(f);
    if exc.ty == "CalledProcessError" {
        exc.ty = if subprocess {
            "subprocess.CalledProcessError".into()
        } else {
            "pre_commit_hooks.util.CalledProcessError".into()
        };
    }
    exc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_repr() {
        assert_eq!(repr_bytes(b""), "b''");
        assert_eq!(repr_bytes(b"fatal: x\n"), "b'fatal: x\\n'");
        assert_eq!(repr_bytes(b"it's"), "b\"it's\"");
        assert_eq!(repr_bytes(b"\x00\xff"), "b'\\x00\\xff'");
    }

    #[test]
    fn epochs() {
        assert!(epoch_in(Epoch(1), 3).is_ok());
        assert!(epoch_in(Epoch(3), 3).is_ok());
        assert!(epoch_in(Epoch(0), 3).is_err());
        assert!(epoch_in(Epoch(4), 3).is_err());
    }
}
