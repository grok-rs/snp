//! `ScriptedBackend` test seam (feature `testing`): fail any git call by an argv glob, record every call
//! (ARCHITECTURE §4.7 fault-injection matrix FI-01…FI-30, S12).
//!
//! Owner: WP-18.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_proc::spawn::{self, Captured, Group, PyStatus, Role, SpawnError, Spec, Stdin};

use crate::GitBackend;

/// Rules (argv glob → [`Response`]) and a call log.
///
/// A glob is matched against the git arguments after `argv[0]`, joined with single spaces (lossy UTF-8): `*`
/// matches any run of characters (spaces included), `?` exactly one; everything else is literal. Rules added later
/// win; a call no rule matches runs the real git ([`Response::Real`]) with `Role::Tool`.
#[derive(Debug, Default)]
pub struct ScriptedBackend {
    rules: Mutex<Vec<(String, Response)>>,
    calls: Mutex<Vec<Vec<OsString>>>,
    cancel: CancelToken,
}

/// What a scripted call does.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Response {
    /// Run the real git.
    Real,
    /// Exit with this status and output (`stream` writes `stdout` into its sink; `inherit` drops both).
    Exit {
        /// Exit status (Python returncode: negative = signal).
        code: i64,
        /// stdout.
        stdout: Vec<u8>,
        /// stderr.
        stderr: Vec<u8>,
    },
    /// Fail to spawn with this errno.
    SpawnError(i32),
    /// Panic inside the backend (panic-path tests).
    Panic,
}

/// `*`/`?` glob over characters (iterative, linear backtracking on the last `*`).
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0_usize, 0_usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi = pi.saturating_add(1);
            }
            Some(&c) if c == '?' || Some(&c) == t.get(ti) => {
                pi = pi.saturating_add(1);
                ti = ti.saturating_add(1);
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp.saturating_add(1);
                    ti = st.saturating_add(1);
                    star = Some((sp, ti));
                }
                None => return false,
            },
        }
    }
    p.iter().skip(pi).all(|&c| c == '*')
}

impl ScriptedBackend {
    /// No rules: every call is `Real`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a rule; later rules win.
    pub fn on(&self, argv_glob: &str, r: Response) -> &Self {
        snp_core::sync::lock(&self.rules).push((argv_glob.to_owned(), r));
        self
    }

    /// Every argv seen (full, `argv[0]` included), in order.
    pub fn calls(&self) -> Vec<Vec<OsString>> {
        snp_core::sync::lock(&self.calls).clone()
    }

    /// Records the call and picks its response.
    fn respond(&self, argv: &[OsString]) -> Response {
        snp_core::sync::lock(&self.calls).push(argv.to_vec());
        let text = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        snp_core::sync::lock(&self.rules)
            .iter()
            .rev()
            .find(|(g, _)| glob_match(g, &text))
            .map_or(Response::Real, |(_, r)| r.clone())
    }

    fn spec<'a>(
        &'a self,
        argv: &'a [OsString],
        env: &'a Env,
        cwd: &'a Path,
        stdin: Stdin,
        group: Group,
    ) -> Spec<'a> {
        Spec {
            argv,
            env,
            cwd,
            stdin,
            group,
            role: Role::Tool,
            deadline: None,
            cancel: &self.cancel,
        }
    }
}

fn spawn_error(argv: &[OsString], errno: i32) -> SpawnError {
    SpawnError {
        program: argv.first().cloned().unwrap_or_default(),
        io: io::Error::from_raw_os_error(errno),
    }
}

#[expect(
    clippy::panic,
    reason = "Response::Panic exists to inject a panic into the caller (ADR-066 panic-path tests)"
)]
fn scripted_panic(argv: &[OsString]) -> ! {
    panic!("ScriptedBackend: scripted panic for {argv:?}")
}

