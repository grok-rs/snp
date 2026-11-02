//! Spawning children (`capture`, `merged`, `stream`, `inherit`) with roles and groups (§3.6, ADR-065).
//!
//! Every child is spawned from an absolute (or cwd-relative) program with `env_clear` + the computed environment and
//! an explicit cwd, keeps std's `posix_spawn` path (no `pre_exec`: no `/bin/sh` fallback on `ENOEXEC`), is registered
//! in [`crate::registry`] until reaped, and is supervised by its caller's thread: output arrives from dedicated reader
//! threads over a bounded channel whose `recv_timeout` ticks observe the deadline and the cancellation token; the exit
//! is detected with `waitid(WEXITED|WNOHANG|WNOWAIT)` and the child is reaped only at the end (no pid reuse).
//!
//! Escalation (ARCHITECTURE §3.6, §10.4) for Hook/Tool children: a deadline sends SIGTERM to the child (its process
//! group when isolated), SIGKILL after the kill grace (`SNP_KILL_GRACE`, 2 s), then output is drained for 500 ms and
//! the readers are abandoned; a cancelled token waits the grace (the signal thread forwards per the §10.4 table) and
//! then kills the same way — at once for a child started on an already-cancelled token. Windows terminates the hook's
//! Job Object instead of signalling. Critical children ignore the token and deadlines and are never signalled, except
//! that a SIGTTIN/SIGTTOU stop kills their group (`stopped_by_tty`).
//!
//! Owner: WP-15.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use snp_core::cancel::CancelToken;
use snp_core::env::{Env, EnvPatch};
use snp_core::failure::pyerr;
use snp_core::spawnlock;
use snp_core::sync;

use crate::registry::{self, Cell, Proc};

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
///
/// A Critical spec with `Group::Inherit` is spawned isolated anyway, and its `deadline` is ignored (it is never
/// killed). Windows places every Hook child in its own Job Object (kept until the run ends, so `quiesce` can
/// terminate a finished hook's descendants).
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
///
/// Accounting: every byte a [`merged`] capture keeps in memory is charged before it is appended. A capture that
/// would exceed the remaining budget moves its bytes into an unnamed spill file and returns its charge; an output
/// returned as [`Output::Memory`] keeps its charge while it is alive (the run holds outputs until the head-of-line
/// printer has written them) and returns it when it is dropped. So the captured output resident in memory never
/// exceeds the budget, whatever the number of partitions or CPUs (B12), and a long run whose outputs are printed and
/// dropped never starts spilling. (A held `Output::Memory` whose vector the caller regrows is released when the
/// budget sees its old allocation reused; nothing in snp mutates outputs.)
/// When neither `spill_dir` nor the OS temp dir accepts a spill file the output stays in memory (output is never
/// lost).
#[derive(Debug)]
pub struct OutputBudget {
    /// Bytes still available for in-memory captures.
    remaining: AtomicUsize,
    /// Where spill files are created.
    spill_dir: PathBuf,
    /// This budget, for the leases of the outputs it charged.
    me: Weak<Self>,
}

/// The charge an alive [`Output::Memory`] holds, keyed by its vector's allocation address.
#[derive(Debug)]
struct Lease {
    /// The charging budget.
    budget: Weak<OutputBudget>,
    /// Bytes charged.
    charge: usize,
}

/// Leases of alive in-memory outputs (allocation address → lease); [`Output`]'s `Drop` returns them.
static LEASES: std::sync::LazyLock<Mutex<HashMap<usize, Lease>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

impl OutputBudget {
    /// A budget of `bytes` spilling into `spill_dir`.
    pub fn new(bytes: usize, spill_dir: PathBuf) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            remaining: AtomicUsize::new(bytes),
            spill_dir,
            me: Weak::clone(me),
        })
    }

    /// Keeps `charge` charged while the output in `v` is alive (`Drop for Output` returns it).
    fn lease(&self, v: &[u8], capacity: usize, charge: usize) {
        if charge == 0 {
            return;
        }
        if capacity == 0 {
            self.give_back(charge);
            return;
        }
        let stale = sync::lock(&LEASES).insert(
            v.as_ptr().addr(),
            Lease {
                budget: Weak::clone(&self.me),
                charge,
            },
        );
        // The address was reused: the output that leased it gave up that allocation (it was regrown or freed).
        if let Some(old) = stale {
            old.release();
        }
    }

    /// Charges `n` bytes; `false` (nothing charged) when fewer remain.
    fn try_take(&self, n: usize) -> bool {
        let mut cur = self.remaining.load(Ordering::Acquire);
        loop {
            let Some(next) = cur.checked_sub(n) else {
                return false;
            };
            match self.remaining.compare_exchange_weak(
                cur,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(seen) => cur = seen,
            }
        }
    }

    /// Returns `n` previously charged bytes.
    fn give_back(&self, n: usize) {
        let mut cur = self.remaining.load(Ordering::Acquire);
        loop {
            match self.remaining.compare_exchange_weak(
                cur,
                cur.saturating_add(n),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(seen) => cur = seen,
            }
        }
    }

    /// An unnamed spill file in `spill_dir`, else in the OS temp dir.
    fn spill_file(&self) -> io::Result<File> {
        snp_os::fs::unnamed_temp(&self.spill_dir)
            .or_else(|_| snp_os::fs::unnamed_temp(&std::env::temp_dir()))
    }
}

impl Lease {
    /// Returns the charge (nothing when the budget is gone).
    fn release(self) {
        if let Some(b) = self.budget.upgrade() {
            b.give_back(self.charge);
        }
    }
}

/// Merged hook output, in memory or spilled.
///
/// An in-memory output holds its [`OutputBudget`] charge until it is dropped.
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
    ///
    /// A spilled output is read with positional reads, so the file offset is irrelevant and `write_to` may be called
    /// any number of times.
    pub fn write_to(&self, w: &mut dyn Write) -> io::Result<()> {
        match self {
            Self::Memory(v) => w.write_all(v),
            Self::Spilled { file, len } => for_each_spilled_chunk(file, *len, |c| w.write_all(c)),
        }
    }

    /// `not output.strip()`-style blank check used by the renderer.
    ///
    /// `bytes.strip()` whitespace: space, `\t`, `\n`, `\r`, `\x0b`, `\x0c`.
    pub fn is_blank(&self) -> bool {
        let blank = |c: &[u8]| c.iter().all(|b| b.is_ascii_whitespace() || *b == 0x0b);
        match self {
            Self::Memory(v) => blank(v),
            Self::Spilled { file, len } => {
                let mut all = true;
                let read = for_each_spilled_chunk(file, *len, |c| {
                    if blank(c) {
                        Ok(())
                    } else {
                        all = false;
                        Err(io::Error::other("not blank"))
                    }
                });
                all && read.is_ok()
            }
        }
    }
}

impl Drop for Output {
    /// Returns the budget charge of an in-memory output.
    fn drop(&mut self) {
        if let Self::Memory(v) = self
            && v.capacity() > 0
        {
            let lease = sync::lock(&LEASES).remove(&v.as_ptr().addr());
            if let Some(l) = lease {
                l.release();
            }
        }
    }
}

/// Calls `f` with consecutive chunks of the first `len` bytes of `file` (positional reads).
fn for_each_spilled_chunk(
    file: &File,
    len: u64,
    mut f: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut buf = vec![0_u8; CHUNK];
    let mut off: u64 = 0;
    while off < len {
        let want = usize::try_from(len.saturating_sub(off))
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let slice = buf.get_mut(..want).unwrap_or_default();
        let n = match read_at(file, slice, off) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "spill file shorter than its recorded length",
                ));
            }
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        f(buf.get(..n).unwrap_or_default())?;
        off = off.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
    }
    Ok(())
}

