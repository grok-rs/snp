//! Signal handling: cancellation, forwarding per the §10.4 table, restore shield (ADR-026, ADR-065).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-15"
)]

use std::io;

use snp_core::cancel::CancelToken;

/// Unix: one signal-hook iterator thread over `SignalsInfo<WithOrigin>` for SIGINT/SIGTERM/SIGHUP (caught, never
/// `SIG_IGN`, so children get default dispositions). Windows: ctrlc with `termination` (`CTRL_C`/`CTRL_BREAK` →
/// Interrupt; `CTRL_CLOSE`/`LOGOFF`/`SHUTDOWN`: Windows ends the process when the handler returns, so these are
/// journal-recovered events, §4.3). On the first signal: cancel(token, reason) and `registry::forward` per the §10.4
/// table; after `SNP_KILL_GRACE` (2 s; parsed with `try_from_secs_f64`, ≤ 60 s) `registry::escalate`. While a
/// `RestoreShield` is alive, further signals only print "snp: restoring stashed changes, please wait" (stderr) once.
/// `subreaper` (run, hook-impl, try-repo on Linux): `PR_SET_CHILD_SUBREAPER` via rustix, so orphaned hook descendants
/// reparent to snp and `registry::quiesce` can find them on the cancel path.
pub fn install(cancel: CancelToken, subreaper: bool) -> io::Result<SignalGuard> {
    todo!()
}

/// The installed handler (thread handle).
#[derive(Debug)]
pub struct SignalGuard {
    /// W0 placeholder; WP-15 stores the handler thread.
    _priv: (),
}

impl SignalGuard {
    /// Shield the stash restore from further signals.
    pub fn shield(&self) -> RestoreShield {
        todo!()
    }

    /// After restore: re-raise the received SIGTERM/SIGHUP with the default disposition
    /// (`signal_hook::low_level::emulate_default_handler`) so the parent sees status 143/129 (ADR-026).
    pub fn die_by_signal_if_terminated(&self) -> Option<i32> {
        todo!()
    }
}

/// Active while the stash is being restored; clears on drop.
#[derive(Debug)]
pub struct RestoreShield {
    /// W0 placeholder.
    _priv: (),
}
