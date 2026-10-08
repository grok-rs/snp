//! Every child spawned by `spawn::*` is registered until reaped (waitid(WNOWAIT) on Unix → no pid reuse; Windows keeps
//! the handle). Critical children are registered for reaping only: nothing here ever signals them (ADR-065).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::time::Duration;

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

/// First phase of §10.4: Inherit hooks/tools only when Sent; Isolated hooks by -pgid always.
pub fn forward(sig: Sig, origin: Origin) {
    todo!()
}

/// After the grace: SIGKILL Hook/Tool children (Isolated by -pgid), Windows job terminate.
pub fn escalate() {
    todo!()
}

/// Cancel path only, before the restore: wait ≤ `bound` for Hook/Tool children to exit, then (Linux, subreaper)
/// SIGKILL and reap every remaining non-Critical descendant found under /proc/self/task/*/children; Windows: the run's
/// hook Job Object is terminated; macOS: Isolated groups by -pgid only (residual risk, D-DATA-02).
pub fn quiesce(bound: Duration) {
    todo!()
}