/// `pread` (Unix) / `ReadFile` with an offset (Windows).
fn read_at(file: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(file, buf, off)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(file, buf, off)
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
    /// `str(OSError)` with the program as `filename` (13 X.2.1, 02 §11.8; `util.py:86-87`), or with the `cwd` when
    /// the child's `chdir` failed (`subprocess.Popen` reports the cwd then, 15 §A4 "Exception texts"). An error that
    /// carries no OS code (e.g. a batch argument with a line break, D-PLAT-03) renders its own message.
    pub fn python_text(&self) -> String {
        if let Some(c) = self
            .io
            .get_ref()
            .and_then(|e| e.downcast_ref::<ChdirFailed>())
        {
            return pyerr::os_error_text(&c.source, Some(&c.cwd), None);
        }
        // Windows: `_winapi.CreateProcess` raises `OSError(winerror)` without a file name (`[WinError 193] %1 is
        // not a valid Win32 application`).
        let program = (!cfg!(windows)).then_some(self.program.as_os_str());
        pyerr::os_error_text(&self.io, program, None)
    }
}

/// A spawn that failed in the child's `chdir(cwd)` (POSIX): the OS error is `source`, the file name is `cwd`.
///
/// Carried inside [`SpawnError::io`] (same kind; the OS code is `source`'s). `Display` is the Python text.
#[derive(Debug)]
struct ChdirFailed {
    /// The working directory as passed.
    cwd: OsString,
    /// The OS error.
    source: io::Error,
}

impl std::fmt::Display for ChdirFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&pyerr::os_error_text(&self.source, Some(&self.cwd), None))
    }
}

impl std::error::Error for ChdirFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// `io` as a [`ChdirFailed`] when a POSIX spawn failed with `ENOENT`/`ENOTDIR`/`EACCES` and `cwd` is not an
/// accessible directory (`chdir` runs before `exec`, so it is the failing call); `io` otherwise (and on Windows,
/// where CPython reports `[WinError 267]` without a file name).
fn attribute_to_cwd(io: io::Error, cwd: &Path) -> io::Error {
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        let code = io.raw_os_error();
        let chdir_like = [Errno::NOENT, Errno::NOTDIR, Errno::ACCESS]
            .iter()
            .any(|e| code == Some(e.raw_os_error()));
        let cwd_ok = std::fs::metadata(cwd).is_ok_and(|m| m.is_dir())
            && rustix::fs::access(cwd, rustix::fs::Access::EXEC_OK).is_ok();
        if chdir_like && !cwd_ok {
            // The errno `chdir(cwd)` itself gives (CPython's child calls it): the spawn's own code can differ
            // (macOS reports a cwd that is a file as `ENOENT`).
            let source = match std::fs::metadata(cwd) {
                Ok(m) if !m.is_dir() => io::Error::from_raw_os_error(Errno::NOTDIR.raw_os_error()),
                Ok(_) => io::Error::from_raw_os_error(Errno::ACCESS.raw_os_error()),
                Err(e) => e,
            };
            return io::Error::new(
                source.kind(),
                ChdirFailed {
                    cwd: cwd.as_os_str().to_os_string(),
                    source,
                },
            );
        }
        io
    }
    #[cfg(not(unix))]
    {
        let _: &Path = cwd;
        io
    }
}

/// Output reading (ADR-065): every pipe/pty read end is owned by a dedicated, non-scoped reader thread
/// (`Builder::spawn`; an OS refusal is a `SpawnError`) that sends chunks over a bounded channel; the caller waits
/// with `recv_timeout` ticks that observe `deadline` and `cancel`. Normal runs read to EOF (parity: a descendant
/// holding the pipe delays the hook exactly as in pre-commit); after a deadline or cancellation the wait ends after
/// the kill escalation plus a 500 ms drain, the partial output is returned and the reader thread is abandoned (it
/// ends at EOF or process exit). No `poll` on any platform (macOS ptys do not support it).
/// Separate pipes, read concurrently. snp's own tools (git, installers, probes) and Critical commands.
///
/// pre-commit's `cmd_output_b` (`util.py:81-113`) with separate stdout/stderr pipes.
pub fn capture(spec: &Spec<'_>) -> Result<Captured, SpawnError> {
    let mut started = start(spec, Stdio::piped(), Stdio::piped(), false)?;
    let (tx, rx) = mpsc::sync_channel(CHANNEL_BOUND);
    if let Err(io) = attach_pipes(&mut started, &tx) {
        return Err(started.abort(io));
    }
    drop(tx);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let waited = started.supervise(spec, Some(&rx), |s, chunk| match s {
        Stream::Out => stdout.extend_from_slice(&chunk),
        Stream::Err => stderr.extend_from_slice(&chunk),
    });
    Ok(Captured {
        status: waited.status,
        stdout,
        stderr,
        stopped_by_tty: waited.stopped_by_tty,
    })
}

/// One pipe for stdout+stderr (`std::io::pipe`, Command dropped before reading), or a POSIX pty when `pty`: master
/// `openpt(RDWR|NOCTTY|CLOEXEC)`, slave `open(ptsname, O_RDWR|O_NOCTTY|O_CLOEXEC)` (both under the spawn lock on
/// macOS), OPOST|ONLCR cleared, stdin /dev/null, 0×0 size, slave dropped before reading, EIO = EOF. pty failure →
/// pipe (X13-EXE-06). Hooks only.
///
/// `cmd_output_b(..., stderr=STDOUT)` / `cmd_output_p` (`util.py:81-200`). In pty mode `Stdin::EmptyPipe` becomes
/// `/dev/null` (pre-commit's `cmd_output_p`); `Stdin::Bytes` (the `use_stdin` extension) is still fed through a pipe.
/// Windows has no pty (`cmd_output_p = cmd_output_b`). A pty that cannot be allocated (no `/dev/ptmx`, out of ptys)
/// falls back to the pipe (D-CRASH-16: pre-commit crashes with exit 3).
pub fn merged(spec: &Spec<'_>, pty: bool, budget: &OutputBudget) -> Result<Merged, SpawnError> {
    merged_impl(spec, pty, budget, false)
}

/// [`merged`] with a test hook that simulates a pty allocation failure.
pub(crate) fn merged_impl(
    spec: &Spec<'_>,
    pty: bool,
    budget: &OutputBudget,
    fail_pty: bool,
) -> Result<Merged, SpawnError> {
    #[cfg(unix)]
    if pty {
        let opened = if fail_pty {
            Err(io::Error::from(io::ErrorKind::NotFound))
        } else {
            pty::open()
        };
        if let Ok(p) = opened {
            return merged_on(spec, MergedSink::Pty(p), budget);
        }
        tracing::debug!(target: "snp::spawn", "pty allocation failed; falling back to a pipe");
    }
    #[cfg(not(unix))]
    let _: (bool, bool) = (pty, fail_pty);
    let pipe = {
        let _lock = spawnlock::guard();
        io::pipe()
    };
    let (r, w) = pipe.map_err(|io| SpawnError {
        program: spec_program(spec),
        io,
    })?;
    merged_on(spec, MergedSink::Pipe(r, w), budget)
}

/// Where merged output comes from.
enum MergedSink {
    /// `std::io::pipe()` read and write ends.
    Pipe(io::PipeReader, io::PipeWriter),
    /// A pty pair.
    #[cfg(unix)]
    Pty(pty::Pty),
}

