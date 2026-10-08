//! Signal handling: cancellation, forwarding per the §10.4 table, restore shield (ADR-026, ADR-065).
//!
//! One handler is installed per process (one signal-hook iterator thread on Unix, one `ctrlc` handler on Windows);
//! it dispatches to the state of the most recent [`install`]. While no [`SignalGuard`] is alive a signal keeps its
//! default action (the process dies by it), so dropping the guard restores pre-commit-like behaviour.
//!
//! Owner: WP-15.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::env::Env;
use snp_core::sync;

use crate::registry::{self, Origin, Sig};

/// The default `SNP_KILL_GRACE`.
const DEFAULT_GRACE: Duration = Duration::from_secs(2);
/// The upper bound of `SNP_KILL_GRACE`.
const MAX_GRACE: Duration = Duration::from_secs(60);

/// What one [`install`] observes.
#[derive(Debug)]
struct State {
    cancel: CancelToken,
    /// The first signal: 0 none, 1 INT, 2 TERM, 3 HUP.
    first: AtomicU8,
    /// Live `RestoreShield`s.
    shields: AtomicUsize,
    /// The restore notice was printed during the current shield.
    notice_printed: AtomicBool,
    /// `SNP_KILL_GRACE`.
    grace: Duration,
}

impl State {
    fn first(&self) -> Option<Sig> {
        match self.first.load(Ordering::Acquire) {
            1 => Some(Sig::Int),
            2 => Some(Sig::Term),
            3 => Some(Sig::Hup),
            _ => None,
        }
    }
}

/// The state signals are dispatched to.
static ACTIVE: Mutex<Option<Arc<State>>> = Mutex::new(None);

/// Unix: one signal-hook iterator thread over `SignalsInfo<WithOrigin>` for SIGINT/SIGTERM/SIGHUP (caught, never
/// `SIG_IGN`, so children get default dispositions). Windows: ctrlc with `termination` (`CTRL_C`/`CTRL_BREAK` →
/// Interrupt; `CTRL_CLOSE`/`LOGOFF`/`SHUTDOWN`: Windows ends the process when the handler returns, so these are
/// journal-recovered events, §4.3). On the first signal: cancel(token, reason) and `registry::forward` per the §10.4
/// table; after `SNP_KILL_GRACE` (2 s; parsed with `try_from_secs_f64`, ≤ 60 s) `registry::escalate`. While a
/// `RestoreShield` is alive, further signals only print "snp: restoring stashed changes, please wait" (stderr) once.
/// `subreaper` (run, hook-impl, try-repo on Linux): `PR_SET_CHILD_SUBREAPER` via rustix, so orphaned hook descendants
/// reparent to snp and `registry::quiesce` can find them on the cancel path.
///
/// `SNP_KILL_GRACE` is read from the process environment through `Env::capture_process` (snp never mutates it, so
/// this equals the startup snapshot); an unparsable, negative or non-finite value means 2 s, values above 60 s are
/// clamped. The grace also bounds the deadline escalation of `spawn` (TERM → grace → KILL). pre-commit itself has no
/// handler: SIGINT raises `KeyboardInterrupt` (`error_handler.py:71-81`), SIGTERM/SIGHUP kill it outright.
pub fn install(cancel: CancelToken, subreaper: bool) -> io::Result<SignalGuard> {
    imp::ensure_handler()?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if subreaper {
        rustix::process::set_child_subreaper(Some(rustix::process::getpid()))?;
        registry::set_subreaper(true);
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let _: bool = subreaper;
    let grace = parse_grace(Env::capture_process().get("SNP_KILL_GRACE"));
    registry::set_kill_grace(grace);
    let state = Arc::new(State {
        cancel,
        first: AtomicU8::new(0),
        shields: AtomicUsize::new(0),
        notice_printed: AtomicBool::new(false),
        grace,
    });
    *sync::lock(&ACTIVE) = Some(Arc::clone(&state));
    Ok(SignalGuard { state })
}

/// `SNP_KILL_GRACE` seconds: `float(v)` semantics via `try_from_secs_f64`, at most 60 s; anything invalid → 2 s.
fn parse_grace(v: Option<&std::ffi::OsStr>) -> Duration {
    v.and_then(|v| v.to_str())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .and_then(|secs| Duration::try_from_secs_f64(secs.min(MAX_GRACE.as_secs_f64())).ok())
        .unwrap_or(DEFAULT_GRACE)
}

/// The installed handler (thread handle).
#[derive(Debug)]
pub struct SignalGuard {
    state: Arc<State>,
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        let mut active = sync::lock(&ACTIVE);
        if active.as_ref().is_some_and(|a| Arc::ptr_eq(a, &self.state)) {
            *active = None;
        }
    }
}

