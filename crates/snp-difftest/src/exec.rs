//! Spawning the tools: cleared environment (10 §7.3.2), explicit cwd, stdin bytes, both streams captured
//! concurrently, a wall-clock deadline, an optional pty (posix) and an optional signal delivered mid-run.
//!
//! Every failure is an `Err(String)`; nothing here panics (the harness is a tool, but a crash would hide the
//! divergence it was looking for).
//!
//! Owner: WP-35.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use snp_core::env::Env;

/// What one process produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Captured {
    /// stdout bytes (stdout + stderr merged in arrival order under a pty).
    pub stdout: Vec<u8>,
    /// stderr bytes (empty under a pty).
    pub stderr: Vec<u8>,
    /// The exit code; a signal death is `-signum` (Python's `returncode`); `None` when killed at the deadline.
    pub exit: Option<i32>,
    /// The deadline expired and the process was killed.
    pub timed_out: bool,
    /// A stream exceeded 64 MiB (`MAX_CAPTURE`): the process (group) was killed and the kept prefix is not the
    /// whole output. The runner reports this as a difference, never as equality.
    pub truncated: bool,
}

/// Most bytes kept per captured stream (64 MiB). A tool stuck in a print loop is killed once a stream reaches it,
/// so the harness's memory stays bounded however long the step deadline is.
pub(crate) const MAX_CAPTURE: usize = 64 * 1024 * 1024;

/// How long the reader threads may still run once the process group was killed (a grandchild that left the group
/// can hold the pipe open forever; its output is then abandoned).
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// One invocation.
#[derive(Debug, Clone)]
pub(crate) struct Invocation {
    /// Absolute program path (never looked up on `PATH` by the harness itself, except `git`/`sh` resolved once).
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<OsString>,
    /// The complete child environment.
    pub env: Env,
    /// The working directory.
    pub cwd: PathBuf,
    /// stdin bytes.
    pub stdin: Vec<u8>,
    /// Run under a pty.
    pub tty: bool,
    /// Kill after this long.
    pub timeout: Duration,
    /// Deliver a signal (posix).
    pub signal: Option<(i32, Trigger)>,
}

/// When [`Invocation::signal`] fires.
#[derive(Debug, Clone)]
pub(crate) struct Trigger {
    /// Wait for this file to exist first (a hook's marker).
    pub(crate) file: Option<PathBuf>,
    /// Then wait this long.
    pub(crate) delay: Duration,
}

/// The default per-step deadline.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs `inv` to completion (or its deadline). On posix the child leads its own process group, so a deadline or an
/// output overflow kills every process it started (hooks included), and the readers' joins are bounded too.
pub(crate) fn run(inv: &Invocation) -> Result<Captured, String> {
    #[cfg(unix)]
    if inv.tty {
        return pty::run(inv);
    }
    let mut cmd = Command::new(&inv.program);
    inv.env.apply_to(&mut cmd);
    cmd.args(&inv.args)
        .current_dir(&inv.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    own_group(&mut cmd);
    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawning {}: {e}", inv.program.display()))?;
    let overflow = Arc::new(AtomicBool::new(false));
    let feeder = feed(&mut child, inv.stdin.clone())?;
    let out_reader = reader(child.stdout.take(), MAX_CAPTURE, &overflow, false)?;
    let err_reader = reader(child.stderr.take(), MAX_CAPTURE, &overflow, false)?;
    let w = wait(&mut child, start, inv, &overflow)?;
    let finish = Finish::new(&w, start, inv.timeout, child.id());
    let (stdout, t1) = finish.bytes(out_reader)?;
    let (stderr, t2) = finish.bytes(err_reader)?;
    finish.unit(feeder);
    Ok(Captured {
        stdout,
        stderr,
        exit: w.exit,
        timed_out: w.timed_out || finish.late(),
        truncated: t1 || t2 || overflow.load(Ordering::Relaxed),
    })
}

/// Puts the child in a new process group (posix) so the whole tree can be killed.
fn own_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        // Windows: only the direct child is killed (no job object); documented in tests/differential/README.md.
        let _: &mut Command = cmd;
    }
}