/// Spawns with stdout and stderr on `sink` and collects the merged output under `budget`.
fn merged_on(
    spec: &Spec<'_>,
    sink: MergedSink,
    budget: &OutputBudget,
) -> Result<Merged, SpawnError> {
    let fail = |io| SpawnError {
        program: spec_program(spec),
        io,
    };
    let (reader, out, err, pty_mode): (Box<dyn Read + Send>, Stdio, Stdio, bool) = match sink {
        MergedSink::Pipe(r, w) => {
            let w2 = {
                let _lock = spawnlock::guard();
                w.try_clone()
            }
            .map_err(fail)?;
            (Box::new(r), Stdio::from(w), Stdio::from(w2), false)
        }
        #[cfg(unix)]
        MergedSink::Pty(p) => {
            let slave2 = {
                let _lock = spawnlock::guard();
                p.slave.try_clone()
            }
            .map_err(fail)?;
            (
                Box::new(File::from(p.master)),
                Stdio::from(p.slave),
                Stdio::from(slave2),
                true,
            )
        }
    };
    // `start` drops the Command (and with it the parent's copies of the write ends) before returning.
    let started = start(spec, out, err, pty_mode)?;
    let (tx, rx) = mpsc::sync_channel(CHANNEL_BOUND);
    if let Err(io) = spawn_reader(reader, Stream::Out, &tx, pty_mode) {
        return Err(started.abort(io));
    }
    drop(tx);
    let mut collector = Collector::new(budget);
    let waited = started.supervise(spec, Some(&rx), |_, chunk| collector.push(&chunk));
    Ok(Merged {
        status: waited.status,
        output: collector.finish(),
        timed_out: waited.timed_out,
        interrupted: waited.interrupted,
    })
}

/// Accumulates merged output in memory under the budget, spilling to an unnamed file beyond it.
struct Collector<'b> {
    budget: &'b OutputBudget,
    mem: Vec<u8>,
    charged: usize,
    spill: Option<(File, u64)>,
}

impl<'b> Collector<'b> {
    const fn new(budget: &'b OutputBudget) -> Self {
        Self {
            budget,
            mem: Vec::new(),
            charged: 0,
            spill: None,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if let Some((file, len)) = &mut self.spill {
            match file.write_all(chunk) {
                Ok(()) => {
                    *len = len.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
                }
                // The spill device failed: keep the rest in memory rather than lose output.
                Err(_) => self.mem.extend_from_slice(chunk),
            }
            return;
        }
        // The budget is charged for the buffer's capacity (what can become resident), grown by doubling.
        let needed = self.mem.len().saturating_add(chunk.len());
        if needed <= self.charged || self.grow(needed) {
            self.mem.extend_from_slice(chunk);
            return;
        }
        match self.budget.spill_file() {
            Ok(mut file) => {
                let written = file
                    .write_all(&self.mem)
                    .and_then(|()| file.write_all(chunk));
                if written.is_ok() {
                    let len = u64::try_from(self.mem.len().saturating_add(chunk.len()))
                        .unwrap_or(u64::MAX);
                    self.budget.give_back(self.charged);
                    self.charged = 0;
                    self.mem = Vec::new();
                    self.spill = Some((file, len));
                } else {
                    self.mem.extend_from_slice(chunk);
                }
            }
            Err(_) => self.mem.extend_from_slice(chunk),
        }
    }

    /// Grows the charged capacity to hold `needed` bytes: doubled when the budget allows, else exactly; `false`
    /// (nothing charged) when even the exact growth does not fit.
    fn grow(&mut self, needed: usize) -> bool {
        let doubled = needed.max(self.charged.saturating_mul(2));
        for target in [doubled, needed] {
            let delta = target.saturating_sub(self.charged);
            if !self.budget.try_take(delta) {
                continue;
            }
            if self
                .mem
                .try_reserve_exact(target.saturating_sub(self.mem.len()))
                .is_ok()
            {
                self.charged = target;
                return true;
            }
            self.budget.give_back(delta);
            return false;
        }
        false
    }

    /// The output: an in-memory one keeps its charge as a lease until it is dropped; the spill path holds none.
    fn finish(self) -> Output {
        match self.spill {
            Some((file, len)) if self.mem.is_empty() => {
                use std::io::Seek;
                let mut file = file;
                let _ignored = file.rewind();
                Output::Spilled { file, len }
            }
            Some((file, len)) => {
                // Spill writes failed part-way: put everything back in memory (rare; output is never lost).
                let mut all = Vec::new();
                let read = for_each_spilled_chunk(&file, len, |c| {
                    all.extend_from_slice(c);
                    Ok(())
                });
                // The charge covered `mem` only; `all` is not charged (rare, never lose output).
                self.budget.give_back(self.charged);
                if read.is_ok() {
                    all.extend_from_slice(&self.mem);
                    Output::Memory(all)
                } else {
                    Output::Memory(self.mem)
                }
            }
            None => {
                self.budget
                    .lease(&self.mem, self.mem.capacity(), self.charged);
                Output::Memory(self.mem)
            }
        }
    }
}

/// stdout streamed into `sink` (diff hashing, stash patch), stderr captured. Returns status and stderr.
///
/// A failing `sink` stops the copying (the rest of stdout is drained and discarded) and is reported as the
/// `SpawnError` after the child exited.
pub fn stream(spec: &Spec<'_>, sink: &mut dyn Write) -> Result<(PyStatus, Vec<u8>), SpawnError> {
    let mut started = start(spec, Stdio::piped(), Stdio::piped(), false)?;
    let (tx, rx) = mpsc::sync_channel(CHANNEL_BOUND);
    if let Err(io) = attach_pipes(&mut started, &tx) {
        return Err(started.abort(io));
    }
    drop(tx);
    let mut stderr = Vec::new();
    let mut sink_error: Option<io::Error> = None;
    let waited = started.supervise(spec, Some(&rx), |s, chunk| match s {
        Stream::Out => {
            if sink_error.is_none()
                && let Err(e) = sink.write_all(&chunk)
            {
                sink_error = Some(e);
            }
        }
        Stream::Err => stderr.extend_from_slice(&chunk),
    });
    if let Some(io) = sink_error.or_else(|| sink.flush().err()) {
        return Err(SpawnError {
            program: spec_program(spec),
            io,
        });
    }
    Ok((waited.status, stderr))
}

/// stdio inherited: legacy hooks, show-diff, hazmat children.
///
/// `subprocess.call(…)` parity (`hook_impl.py:40-46`, `staged_files_only.py`'s show-diff): stdout and stderr are
/// snp's own; stdin per `spec.stdin`.
pub fn inherit(spec: &Spec<'_>) -> Result<PyStatus, SpawnError> {
    let started = start(spec, Stdio::inherit(), Stdio::inherit(), false)?;
    let waited = started.supervise(spec, None, |_, _| {});
    Ok(waited.status)
}

/// Longest wait between checks of `deadline` / `cancel`.
const TICK: Duration = Duration::from_millis(50);
/// How long output is still collected after a deadline or cancellation killed the child (§3.6).
const DRAIN: Duration = Duration::from_millis(500);
/// Bounded reader channel (chunks of at most [`CHUNK`] bytes).
const CHANNEL_BOUND: usize = 4;
/// Largest read size of the reader threads.
const CHUNK: usize = 64 * 1024;
/// First read size of a reader thread: most children print nothing or a few lines, so the buffer starts small and
/// doubles whenever a read fills it, up to [`CHUNK`] (a 64 KiB buffer per stream of every spawn was 64 % of all
/// bytes a quiet `run` allocated, budget B3 `alloc_mib`).
const FIRST_CHUNK: usize = 4 * 1024;
/// Stack of the helper threads (their buffers live on the heap).
const HELPER_STACK: usize = 256 * 1024;

/// `CREATE_NEW_PROCESS_GROUP`.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// `CREATE_NO_WINDOW`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_SUSPENDED` (needed by process-wrap's Job Object assignment).
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

/// Which pipe a chunk came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

/// A chunk of output.
type Chunk = (Stream, Vec<u8>);

/// How the supervision of a child ended.
#[derive(Debug)]
struct Waited {
    status: PyStatus,
    timed_out: bool,
    interrupted: bool,
    stopped_by_tty: bool,
}

/// The escalation state of a supervised child.
#[derive(Copy, Clone, Debug)]
enum Phase {
    /// Running normally.
    Normal,
    /// The deadline sent SIGTERM at this instant.
    #[cfg_attr(
        windows,
        expect(
            dead_code,
            reason = "Windows has no SIGTERM: the job is terminated at once"
        )
    )]
    Terminating(Instant),
    /// Cancellation was observed at this instant.
    Cancelling(Instant),
    /// SIGKILL was sent at this instant; output is drained until [`DRAIN`] after it.
    Draining(Instant),
}

