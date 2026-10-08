//! Spawning children (`capture`, `merged`, `stream`, `inherit`) with roles and groups (§3.6, ADR-065).
//!
//! `capture`, `stream` and `inherit` are W0 working baselines (ADR-071) over `std::process::Command`: absolute
//! program, `env_clear` + the computed environment, explicit cwd, the stdin modes, Python returncode semantics,
//! isolated process groups and the Critical role's environment; reader threads feed a bounded channel whose wait
//! observes `deadline` and `cancel` (kill → 500 ms drain → partial output). The §10.4 escalation (TERM → grace →
//! KILL, group signals), the child registry, `merged` and the output budget are WP-15's.
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-15"
)]

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use snp_core::cancel::CancelToken;
use snp_core::env::{Env, EnvPatch};
use snp_core::failure::pyerr;
use snp_core::spawnlock;
use snp_os as _;

/// What the child's stdin is (ADR-019).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Stdin {
    /// A pipe whose write end is dropped at once (non-pty hooks, pre-commit parity).
    EmptyPipe,
    /// `/dev/null` (`NUL`).
    Null,
    /// These bytes, then EOF.
    Bytes(Arc<[u8]>),
    /// snp's own stdin.
    Inherit,
}

/// Inherit: child stays in snp's process group (terminal ^C reaches it, parity).
/// Isolated: Unix `process_group(0)`, Windows `CREATE_NEW_PROCESS_GROUP` — hooks with a `timeout` and every
/// `Role::Critical` command (ADR-026). Both keep std's `posix_spawn` path (no `pre_exec`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Group {
    /// Same process group as snp.
    Inherit,
    /// A new process group.
    Isolated,
}

/// What cancellation may do to the child (ADR-065; normative table in §10.4).
/// Hook: hooks and hook partitions (Windows: placed in a per-run Job Object without `KILL_ON_JOB_CLOSE`).
/// Tool: snp's own git/installer/probe/clone commands.
/// Critical: restore-critical git (stash checkout, applies, rollback checkout, ITA rm/re-add, `stage_fixed` add);
/// requires `Group::Isolated`; never signalled or killed by anything in this crate and ignores `cancel`; env gets
/// `GIT_TERMINAL_PROMPT=0`, Windows adds `CREATE_NO_WINDOW` (no console event reaches it); reaped with
/// waitid(WEXITED|WSTOPPED|WNOWAIT): a SIGTTIN/SIGTTOU stop SIGKILLs its group and the call fails (`stopped_by_tty`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Role {
    /// Hooks and hook partitions.
    Hook,
    /// snp's own tools.
    Tool,
    /// Restore-critical git commands.
    Critical,
}

/// One spawn request.
#[derive(Clone, Debug)]
pub struct Spec<'a> {
    /// `argv[0]` is the RESOLVED program (`normalize_cmd` already applied).
    pub argv: &'a [OsString],
    /// The complete child environment (`env_clear` + these).
    pub env: &'a Env,
    /// Working directory.
    pub cwd: &'a Path,
    /// stdin mode.
    pub stdin: Stdin,
    /// Process group.
    pub group: Group,
    /// Cancellation role.
    pub role: Role,
    /// Hook `timeout` deadline.
    pub deadline: Option<Instant>,
    /// Never prevents a spawn (the scheduler and the pool are the only gate for new work). For Hook/Tool children a
    /// cancelled token starts the §10.4 escalation and bounds the output wait (kill → 500 ms drain → partial output);
    /// a child started on an already-cancelled token is escalated at once. Critical children ignore it.
    pub cancel: &'a CancelToken,
}

/// Python returncode semantics: Unix signal N → -N; Windows: the u32 exit code as i64 (14 A.1 #4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PyStatus(pub i64);

/// One global in-memory budget for captured output across all running children (default 64 MiB, ADR-036);
/// beyond it a capture spills to an unnamed file (`snp_os::fs::unnamed_temp`) in `spill_dir` (store `spill/`, 0700,
/// or the OS temp dir).
#[derive(Debug)]
pub struct OutputBudget {
    /// W0 placeholder; WP-15 stores the remaining bytes (`AtomicUsize`) and `spill_dir`.
    _priv: (),
}