/// SIGKILL to the process group led by `pid` (posix); errors (an already empty group) are ignored.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    if let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ignored: rustix::io::Result<()> =
            rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    #[cfg(not(unix))]
    {
        let _: u32 = pid;
    }
}

/// A reader's result: the kept bytes and whether more were discarded.
type BytesHandle = JoinHandle<std::io::Result<(Vec<u8>, bool)>>;

/// Reads `src` to EOF keeping at most `cap` bytes; past the cap it raises `overflow` (the waiter then kills the
/// process group) and keeps draining into nothing so the writer never blocks. `eof_on_error`: a pty master reports
/// the slave's close as `EIO`.
fn capture<R: Read>(
    src: &mut R,
    cap: usize,
    overflow: &AtomicBool,
    eof_on_error: bool,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut truncated = false;
    loop {
        let n = match src.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) if eof_on_error => break,
            Err(e) => return Err(e),
        };
        let data = chunk.get(..n).unwrap_or_default();
        let room = cap.saturating_sub(buf.len());
        if data.len() > room {
            buf.extend_from_slice(data.get(..room).unwrap_or_default());
            if !truncated {
                truncated = true;
                overflow.store(true, Ordering::Relaxed);
            }
        } else {
            buf.extend_from_slice(data);
        }
    }
    Ok((buf, truncated))
}

fn reader<R: Read + Send + 'static>(
    src: Option<R>,
    cap: usize,
    overflow: &Arc<AtomicBool>,
    eof_on_error: bool,
) -> Result<Option<BytesHandle>, String> {
    let Some(mut src) = src else { return Ok(None) };
    let overflow = Arc::clone(overflow);
    std::thread::Builder::new()
        .name(String::from("difftest-reader"))
        .spawn(move || capture(&mut src, cap, &overflow, eof_on_error))
        .map(Some)
        .map_err(|e| format!("starting a reader thread: {e}"))
}

/// Bounded joins of the helper threads once the child itself is gone. While the step's deadline has not passed
/// they may take until then (a grandchild still writing); then the group is killed, and after [`DRAIN_GRACE`] a
/// thread still blocked (a process that left the group holds the pipe) is abandoned.
struct Finish {
    /// Join until this instant before killing the group.
    soft: Instant,
    /// Whether the group was already killed (deadline or overflow).
    killed: std::cell::Cell<bool>,
    /// Whether any thread had to be waited for past the deadline or abandoned.
    late: std::cell::Cell<bool>,
    pid: u32,
}

impl Finish {
    fn new(w: &Waited, start: Instant, timeout: Duration, pid: u32) -> Self {
        let soft = if w.killed {
            Instant::now()
        } else {
            start.checked_add(timeout).unwrap_or_else(Instant::now)
        };
        Self {
            soft,
            killed: std::cell::Cell::new(w.killed),
            late: std::cell::Cell::new(false),
            pid,
        }
    }

    fn late(&self) -> bool {
        self.late.get()
    }