/// What an exit poll found.
enum Poll {
    Running,
    Exited(PyStatus),
    /// Critical child stopped by SIGTTIN/SIGTTOU.
    #[cfg_attr(
        windows,
        expect(dead_code, reason = "job-control stops are POSIX-only")
    )]
    TtyStopped,
}

/// A started, registered child plus its pipes.
struct Started {
    cell: Arc<Cell>,
    program: OsString,
    spawned_cancelled: bool,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
}

impl Started {
    /// Kills, reaps and unregisters a child whose plumbing failed after the spawn; the error names the program.
    fn abort(self, io: io::Error) -> SpawnError {
        {
            let mut st = self.cell.lock();
            if let Some(child) = st.child.as_mut() {
                let _killed = child.kill();
                #[cfg(unix)]
                let _reaped = child.wait();
                #[cfg(windows)]
                let _reaped = child.try_wait();
            }
            st.reaped = true;
        }
        registry::unregister(&self.cell);
        SpawnError {
            program: self.program,
            io,
        }
    }

    /// Supervises the child until it exited and every reader reached EOF, or until the escalation abandoned it
    /// (module docs); `on_chunk` receives the output in arrival order.
    fn supervise(
        self,
        spec: &Spec<'_>,
        rx: Option<&Receiver<Chunk>>,
        mut on_chunk: impl FnMut(Stream, Vec<u8>),
    ) -> Waited {
        let critical = spec.role == Role::Critical;
        let grace = registry::kill_grace();
        let mut phase = Phase::Normal;
        let mut readers_open = rx.is_some();
        let mut exit: Option<PyStatus> = None;
        let mut stopped_by_tty = false;
        let mut timed_out = false;
        let mut interrupted = false;
        let mut backoff = Duration::from_millis(1);
        loop {
            if exit.is_none() {
                match poll_exit(&self.cell, critical) {
                    Poll::Exited(st) => exit = Some(st),
                    Poll::TtyStopped => {
                        stopped_by_tty = true;
                        #[cfg(unix)]
                        registry::signal_cell(&self.cell, rustix::process::Signal::KILL);
                    }
                    Poll::Running => {}
                }
            }
            if !readers_open && exit.is_some() {
                break;
            }
            let now = Instant::now();
            match phase {
                Phase::Normal if !critical => {
                    if spec.deadline.is_some_and(|d| now >= d) {
                        timed_out = true;
                        phase = terminate(&self.cell, now);
                    } else if spec.cancel.is_cancelled() {
                        interrupted = true;
                        if self.spawned_cancelled {
                            registry::kill_now(&self.cell);
                            phase = Phase::Draining(now);
                        } else {
                            phase = Phase::Cancelling(now);
                        }
                    }
                }
                Phase::Terminating(since) | Phase::Cancelling(since) => {
                    if since.checked_add(grace).is_none_or(|t| now >= t) {
                        registry::kill_now(&self.cell);
                        phase = Phase::Draining(now);
                    }
                }
                Phase::Draining(since) => {
                    if since.checked_add(DRAIN).is_none_or(|t| now >= t) {
                        break;
                    }
                }
                Phase::Normal => {}
            }
            let wake = next_wake(phase, spec, grace, now, critical);
            if let (true, Some(rx)) = (readers_open, rx) {
                match rx.recv_timeout(wake) {
                    Ok((s, chunk)) => on_chunk(s, chunk),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => readers_open = false,
                }
            } else {
                let d = backoff.min(wake);
                backoff = backoff.saturating_mul(2).min(TICK);
                if critical {
                    std::thread::sleep(d);
                } else {
                    spec.cancel.wait_timeout(d);
                }
            }
        }
        let status = if let Some(st) = exit {
            reap(&self.cell).unwrap_or(st)
        } else {
            abandon(Arc::clone(&self.cell));
            killed_status()
        };
        Waited {
            status,
            timed_out,
            interrupted,
            stopped_by_tty,
        }
    }
}

/// The status reported for a child that was killed but could not be reaped within the drain.
const fn killed_status() -> PyStatus {
    if cfg!(windows) {
        PyStatus(1)
    } else {
        PyStatus(-9)
    }
}

/// The deadline's first step: SIGTERM (Unix; the group when isolated), or terminating the hook's Job Object
/// (Windows, which has no TERM).
fn terminate(cell: &Cell, now: Instant) -> Phase {
    #[cfg(unix)]
    {
        registry::signal_cell(cell, rustix::process::Signal::TERM);
        Phase::Terminating(now)
    }
    #[cfg(windows)]
    {
        registry::kill_now(cell);
        Phase::Draining(now)
    }
}

/// The longest wait before the next state check.
fn next_wake(
    phase: Phase,
    spec: &Spec<'_>,
    grace: Duration,
    now: Instant,
    critical: bool,
) -> Duration {
    let until = |t: Option<Instant>| {
        t.map_or(TICK, |t| {
            t.saturating_duration_since(now)
                .clamp(Duration::from_millis(1), TICK)
        })
    };
    match phase {
        Phase::Normal if !critical => until(spec.deadline),
        Phase::Normal => TICK,
        Phase::Terminating(since) | Phase::Cancelling(since) => until(since.checked_add(grace)),
        Phase::Draining(since) => until(since.checked_add(DRAIN)),
    }
}

/// Non-blocking exit check that leaves the child unreaped (Unix `waitid(WNOWAIT)`; Critical children also report
/// stops).
#[cfg(unix)]
fn poll_exit(cell: &Cell, critical: bool) -> Poll {
    use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};
    let Some(pid) = i32::try_from(cell.pid).ok().and_then(Pid::from_raw) else {
        return Poll::Exited(PyStatus(-1));
    };
    let mut opts = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    if critical {
        opts |= WaitIdOptions::STOPPED;
    }
    match rustix::process::waitid(WaitId::Pid(pid), opts) {
        Ok(Some(s)) if s.exited() => {
            Poll::Exited(PyStatus(i64::from(s.exit_status().unwrap_or(0))))
        }
        Ok(Some(s)) if s.killed() || s.dumped() => Poll::Exited(PyStatus(
            i64::from(s.terminating_signal().unwrap_or(0)).saturating_neg(),
        )),
        Ok(Some(s)) if s.stopped() => {
            let tty = s
                .stopping_signal()
                .is_some_and(|n| n == Signal::TTIN.as_raw() || n == Signal::TTOU.as_raw());
            if tty { Poll::TtyStopped } else { Poll::Running }
        }
        Ok(_) => Poll::Running,
        // ECHILD (reaped elsewhere) or EINTR: fall back to std's view under the cell lock.
        Err(_) => {
            let mut st = cell.lock();
            match st.child.as_mut().map(std::process::Child::try_wait) {
                Some(Ok(Some(es))) => {
                    st.reaped = true;
                    Poll::Exited(py_status(es))
                }
                Some(Ok(None)) => Poll::Running,
                Some(Err(_)) | None => Poll::Exited(PyStatus(-1)),
            }
        }
    }
}

