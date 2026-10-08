//! Every child spawned by `spawn::*` is registered until reaped (waitid(WNOWAIT) on Unix → no pid reuse; Windows keeps
//! the handle). Critical children are registered for reaping only: nothing here ever signals them (ADR-065).
//!
//! A child's cell holds the `Child` behind a mutex. Its worker detects the exit with
//! `waitid(WEXITED|WNOHANG|WNOWAIT)` (the zombie keeps its pid and, for an isolated child, its process group id
//! reserved), and reaps it only at the very end, under the cell's mutex, setting `reaped`. Every kill takes the same
//! mutex and skips reaped cells, so a signal can never reach a recycled pid (ARCHITECTURE §3.6 "reaping").
//!
//! Owner: WP-15.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use snp_core::cancel::CancelToken;
use snp_core::sync;

use crate::spawn::{Group, Role};

/// A terminating signal snp reacts to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sig {
    /// SIGINT / `CTRL_C`.
    Int,
    /// SIGTERM.
    Term,
    /// SIGHUP.
    Hup,
}

impl Sig {
    /// The POSIX signal.
    #[cfg(unix)]
    pub(crate) const fn signal(self) -> rustix::process::Signal {
        use rustix::process::Signal;
        match self {
            Self::Int => Signal::INT,
            Self::Term => Signal::TERM,
            Self::Hup => Signal::HUP,
        }
    }
}

/// Kernel = tty-generated (^C, hangup: `si_code` `SI_KERNEL` / `si_pid` 0, already delivered to the foreground group);
/// Sent = kill(2)/sigqueue (`SI_USER`/`SI_QUEUE`); Unknown = origin unavailable (treated as Sent for TERM/HUP, Kernel
/// for INT).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Origin {
    /// tty-generated.
    Kernel,
    /// kill(2)/sigqueue.
    Sent,
    /// Origin unavailable.
    Unknown,
}

/// Whether an Inherit-group child must be sent `sig` (ARCHITECTURE §10.4): only signals sent by a process; an
/// unknown origin counts as sent for TERM/HUP and as kernel-generated for INT.
#[cfg(any(unix, test))]
const fn reaches_inherit_children(sig: Sig, origin: Origin) -> bool {
    match origin {
        Origin::Sent => true,
        Origin::Kernel => false,
        Origin::Unknown => !matches!(sig, Sig::Int),
    }
}

/// The platform child object a [`Cell`] owns.
#[cfg(unix)]
pub(crate) type Proc = std::process::Child;

/// The platform child object a [`Cell`] owns: a plain child, or a hook inside its own Job Object.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) enum Proc {
    /// Tool and Critical children.
    Plain(std::process::Child),
    /// Hook children (process-wrap Job Object without `KILL_ON_JOB_CLOSE`).
    Job(Box<dyn process_wrap::std::ChildWrapper>),
}

#[cfg(windows)]
impl Proc {
    /// `TerminateJobObject` / `TerminateProcess`.
    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(c) => c.kill(),
            Self::Job(c) => c.start_kill(),
        }
    }

    /// Non-blocking exit check (the direct child only; job members may live on).
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            Self::Plain(c) => c.try_wait(),
            Self::Job(c) => c.try_wait(),
        }
    }
}

/// What a cell's mutex protects.
#[derive(Debug)]
pub(crate) struct CellState {
    /// The child; `None` only after it was handed to a background reaper or released.
    pub(crate) child: Option<Proc>,
    /// Reaped (Unix) / finished and released (Windows): never signal again.
    pub(crate) reaped: bool,
}

/// One registered child.
#[derive(Debug)]
pub(crate) struct Cell {
    /// Process id (and, for an isolated child, its process group id).
    pub(crate) pid: u32,
    /// The role of ADR-065.
    pub(crate) role: Role,
    /// Its process group.
    pub(crate) group: Group,
    /// The child and its reaped flag.
    pub(crate) state: Mutex<CellState>,
    /// The token of its spawn: `forward`, `escalate` and `quiesce` act only on children whose token is cancelled
    /// (a child spawned with an independent token, such as a restore-time helper, has opted out).
    cancel: CancelToken,
    /// Registry id.
    id: u64,
}

impl Cell {
    /// Locks the cell (poison-proof, S14).
    pub(crate) fn lock(&self) -> MutexGuard<'_, CellState> {
        sync::lock(&self.state)
    }
}

/// Registered children and (Windows) the Job Objects of finished hooks, kept for `quiesce`.
#[derive(Debug, Default)]
struct Registry {
    live: Vec<Arc<Cell>>,
    #[cfg(windows)]
    retired_jobs: std::collections::VecDeque<Proc>,
}

/// The process-wide registry; [`CHANGED`] is notified whenever a child is unregistered.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    live: Vec::new(),
    #[cfg(windows)]
    retired_jobs: std::collections::VecDeque::new(),
});
static CHANGED: Condvar = Condvar::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Spawns hold this shared across `spawn()` + [`register`]; the Linux descendant sweep of [`quiesce`] holds it
/// exclusively, so it never mistakes a child that is being registered for an orphan.
static SPAWN_GATE: RwLock<()> = RwLock::new(());