impl OutputBudget {
    /// A budget of `bytes` spilling into `spill_dir`.
    pub fn new(bytes: usize, spill_dir: PathBuf) -> Arc<Self> {
        todo!()
    }
}

/// Merged hook output, in memory or spilled.
#[derive(Debug)]
#[non_exhaustive]
pub enum Output {
    /// Within the budget.
    Memory(Vec<u8>),
    /// Spilled to an unnamed temporary file.
    Spilled {
        /// The spill file (rewound by the reader).
        file: File,
        /// Bytes written.
        len: u64,
    },
}

impl Output {
    /// Copies the output into `w`.
    pub fn write_to(&self, w: &mut dyn Write) -> io::Result<()> {
        todo!()
    }

    /// `not output.strip()`-style blank check used by the renderer.
    pub fn is_blank(&self) -> bool {
        todo!()
    }
}

/// Result of [`capture`].
#[derive(Debug)]
pub struct Captured {
    /// Exit status.
    pub status: PyStatus,
    /// stdout bytes.
    pub stdout: Vec<u8>,
    /// stderr bytes.
    pub stderr: Vec<u8>,
    /// Critical only: the child was stopped by SIGTTIN/SIGTTOU and its group killed.
    pub stopped_by_tty: bool,
}

/// Result of [`merged`].
#[derive(Debug)]
pub struct Merged {
    /// Exit status.
    pub status: PyStatus,
    /// stdout+stderr in arrival order.
    pub output: Output,
    /// The deadline expired.
    pub timed_out: bool,
    /// The token was cancelled while the child ran.
    pub interrupted: bool,
}

/// `OSError` from spawning.
#[derive(Debug)]
pub struct SpawnError {
    /// The program that failed to start.
    pub program: OsString,
    /// The OS error.
    pub io: io::Error,
}

impl SpawnError {
    /// "[Errno 8] Exec format error: './x.sh'" / "[`WinError` N] …".
    ///
    /// `str(OSError)` with the program as `filename` (13 X.2.1, 02 §11.8).
    pub fn python_text(&self) -> String {
        pyerr::os_error_text(&self.io, Some(&self.program), None)
    }
}

