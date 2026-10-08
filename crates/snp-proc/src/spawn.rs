//! Spawning children (`capture`, `merged`, `stream`, `inherit`) with roles and groups (§3.6, ADR-065).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-15"
)]

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
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
    pub fn write_to(&self, w: &mut dyn io::Write) -> io::Result<()> {
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
    pub fn python_text(&self) -> String {
        todo!()
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
    todo!()
}

/// One pipe for stdout+stderr (`std::io::pipe`, Command dropped before reading), or a POSIX pty when `pty`: master
/// `openpt(RDWR|NOCTTY|CLOEXEC)`, slave `open(ptsname, O_RDWR|O_NOCTTY|O_CLOEXEC)` (both under the spawn lock on
/// macOS), OPOST|ONLCR cleared, stdin /dev/null, 0×0 size, slave dropped before reading, EIO = EOF. pty failure →
/// pipe (X13-EXE-06). Hooks only.
pub fn merged(spec: &Spec<'_>, pty: bool, budget: &OutputBudget) -> Result<Merged, SpawnError> {
    todo!()
}

/// stdout streamed into `sink` (diff hashing, stash patch), stderr captured. Returns status and stderr.
pub fn stream(
    spec: &Spec<'_>,
    sink: &mut dyn io::Write,
) -> Result<(PyStatus, Vec<u8>), SpawnError> {
    todo!()
}

/// stdio inherited: legacy hooks, show-diff, hazmat children.
pub fn inherit(spec: &Spec<'_>) -> Result<PyStatus, SpawnError> {
    todo!()
}