/// Non-blocking exit check of the direct child (Windows: the handle stays valid until the cell is dropped).
#[cfg(windows)]
fn poll_exit(cell: &Cell, _critical: bool) -> Poll {
    let mut st = cell.lock();
    match st.child.as_mut().map(Proc::try_wait) {
        Some(Ok(Some(es))) => Poll::Exited(py_status(es)),
        Some(Ok(None)) => Poll::Running,
        Some(Err(_)) | None => Poll::Exited(PyStatus(1)),
    }
}

/// Reaps an exited child under its cell lock (instant: it is a zombie), marks it reaped and unregisters it.
fn reap(cell: &Arc<Cell>) -> Option<PyStatus> {
    let status = {
        let mut st = cell.lock();
        #[cfg(unix)]
        let status = st.child.as_mut().and_then(|c| c.wait().ok()).map(py_status);
        #[cfg(windows)]
        let status = st
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
            .map(py_status);
        st.reaped = true;
        status
    };
    registry::unregister(cell);
    status
}

/// Hands a killed child that did not exit within the drain to a background reaper thread (it stays registered, and
/// thus killable, until reaped).
fn abandon(cell: Arc<Cell>) {
    let spawned = std::thread::Builder::new()
        .name("snp-reaper".to_owned())
        .stack_size(HELPER_STACK)
        .spawn(move || {
            loop {
                {
                    let mut st = cell.lock();
                    let done = match st.child.as_mut() {
                        Some(c) => !matches!(c.try_wait(), Ok(None)),
                        None => true,
                    };
                    if done {
                        st.reaped = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            registry::unregister(&cell);
        });
    // A refused thread leaves the child registered (and a zombie until snp exits): never a panic.
    drop(spawned);
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

/// The D-PLAT-03 refusal: a `.bat`/`.cmd` program receiving an argument with a CR or LF (Rust's `BatBadBut` guard
/// would refuse it with an opaque "batch file arguments are invalid").
#[cfg(any(test, windows))]
fn batch_line_break(program: &OsStr, args: &[OsString]) -> Option<String> {
    let lower = program.as_encoded_bytes().to_ascii_lowercase();
    if !(lower.ends_with(b".bat") || lower.ends_with(b".cmd")) {
        return None;
    }
    let bad = args.iter().find(|a| {
        a.as_encoded_bytes()
            .iter()
            .any(|b| *b == b'\r' || *b == b'\n')
    })?;
    Some(crate::messages::batch_line_break(program, bad))
}

/// The program `Command` runs: a relative path with a separator is relative to the child's cwd (snp never chdirs;
/// Python execs after `chdir(cwd)`).
fn program_path(arg0: &OsStr, cwd: &Path) -> PathBuf {
    if has_sep(arg0) && !Path::new(arg0).is_absolute() {
        cwd.join(arg0)
    } else {
        PathBuf::from(arg0)
    }
}

/// Builds and spawns the command of `spec` (stdout/stderr as given; stdin per `spec.stdin`, `/dev/null` instead of
/// an empty pipe in pty mode), registers the child and arranges its stdin.
fn start(
    spec: &Spec<'_>,
    stdout: Stdio,
    stderr: Stdio,
    pty_mode: bool,
) -> Result<Started, SpawnError> {
    let Some((arg0, args)) = spec.argv.split_first() else {
        return Err(SpawnError {
            program: OsString::new(),
            io: io::Error::new(io::ErrorKind::InvalidInput, "empty argv"),
        });
    };
    let fail = |io| SpawnError {
        program: arg0.clone(),
        io,
    };
    // Critical commands always run in their own process group (ADR-026).
    let group = if spec.role == Role::Critical {
        Group::Isolated
    } else {
        spec.group
    };
    #[cfg(windows)]
    if let Some(msg) = batch_line_break(arg0, args) {
        return Err(fail(io::Error::new(io::ErrorKind::InvalidInput, msg)));
    }
    let mut cmd = Command::new(program_path(arg0, spec.cwd));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0(arg0);
        if group == Group::Isolated {
            cmd.process_group(0);
        }
    }
    cmd.args(args).current_dir(spec.cwd);
    let env = if spec.role == Role::Critical {
        spec.env
            .patched(&EnvPatch::default().set("GIT_TERMINAL_PROMPT", "0"))
    } else {
        spec.env.clone()
    };
    env.apply_to(&mut cmd);
    let stdin_mode = match (&spec.stdin, pty_mode) {
        (Stdin::EmptyPipe, true) | (Stdin::Null, _) => Stdio::null(),
        (Stdin::EmptyPipe | Stdin::Bytes(_), _) => Stdio::piped(),
        (Stdin::Inherit, _) => Stdio::inherit(),
    };
    cmd.stdin(stdin_mode).stdout(stdout).stderr(stderr);
    crate::trace::log_spawn(spec.argv, &env, spec.cwd, spec.role, group);
    #[cfg(feature = "testing")]
    crate::spawn_log::record(spec.role, group, arg0);

    // The gate is taken before the token is read: `registry::forward` holds it exclusively, so a signal either
    // finds this child registered (and forwards to it) or was dispatched before the token is read here (and the
    // child is killed at once as spawned-cancelled). Without it a SIGINT landing between the exec and `register`
    // reached nobody and the hook got a silent SIGKILL after the grace (x13-eh-04, D-DATA-18).
    let gate = registry::spawn_gate();
    let spawned_cancelled = spec.role != Role::Critical && spec.cancel.is_cancelled();
    let spawned = {
        let _lock = spawnlock::guard();
        spawn_platform(cmd, spec.role, group)
    };
    let (proc_, pid, stdin, out, err) = spawned
        .map_err(|io| attribute_to_cwd(io, spec.cwd))
        .map_err(fail)?;
    #[cfg(all(test, unix))]
    tests::before_register();
    let cell = registry::register(proc_, pid, spec.role, group, spec.cancel.clone());
    drop(gate);
    let started = Started {
        cell,
        program: arg0.clone(),
        spawned_cancelled,
        stdout: out,
        stderr: err,
    };
    match (&spec.stdin, stdin) {
        // The write end is closed at once: the child sees an empty, non-tty stdin (ADR-019).
        (Stdin::Bytes(bytes), Some(mut w)) => {
            let bytes = Arc::clone(bytes);
            let fed = std::thread::Builder::new()
                .name("snp-stdin".to_owned())
                .stack_size(HELPER_STACK)
                .spawn(move || {
                    // A child that exits without reading gives EPIPE: not an error of the spawn.
                    let _ignored: io::Result<()> = w.write_all(&bytes);
                });
            if let Err(io) = fed {
                return Err(started.abort(io));
            }
        }
        (_, w) => drop(w),
    }
    Ok(started)
}

/// The pieces of a freshly spawned child.
type Spawned = (
    Proc,
    u32,
    Option<std::process::ChildStdin>,
    Option<std::process::ChildStdout>,
    Option<std::process::ChildStderr>,
);

/// `ENOEXEC` for a program the kernel cannot exec (no `#!`, no Mach-O magic), which Apple's `posix_spawnp` would
/// otherwise run with `/bin/sh` (its `execvp` emulation, ADR-086): CPython's `fork` + `execve` raises `OSError: [Errno
/// 8] Exec format error` there, like the kernel on every other Unix. `None` when the file cannot be read or is not an
/// executable regular file: the spawn then reports the kernel's own error.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn exec_format_error(program: &Path) -> Option<io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    // Mach-O 32/64 (both byte orders) and universal ("fat") binaries.
    const MAGICS: [[u8; 4]; 6] = [
        [0xfe, 0xed, 0xfa, 0xce],
        [0xfe, 0xed, 0xfa, 0xcf],
        [0xce, 0xfa, 0xed, 0xfe],
        [0xcf, 0xfa, 0xed, 0xfe],
        [0xca, 0xfe, 0xba, 0xbe],
        [0xbe, 0xba, 0xfe, 0xca],
    ];
    if !has_sep(program.as_os_str()) {
        return None;
    }
    let md = std::fs::metadata(program).ok()?;
    if !md.is_file() || md.permissions().mode() & 0o111 == 0 {
        return None;
    }
    let mut head = [0_u8; 4];
    let mut f = File::open(program).ok()?;
    let mut n = 0;
    while n < head.len() {
        match f.read(head.get_mut(n..)?) {
            Ok(0) => break,
            Ok(k) => n = n.saturating_add(k),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    let head = head.get(..n)?;
    let execable = head.starts_with(b"#!") || MAGICS.iter().any(|m| head == m.as_slice());
    (!execable).then(|| io::Error::from_raw_os_error(rustix::io::Errno::NOEXEC.raw_os_error()))
}

/// `Command::spawn` (Unix).
#[cfg(unix)]
fn spawn_platform(mut cmd: Command, _role: Role, _group: Group) -> io::Result<Spawned> {
    #[cfg(target_os = "macos")]
    if let Some(e) = exec_format_error(Path::new(cmd.get_program())) {
        return Err(e);
    }
    let spawned = cmd.spawn();
    drop(cmd);
    let mut child = spawned?;
    let pid = child.id();
    let (i, o, e) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    Ok((child, pid, i, o, e))
}

/// Windows: creation flags for groups and Critical commands; hooks inside their own Job Object (process-wrap).
#[cfg(windows)]
fn spawn_platform(mut cmd: Command, role: Role, group: Group) -> io::Result<Spawned> {
    use std::os::windows::process::CommandExt;
    let mut flags = 0;
    if group == Group::Isolated {
        flags |= CREATE_NEW_PROCESS_GROUP;
    }
    if role == Role::Critical {
        flags |= CREATE_NO_WINDOW;
    }
    if role != Role::Hook {
        cmd.creation_flags(flags);
        let spawned = cmd.spawn();
        drop(cmd);
        let mut child = spawned?;
        let pid = child.id();
        let (i, o, e) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        return Ok((Proc::Plain(child), pid, i, o, e));
    }
    let mut wrap = process_wrap::std::CommandWrap::from(cmd);
    wrap.wrap(process_wrap::std::JobObject)
        .wrap(WinFlags(flags));
    let spawned = wrap.spawn();
    drop(wrap);
    let mut child = spawned?;
    let pid = child.id();
    let (i, o, e) = (
        child.stdin().take(),
        child.stdout().take(),
        child.stderr().take(),
    );
    Ok((Proc::Job(child), pid, i, o, e))
}

/// Re-applies snp's creation flags after process-wrap's `JobObject` set `CREATE_SUSPENDED` (its `pre_spawn`
/// replaces the flags; the job assignment resumes the child).
#[cfg(windows)]
#[derive(Debug)]
struct WinFlags(u32);

#[cfg(windows)]
impl process_wrap::std::CommandWrapper for WinFlags {
    fn pre_spawn(
        &mut self,
        command: &mut Command,
        _core: &process_wrap::std::CommandWrap,
    ) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        command.creation_flags(self.0 | CREATE_SUSPENDED);
        Ok(())
    }
}

/// Moves the child's stdout and stderr pipes into reader threads feeding `tx`.
fn attach_pipes(started: &mut Started, tx: &SyncSender<Chunk>) -> io::Result<()> {
    if let Some(out) = started.stdout.take() {
        spawn_reader(Box::new(out), Stream::Out, tx, false)?;
    }
    if let Some(err) = started.stderr.take() {
        spawn_reader(Box::new(err), Stream::Err, tx, false)?;
    }
    Ok(())
}

/// A dedicated, non-scoped reader thread: reads `src` to EOF (`EIO` = EOF on a pty master) and sends the chunks;
/// it stops early when the receiver is gone (an abandoned wait).
fn spawn_reader(
    mut src: Box<dyn Read + Send>,
    which: Stream,
    tx: &SyncSender<Chunk>,
    pty_mode: bool,
) -> io::Result<()> {
    let tx = tx.clone();
    let name = match which {
        Stream::Out => "snp-stdout",
        Stream::Err => "snp-stderr",
    };
    std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(HELPER_STACK)
        .spawn(move || {
            let mut buf = vec![0_u8; FIRST_CHUNK];
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = buf.get(..n).unwrap_or_default().to_vec();
                        if n == buf.len() && buf.len() < CHUNK {
                            buf.resize(buf.len().saturating_mul(2).min(CHUNK), 0);
                        }
                        if tx.send((which, chunk)).is_err() {
                            break; // the waiter gave up (deadline / cancellation)
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    // A pty master reports EIO once every slave fd is closed (`util.py:188-194`).
                    Err(e) if pty_mode && is_eio(&e) => break,
                    Err(_) => break,
                }
            }
        })
        .map(drop)
}

/// `EIO`.
fn is_eio(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error())
    }
    #[cfg(not(unix))]
    {
        let _: &io::Error = e;
        false
    }
}

