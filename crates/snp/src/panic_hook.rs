//! The panic hook (§10.1, ADR-066): records payload, location and backtrace into a `try_lock` slot and cancels the
//! process token with `CancelReason::Panic`. It runs on the panicking thread, possibly while that thread holds the
//! Printer, registry or other locks, so it never prints, never blocks on a lock (a busy slot drops the record),
//! never touches the Printer or the child registry and never restores; the envelope runs on the main thread after
//! `catch_unwind` (`take_failure`).
//!
//! Owner: WP-33 (created by WP-00).

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::sync::{Mutex, OnceLock, TryLockError};

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::{ExcKind, Failure, PyExc};

/// What the hook keeps of the first panic.
#[derive(Debug)]
struct Record {
    payload: String,
    location: Option<String>,
    backtrace: Backtrace,
}

/// The first recorded panic (later panics and panics racing a busy slot are dropped).
static SLOT: Mutex<Option<Record>> = Mutex::new(None);
/// The process-wide cancel token the hook cancels (lock-free compare-exchange).
static TOKEN: OnceLock<CancelToken> = OnceLock::new();

/// Installs the hook (replacing std's printing default hook) and returns the process cancel token.
pub(crate) fn install() -> CancelToken {
    let token = TOKEN.get_or_init(CancelToken::new).clone();
    std::panic::set_hook(Box::new(record_panic));
    token
}

/// The hook itself: `try_lock` only, then the lock-free cancel.
fn record_panic(info: &PanicHookInfo<'_>) {
    let slot = match SLOT.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    };
    if let Some(mut slot) = slot
        && slot.is_none()
    {
        *slot = Some(Record {
            payload: info.payload_as_str().unwrap_or("Box<dyn Any>").to_owned(),
            location: info.location().map(ToString::to_string),
            // std's capture lock is internal to the backtrace machinery, never one of snp's locks.
            backtrace: Backtrace::force_capture(),
        });
    }
    if let Some(token) = TOKEN.get() {
        token.cancel(CancelReason::Panic);
    }
}

/// The recorded panic as a `PanicException` failure (exit 3), consuming the record. Main thread only.
pub(crate) fn take_failure() -> Failure {
    let record = snp_core::sync::lock(&SLOT).take();
    let exc = match record {
        Some(r) => PyExc::panic(
            &r.payload,
            r.location.as_deref(),
            Some(r.backtrace.to_string()),
        ),
        None => PyExc::panic("panic payload unavailable", None, None),
    };
    Failure::Exc {
        kind: ExcKind::Unexpected,
        exc,
    }
}

/// The recorded panic's payload (the `__hazmat` path, which has no envelope).
pub(crate) fn take_payload() -> Option<String> {
    snp_core::sync::lock(&SLOT).take().map(|r| r.payload)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// The hook must complete although another thread holds the slot (and, by construction, every other lock).
    #[test]
    fn hook_never_blocks_on_a_busy_slot() {
        let token = install();
        let held = snp_core::sync::lock(&SLOT);
        let (tx, rx) = mpsc::channel();
        let joiner = std::thread::Builder::new()
            .spawn(move || {
                let panicker = std::thread::Builder::new()
                    .spawn(|| panic!("boom while the slot is busy"))
                    .unwrap();
                tx.send(panicker.join().is_err()).unwrap();
            })
            .unwrap();
        let panicked = rx.recv_timeout(Duration::from_secs(20));
        drop(held);
        joiner.join().unwrap();
        assert_eq!(panicked, Ok(true), "the panic hook blocked on a held lock");
        assert_eq!(token.reason(), Some(CancelReason::Panic));
        // The busy slot dropped the record instead of waiting for it.
        assert!(take_payload().is_none());
        // Give the other tests of this binary std's printing hook back.
        drop(std::panic::take_hook());
    }
}