/// Output reading (ADR-065): every pipe/pty read end is owned by a dedicated, non-scoped reader thread
/// (`Builder::spawn`; an OS refusal is a `SpawnError`) that sends chunks over a bounded channel; the caller waits
/// with `recv_timeout` ticks that observe `deadline` and `cancel`. Normal runs read to EOF (parity: a descendant
/// holding the pipe delays the hook exactly as in pre-commit); after a deadline or cancellation the wait ends after
/// the kill escalation plus a 500 ms drain, the partial output is returned and the reader thread is abandoned (it
/// ends at EOF or process exit). No `poll` on any platform (macOS ptys do not support it).
/// Separate pipes, read concurrently. snp's own tools (git, installers, probes) and Critical commands.
pub fn capture(spec: &Spec<'_>) -> Result<Captured, SpawnError> {
    let mut sp = start(spec, Stdio::piped(), Stdio::piped())?;
    let (tx, rx) = mpsc::sync_channel(CHANNEL_BOUND);
    let readers = attach_reader(&mut sp, Stream::Out, &tx)
        .and_then(|()| attach_reader(&mut sp, Stream::Err, &tx));
    drop(tx);
    if let Err(io) = readers {
        return Err(sp.abort(io));
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    pump(&mut sp.child, spec, &rx, |s, chunk| match s {
        Stream::Out => stdout.extend_from_slice(chunk),
        Stream::Err => stderr.extend_from_slice(chunk),
    });
    let status = sp.finish()?;
    Ok(Captured {
        status,
        stdout,
        stderr,
        stopped_by_tty: false,
    })
}

/// One pipe for stdout+stderr (`std::io::pipe`, Command dropped before reading), or a POSIX pty when `pty`: master
/// `openpt(RDWR|NOCTTY|CLOEXEC)`, slave `open(ptsname, O_RDWR|O_NOCTTY|O_CLOEXEC)` (both under the spawn lock on
/// macOS), OPOST|ONLCR cleared, stdin /dev/null, 0×0 size, slave dropped before reading, EIO = EOF. pty failure →
/// pipe (X13-EXE-06). Hooks only.
pub fn merged(spec: &Spec<'_>, pty: bool, budget: &OutputBudget) -> Result<Merged, SpawnError> {
    todo!()
}

/// stdout streamed into `sink` (diff hashing, stash patch), stderr captured. Returns status and stderr.
///
/// A failing `sink` stops the copying (the rest of stdout is drained and discarded) and is reported as the
/// `SpawnError` after the child exited.
pub fn stream(spec: &Spec<'_>, sink: &mut dyn Write) -> Result<(PyStatus, Vec<u8>), SpawnError> {
    let mut sp = start(spec, Stdio::piped(), Stdio::piped())?;
    let (tx, rx) = mpsc::sync_channel(CHANNEL_BOUND);
    let readers = attach_reader(&mut sp, Stream::Out, &tx)
        .and_then(|()| attach_reader(&mut sp, Stream::Err, &tx));
    drop(tx);
    if let Err(io) = readers {
        return Err(sp.abort(io));
    }
    let mut stderr = Vec::new();
    let mut sink_error: Option<io::Error> = None;
    pump(&mut sp.child, spec, &rx, |s, chunk| match s {
        Stream::Out => {
            if sink_error.is_none()
                && let Err(e) = sink.write_all(chunk)
            {
                sink_error = Some(e);
            }
        }
        Stream::Err => stderr.extend_from_slice(chunk),
    });
    let status = sp.finish()?;
    if let Some(io) = sink_error.or_else(|| sink.flush().err()) {
        return Err(SpawnError {
            program: spec_program(spec),
            io,
        });
    }
    Ok((status, stderr))
}

/// stdio inherited: legacy hooks, show-diff, hazmat children.
pub fn inherit(spec: &Spec<'_>) -> Result<PyStatus, SpawnError> {
    let mut sp = start(spec, Stdio::inherit(), Stdio::inherit())?;
    let honours_cancel = spec.role != Role::Critical;
    if spec.deadline.is_none() && !honours_cancel {
        return sp.finish();
    }
    loop {
        match sp.child.try_wait() {
            Ok(Some(_)) => return sp.finish(),
            Ok(None) => {}
            Err(io) => return Err(sp.abort(io)),
        }
        if must_stop(spec) {
            let _ignored: io::Result<()> = sp.child.kill();
            return sp.finish();
        }
        let tick = tick_until(spec.deadline);
        if honours_cancel {
            spec.cancel.wait_timeout(tick);
        } else {
            std::thread::sleep(tick);
        }
    }
}

/// Longest wait between checks of `deadline` / `cancel`.
const TICK: Duration = Duration::from_millis(50);
/// How long output is still collected after a deadline or cancellation killed the child (§3.6).
const DRAIN: Duration = Duration::from_millis(500);
/// Bounded reader channel (chunks of at most [`CHUNK`] bytes).
const CHANNEL_BOUND: usize = 8;
const CHUNK: usize = 64 * 1024;

/// `CREATE_NEW_PROCESS_GROUP`.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// `CREATE_NO_WINDOW`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Which pipe a chunk came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

/// A started child plus what is needed to report on it.
#[derive(Debug)]
struct Started {
    child: Child,
    program: OsString,
}

impl Started {
    /// Reaps the child: Python's `returncode`.
    fn finish(mut self) -> Result<PyStatus, SpawnError> {
        match self.child.wait() {
            Ok(st) => Ok(py_status(st)),
            Err(io) => Err(SpawnError {
                program: self.program,
                io,
            }),
        }
    }

    /// Kills and reaps a child whose plumbing failed after the spawn; the error names the program.
    fn abort(mut self, io: io::Error) -> SpawnError {
        let _killed: io::Result<()> = self.child.kill();
        let _reaped: io::Result<ExitStatus> = self.child.wait();
        SpawnError {
            program: self.program,
            io,
        }
    }
}

fn spec_program(spec: &Spec<'_>) -> OsString {
    spec.argv.first().cloned().unwrap_or_default()
}

/// `os.sep` / `os.altsep` in `s`.
fn has_sep(s: &OsStr) -> bool {
    let b = s.as_encoded_bytes();
    b.contains(&b'/') || (cfg!(windows) && b.contains(&b'\\'))
}

/// Python returncode semantics: Unix signal N → -N; Windows: the u32 exit code (14 A.1 #4).
fn py_status(st: ExitStatus) -> PyStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        match (st.code(), st.signal()) {
            (Some(c), _) => PyStatus(i64::from(c)),
            (None, Some(sig)) => PyStatus(i64::from(sig).saturating_neg()),
            // Not reachable after `wait` (no WUNTRACED); keep a failing status rather than a success.
            (None, None) => PyStatus(-1),
        }
    }
    #[cfg(not(unix))]
    {
        PyStatus(
            st.code()
                .map_or(-1, |c| i64::from(u32::from_ne_bytes(c.to_ne_bytes()))),
        )
    }
}