/// POSIX pseudo-terminals for colour mode.
#[cfg(unix)]
mod pty {
    use std::io;
    use std::os::fd::OwnedFd;

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::OpenptFlags;
    use rustix::termios::{OptionalActions, OutputModes};
    use snp_core::spawnlock;

    /// A master/slave pair; the slave is configured like `cmd_output_p`'s.
    #[derive(Debug)]
    pub(super) struct Pty {
        pub(super) master: OwnedFd,
        pub(super) slave: OwnedFd,
    }

    /// `openpt(RDWR|NOCTTY|CLOEXEC)` + `grantpt` + `unlockpt`, slave `open(ptsname, O_RDWR|O_NOCTTY|O_CLOEXEC)`,
    /// `OPOST|ONLCR` cleared (`util.py:132-141`), 0×0 window size, no `setsid` (the pty is never a controlling
    /// terminal). Under the spawn lock: on macOS `posix_openpt` has no `O_CLOEXEC`.
    pub(super) fn open() -> io::Result<Pty> {
        let _lock = spawnlock::guard();
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd",
            target_os = "netbsd"
        ))]
        let master =
            rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd",
            target_os = "netbsd"
        )))]
        let master = {
            let m = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
            rustix::io::fcntl_setfd(&m, rustix::io::FdFlags::CLOEXEC)?;
            m
        };
        rustix::pty::grantpt(&master)?;
        rustix::pty::unlockpt(&master)?;
        let name = rustix::pty::ptsname(&master, Vec::new())?;
        let slave = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let mut t = rustix::termios::tcgetattr(&slave)?;
        t.output_modes
            .remove(OutputModes::OPOST | OutputModes::ONLCR);
        rustix::termios::tcsetattr(&slave, OptionalActions::Now, &t)?;
        Ok(Pty { master, slave })
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    /// Test-only hook run between the exec and `register` (the window of the x13-eh-04 race).
    static BEFORE_REGISTER: Mutex<Option<fn()>> = Mutex::new(None);

    pub(super) fn before_register() {
        let hook = *sync::lock(&BEFORE_REGISTER);
        if let Some(f) = hook {
            f();
        }
    }

    static RACE_TOKEN: std::sync::OnceLock<CancelToken> = std::sync::OnceLock::new();

    /// A SIGINT sent (kill(2)) while a child is between its exec and `register` must still be forwarded to it:
    /// the forwarder waits for the registration instead of missing the child (x13-eh-04 / D-DATA-18).
    #[test]
    fn sigint_between_exec_and_register_is_forwarded() {
        fn interrupt_mid_spawn() {
            let forwarder = std::thread::Builder::new()
                .spawn(|| {
                    RACE_TOKEN
                        .get()
                        .unwrap()
                        .cancel(snp_core::cancel::CancelReason::Interrupt);
                    registry::forward(registry::Sig::Int, registry::Origin::Sent);
                })
                .unwrap();
            // Give the forwarder ample time to run (and, before the fix, to miss this child).
            std::thread::sleep(Duration::from_millis(300));
            // One-shot: later spawns in this process are unaffected.
            *sync::lock(&BEFORE_REGISTER) = None;
            drop(forwarder);
        }
        registry::set_kill_grace(Duration::from_secs(3));
        let cancel = RACE_TOKEN.get_or_init(CancelToken::new).clone();
        *sync::lock(&BEFORE_REGISTER) = Some(interrupt_mid_spawn);
        let argv: Vec<OsString> = ["/bin/sleep", "10"].iter().map(OsString::from).collect();
        let env = env();
        let started = Instant::now();
        let c = capture(&spec(&argv, &env, Path::new("/"), &cancel)).unwrap();
        // Forwarded: the child dies by SIGINT at once; missed: SIGKILL after the 3 s grace.
        assert_eq!(c.status, PyStatus(-2), "child was not sent SIGINT");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

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

    fn budget(bytes: usize) -> Arc<OutputBudget> {
        OutputBudget::new(bytes, std::env::temp_dir())
    }

    fn bytes_of(o: &Output) -> Vec<u8> {
        let mut v = Vec::new();
        o.write_to(&mut v).unwrap();
        v
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

    /// The reader buffer starts at `FIRST_CHUNK` and grows: small and large outputs both arrive whole, in order.
    #[test]
    fn growing_reader_buffers_keep_every_byte() {
        let (env, cancel) = (env(), CancelToken::new());
        for n in [
            0_usize,
            1,
            FIRST_CHUNK - 1,
            FIRST_CHUNK,
            FIRST_CHUNK + 1,
            3 * CHUNK + 17,
        ] {
            // BSD head refuses `-c 0`: no output then.
            let argv = sh(&format!(
                "[ {n} -gt 0 ] && head -c {n} /dev/zero | tr '\\0' x; printf '%s' end >&2"
            ));
            let c = capture(&spec(&argv, &env, Path::new("/"), &cancel)).unwrap();
            assert_eq!(c.stdout.len(), n);
            assert!(c.stdout.iter().all(|b| *b == b'x'));
            assert_eq!(c.stderr, b"end");
        }
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
    fn stream_reports_a_failing_sink() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let argv = sh("echo data");
        let (env, cancel) = (env(), CancelToken::new());
        let e = stream(&spec(&argv, &env, Path::new("/"), &cancel), &mut Broken).unwrap_err();
        assert_eq!(e.io.to_string(), "disk full");
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
    fn deadline_terminates_and_returns_partial_output() {
        let argv = sh("echo started; exec sleep 30");
        let (env, cancel) = (env(), CancelToken::new());
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.deadline = Instant::now().checked_add(Duration::from_millis(300));
        let t0 = Instant::now();
        let c = capture(&s).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(10));
        assert_eq!(c.stdout, b"started\n");
        // SIGTERM at the deadline ends `sleep`.
        assert_eq!(c.status, PyStatus(-15));
    }

    #[test]
    fn deadline_kills_a_term_ignoring_group_after_the_grace() {
        let argv = sh("trap '' TERM; echo started; while :; do sleep 1; done");
        let (env, cancel) = (env(), CancelToken::new());
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Hook;
        s.group = Group::Isolated;
        s.deadline = Instant::now().checked_add(Duration::from_millis(200));
        let budget = budget(1 << 20);
        let t0 = Instant::now();
        let m = merged(&s, false, &budget).unwrap();
        let took = t0.elapsed();
        assert!(m.timed_out && !m.interrupted);
        assert_eq!(m.status, PyStatus(-9));
        assert_eq!(bytes_of(&m.output), b"started\n");
        // deadline + grace (2 s) + drain at most.
        assert!(took >= Duration::from_secs(2), "{took:?}");
        assert!(took < Duration::from_secs(8), "{took:?}");
    }

    #[test]
    fn cancelled_token_stops_tools_but_not_critical() {
        let (env, cancel) = (env(), CancelToken::new());
        cancel.cancel(snp_core::cancel::CancelReason::Interrupt);
        // `exec`: the killed process is sleep itself, so no orphan keeps the test's stdio open.
        let argv = sh("exec sleep 30");
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
    fn cancellation_while_running_kills_after_the_grace() {
        let (env, cancel) = (env(), CancelToken::new());
        let argv = sh("trap '' INT TERM; echo up; while :; do sleep 1; done");
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Hook;
        s.group = Group::Isolated;
        let c2 = cancel.clone();
        let t = std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                c2.cancel(snp_core::cancel::CancelReason::Terminate);
            })
            .unwrap();
        let t0 = Instant::now();
        let m = merged(&s, false, &budget(1 << 20)).unwrap();
        t.join().unwrap();
        assert!(m.interrupted && !m.timed_out);
        assert_eq!(m.status, PyStatus(-9));
        assert_eq!(bytes_of(&m.output), b"up\n");
        assert!(t0.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn critical_tty_stop_kills_the_group() {
        let (env, cancel) = (env(), CancelToken::new());
        let argv = sh("kill -TTIN $$; echo unreachable");
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Critical;
        s.group = Group::Isolated;
        let t0 = Instant::now();
        let c = capture(&s).unwrap();
        assert!(c.stopped_by_tty);
        assert_eq!(c.status, PyStatus(-9));
        assert_eq!(c.stdout, b"");
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn merged_preserves_order_and_budget_spills() {
        let (env, cancel) = (env(), CancelToken::new());
        let argv = sh("echo out1; echo err1 >&2; echo out2; echo err2 >&2");
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Hook;
        let m = merged(&s, false, &budget(1 << 20)).unwrap();
        assert_eq!(bytes_of(&m.output), b"out1\nerr1\nout2\nerr2\n");
        assert!(matches!(m.output, Output::Memory(_)));
        assert!(!m.output.is_blank());
        // A budget of 10 bytes: the 300 KB output spills; the bytes are identical.
        let argv = sh("seq 1 60000");
        let s = Spec { argv: &argv, ..s };
        let small = budget(10);
        let m = merged(&s, false, &small).unwrap();
        let Output::Spilled { len, .. } = &m.output else {
            panic!("expected a spill");
        };
        let expected = capture(&s).unwrap().stdout;
        assert_eq!(*len, u64::try_from(expected.len()).unwrap());
        assert_eq!(bytes_of(&m.output), expected);
        assert_eq!(bytes_of(&m.output), expected, "write_to is repeatable");
        // The spill returned its charge.
        assert_eq!(small.remaining.load(Ordering::Relaxed), 10);
        // Blank detection on both kinds.
        let argv = sh("printf ' \\n\\t\\r\\n'");
        let s = Spec { argv: &argv, ..s };
        assert!(
            merged(&s, false, &budget(1 << 20))
                .unwrap()
                .output
                .is_blank()
        );
        assert!(merged(&s, false, &budget(0)).unwrap().output.is_blank());
    }

    #[test]
    fn pty_mode_gives_a_tty_without_crlf() {
        let (env, cancel) = (env(), CancelToken::new());
        let argv = sh(
            "[ -t 1 ] && echo out-tty; [ -t 2 ] && echo err-tty >&2; [ -t 0 ] || echo in-not-tty; printf 'a\\nb\\n'",
        );
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Hook;
        let m = merged(&s, true, &budget(1 << 20)).unwrap();
        assert_eq!(m.status, PyStatus(0));
        assert_eq!(bytes_of(&m.output), b"out-tty\nerr-tty\nin-not-tty\na\nb\n");
    }

    /// X13-EXE-06 / D-CRASH-16 (`util.py:133,188-194`): a pty that cannot be allocated (no `/dev/ptmx`, out of
    /// ptys) makes pre-commit crash with exit 3; snp runs the hook on a pipe instead.
    #[test]
    fn x13_exe_06_pty_failure_falls_back_to_pipes() {
        let (env, cancel) = (env(), CancelToken::new());
        let argv = sh("[ -t 1 ] && echo out-tty; [ -t 0 ] || echo in-not-tty; echo err >&2");
        let mut s = spec(&argv, &env, Path::new("/"), &cancel);
        s.role = Role::Hook;
        let m = merged_impl(&s, true, &budget(1 << 20), true).unwrap();
        assert_eq!(m.status, PyStatus(0));
        assert_eq!(bytes_of(&m.output), b"in-not-tty\nerr\n");
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
        // A child forked by a concurrent test may hold the just-written file open for a moment (ETXTBSY).
        let retry = |f: &dyn Fn() -> SpawnError| {
            let mut e = f();
            for _ in 0..20 {
                if e.io.raw_os_error() != Some(26) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
                e = f();
            }
            e
        };
        let e = retry(&|| capture(&spec(&argv, &env, d.path(), &cancel)).unwrap_err());
        assert_eq!(e.python_text(), "[Errno 8] Exec format error: './x.sh'");
        let budget = budget(1024);
        let e =
            retry(&|| merged(&spec(&argv, &env, d.path(), &cancel), false, &budget).unwrap_err());
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

    /// The macOS pre-check: only executable regular files that are neither `#!` scripts nor Mach-O binaries are
    /// `ENOEXEC` (what `execve` returns there); anything else is left to the spawn.
    #[test]
    fn exec_format_error_matches_the_kernel_rule() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let file = |name: &str, body: &[u8], mode: u32| {
            let p = d.path().join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        let noexec = |p: &Path| {
            exec_format_error(p).and_then(|e| e.raw_os_error())
                == Some(rustix::io::Errno::NOEXEC.raw_os_error())
        };
        assert!(noexec(&file("plain", b"echo hi\n", 0o755)));
        assert!(noexec(&file("empty", b"", 0o755)));
        assert!(noexec(&file("short", b"#", 0o755)));
        assert!(!noexec(&file("script", b"#!/bin/sh\n", 0o755)));
        assert!(!noexec(&file(
            "macho",
            &[0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1],
            0o755
        )));
        assert!(!noexec(&file(
            "fat",
            &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2],
            0o755
        )));
        assert!(!noexec(&file("not-exec", b"echo hi\n", 0o644)));
        assert!(!noexec(&d.path().join("missing")));
        assert!(!noexec(d.path()));
        // A bare name is a PATH search: not this check's.
        assert!(exec_format_error(Path::new("plain")).is_none());
    }

    /// `subprocess.Popen` names the cwd when the child's chdir fails (CPython 3.12:
    /// `[Errno 2] No such file or directory: '/nonexistent-dir'`, `[Errno 20] Not a directory: '/etc/passwd'`), even
    /// when the program is missing too (chdir runs first).
    #[cfg(unix)]
    #[test]
    fn spawn_error_names_a_failing_cwd() {
        let d = tempfile::tempdir().unwrap();
        let (env, cancel) = (env(), CancelToken::new());
        let missing = d.path().join("nonexistent-dir");
        let file = d.path().join("plain");
        std::fs::write(&file, b"").unwrap();
        let text = |prog: &str, cwd: &Path| {
            let argv = [OsString::from(prog)];
            let e = capture(&spec(&argv, &env, cwd, &cancel)).unwrap_err();
            (e.python_text(), e.io.kind())
        };
        let m = missing.to_str().unwrap();
        assert_eq!(
            text("/bin/true", &missing),
            (
                format!("[Errno 2] No such file or directory: '{m}'"),
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            text("/nonexistent/prog", &missing).0,
            format!("[Errno 2] No such file or directory: '{m}'")
        );
        assert_eq!(
            text("/bin/true", &file).0,
            format!("[Errno 20] Not a directory: '{}'", file.to_str().unwrap())
        );
        // A valid cwd keeps the program as the file name.
        assert_eq!(
            text("/nonexistent/prog", d.path()).0,
            "[Errno 2] No such file or directory: '/nonexistent/prog'"
        );
    }

    #[test]
    fn batch_line_break_refusal() {
        let args = [OsString::from("ok"), OsString::from("a\nb")];
        assert_eq!(
            batch_line_break(OsStr::new("C:\\x\\run.CMD"), &args).as_deref(),
            Some(
                "Executable `C:\\x\\run.CMD` cannot receive an argument containing a line break: 'a\\nb'"
            )
        );
        assert!(batch_line_break(OsStr::new("run.exe"), &args).is_none());
        assert!(batch_line_break(OsStr::new("run.bat"), &args[..1]).is_none());
    }

    #[test]
    fn collector_keeps_memory_charged_until_dropped_and_refunds_spills() {
        let b = budget(100);
        let mut c = Collector::new(&b);
        c.push(&[b'x'; 60]);
        assert_eq!(b.remaining.load(Ordering::Relaxed), 40);
        let held = c.finish();
        assert!(matches!(held, Output::Memory(ref v) if v.len() == 60));
        // A held memory output keeps its charge.
        assert_eq!(b.remaining.load(Ordering::Relaxed), 40);
        let mut c = Collector::new(&b);
        c.push(&[b'y'; 30]);
        c.push(&[b'z'; 30]);
        assert_eq!(b.remaining.load(Ordering::Relaxed), 40);
        let out = c.finish();
        assert!(matches!(out, Output::Spilled { len: 60, .. }));
        assert_eq!(bytes_of(&out), [vec![b'y'; 30], vec![b'z'; 30]].concat());
        drop(out);
        assert_eq!(b.remaining.load(Ordering::Relaxed), 40);
        // Dropping the memory output returns its charge.
        drop(held);
        assert_eq!(b.remaining.load(Ordering::Relaxed), 100);
        // An output that outlives its budget drops cleanly.
        let gone = budget(10);
        let mut c = Collector::new(&gone);
        c.push(b"abc");
        let out = c.finish();
        drop(gone);
        drop(out);
    }

    /// Regression: sequential captures, each below the budget but together far above it, all stay in memory once
    /// the previous output was dropped (the budget is not a cap on the run's total output).
    #[cfg(unix)]
    #[test]
    fn sequential_captures_below_the_budget_never_spill() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
        let cancel = CancelToken::new();
        let b = budget(4096);
        let argv = [
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("head -c 1500 /dev/zero"),
        ];
        for i in 0..5 {
            let m = merged(&spec(&argv, &env, d.path(), &cancel), false, &b).unwrap();
            assert!(
                matches!(m.output, Output::Memory(ref v) if v.len() == 1500),
                "capture {i} spilled: {:?}",
                m.output
            );
            drop(m);
        }
        assert_eq!(b.remaining.load(Ordering::Relaxed), 4096);
    }
}