/// The kill grace in milliseconds (`SNP_KILL_GRACE`, default 2 s), set by `signals::install`.
static KILL_GRACE_MS: AtomicU64 = AtomicU64::new(2_000);

/// Whether this process is a child subreaper (Linux; set by `signals::install`).
#[cfg(any(target_os = "linux", target_os = "android"))]
static SUBREAPER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Most Job Objects of finished hooks kept for `quiesce` (Windows).
#[cfg(windows)]
const RETIRED_JOBS_MAX: usize = 4096;

/// The grace between the first signal (or a deadline's SIGTERM) and SIGKILL.
pub(crate) fn kill_grace() -> Duration {
    Duration::from_millis(KILL_GRACE_MS.load(Ordering::Relaxed))
}

/// Sets the grace (`signals::install`).
pub(crate) fn set_kill_grace(d: Duration) {
    KILL_GRACE_MS.store(
        u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

/// Records that `PR_SET_CHILD_SUBREAPER` is in effect.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn set_subreaper(on: bool) {
    SUBREAPER.store(on, Ordering::Relaxed);
}

/// The shared side of the spawn gate.
pub(crate) fn spawn_gate() -> RwLockReadGuard<'static, ()> {
    sync::read(&SPAWN_GATE)
}

/// Registers a freshly spawned child.
pub(crate) fn register(
    child: Proc,
    pid: u32,
    role: Role,
    group: Group,
    cancel: CancelToken,
) -> Arc<Cell> {
    let cell = Arc::new(Cell {
        pid,
        role,
        group,
        state: Mutex::new(CellState {
            child: Some(child),
            reaped: false,
        }),
        cancel,
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
    });
    sync::lock(&REGISTRY).live.push(Arc::clone(&cell));
    cell
}

/// Removes a reaped (or released) child and wakes `quiesce`.
pub(crate) fn unregister(cell: &Cell) {
    #[cfg(windows)]
    let retired = if cell.role == Role::Hook {
        cell.lock().child.take()
    } else {
        None
    };
    let mut reg = sync::lock(&REGISTRY);
    reg.live.retain(|c| c.id != cell.id);
    #[cfg(windows)]
    if let Some(job @ Proc::Job(_)) = retired {
        if reg.retired_jobs.len() >= RETIRED_JOBS_MAX {
            reg.retired_jobs.pop_front();
        }
        reg.retired_jobs.push_back(job);
    }
    drop(reg);
    CHANGED.notify_all();
    tracing::trace!(
        target: "snp::spawn",
        pid = cell.pid,
        role = ?cell.role,
        group = ?cell.group,
        "child released"
    );
}

/// A snapshot of the live cells.
fn live() -> Vec<Arc<Cell>> {
    sync::lock(&REGISTRY).live.clone()
}