    /// Waits for `h` to finish; `None` when it had to be abandoned.
    fn join<T>(&self, h: JoinHandle<T>) -> Option<std::thread::Result<T>> {
        while !h.is_finished() && Instant::now() < self.soft {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !h.is_finished() && !self.killed.get() {
            // Past the deadline with the pipe still open: kill whatever of the tree is left.
            self.late.set(true);
            kill_group(self.pid);
            self.killed.set(true);
        }
        let hard = Instant::now().checked_add(DRAIN_GRACE);
        while !h.is_finished() && hard.is_some_and(|t| Instant::now() < t) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if h.is_finished() {
            Some(h.join())
        } else {
            // Detached: it ends when the last writer closes the pipe; it keeps at most MAX_CAPTURE bytes.
            self.late.set(true);
            None
        }
    }

    /// A reader's bytes and whether they are incomplete.
    fn bytes(&self, h: Option<BytesHandle>) -> Result<(Vec<u8>, bool), String> {
        let Some(h) = h else {
            return Ok((Vec::new(), false));
        };
        match self.join(h) {
            None => Ok((Vec::new(), true)),
            Some(Ok(Ok(r))) => Ok(r),
            Some(Ok(Err(e))) => Err(format!("reading child output: {e}")),
            Some(Err(_)) => Err(String::from("a reader thread panicked")),
        }
    }

    fn unit(&self, h: Option<JoinHandle<()>>) {
        if let Some(h) = h {
            drop(self.join(h));
        }
    }
}

fn feed(child: &mut Child, bytes: Vec<u8>) -> Result<Option<JoinHandle<()>>, String> {
    let Some(mut stdin) = child.stdin.take() else {
        return Ok(None);
    };
    std::thread::Builder::new()
        .name(String::from("difftest-stdin"))
        .spawn(move || {
            // A child that exits without reading its stdin gives EPIPE: not an error of the harness.
            drop(stdin.write_all(&bytes));
            drop(stdin);
        })
        .map(Some)
        .map_err(|e| format!("starting the stdin thread: {e}"))
}

fn status_code(status: std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Some(sig.checked_neg().unwrap_or(i32::MIN));
        }
    }
    status.code()
}

/// How the child ended.
struct Waited {
    exit: Option<i32>,
    timed_out: bool,
    /// The process group was killed (deadline or output overflow).
    killed: bool,
}