impl GitBackend for ScriptedBackend {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        match self.respond(argv) {
            Response::Real => spawn::capture(&self.spec(argv, env, cwd, stdin, group)),
            Response::Exit {
                code,
                stdout,
                stderr,
            } => Ok(Captured {
                status: PyStatus(code),
                stdout,
                stderr,
                stopped_by_tty: false,
            }),
            Response::SpawnError(errno) => Err(spawn_error(argv, errno)),
            Response::Panic => scripted_panic(argv),
        }
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        match self.respond(argv) {
            Response::Real => spawn::stream(
                &self.spec(argv, env, cwd, Stdin::Null, Group::Inherit),
                sink,
            ),
            Response::Exit {
                code,
                stdout,
                stderr,
            } => {
                sink.write_all(&stdout)
                    .and_then(|()| sink.flush())
                    .map_err(|io| SpawnError {
                        program: argv.first().cloned().unwrap_or_default(),
                        io,
                    })?;
                Ok((PyStatus(code), stderr))
            }
            Response::SpawnError(errno) => Err(spawn_error(argv, errno)),
            Response::Panic => scripted_panic(argv),
        }
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        match self.respond(argv) {
            Response::Real => spawn::inherit(&self.spec(argv, env, cwd, stdin, Group::Inherit)),
            Response::Exit { code, .. } => Ok(PyStatus(code)),
            Response::SpawnError(errno) => Err(spawn_error(argv, errno)),
            Response::Panic => scripted_panic(argv),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("rev-parse *", "rev-parse --show-cdup"));
        assert!(glob_match(
            "*apply*",
            "-c core.autocrlf=false apply --whitespace=nowarn /p"
        ));
        assert!(glob_match("write-tree", "write-tree"));
        assert!(!glob_match("write-tree", "write-tree x"));
        assert!(glob_match("diff-index * --", "diff-index --binary abc --"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("**x*", "aaxbb"));
        assert!(!glob_match("*x", "aaxbb"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "a"));
    }

    #[test]
    fn rules_and_log() {
        let b = ScriptedBackend::new();
        b.on(
            "write-tree",
            Response::Exit {
                code: 128,
                stdout: Vec::new(),
                stderr: b"boom".to_vec(),
            },
        )
        .on("write-tree", Response::SpawnError(2));
        let env = Env::from_pairs([("PATH", "")]);
        let argv = vec![OsString::from("git"), OsString::from("write-tree")];
        let e = b
            .capture(&argv, &env, Path::new("/"), Stdin::Null, Group::Inherit)
            .unwrap_err();
        assert_eq!(e.io.raw_os_error(), Some(2));
        b.on(
            "write-tree",
            Response::Exit {
                code: 3,
                stdout: b"out".to_vec(),
                stderr: b"err".to_vec(),
            },
        );
        let c = b
            .capture(&argv, &env, Path::new("/"), Stdin::Null, Group::Inherit)
            .unwrap();
        assert_eq!(
            (c.status, c.stdout.as_slice(), c.stderr.as_slice()),
            (PyStatus(3), &b"out"[..], &b"err"[..])
        );
        let mut sink = Vec::new();
        let (st, err) = b.stream(&argv, &env, Path::new("/"), &mut sink).unwrap();
        assert_eq!(
            (st, sink.as_slice(), err.as_slice()),
            (PyStatus(3), &b"out"[..], &b"err"[..])
        );
        assert_eq!(
            b.inherit(&argv, &env, Path::new("/"), Stdin::Null).unwrap(),
            PyStatus(3)
        );
        assert_eq!(b.calls().len(), 4);
        assert_eq!(b.calls()[0], argv);
    }

    #[test]
    fn panics_when_scripted() {
        let b = ScriptedBackend::new();
        b.on("*", Response::Panic);
        let env = Env::from_pairs([("PATH", "")]);
        let argv = vec![OsString::from("git"), OsString::from("status")];
        let r = std::panic::catch_unwind(|| {
            b.capture(&argv, &env, Path::new("/"), Stdin::Null, Group::Inherit)
        });
        assert!(r.is_err());
        assert_eq!(b.calls().len(), 1);
    }
}
