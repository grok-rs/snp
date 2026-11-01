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
}

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

/// Runs `inv` to completion (or its deadline).
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
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawning {}: {e}", inv.program.display()))?;
    let feeder = feed(&mut child, inv.stdin.clone())?;
    let out_reader = reader(child.stdout.take())?;
    let err_reader = reader(child.stderr.take())?;
    let (exit, timed_out) = wait(&mut child, inv.timeout, inv.signal.as_ref())?;
    let stdout = join_bytes(out_reader)?;
    let stderr = join_bytes(err_reader)?;
    join_unit(feeder);
    Ok(Captured {
        stdout,
        stderr,
        exit,
        timed_out,
    })
}

type BytesHandle = std::thread::JoinHandle<std::io::Result<Vec<u8>>>;

fn reader<R: Read + Send + 'static>(src: Option<R>) -> Result<Option<BytesHandle>, String> {
    let Some(mut src) = src else { return Ok(None) };
    std::thread::Builder::new()
        .name(String::from("difftest-reader"))
        .spawn(move || {
            let mut buf = Vec::new();
            src.read_to_end(&mut buf).map(|_| buf)
        })
        .map(Some)
        .map_err(|e| format!("starting a reader thread: {e}"))
}

fn join_bytes(h: Option<BytesHandle>) -> Result<Vec<u8>, String> {
    match h {
        None => Ok(Vec::new()),
        Some(h) => match h.join() {
            Ok(Ok(b)) => Ok(b),
            Ok(Err(e)) => Err(format!("reading child output: {e}")),
            Err(_) => Err(String::from("a reader thread panicked")),
        },
    }
}

fn feed(child: &mut Child, bytes: Vec<u8>) -> Result<Option<std::thread::JoinHandle<()>>, String> {
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

fn join_unit(h: Option<std::thread::JoinHandle<()>>) {
    if let Some(h) = h {
        drop(h.join());
    }
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

fn wait(
    child: &mut Child,
    timeout: Duration,
    signal: Option<&(i32, Trigger)>,
) -> Result<(Option<i32>, bool), String> {
    let start = Instant::now();
    let mut signalled = false;
    // When the marker file appeared (the delay counts from there).
    let mut armed: Option<Instant> = None;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("waiting for the child: {e}"))?
        {
            return Ok((status_code(status), false));
        }
        let elapsed = start.elapsed();
        if let Some((sig, trigger)) = signal
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
        if elapsed >= timeout {
            drop(child.kill());
            drop(child.wait());
            return Ok((None, true));
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
    use std::io::Read as _;
    use std::process::{Command, Stdio};

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::{OptionalActions, OutputModes, tcgetattr, tcsetattr};

    use super::{Captured, Invocation, feed, join_unit, wait};

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
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawning {}: {e}", inv.program.display()))?;
        drop(cmd);
        let feeder = feed(&mut child, inv.stdin.clone())?;
        let mut master = std::fs::File::from(master);
        let reader = std::thread::Builder::new()
            .name(String::from("difftest-pty"))
            .spawn(move || {
                let mut merged = Vec::new();
                let mut buf = [0_u8; 4096];
                loop {
                    match master.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => merged.extend_from_slice(buf.get(..n).unwrap_or_default()),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                merged
            })
            .map_err(|e| format!("starting the pty reader: {e}"))?;
        let (exit, timed_out) = wait(&mut child, inv.timeout, inv.signal.as_ref())?;
        let stdout = reader
            .join()
            .map_err(|_| String::from("the pty reader panicked"))?;
        join_unit(feeder);
        Ok(Captured {
            stdout,
            stderr: Vec::new(),
            exit,
            timed_out,
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

    #[test]
    fn signal_names() {
        assert_eq!(signal_number("SIGTERM"), Some(15));
        assert_eq!(signal_number("INT"), Some(2));
        assert_eq!(signal_number("SIGUSR1"), None);
    }
}
