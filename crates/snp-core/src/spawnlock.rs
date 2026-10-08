//! fd hygiene under concurrent spawning (S15, ADR-037): a process-wide lock serialising every
//! `Command::spawn` and every non-atomically-CLOEXEC fd creation on macOS and other non-Linux
//! Unix; a no-op on Linux (atomic `O_CLOEXEC` everywhere) and Windows (std serialises handle
//! inheritance).

use std::sync::MutexGuard;

/// Held across a spawn or an fd creation. `!Send` (a `MutexGuard`): held only within one thread;
/// excluded from the `api.rs` Send assertions.
#[derive(Debug)]
pub struct SpawnGuard(
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "held only for its Drop (releases the lock)")
    )]
    Option<MutexGuard<'static, ()>>,
);

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes the spawn lock (poison-proof) where the platform needs it.
pub fn guard() -> SpawnGuard {
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    {
        SpawnGuard(Some(crate::sync::lock(&SPAWN_LOCK)))
    }
    #[cfg(not(all(unix, not(any(target_os = "linux", target_os = "android")))))]
    {
        SpawnGuard(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_is_reentrant_across_sequential_uses() {
        let g = guard();
        assert_eq!(
            g.0.is_some(),
            cfg!(all(
                unix,
                not(any(target_os = "linux", target_os = "android"))
            ))
        );
        drop(g);
        let _again = guard();
    }
}