/// Polls the child until it exits, the deadline passes or a stream overflows; the latter two kill the whole
/// process group (before the child is reaped, so its pid cannot have been reused).
fn wait(
    child: &mut Child,
    start: Instant,
    inv: &Invocation,
    overflow: &AtomicBool,
) -> Result<Waited, String> {
    let mut signalled = false;
    // When the marker file appeared (the delay counts from there).
    let mut armed: Option<Instant> = None;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("waiting for the child: {e}"))?
        {
            return Ok(Waited {
                exit: status_code(status),
                timed_out: false,
                killed: false,
            });
        }
        let elapsed = start.elapsed();
        if let Some((sig, trigger)) = inv.signal.as_ref()
            && !signalled
        {
            if armed.is_none() && trigger.file.as_deref().is_none_or(Path::exists) {
                armed = Some(Instant::now());
            }
            if armed.is_some_and(|t| t.elapsed() >= trigger.delay) {
                send_signal(child.id(), *sig)?;
                signalled = true;
            }
        }
        let timed_out = elapsed >= inv.timeout;
        if timed_out || overflow.load(Ordering::Relaxed) {
            kill_group(child.id());
            drop(child.kill());
            let status = child.wait().ok();
            return Ok(Waited {
                exit: if timed_out {
                    None
                } else {
                    status.and_then(status_code)
                },
                timed_out,
                killed: true,
            });
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn send_signal(pid: u32, sig: i32) -> Result<(), String> {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or("bad child pid")?;
    let sig = rustix::process::Signal::from_named_raw(sig)
        .ok_or_else(|| format!("unknown signal {sig}"))?;
    rustix::process::kill_process(pid, sig).map_err(|e| format!("kill: {e}"))
}

#[cfg(not(unix))]
fn send_signal(_pid: u32, _sig: i32) -> Result<(), String> {
    Err(String::from("signals are posix-only"))
}

/// `SIGINT` → 2, … (posix numbers; the same on Linux and macOS for these four).
pub(crate) fn signal_number(name: &str) -> Option<i32> {
    match name.trim_start_matches("SIG") {
        "HUP" => Some(1),
        "INT" => Some(2),
        "KILL" => Some(9),
        "TERM" => Some(15),
        _ => None,
    }
}

/// Finds `name` on the `PATH` of `env` (`.exe`/`PATHEXT` on Windows): the harness resolves `git`, `sh` and
/// `taskset` once per run.
pub(crate) fn which(name: &str, env: &Env) -> Option<PathBuf> {
    let path = env.get("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        env.get("PATHEXT").map_or_else(
            || vec![String::from(".exe")],
            |p| {
                p.to_string_lossy()
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(str::to_owned)
                    .collect()
            },
        )
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path).find_map(|dir| {
        exts.iter().find_map(|ext| {
            let p = dir.join(format!("{name}{ext}"));
            is_executable(&p).then_some(p)
        })
    })
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

#[cfg(unix)]
mod pty {
    use std::process::{Command, Stdio};

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::{OptionalActions, OutputModes, tcgetattr, tcsetattr};

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::{Captured, Finish, Invocation, MAX_CAPTURE, feed, own_group, reader, wait};

    /// stdout and stderr on one pty (`OPOST`/`ONLCR` cleared so the bytes are the program's own), stdin a pipe —
    /// pre-commit's colour mode (10 §6 item 2).
    pub(super) fn run(inv: &Invocation) -> Result<Captured, String> {
        let master = {
            let _guard = snp_core::spawnlock::guard();
            let m = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)
                .map_err(|e| format!("openpt: {e}"))?;
            rustix::io::fcntl_setfd(&m, rustix::io::FdFlags::CLOEXEC)
                .map_err(|e| format!("FD_CLOEXEC: {e}"))?;
            m
        };
        grantpt(&master).map_err(|e| format!("grantpt: {e}"))?;
        unlockpt(&master).map_err(|e| format!("unlockpt: {e}"))?;
        let name = ptsname(&master, Vec::new()).map_err(|e| format!("ptsname: {e}"))?;
        let slave = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("opening the pty slave: {e}"))?;
        let mut t = tcgetattr(&slave).map_err(|e| format!("tcgetattr: {e}"))?;
        t.output_modes
            .remove(OutputModes::OPOST | OutputModes::ONLCR);
        tcsetattr(&slave, OptionalActions::Now, &t).map_err(|e| format!("tcsetattr: {e}"))?;
        let slave = std::fs::File::from(slave);
        let mut cmd = Command::new(&inv.program);
        inv.env.apply_to(&mut cmd);
        cmd.args(&inv.args)
            .current_dir(&inv.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(
                slave.try_clone().map_err(|e| format!("dup: {e}"))?,
            ))
            .stderr(Stdio::from(slave));
        own_group(&mut cmd);
        let start = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawning {}: {e}", inv.program.display()))?;
        drop(cmd);
        let feeder = feed(&mut child, inv.stdin.clone())?;
        let master = std::fs::File::from(master);
        let overflow = Arc::new(AtomicBool::new(false));
        let merged = reader(Some(master), MAX_CAPTURE, &overflow, true)?;
        let w = wait(&mut child, start, inv, &overflow)?;
        let finish = Finish::new(&w, start, inv.timeout, child.id());
        let (stdout, truncated) = finish.bytes(merged)?;
        finish.unit(feeder);
        Ok(Captured {
            stdout,
            stderr: Vec::new(),
            exit: w.exit,
            timed_out: w.timed_out || finish.late(),
            truncated: truncated || overflow.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        let p = Env::capture_process();
        Env::from_pairs(p.get("PATH").map(|v| ("PATH", v.to_os_string())))
    }

    #[cfg(unix)]
    #[test]
    fn captures_streams_exit_and_stdin() {
        let e = env();
        let sh = which("sh", &e).unwrap();
        let inv = Invocation {
            program: sh,
            args: vec!["-c".into(), "cat; echo err >&2; exit 3".into()],
            env: e,
            cwd: std::env::temp_dir(),
            stdin: b"in\n".to_vec(),
            tty: false,
            timeout: Duration::from_secs(30),
            signal: None,
        };
        let c = run(&inv).unwrap();
        assert_eq!(c.stdout, b"in\n");
        assert_eq!(c.stderr, b"err\n");
        assert_eq!(c.exit, Some(3));
        assert!(!c.timed_out);
    }

    #[cfg(unix)]
    #[test]
    fn deadline_and_signal() {
        let e = env();
        let sh = which("sh", &e).unwrap();
        let mut inv = Invocation {
            program: sh,
            args: vec!["-c".into(), "sleep 5".into()],
            env: e,
            cwd: std::env::temp_dir(),
            stdin: Vec::new(),
            tty: false,
            timeout: Duration::from_millis(200),
            signal: None,
        };
        let c = run(&inv).unwrap();
        assert!(c.timed_out);
        assert_eq!(c.exit, None);
        inv.timeout = Duration::from_secs(30);
        inv.args = vec!["-c".into(), "exec sleep 5".into()];
        inv.signal = Some((
            15,
            Trigger {
                file: None,
                delay: Duration::from_millis(100),
            },
        ));
        let c = run(&inv).unwrap();
        assert_eq!(c.exit, Some(-15));
    }

    #[cfg(unix)]
    #[test]
    fn pty_merges_streams() {
        let e = env();
        let sh = which("sh", &e).unwrap();
        let inv = Invocation {
            program: sh,
            args: vec!["-c".into(), "[ -t 1 ] && echo tty; echo err >&2".into()],
            env: e,
            cwd: std::env::temp_dir(),
            stdin: Vec::new(),
            tty: true,
            timeout: Duration::from_secs(30),
            signal: None,
        };
        let c = run(&inv).unwrap();
        assert_eq!(c.stdout, b"tty\nerr\n");
        assert_eq!(c.stderr, b"");
    }

    #[cfg(unix)]
    fn sh_inv(script: &str, timeout: Duration, tty: bool) -> Invocation {
        let e = env();
        Invocation {
            program: which("sh", &e).unwrap(),
            args: vec!["-c".into(), script.into()],
            env: e,
            cwd: std::env::temp_dir(),
            stdin: Vec::new(),
            tty,
            timeout,
            signal: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_flood_is_capped_and_killed() {
        // `yes` writes GB/s: before the cap this grew the harness at pipe speed until the OOM killer struck.
        for tty in [false, true] {
            let start = Instant::now();
            let c = run(&sh_inv(
                "exec yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                Duration::from_secs(60),
                tty,
            ))
            .unwrap();
            assert!(c.truncated, "tty={tty}");
            assert!(
                c.stdout.len() <= MAX_CAPTURE,
                "tty={tty}: {}",
                c.stdout.len()
            );
            assert!(!c.timed_out, "tty={tty}");
            assert!(start.elapsed() < Duration::from_secs(30), "tty={tty}");
        }
        // stderr too, and through a grandchild.
        let c = run(&sh_inv("yes e >&2; true", Duration::from_secs(60), false)).unwrap();
        assert!(c.truncated);
        assert!(c.stderr.len() <= MAX_CAPTURE);
    }

    #[test]
    fn capture_keeps_a_bounded_prefix() {
        let flag = AtomicBool::new(false);
        let mut src = std::io::repeat(b'a').take(10_000);
        let (b, t) = capture(&mut src, 100, &flag, false).unwrap();
        assert_eq!(b.len(), 100);
        assert!(t && flag.load(Ordering::Relaxed));
        let flag = AtomicBool::new(false);
        let (b, t) = capture(&mut &b"abc"[..], 100, &flag, false).unwrap();
        assert_eq!((b.as_slice(), t), (&b"abc"[..], false));
    }

    #[cfg(unix)]
    #[test]
    fn deadline_kills_grandchildren_holding_the_pipes() {
        // A backgrounded grandchild keeps stdout open: the step must still end at its deadline, not the
        // grandchild's.
        for tty in [false, true] {
            let start = Instant::now();
            let c = run(&sh_inv(
                "sleep 40 & exec sleep 100",
                Duration::from_millis(500),
                tty,
            ))
            .unwrap();
            assert!(c.timed_out, "tty={tty}");
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "tty={tty}: {:?}",
                start.elapsed()
            );
        }
        // The direct child exits at once but a grandchild holds the pipe past the deadline.
        let start = Instant::now();
        let c = run(&sh_inv(
            "sleep 40 & echo hi",
            Duration::from_millis(500),
            false,
        ))
        .unwrap();
        assert!(c.timed_out);
        assert_eq!(c.exit, Some(0));
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn signal_names() {
        assert_eq!(signal_number("SIGTERM"), Some(15));
        assert_eq!(signal_number("INT"), Some(2));
        assert_eq!(signal_number("SIGUSR1"), None);
    }
}