/// Sends `sig` to the child of `cell` (its process group when isolated) unless it was reaped. Errors (`ESRCH`)
/// are ignored. Never called for Critical cells except by their own worker (the tty-stop kill).
#[cfg(unix)]
pub(crate) fn signal_cell(cell: &Cell, sig: rustix::process::Signal) {
    let st = cell.lock();
    if st.reaped {
        return;
    }
    let Some(pid) = i32::try_from(cell.pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let _ignored = match cell.group {
        Group::Isolated => rustix::process::kill_process_group(pid, sig),
        Group::Inherit => rustix::process::kill_process(pid, sig),
    };
    drop(st);
}

/// Terminates the child of `cell` (Windows: its Job Object for hooks, the process otherwise) unless released.
#[cfg(windows)]
pub(crate) fn kill_cell(cell: &Cell) {
    let mut st = cell.lock();
    if st.reaped {
        return;
    }
    if let Some(child) = st.child.as_mut() {
        let _ignored = child.kill();
    }
}

/// SIGKILL (Unix) / terminate (Windows) the child of a non-Critical cell.
pub(crate) fn kill_now(cell: &Cell) {
    if cell.role == Role::Critical {
        return;
    }
    #[cfg(unix)]
    signal_cell(cell, rustix::process::Signal::KILL);
    #[cfg(windows)]
    kill_cell(cell);
}

/// First phase of §10.4: Inherit hooks/tools only when Sent; Isolated hooks by -pgid always.
///
/// Called by the signal thread on the first signal (ADR-065). Kernel-generated signals already reached every
/// Inherit-group child through the terminal's foreground process group and are never re-sent (a double SIGINT is
/// "force quit" for npm, cargo, uv); isolated groups (hooks with a `timeout`) never see the terminal and always get
/// the signal by `-pgid`; Critical children are never signalled. Windows: a no-op (the console delivers `CTRL_C` to
/// Inherit children; isolated groups are terminated with their Job Object after the grace).
pub fn forward(sig: Sig, origin: Origin) {
    #[cfg(unix)]
    for cell in live() {
        if !affected(&cell) {
            continue;
        }
        match cell.group {
            Group::Isolated => signal_cell(&cell, sig.signal()),
            Group::Inherit if reaches_inherit_children(sig, origin) => {
                signal_cell(&cell, sig.signal());
            }
            Group::Inherit => {}
        }
    }
    #[cfg(not(unix))]
    let _: (Sig, Origin) = (sig, origin);
}

/// After the grace: SIGKILL Hook/Tool children (Isolated by -pgid), Windows job terminate.
///
/// The second phase of the §10.4 table (pid-reuse-safe: reaped cells are skipped). Critical children, and children
/// whose spawn token is not cancelled, are never touched.
pub fn escalate() {
    for cell in live() {
        if affected(&cell) {
            kill_now(&cell);
        }
    }
}

/// Whether the cancellation reaches `cell`: a Hook/Tool child whose spawn token is cancelled (the run's token or a
/// child of it). Critical children and children spawned with an independent, uncancelled token are never touched.
fn affected(cell: &Cell) -> bool {
    cell.role != Role::Critical && cell.cancel.is_cancelled()
}

/// Cancel path only, before the restore: wait ≤ `bound` for Hook/Tool children to exit, then (Linux, subreaper)
/// SIGKILL and reap every remaining non-Critical descendant found under /proc/self/task/*/children; Windows: the run's
/// hook Job Object is terminated; macOS: Isolated groups by -pgid only (residual risk, D-DATA-02).
///
/// Remaining registered Hook/Tool children of the cancelled token are killed with SIGKILL (by `-pgid` when isolated)
/// once `bound` elapses; their workers reap them. On Linux with `PR_SET_CHILD_SUBREAPER` (`signals::install(_, true)`) the orphaned descendants
/// of finished hooks have been reparented to snp: every child of this process that is not registered is killed and
/// reaped, in rounds, until none is left (their own children reparent in turn); Critical children and their
/// descendants are never touched. Windows terminates the Job Objects of live and finished hooks.
pub fn quiesce(bound: Duration) {
    let deadline = Instant::now().checked_add(bound);
    {
        let mut reg = sync::lock(&REGISTRY);
        loop {
            if !reg.live.iter().any(|c| affected(c)) {
                break;
            }
            let left = deadline.map_or(Duration::from_millis(50), |d| {
                d.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                break;
            }
            reg = match CHANGED.wait_timeout(reg, left.min(Duration::from_millis(50))) {
                Ok((g, _)) => g,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
    escalate();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if SUBREAPER.load(Ordering::Relaxed) {
        sweep_orphans();
    }
    #[cfg(windows)]
    {
        let mut reg = sync::lock(&REGISTRY);
        for job in &mut reg.retired_jobs {
            let _ignored = job.kill();
        }
        reg.retired_jobs.clear();
    }
}

/// The direct children of this process (every thread's `/proc/self/task/<tid>/children`).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn proc_children() -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return out;
    };
    for task in tasks.flatten() {
        let Ok(text) = std::fs::read_to_string(task.path().join("children")) else {
            continue;
        };
        out.extend(
            text.split_ascii_whitespace()
                .filter_map(|p| p.parse::<i32>().ok()),
        );
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// SIGKILLs and reaps every unregistered child (orphaned hook descendants reparented to this subreaper), in
/// rounds bounded to ~2 s in total.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn sweep_orphans() {
    use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};
    let _gate = sync::write(&SPAWN_GATE);
    let start = Instant::now();
    loop {
        let registered: Vec<u32> = live().iter().map(|c| c.pid).collect();
        let orphans: Vec<Pid> = proc_children()
            .into_iter()
            .filter(|p| u32::try_from(*p).is_ok_and(|p| !registered.contains(&p)))
            .filter_map(Pid::from_raw)
            .collect();
        if orphans.is_empty() {
            return;
        }
        for pid in &orphans {
            // Our child (zombie or alive) until we reap it: the pid cannot have been recycled.
            let _ignored = rustix::process::kill_process(*pid, Signal::KILL);
        }
        for pid in orphans {
            loop {
                match rustix::process::waitid(
                    WaitId::Pid(pid),
                    WaitIdOptions::EXITED | WaitIdOptions::NOHANG,
                ) {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => {
                        if start.elapsed() > Duration::from_secs(2) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        }
        if start.elapsed() > Duration::from_secs(2) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarding_table_for_inherit_children() {
        use Origin::{Kernel, Sent, Unknown};
        use Sig::{Hup, Int, Term};
        // ARCHITECTURE §10.4: kernel-origin signals are never re-sent; sent ones always; unknown = sent for
        // TERM/HUP, kernel for INT.
        for (sig, origin, expected) in [
            (Int, Kernel, false),
            (Int, Sent, true),
            (Int, Unknown, false),
            (Term, Kernel, false),
            (Term, Sent, true),
            (Term, Unknown, true),
            (Hup, Kernel, false),
            (Hup, Sent, true),
            (Hup, Unknown, true),
        ] {
            assert_eq!(
                reaches_inherit_children(sig, origin),
                expected,
                "{sig:?} {origin:?}"
            );
        }
    }
}
