//! Poison-proof locking (S14, ADR-040): every safety-path lock goes through these, recovering the
//! guard with `PoisonError::into_inner` so a panicking worker cannot silence restore messages or
//! the crash banner.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// `m.lock()`, recovering a poisoned mutex.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `l.read()`, recovering a poisoned lock.
pub fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

/// `l.write()`, recovering a poisoned lock.
pub fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn poison(f: impl FnOnce() + Send + 'static) {
        let res = std::thread::Builder::new().spawn(f).unwrap().join();
        assert!(res.is_err());
    }

    #[test]
    fn lock_recovers_a_poisoned_mutex() {
        let m = Arc::new(Mutex::new(1));
        let m2 = Arc::clone(&m);
        poison(move || {
            let mut g = m2.lock().unwrap();
            *g = 2;
            panic!("poison");
        });
        assert!(m.is_poisoned());
        assert_eq!(*lock(&m), 2);
    }

    #[test]
    fn rwlock_recovers() {
        let l = Arc::new(RwLock::new(1));
        let l2 = Arc::clone(&l);
        poison(move || {
            let _g = l2.write().unwrap();
            panic!("poison");
        });
        assert!(l.is_poisoned());
        *write(&l) = 3;
        assert_eq!(*read(&l), 3);
    }
}
