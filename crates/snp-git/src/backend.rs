//! The `GitBackend` seam (S12) and its snp-proc implementation (ADR-065).
//!
//! Owner: WP-18.

use std::ffi::OsString;
use std::fmt::Debug;
use std::io;
use std::path::Path;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_proc::spawn::{self, Captured, Group, PyStatus, Role, SpawnError, Spec, Stdin};

/// Seam (S12): real = snp-proc; tests = `testing::ScriptedBackend` (fail any call by argv pattern, record calls).
///
/// `argv[0]` is the resolved git (`GitExe::argv0`); the backend never resolves or rewrites argv.
pub trait GitBackend: Send + Sync + Debug {
    /// `spawn::capture`: stdout and stderr captured separately (`cmd_output_b`, `util.py:81-113`).
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError>;
    /// `spawn::stream`: stdout streamed into `sink`, stderr captured (stash patch, diff fingerprint).
    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError>;
    /// `spawn::inherit`: stdio inherited (`subprocess.call`: show-diff, the pre-push `cat-file -e` probe).
    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError>;
}

/// `role` is used for every spawn: the run's backend uses `Role::Tool` with the run token; the stash builds its own
/// backend with `Role::Critical` and a fresh token that is never cancelled (ADR-065).
#[derive(Debug)]
pub struct ProcBackend {
    /// Cancellation of Hook/Tool spawns (Critical spawns ignore it).
    pub cancel: CancelToken,
    /// The role of every spawn.
    pub role: Role,
}

impl ProcBackend {
    /// A Critical command always runs in its own process group (§3.6: "requires `Group::Isolated`").
    fn group(&self, requested: Group) -> Group {
        if self.role == Role::Critical {
            Group::Isolated
        } else {
            requested
        }
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
            group: self.group(group),
            role: self.role,
            deadline: None,
            cancel: &self.cancel,
        }
    }
}

impl GitBackend for ProcBackend {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        spawn::capture(&self.spec(argv, env, cwd, stdin, group))
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        spawn::stream(
            &self.spec(argv, env, cwd, Stdin::Null, Group::Inherit),
            sink,
        )
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        spawn::inherit(&self.spec(argv, env, cwd, stdin, Group::Inherit))
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<OsString> {
        ["/bin/sh", "-c", script]
            .iter()
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn critical_forces_an_isolated_group_and_disables_prompts() {
        let b = ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Critical,
        };
        assert_eq!(b.group(Group::Inherit), Group::Isolated);
        let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
        let out = b
            .capture(
                &sh("printf '%s' \"$GIT_TERMINAL_PROMPT\"; test \"$(ps -o pgid= $$)\" -eq $$ && echo :own"),
                &env,
                Path::new("/"),
                Stdin::Null,
                Group::Inherit,
            )
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0:own");
    }

    #[test]
    fn tool_role_streams_and_inherits() {
        let b = ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        };
        assert_eq!(b.group(Group::Inherit), Group::Inherit);
        let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
        let mut sink = Vec::new();
        let (st, err) = b
            .stream(
                &sh("printf out; printf err >&2; exit 3"),
                &env,
                Path::new("/"),
                &mut sink,
            )
            .unwrap();
        assert_eq!(
            (st, sink.as_slice(), err.as_slice()),
            (PyStatus(3), &b"out"[..], &b"err"[..])
        );
        let st = b
            .inherit(&sh("exit 5"), &env, Path::new("/"), Stdin::Null)
            .unwrap();
        assert_eq!(st, PyStatus(5));
    }
}