impl SignalGuard {
    /// Shield the stash restore from further signals.
    ///
    /// While any shield is alive a signal prints `snp: restoring stashed changes, please wait` once (per shielded
    /// period) and does nothing else beyond recording a first signal.
    pub fn shield(&self) -> RestoreShield {
        if self.state.shields.fetch_add(1, Ordering::AcqRel) == 0 {
            self.state.notice_printed.store(false, Ordering::Release);
        }
        RestoreShield {
            state: Arc::clone(&self.state),
        }
    }

    /// After restore: re-raise the received SIGTERM/SIGHUP with the default disposition
    /// (`signal_hook::low_level::emulate_default_handler`) so the parent sees status 143/129 (ADR-026).
    ///
    /// Returns `None` when no SIGTERM/SIGHUP was received (SIGINT exits 130 through the envelope) and on Windows.
    /// When the process survives the re-raise, returns the status to exit with instead (`Some(143)` / `Some(129)`).
    pub fn die_by_signal_if_terminated(&self) -> Option<i32> {
        let sig = self.state.first()?;
        #[cfg(unix)]
        {
            let (raw, code) = match sig {
                Sig::Term => (signal_hook::consts::SIGTERM, 143),
                Sig::Hup => (signal_hook::consts::SIGHUP, 129),
                Sig::Int => return None,
            };
            let _ignored = signal_hook::low_level::emulate_default_handler(raw);
            Some(code)
        }
        #[cfg(not(unix))]
        {
            let _: Sig = sig;
            None
        }
    }
}

/// Active while the stash is being restored; clears on drop.
#[derive(Debug)]
pub struct RestoreShield {
    state: Arc<State>,
}

