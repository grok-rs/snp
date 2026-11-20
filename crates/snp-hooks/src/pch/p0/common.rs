//! Helpers shared by the P0 natives and their builtin variants: argument parsing in the hook's context, file I/O
//! with Python's exception texts, and `pre_commit_hooks.util.cmd_output` over [`snp_git::pch_facts::PchFacts`].
//!
//! Owner: WP-26.

use std::ffi::{OsStr, OsString};

use snp_argparse::{Namespace, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;
use snp_pycompat::codec::decode_utf8;
use snp_pycompat::repr::{repr_str, repr_str_tuple};

use crate::content_cache::Bytes;
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

/// `open(name, 'rb').read()` relative to the hook's cwd: through the window's content cache when there is one
/// (ADR-131: a validated hit, else a fill), else `std::fs::read`.
pub(crate) fn read_all(cx: &HookCx<'_>, name: &OsStr) -> Result<Bytes, PyExc> {
    let path = framework::at(cx, name);
    match cache(cx) {
        Some(c) => c.read(name, &path),
        None => std::fs::read(path).map(Bytes::Owned),
    }
    .map_err(|e| io_exc(&e, name))
}

/// The window's content cache when this hook may use it (its cwd is the cache's root, ADR-131).
pub(crate) fn cache<'a>(cx: &HookCx<'a>) -> Option<&'a crate::ContentCache> {
    cx.cache.and_then(|c| c.view(cx.root))
}

/// The leading bytes [`read_head`] read, held inline (at most [`Head::CAP`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Head {
    buf: [u8; Self::CAP],
    len: usize,
}

impl Head {
    /// The most bytes a probe asks for (the callers ask for 2 or 3).
    pub(crate) const CAP: usize = 4;

    /// The bytes read.
    pub(crate) fn bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    /// At most `n` (clamped to [`Head::CAP`]) leading bytes of `r`: `read_to_end` over `take(n)` without the heap
    /// (`read` until `n` bytes or end of file, an interrupted read retried).
    fn read_from(mut r: impl std::io::Read, n: usize) -> std::io::Result<Self> {
        let mut head = Self {
            buf: [0; Self::CAP],
            len: 0,
        };
        let want = n.min(Self::CAP);
        while let Some(rest) = head.buf.get_mut(head.len..want)
            && !rest.is_empty()
        {
            match r.read(rest) {
                Ok(0) => break,
                Ok(k) => head.len = head.len.saturating_add(k).min(want),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(head)
    }
}

/// `open(name, 'rb').read(n)`: at most `n` leading bytes (`n` ≤ [`Head::CAP`]). A validated cache hit serves them
/// (ADR-131) through the same bounded read; a miss reads the file as before and fills nothing.
pub(crate) fn read_head(cx: &HookCx<'_>, name: &OsStr, n: usize) -> Result<Head, PyExc> {
    let path = framework::at(cx, name);
    if let Some(b) = cache(cx).and_then(|c| c.hit(name, &path)) {
        return Head::read_from(b.as_slice(), n).map_err(|e| io_exc(&e, name));
    }
    let f = std::fs::File::open(path).map_err(|e| io_exc(&e, name))?;
    Head::read_from(f, n).map_err(|e| io_exc(&e, name))
}

/// `open(name, 'wb').write(bytes)` on the same inode (09 §2.8), through the hook's write token (ADR-132: the first
/// write of a gated hook takes the baseline `git diff` first). The name's cache entry is evicted first, whatever the
/// write then does (ADR-131).
pub(crate) fn rewrite(cx: &HookCx<'_>, name: &OsStr, bytes: &[u8]) -> Result<(), PyExc> {
    cx.writer()?
        .rewrite(name, bytes)
        .map_err(|e| io_exc(&e, name))
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

    /// A reader that returns its chunks one `read` at a time (a chunk longer than the buffer is split), an
    /// `Interrupted` error before each, then end of file (or `fail` once the chunks run out).
    struct Chunky {
        chunks: std::collections::VecDeque<Vec<u8>>,
        interrupt: bool,
        fail: bool,
        reads: usize,
    }

    impl std::io::Read for Chunky {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            if self.interrupt {
                self.interrupt = false;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            self.interrupt = true;
            let Some(c) = self.chunks.pop_front() else {
                return if self.fail {
                    Err(std::io::Error::other("boom"))
                } else {
                    Ok(0)
                };
            };
            let k = c.len().min(buf.len());
            for (d, s) in buf.iter_mut().zip(&c) {
                *d = *s;
            }
            if c.len() > k {
                self.chunks.push_front(c[k..].to_vec());
            }
            Ok(k)
        }
    }

    fn chunky(chunks: &[&[u8]], fail: bool) -> Chunky {
        Chunky {
            // an empty `read` is end of file: empty chunks are not reads
            chunks: chunks
                .iter()
                .filter(|c| !c.is_empty())
                .map(|c| c.to_vec())
                .collect(),
            interrupt: true,
            fail,
            reads: 0,
        }
    }

    /// `Head::read_from` is `read_to_end` over `take(n)`: short and empty files, reads split anywhere, interrupted
    /// reads retried, errors kept, `n` clamped to the inline capacity.
    #[test]
    fn read_head_is_read_to_end_over_take() {
        let data: &[u8] = b"\xef\xbb\xbfrest";
        for len in 0..=data.len() {
            for split in 0..=len {
                for n in 0..=6_usize {
                    let mut want = Vec::new();
                    std::io::Read::read_to_end(
                        &mut std::io::Read::take(&data[..len], u64::try_from(n).unwrap()),
                        &mut want,
                    )
                    .unwrap();
                    want.truncate(Head::CAP);
                    let got =
                        Head::read_from(chunky(&[&data[..split], &data[split..len]], false), n)
                            .unwrap();
                    assert_eq!(
                        got.bytes(),
                        want.as_slice(),
                        "len {len} split {split} n {n}"
                    );
                }
            }
        }
        // an error before `n` bytes is the error; once `n` bytes are in, nothing more is read
        assert!(Head::read_from(chunky(&[b"#"], true), 2).is_err());
        let mut r = chunky(&[b"#!", b"/bin/sh"], true);
        assert_eq!(Head::read_from(&mut r, 2).unwrap().bytes(), b"#!");
        assert_eq!(r.reads, 2);
        assert_eq!(Head::read_from(chunky(&[], false), 3).unwrap().bytes(), b"");
    }
}