/// Builds and spawns the command of `spec` (stdout/stderr as given); stdin per `spec.stdin`.
fn start(spec: &Spec<'_>, stdout: Stdio, stderr: Stdio) -> Result<Started, SpawnError> {
    let Some((arg0, args)) = spec.argv.split_first() else {
        return Err(SpawnError {
            program: OsString::new(),
            io: io::Error::new(io::ErrorKind::InvalidInput, "empty argv"),
        });
    };
    // argv[0] is resolved by normalize_cmd; a relative path is relative to the child's cwd (snp never chdirs).
    let program: PathBuf = if has_sep(arg0) && !Path::new(arg0).is_absolute() {
        spec.cwd.join(arg0)
    } else {
        PathBuf::from(arg0)
    };
    let mut cmd = Command::new(&program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0(arg0);
        if spec.group == Group::Isolated {
            cmd.process_group(0);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut flags = 0;
        if spec.group == Group::Isolated {
            flags |= CREATE_NEW_PROCESS_GROUP;
        }
        if spec.role == Role::Critical {
            flags |= CREATE_NO_WINDOW;
        }
        cmd.creation_flags(flags);
    }
    cmd.args(args).current_dir(spec.cwd);
    if spec.role == Role::Critical {
        spec.env
            .patched(&EnvPatch::default().set("GIT_TERMINAL_PROMPT", "0"))
            .apply_to(&mut cmd);
    } else {
        spec.env.apply_to(&mut cmd);
    }
    cmd.stdin(match spec.stdin {
        Stdin::EmptyPipe | Stdin::Bytes(_) => Stdio::piped(),
        Stdin::Null => Stdio::null(),
        Stdin::Inherit => Stdio::inherit(),
    });
    cmd.stdout(stdout).stderr(stderr);

    let spawned = {
        let _guard = spawnlock::guard();
        cmd.spawn()
    };
    drop(cmd);
    let child = spawned.map_err(|io| SpawnError {
        program: arg0.clone(),
        io,
    })?;
    let mut sp = Started {
        child,
        program: arg0.clone(),
    };
    match &spec.stdin {
        // The write end is closed at once: the child sees an empty, non-tty stdin (ADR-019).
        Stdin::EmptyPipe => drop(sp.child.stdin.take()),
        Stdin::Bytes(bytes) => {
            if let Some(mut w) = sp.child.stdin.take() {
                let bytes = Arc::clone(bytes);
                let fed = std::thread::Builder::new()
                    .name("snp-stdin".to_owned())
                    .spawn(move || {
                        // A child that exits without reading gives EPIPE: not an error of the spawn.
                        let _ignored: io::Result<()> = w.write_all(&bytes);
                    });
                if let Err(io) = fed {
                    return Err(sp.abort(io));
                }
            }
        }
        Stdin::Null | Stdin::Inherit => {}
    }
    Ok(sp)
}

/// Moves the child's `which` pipe into a dedicated reader thread feeding `tx`.
fn attach_reader(
    sp: &mut Started,
    which: Stream,
    tx: &SyncSender<(Stream, Vec<u8>)>,
) -> io::Result<()> {
    fn boxed(r: impl Read + Send + 'static) -> Box<dyn Read + Send> {
        Box::new(r)
    }
    let pipe = match which {
        Stream::Out => sp.child.stdout.take().map(boxed),
        Stream::Err => sp.child.stderr.take().map(boxed),
    };
    let Some(mut pipe) = pipe else {
        return Ok(());
    };
    let tx = tx.clone();
    let name = match which {
        Stream::Out => "snp-stdout",
        Stream::Err => "snp-stderr",
    };
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut buf = vec![0_u8; CHUNK];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = buf.get(..n).unwrap_or_default().to_vec();
                        if tx.send((which, chunk)).is_err() {
                            break; // the waiter gave up (deadline / cancellation)
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })
        .map(drop)
}