impl Drop for RestoreShield {
    fn drop(&mut self) {
        self.state.shields.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reacts to one signal (the handler thread): the restore notice while shielded; on the first signal cancel,
/// forward per §10.4 and schedule the escalation after the grace. Returns `false` when no guard is active (the
/// caller applies the default action).
fn dispatch(sig: Sig, origin: Origin) -> bool {
    let Some(state) = sync::lock(&ACTIVE).clone() else {
        return false;
    };
    if state.shields.load(Ordering::Acquire) > 0
        && !state.notice_printed.swap(true, Ordering::AcqRel)
    {
        use std::io::Write;
        let _ignored = io::stderr().write_all(crate::messages::RESTORE_NOTICE.as_bytes());
    }
    let code = match sig {
        Sig::Int => 1,
        Sig::Term => 2,
        Sig::Hup => 3,
    };
    if state
        .first
        .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return true;
    }
    state.cancel.cancel(match sig {
        Sig::Int => CancelReason::Interrupt,
        Sig::Term => CancelReason::Terminate,
        Sig::Hup => CancelReason::Hangup,
    });
    registry::forward(sig, origin);
    let grace = state.grace;
    // The workers also kill their own children at the end of the grace; this thread covers children whose
    // workers are not polling (e.g. blocked writers). A refused thread is not fatal.
    let escalator = std::thread::Builder::new()
        .name("snp-escalate".to_owned())
        .stack_size(128 * 1024)
        .spawn(move || {
            std::thread::sleep(grace);
            registry::escalate();
        });
    drop(escalator);
    true
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::sync::OnceLock;

    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::SignalsInfo;
    use signal_hook::iterator::exfiltrator::origin::WithOrigin;
    use signal_hook::low_level::siginfo::{Cause, Origin as HookOrigin};

    use crate::registry::{Origin, Sig};

    /// The handler thread, started once (its error is kept as text: `io::Error` is not `Clone`).
    static HANDLER: OnceLock<Result<(), (io::ErrorKind, String)>> = OnceLock::new();

    pub(super) fn ensure_handler() -> io::Result<()> {
        HANDLER
            .get_or_init(|| start().map_err(|e| (e.kind(), e.to_string())))
            .clone()
            .map_err(|(kind, msg)| io::Error::new(kind, msg))
    }

    fn start() -> io::Result<()> {
        let mut signals = SignalsInfo::<WithOrigin>::new([SIGINT, SIGTERM, SIGHUP])?;
        std::thread::Builder::new()
            .name("snp-signals".to_owned())
            .stack_size(256 * 1024)
            .spawn(move || {
                for info in signals.forever() {
                    let sig = match info.signal {
                        SIGINT => Sig::Int,
                        SIGTERM => Sig::Term,
                        SIGHUP => Sig::Hup,
                        _ => continue,
                    };
                    if !super::dispatch(sig, origin_of(&info)) {
                        // No guard: the default action (death by the signal).
                        let _ignored = signal_hook::low_level::emulate_default_handler(info.signal);
                    }
                }
            })
            .map(drop)
    }

    /// `si_code`/`si_pid` → §10.4 origin: `SI_KERNEL` (Linux) is Kernel, `SI_USER`/`SI_QUEUE`/`SI_TKILL` is Sent
    /// (also from another pid namespace, where `si_pid` reads 0). macOS leaves `si_code` 0: a sender pid means
    /// Sent, none (pid 0) a terminal-generated signal (Kernel). Anything else is Unknown.
    fn origin_of(info: &HookOrigin) -> Origin {
        match info.cause {
            Cause::Kernel => Origin::Kernel,
            Cause::Sent(_) => Origin::Sent,
            _ => match info.process {
                Some(p) if p.pid > 0 => Origin::Sent,
                _ if cfg!(target_os = "macos") => Origin::Kernel,
                _ => Origin::Unknown,
            },
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::sync::OnceLock;

    use crate::registry::{Origin, Sig};

    /// `ctrlc::set_handler` succeeds once per process.
    static HANDLER: OnceLock<Result<(), String>> = OnceLock::new();

    pub(super) fn ensure_handler() -> io::Result<()> {
        HANDLER
            .get_or_init(|| {
                ctrlc::set_handler(|| {
                    // CTRL_C/CTRL_BREAK (and, with `termination`, close/logoff/shutdown): the console delivered the
                    // event to every attached process already (Kernel origin).
                    let _handled = super::dispatch(Sig::Int, Origin::Kernel);
                })
                .map_err(|e| e.to_string())
            })
            .clone()
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn grace_parsing() {
        let g = |s: &str| parse_grace(Some(OsStr::new(s)));
        assert_eq!(parse_grace(None), DEFAULT_GRACE);
        assert_eq!(g("0.5"), Duration::from_millis(500));
        assert_eq!(g(" 3 "), Duration::from_secs(3));
        assert_eq!(g("0"), Duration::ZERO);
        assert_eq!(g("61"), MAX_GRACE);
        assert_eq!(g("1e300"), MAX_GRACE);
        for bad in ["-1", "nan", "inf", "", "two", "1s"] {
            assert_eq!(g(bad), DEFAULT_GRACE, "{bad}");
        }
    }
}
