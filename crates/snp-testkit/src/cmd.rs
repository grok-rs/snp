//! A process invocation with the hermetic env (10 §7.3.2): never the parent's environment, never a `PATH` lookup of
//! the program, an explicit cwd.
//!
//! Owner: WP-18.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use snp_core::env::{Env, EnvPatch};

use crate::harness::{OrFail, fail};

/// A process invocation with the hermetic env.
#[derive(Debug)]
pub struct Cmd {
    program: PathBuf,
    args: Vec<OsString>,
    env: Env,
    cwd: PathBuf,
    stdin: Vec<u8>,
    tty: bool,
}

impl Cmd {
    /// `program` (an absolute path) with `env` in `cwd`.
    pub(crate) fn new(program: PathBuf, env: Env, cwd: &Path) -> Self {
        Self {
            program,
            args: Vec::new(),
            env,
            cwd: cwd.to_path_buf(),
            stdin: Vec::new(),
            tty: false,
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, a: impl AsRef<OsStr>) -> Self {
        self.args.push(a.as_ref().to_os_string());
        self
    }

    /// Appends arguments.
    #[must_use]
    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, a: I) -> Self {
        self.args
            .extend(a.into_iter().map(|s| s.as_ref().to_os_string()));
        self
    }

    /// Sets a variable.
    #[must_use]
    pub fn env(mut self, k: &str, v: impl AsRef<OsStr>) -> Self {
        self.env = self
            .env
            .patched(&EnvPatch::default().set(k.to_owned(), v.as_ref().to_os_string()));
        self
    }

    /// Removes a variable.
    #[must_use]
    pub fn env_remove(mut self, k: &str) -> Self {
        self.env = self.env.without(&[k]);
        self
    }

    /// stdin bytes (default: empty).
    #[must_use]
    pub fn stdin(mut self, bytes: &[u8]) -> Self {
        self.stdin = bytes.to_vec();
        self
    }

    /// stdout/stderr on a pty (posix): both land in [`Output::stdout`] in arrival order; `OPOST`/`ONLCR` are
    /// cleared so the bytes are the program's own. A no-op on Windows.
    #[must_use]
    pub fn tty(mut self) -> Self {
        self.tty = true;
        self
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        self.env.apply_to(&mut cmd);
        cmd.args(&self.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped());
        cmd
    }

    /// Runs to completion.
    pub fn output(self) -> Output {
        #[cfg(unix)]
        if self.tty {
            return self.output_pty();
        }
        #[cfg(not(unix))]
        let _no_pty: bool = self.tty;
        let mut cmd = self.command();
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .or_fail(&format!("spawning {}", self.program.display()));
        drop(cmd);
        let feeder = feed(&mut child, self.stdin);
        let out = child.wait_with_output().or_fail("waiting for the child");
        join(feeder);
        Output {
            code: exit_code(out.status),
            stdout: out.stdout,
            stderr: out.stderr,
        }
    }

    #[cfg(unix)]
    fn output_pty(self) -> Output {
        use rustix::fs::{Mode, OFlags};
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        use rustix::termios::{OptionalActions, OutputModes, tcgetattr, tcsetattr};

        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
            .or_fail("openpt");
        grantpt(&master).or_fail("grantpt");
        unlockpt(&master).or_fail("unlockpt");
        let name = ptsname(&master, Vec::new()).or_fail("ptsname");
        let slave = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .or_fail("opening the pty slave");
        let mut t = tcgetattr(&slave).or_fail("tcgetattr");
        t.output_modes
            .remove(OutputModes::OPOST | OutputModes::ONLCR);
        tcsetattr(&slave, OptionalActions::Now, &t).or_fail("tcsetattr");
        let slave = std::fs::File::from(slave);
        let mut cmd = self.command();
        cmd.stdout(Stdio::from(
            slave.try_clone().or_fail("dup of the pty slave"),
        ))
        .stderr(Stdio::from(slave));
        let mut child = cmd
            .spawn()
            .or_fail(&format!("spawning {}", self.program.display()));
        // The parent's copies of the slave must close, or the master never reports EOF.
        drop(cmd);
        let feeder = feed(&mut child, self.stdin);
        let mut master = std::fs::File::from(master);
        let mut merged = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            match master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => merged.extend_from_slice(buf.get(..n).unwrap_or_default()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                // EIO: every slave handle is closed (Linux); the same as EOF.
                Err(_) => break,
            }
        }
        let status = child.wait().or_fail("waiting for the child");
        join(feeder);
        Output {
            code: exit_code(status),
            stdout: merged,
            stderr: Vec::new(),
        }
    }
}

type Feeder = Option<std::thread::JoinHandle<()>>;

/// Writes `bytes` to the child's stdin on a thread and closes it (a child that never reads gets EPIPE: ignored).
fn feed(child: &mut std::process::Child, bytes: Vec<u8>) -> Feeder {
    let mut w = child.stdin.take()?;
    std::thread::Builder::new()
        .name("snp-testkit-stdin".to_owned())
        .spawn(move || {
            let _ignored: std::io::Result<()> = w.write_all(&bytes);
        })
        .ok()
}

fn join(feeder: Feeder) {
    if let Some(h) = feeder
        && h.join().is_err()
    {
        fail("the stdin feeder thread panicked");
    }
}

/// The exit code; Unix signal deaths as `128 + n` (shell convention).
fn exit_code(status: ExitStatus) -> i32 {
    if let Some(c) = status.code() {
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return sig.saturating_add(128);
        }
    }
    -1
}

/// Exit code and output of a [`Cmd`].
#[derive(Debug)]
pub struct Output {
    /// Exit code (signal deaths as `128 + n` on Unix).
    pub code: i32,
    /// stdout bytes (with [`Cmd::tty`]: stdout and stderr merged).
    pub stdout: Vec<u8>,
    /// stderr bytes.
    pub stderr: Vec<u8>,
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Cmd {
        Cmd::new(
            PathBuf::from("/bin/sh"),
            Env::from_pairs([("PATH", "/usr/bin:/bin"), ("KEEP", "1"), ("DROP", "1")]),
            Path::new("/"),
        )
        .args(["-c", script, "sh"])
    }

    #[test]
    fn pipes_env_cwd_stdin_and_codes() {
        let out =
            sh("printf '%s|%s|%s' \"$KEEP\" \"${DROP-unset}\" \"$NEW\"; cat; pwd >&2; exit 7")
                .env("NEW", "n")
                .env_remove("DROP")
                .stdin(b"<in>")
                .output();
        assert_eq!(out.code, 7);
        assert_eq!(out.stdout, b"1|unset|n<in>");
        assert_eq!(out.stderr, b"/\n");
        assert_eq!(sh("kill -9 $$").output().code, 137);
    }

    #[test]
    fn tty_merges_streams_on_a_pty() {
        let out = sh(
            "[ -t 1 ] && echo out-tty; [ -t 2 ] && echo err-tty >&2; [ -t 0 ] || echo stdin-pipe",
        )
        .arg("x")
        .tty()
        .output();
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"out-tty\nerr-tty\nstdin-pipe\n");
        assert_eq!(out.stderr, b"");
    }
}
