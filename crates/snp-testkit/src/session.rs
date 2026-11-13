//! A command on its own pseudo-terminal that is the session's controlling terminal (ADR-085 tests): the shape an
//! interactive shell gives `git commit` — the command leads a new session in the terminal's foreground process
//! group, so `/dev/tty` opens, reads do not stop it, and ^C written to the master is a kernel SIGINT to that group.
//!
//! The session is started with util-linux `setsid --ctty` (`setsid(2)` + `TIOCSCTTY` on its stdin, then exec), which
//! needs no `pre_exec` (`unsafe_code = forbid`). [`crate::Cmd::session`] returns `None` where it is missing (macOS,
//! BSDs), so callers skip.
//!
//! Owner: WP-18 (ADR-085).

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cmd::Output;
use crate::harness::{OrFail, fail};

/// util-linux `setsid` with `--ctty` support.
pub(crate) fn setsid() -> Option<PathBuf> {
    ["/usr/bin/setsid", "/bin/setsid"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .filter(|p| {
            Command::new(p)
                .arg("--help")
                .stdin(Stdio::null())
                .output()
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("--ctty"))
        })
}

/// A running command on a pty it controls.
#[derive(Debug)]
pub struct Session {
    master: File,
    out: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    child: Child,
}

/// Starts `cmd` (program, args, env, cwd already applied) as the leader of a new session on a fresh pty; with
/// `stdin_null` the program's own stdin is `/dev/null` (a git hook's), while `/dev/tty` stays the pty.
pub(crate) fn start(
    program: &Path,
    args: &[std::ffi::OsString],
    env: &snp_core::env::Env,
    cwd: &Path,
    stdin_null: bool,
) -> Option<Session> {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::{Winsize, tcsetwinsize};

    let setsid = setsid()?;
    let master = {
        let _guard = snp_core::spawnlock::guard();
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).or_fail("openpt");
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC).or_fail("FD_CLOEXEC");
        master
    };
    grantpt(&master).or_fail("grantpt");
    unlockpt(&master).or_fail("unlockpt");
    let name = ptsname(&master, Vec::new()).or_fail("ptsname");
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .or_fail("opening the pty slave");
    let size = Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    tcsetwinsize(&slave, size).or_fail("TIOCSWINSZ");
    let slave = File::from(slave);
    let mut cmd = Command::new(setsid);
    cmd.arg("--ctty");
    if stdin_null {
        cmd.args(["/bin/sh", "-c", "exec \"$0\" \"$@\" </dev/null"]);
    }
    cmd.arg(program).args(args).current_dir(cwd);
    env.apply_to(&mut cmd);
    cmd.stdin(Stdio::from(
        slave.try_clone().or_fail("dup of the pty slave"),
    ))
    .stdout(Stdio::from(
        slave.try_clone().or_fail("dup of the pty slave"),
    ))
    .stderr(Stdio::from(slave));
    let child = cmd.spawn().or_fail("spawning setsid --ctty");
    // the parent's copies of the slave must close, or the master never reports EOF
    drop(cmd);
    let master = File::from(master);
    let mut reader_end = master.try_clone().or_fail("dup of the pty master");
    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&out);
    let reader = std::thread::Builder::new()
        .name("snp-testkit-pty".to_owned())
        .spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match reader_end.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut o) = sink.lock() {
                            o.extend_from_slice(buf.get(..n).unwrap_or_default());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    // EIO: every slave handle is closed (Linux); the same as EOF
                    Err(_) => break,
                }
            }
        })
        .or_fail("spawning the pty reader");
    Some(Session {
        master,
        out,
        reader: Some(reader),
        child,
    })
}

impl Session {
    /// Everything the terminal showed so far, `\r\n` folded to `\n`.
    pub fn transcript(&self) -> String {
        let raw = self.out.lock().map_or_default(|o| o.clone());
        String::from_utf8_lossy(&raw).replace("\r\n", "\n")
    }

    /// Waits until the transcript contains `needle`; fails the test with the transcript after `timeout`.
    pub fn expect(&self, needle: &str, timeout: Duration) {
        let end = Instant::now().checked_add(timeout);
        loop {
            if self.transcript().contains(needle) {
                return;
            }
            if end.is_none_or(|e| Instant::now() >= e) {
                fail(&format!(
                    "timed out waiting for {needle:?}; terminal so far:\n{}",
                    self.transcript()
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Types `bytes` on the terminal (`\x03` is ^C, delivered by the line discipline as SIGINT).
    pub fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).or_fail("writing to the pty");
        self.master.flush().or_fail("flushing the pty");
    }

    /// Waits for the command (killed and failed after `timeout`) and for its output; the transcript is the stdout.
    pub fn wait(mut self, timeout: Duration) -> Output {
        let end = Instant::now().checked_add(timeout);
        let status = loop {
            if let Some(st) = self.child.try_wait().or_fail("waiting for the session") {
                break st;
            }
            if end.is_none_or(|e| Instant::now() >= e) {
                let _killed = self.child.kill();
                let _reaped = self.child.wait();
                fail(&format!(
                    "the session did not end within {timeout:?}; terminal:\n{}",
                    self.transcript()
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // the reader ends once every slave handle is closed (a leftover descendant may hold one: bounded)
        let drain = Instant::now().checked_add(Duration::from_secs(5));
        while self.reader.as_ref().is_some_and(|r| !r.is_finished())
            && drain.is_some_and(|d| Instant::now() < d)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(r) = self.reader.take()
            && r.is_finished()
        {
            let _joined = r.join();
        }
        Output {
            code: crate::cmd::exit_code(status),
            stdout: self.transcript().into_bytes(),
            stderr: Vec::new(),
        }
    }
}