/// Whether a Hook/Tool child must be stopped now (cancelled token or expired deadline). Critical children ignore
/// the token; a deadline applies to every role that has one.
fn must_stop(spec: &Spec<'_>) -> bool {
    (spec.role != Role::Critical && spec.cancel.is_cancelled())
        || spec.deadline.is_some_and(|d| Instant::now() >= d)
}

/// The next wait slice: [`TICK`], shortened to the deadline.
fn tick_until(deadline: Option<Instant>) -> Duration {
    deadline.map_or(TICK, |d| {
        d.saturating_duration_since(Instant::now())
            .clamp(Duration::from_millis(1), TICK)
    })
}

/// Feeds chunks to `on_chunk` until every reader reached EOF; on a deadline or cancellation the child is killed
/// and output is drained for at most [`DRAIN`], after which the readers are abandoned.
fn pump(
    child: &mut Child,
    spec: &Spec<'_>,
    rx: &Receiver<(Stream, Vec<u8>)>,
    mut on_chunk: impl FnMut(Stream, &[u8]),
) {
    let mut killed_at: Option<Instant> = None;
    loop {
        if killed_at.is_none() && must_stop(spec) {
            let _ignored: io::Result<()> = child.kill();
            killed_at = Some(Instant::now());
        }
        let tick = match killed_at {
            Some(k) => {
                let left = DRAIN.saturating_sub(k.elapsed());
                if left.is_zero() {
                    return;
                }
                left.min(TICK)
            }
            None => tick_until(spec.deadline),
        };
        match rx.recv_timeout(tick) {
            Ok((which, chunk)) => on_chunk(which, &chunk),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<OsString> {
        ["/bin/sh", "-c", script, "--"]
            .iter()
            .map(OsString::from)
            .collect()
    }

    fn spec<'a>(
        argv: &'a [OsString],
        env: &'a Env,
        cwd: &'a Path,
        cancel: &'a CancelToken,
    ) -> Spec<'a> {
        Spec {
            argv,
            env,
            cwd,
            stdin: Stdin::EmptyPipe,
            group: Group::Inherit,
            role: Role::Tool,
            deadline: None,
            cancel,
        }
    }

    fn env() -> Env {
        Env::from_pairs([("PATH", "/usr/bin:/bin"), ("SNP_TEST_VAR", "hello")])
    }

    #[test]
    fn capture_separates_streams_and_uses_env_and_cwd() {
        let d = tempfile::tempdir().unwrap();
        let argv = sh("printf '%s' \"$SNP_TEST_VAR\"; pwd >&2; exit 3");
        let (env, cancel) = (env(), CancelToken::new());
        let c = capture(&spec(&argv, &env, d.path(), &cancel)).unwrap();
        assert_eq!(c.status, PyStatus(3));
        assert_eq!(c.stdout, b"hello");
        let printed = String::from_utf8(c.stderr).unwrap();
        let same = |a: &Path, b: &Path| {
            use std::os::unix::fs::MetadataExt;
            let (a, b) = (std::fs::metadata(a).unwrap(), std::fs::metadata(b).unwrap());
            (a.dev(), a.ino()) == (b.dev(), b.ino())
        };
        assert!(same(Path::new(printed.trim_end()), d.path()));
        assert!(!c.stopped_by_tty);
    }

    #[test]
    fn env_is_cleared() {
        let argv = sh("printf '%s' \"${HOME-unset}\"");
        let (env, cancel) = (env(), CancelToken::new());
        let c = capture(&spec(&argv, &env, Path::new("/"), &cancel)).unwrap();
        assert_eq!(c.stdout, b"unset");
    }

    #[test]
    fn signal_death_is_negative() {
        let argv = sh("kill -9 $$");
        let (env, cancel) = (env(), CancelToken::new());
        let c = capture(&spec(&argv, &env, Path::new("/"), &cancel)).unwrap();
        assert_eq!(c.status, PyStatus(-9));
    }

    #[test]
    fn stdin_modes() {
        let (env, cancel) = (env(), CancelToken::new());
        let argv = sh("cat; if [ -t 0 ]; then echo tty; fi");
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        assert_eq!(capture(&s).unwrap().stdout, b"");
        s.stdin = Stdin::Null;
        assert_eq!(capture(&s).unwrap().stdout, b"");
        s.stdin = Stdin::Bytes(Arc::from(&b"fed bytes"[..]));
        assert_eq!(capture(&s).unwrap().stdout, b"fed bytes");
    }

    #[test]
    fn critical_role_disables_git_prompts() {
        let argv = sh("printf '%s' \"$GIT_TERMINAL_PROMPT\"");
        let (env, cancel) = (env(), CancelToken::new());
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Critical;
        s.group = Group::Isolated;
        assert_eq!(capture(&s).unwrap().stdout, b"0");
    }

    #[test]
    fn stream_writes_stdout_into_the_sink() {
        let argv = sh("printf 'a%.0s' $(seq 1 100000); printf err >&2");
        let (env, cancel) = (env(), CancelToken::new());
        let mut sink = Vec::new();
        let (st, err) = stream(&spec(&argv, &env, Path::new("/"), &cancel), &mut sink).unwrap();
        assert_eq!(st, PyStatus(0));
        assert_eq!(err, b"err");
        assert_eq!(sink.len(), 100_000);
    }

    #[test]
    fn inherit_returns_the_status() {
        let argv = sh("exit 7");
        let (env, cancel) = (env(), CancelToken::new());
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.stdin = Stdin::Null;
        assert_eq!(inherit(&s).unwrap(), PyStatus(7));
    }

    #[test]
    fn deadline_kills_and_returns_partial_output() {
        let argv = sh("echo started; exec sleep 30");
        let (env, cancel) = (env(), CancelToken::new());
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.deadline = Instant::now().checked_add(Duration::from_millis(300));
        let t0 = Instant::now();
        let c = capture(&s).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(10));
        assert_eq!(c.stdout, b"started\n");
        assert_eq!(c.status, PyStatus(-9));
    }

    #[test]
    fn cancelled_token_stops_tools_but_not_critical() {
        let (env, cancel) = (env(), CancelToken::new());
        cancel.cancel(snp_core::cancel::CancelReason::Interrupt);
        let argv = sh("sleep 30");
        let s = spec(&argv, &env, Path::new("/"), &cancel);
        let t0 = Instant::now();
        assert_eq!(inherit(&s).unwrap(), PyStatus(-9));
        assert_eq!(capture(&s).unwrap().status, PyStatus(-9));
        assert!(t0.elapsed() < Duration::from_secs(10));
        let argv = sh("echo done");
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Critical;
        s.group = Group::Isolated;
        assert_eq!(capture(&s).unwrap().stdout, b"done\n");
    }

    #[test]
    fn spawn_errors_render_like_python() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("x.sh"), b"no shebang").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                d.path().join("x.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let argv = [OsString::from("./x.sh")];
        let (env, cancel) = (env(), CancelToken::new());
        let e = capture(&spec(&argv, &env, d.path(), &cancel)).unwrap_err();
        assert_eq!(e.python_text(), "[Errno 8] Exec format error: './x.sh'");
        let argv = [OsString::from("/nonexistent/prog")];
        let e = capture(&spec(&argv, &env, d.path(), &cancel)).unwrap_err();
        assert_eq!(
            e.python_text(),
            "[Errno 2] No such file or directory: '/nonexistent/prog'"
        );
        let e = capture(&spec(&[], &env, d.path(), &cancel)).unwrap_err();
        assert_eq!(e.io.kind(), io::ErrorKind::InvalidInput);
    }
}
