//! Run cancellation (ADR-065, ADR-066): one token per run, cancelled by the signal thread, the
//! Printer (EPIPE) or the panic hook. `fail_fast` is a scheduler decision, never a cancellation.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Why a run was cancelled. `Panic` is set by the panic hook.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CancelReason {
    /// SIGINT / console Ctrl-C (exit 130 after the restore).
    Interrupt,
    /// SIGTERM (re-raised after the restore: 143).
    Terminate,
    /// SIGHUP (re-raised after the restore: 129).
    Hangup,
    /// stdout returned EPIPE (exit 120, silent).
    StdoutClosed,
    /// A thread panicked (exit 3, crash log).
    Panic,
}

impl CancelReason {
    const fn code(self) -> u8 {
        match self {
            Self::Interrupt => 1,
            Self::Terminate => 2,
            Self::Hangup => 3,
            Self::StdoutClosed => 4,
            Self::Panic => 5,
        }
    }

    const fn from_code(c: u8) -> Option<Self> {
        match c {
            1 => Some(Self::Interrupt),
            2 => Some(Self::Terminate),
            3 => Some(Self::Hangup),
            4 => Some(Self::StdoutClosed),
            5 => Some(Self::Panic),
            _ => None,
        }
    }
}

/// Longest single condvar wait: bounds the delay of a wake-up lost to `cancel`'s `try_lock`.
const WAIT_SLICE: Duration = Duration::from_millis(10);

/// Wake-up channel shared by a token and all its descendants.
#[derive(Debug, Default)]
struct Notify {
    lock: Mutex<()>,
    cv: Condvar,
}

#[derive(Debug)]
struct Inner {
    /// 0 = not cancelled, else [`CancelReason::code`]; written once (compare-exchange).
    reason: AtomicU8,
    parent: Option<Arc<Self>>,
    notify: Arc<Notify>,
}

impl Inner {
    /// The first reason along the chain self → root.
    fn reason(&self) -> Option<CancelReason> {
        let mut cur = Some(self);
        while let Some(i) = cur {
            if let Some(r) = CancelReason::from_code(i.reason.load(Ordering::Acquire)) {
                return Some(r);
            }
            cur = i.parent.as_deref();
        }
        None
    }
}

/// A cancellation token (cheap to clone; clones share the state).
#[derive(Clone, Debug)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    /// A fresh, uncancelled root token.
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            reason: AtomicU8::new(0),
            parent: None,
            notify: Arc::new(Notify::default()),
        }))
    }

    /// Cancels with `r`; the first reason wins (atomic compare-exchange; a token whose ancestor is
    /// already cancelled records the ancestor's reason). Waiters are woken with a `try_lock` +
    /// `notify_all`, so `cancel` never blocks: it is safe from the signal thread and from the panic
    /// hook, even while the cancelling thread holds other locks.
    pub fn cancel(&self, r: CancelReason) {
        let effective = self
            .0
            .parent
            .as_deref()
            .and_then(Inner::reason)
            .unwrap_or(r);
        let won = self
            .0
            .reason
            .compare_exchange(0, effective.code(), Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if won {
            let guard = self.0.notify.lock.try_lock();
            self.0.notify.cv.notify_all();
            drop(guard);
        }
    }

    /// The reason of this token or, failing that, of the nearest cancelled ancestor.
    pub fn reason(&self) -> Option<CancelReason> {
        self.0.reason()
    }

    /// Whether this token or an ancestor is cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.reason().is_some()
    }

    /// Waits up to `d` for cancellation; `true` if cancelled.
    pub fn wait_timeout(&self, d: Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        let deadline = Instant::now().checked_add(d);
        let mut guard = crate::sync::lock(&self.0.notify.lock);
        loop {
            if self.is_cancelled() {
                return true;
            }
            let remaining = deadline.map_or(WAIT_SLICE, |dl| {
                dl.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() {
                return false;
            }
            guard = match self
                .0
                .notify
                .cv
                .wait_timeout(guard, remaining.min(WAIT_SLICE))
            {
                Ok((g, _)) => g,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// A child token: cancelled whenever this token is; its own cancellation does not propagate up.
    #[must_use]
    pub fn child(&self) -> Self {
        Self(Arc::new(Inner {
            reason: AtomicU8::new(0),
            parent: Some(Arc::clone(&self.0)),
            notify: Arc::clone(&self.0.notify),
        }))
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_reason_wins() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled() && t.reason().is_none());
        t.cancel(CancelReason::Terminate);
        t.cancel(CancelReason::Interrupt);
        assert_eq!(t.reason(), Some(CancelReason::Terminate));
        assert!(t.is_cancelled());
    }

    #[test]
    fn children_follow_parents_not_vice_versa() {
        let parent = CancelToken::default();
        let child = parent.child();
        child.cancel(CancelReason::StdoutClosed);
        assert!(child.is_cancelled() && !parent.is_cancelled());
        let other = parent.child();
        parent.cancel(CancelReason::Hangup);
        assert_eq!(other.reason(), Some(CancelReason::Hangup));
        assert_eq!(
            child.reason(),
            Some(CancelReason::StdoutClosed),
            "own first reason kept"
        );
        other.cancel(CancelReason::Panic);
        assert_eq!(
            other.reason(),
            Some(CancelReason::Hangup),
            "the ancestor's earlier reason wins"
        );
    }

    #[test]
    fn wait_timeout_returns_on_cancel_and_on_timeout() {
        let t = CancelToken::new();
        assert!(!t.wait_timeout(Duration::from_millis(20)));
        let child = t.child();
        let waiter = std::thread::Builder::new()
            .spawn(move || child.wait_timeout(Duration::from_secs(30)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let start = Instant::now();
        t.cancel(CancelReason::Interrupt);
        assert!(waiter.join().unwrap());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(t.wait_timeout(Duration::MAX));
    }

    #[test]
    fn cancel_never_blocks_while_the_lock_is_held() {
        let t = CancelToken::new();
        let held = t.0.notify.lock.lock().unwrap();
        t.cancel(CancelReason::Panic);
        drop(held);
        assert_eq!(t.reason(), Some(CancelReason::Panic));
    }
}
