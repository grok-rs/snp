//! The global bound on concurrent children and native partitions (S7, ADR-065, ADR-070).
//!
//! Owner: WP-15.

use std::num::NonZeroUsize;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::{ExcKind, Failure, PyExc};
use snp_core::sync;

/// How often a blocked `acquire` re-checks its cancellation token.
const POLL: Duration = Duration::from_millis(50);

/// N (S7, ADR-065): `explicit` (`-j`/`SNP_JOBS`) when given, else min(target, `available_parallelism()`) — cgroup-aware,
/// and invisible in output because partition contents always use pre-commit's `jobs`; then clamped to the fd budget
/// max(1, (`RLIMIT_NOFILE` soft − 64) / 8) on Unix. The rlimit itself is never raised (children would inherit it).
///
/// pre-commit runs `min(partitions, target_concurrency())` threads per hook (`xargs.py:166-170`); snp bounds all
/// concurrent children of the run with one pool of this size.
pub fn pool_size(explicit: Option<NonZeroUsize>, target: NonZeroUsize) -> NonZeroUsize {
    let n = explicit.unwrap_or_else(|| {
        std::thread::available_parallelism().map_or(target, |avail| target.min(avail))
    });
    n.min(fd_budget())
}

/// max(1, (`RLIMIT_NOFILE` soft − 64) / 8): each partition holds ~8 fds (pipe or pty pair, spill file, reader and
/// stdin threads' handles). Unlimited or unknown → no clamp.
fn fd_budget() -> NonZeroUsize {
    #[cfg(unix)]
    {
        let soft = rustix::process::getrlimit(rustix::process::Resource::Nofile).current;
        fd_budget_for(soft)
    }
    #[cfg(not(unix))]
    {
        NonZeroUsize::MAX
    }
}

/// The clamp for a soft `RLIMIT_NOFILE` (`None` = unlimited).
#[cfg(any(unix, test))]
fn fd_budget_for(soft: Option<u64>) -> NonZeroUsize {
    soft.map_or(NonZeroUsize::MAX, |s| {
        let per = s.saturating_sub(64) / 8;
        NonZeroUsize::new(usize::try_from(per).unwrap_or(usize::MAX)).unwrap_or(NonZeroUsize::MIN)
    })
}

thread_local! {
    /// Counted permits held by this thread (ADR-070 re-entrancy). Shared with the permits it acquired, so a permit
    /// dropped on another thread still decrements the acquiring thread's count.
    static HELD: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
}

/// This thread's held-permit counter (`None` while thread-local storage is being torn down).
fn held_counter() -> Option<Arc<AtomicUsize>> {
    HELD.try_with(Arc::clone).ok()
}

/// The FIFO state of a [`JobPool`].
#[derive(Debug)]
struct PoolState {
    /// Permits not handed out.
    available: usize,
    /// The next ticket to hand out.
    next_ticket: u64,
    /// The ticket allowed to take the next permit.
    serving: u64,
}

/// The global bound on concurrent child processes / native partitions (S7). Fair FIFO handout.
/// Re-entrancy (ADR-070): permits are held only by process partitions and native partitions. A thread that already
/// holds a permit (thread-local flag) runs nested `map_ordered` work inline and sequentially without acquiring
/// (debug builds assert that a permit holder never blocks in `acquire`); bucket-level hook threads hold no permit.
/// So `PRE_COMMIT_NO_CONCURRENCY=1` (N = 1) cannot deadlock a meta hook that classifies files.
///
/// A permit holder that calls [`JobPool::acquire`] gets an uncounted nested permit at once (it already occupies a
/// slot with its own thread), so a holder can never block there. A permit may be dropped on any thread: it always
/// clears the acquiring thread's flag.
#[derive(Debug)]
pub struct JobPool {
    state: Mutex<PoolState>,
    cv: Condvar,
    cancel: CancelToken,
    permits: NonZeroUsize,
}

/// One permit of a [`JobPool`]; releases on drop.
#[derive(Debug)]
pub struct Permit<'a> {
    pool: &'a JobPool,
    /// Counted against the pool (false for a nested permit of a holder).
    counted: bool,
    /// The acquiring thread's held-permit counter (counted permits only), decremented on drop on any thread.
    held: Option<Arc<AtomicUsize>>,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        if !self.counted {
            return;
        }
        if let Some(h) = self.held.take() {
            // Incremented when this permit was handed out, so it is at least 1.
            h.fetch_sub(1, Ordering::AcqRel);
        }
        let mut st = sync::lock(&self.pool.state);
        st.available = st.available.saturating_add(1);
        drop(st);
        self.pool.cv.notify_all();
    }
}

impl JobPool {
    /// A pool with `permits` permits, refusing new work once `cancel` is cancelled.
    pub fn new(permits: NonZeroUsize, cancel: CancelToken) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PoolState {
                available: permits.get(),
                next_ticket: 0,
                serving: 0,
            }),
            cv: Condvar::new(),
            cancel,
            permits,
        })
    }

    /// The number of permits.
    pub fn permits(&self) -> NonZeroUsize {
        self.permits
    }

    /// None once cancelled (stop starting work).
    ///
    /// Waiters are served in arrival order (tickets). A thread that already holds a permit gets an uncounted
    /// nested permit immediately.
    pub fn acquire(&self) -> Option<Permit<'_>> {
        if self.cancel.is_cancelled() {
            return None;
        }
        if Self::holds_permit() {
            return Some(Permit {
                pool: self,
                counted: false,
                held: None,
            });
        }
        let mut st = sync::lock(&self.state);
        let ticket = st.next_ticket;
        st.next_ticket = st.next_ticket.wrapping_add(1);
        loop {
            if self.cancel.is_cancelled() {
                // Hand our turn on so later tickets are not stuck behind us.
                if st.serving == ticket {
                    st.serving = st.serving.wrapping_add(1);
                }
                drop(st);
                self.cv.notify_all();
                return None;
            }
            if st.serving == ticket && st.available > 0 {
                st.available = st.available.saturating_sub(1);
                st.serving = st.serving.wrapping_add(1);
                drop(st);
                self.cv.notify_all();
                let held = held_counter();
                if let Some(h) = &held {
                    h.fetch_add(1, Ordering::AcqRel);
                }
                return Some(Permit {
                    pool: self,
                    counted: true,
                    held,
                });
            }
            st = match self.cv.wait_timeout(st, POLL) {
                Ok((g, _)) => g,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// Runs `f(0..n)` on ≤ min(limit, n) scoped threads (`Builder::spawn_scoped`, every handle joined), each call
    /// holding a permit; results in index order. n == 1, or a caller that holds a permit, runs inline. A panicking
    /// call becomes `Err(Failure::Exc(PanicException …))`; a refused thread spawn becomes `Err(Failure::from_io)`.
    ///
    /// `xargs.py:166-184` (`ThreadPoolExecutor(threads).map` keeps results in partition order). Indices are handed
    /// out in order; an index whose permit is refused (cancellation) yields `Err(Failure::Interrupted(reason))`.
    /// When the OS refuses even the first worker thread every result is that `Failure`; later refusals just leave
    /// fewer workers.
    pub fn map_ordered<T: Send, F: Fn(usize) -> Result<T, Failure> + Sync>(
        &self,
        limit: NonZeroUsize,
        n: usize,
        f: F,
    ) -> Vec<Result<T, Failure>> {
        if n == 0 {
            return Vec::new();
        }
        if Self::holds_permit() {
            return (0..n).map(|i| call(&f, i)).collect();
        }
        if n == 1 {
            return vec![self.run_one(&f, 0)];
        }
        let threads = limit.get().min(n);
        let slots: Vec<Mutex<Option<Result<T, Failure>>>> =
            (0..n).map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        let worker = || {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let r = self.run_one(&f, i);
                if let Some(slot) = slots.get(i) {
                    *sync::lock(slot) = Some(r);
                }
            }
        };
        let spawn_error = std::thread::scope(|s| {
            let mut handles = Vec::new();
            let mut first_error = None;
            for k in 0..threads {
                match std::thread::Builder::new()
                    .name(format!("snp-pool-{k}"))
                    .spawn_scoped(s, worker)
                {
                    Ok(h) => handles.push(h),
                    Err(e) => {
                        first_error = Some(e);
                        break;
                    }
                }
            }
            let none_started = handles.is_empty();
            for h in handles {
                // `worker` catches every panic of `f`; a join error cannot carry a result.
                let _joined = h.join();
            }
            if none_started { first_error } else { None }
        });
        if let Some(e) = spawn_error {
            return (0..n)
                .map(|_| Err(Failure::from_io(&e, None, None)))
                .collect();
        }
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .unwrap_or_else(|| Err(Failure::Interrupted(self.reason())))
            })
            .collect()
    }

    /// Acquires a permit and runs `f(i)` under it.
    fn run_one<T, F: Fn(usize) -> Result<T, Failure>>(
        &self,
        f: &F,
        i: usize,
    ) -> Result<T, Failure> {
        match self.acquire() {
            Some(permit) => {
                let r = call(f, i);
                drop(permit);
                r
            }
            None => Err(Failure::Interrupted(self.reason())),
        }
    }

    /// The cancellation reason (`Interrupt` if the token reports none).
    fn reason(&self) -> CancelReason {
        self.cancel.reason().unwrap_or(CancelReason::Interrupt)
    }

    /// The calling thread's flag (for nested callers).
    pub fn holds_permit() -> bool {
        HELD.try_with(|h| h.load(Ordering::Acquire) > 0)
            .unwrap_or(false)
    }
}

/// `f(i)` with a panic turned into `PanicException` (exit 3, ADR-066).
fn call<T, F: Fn(usize) -> Result<T, Failure>>(f: &F, i: usize) -> Result<T, Failure> {
    match std::panic::catch_unwind(AssertUnwindSafe(|| f(i))) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "Box<dyn Any>".to_owned());
            Err(Failure::Exc {
                kind: ExcKind::Unexpected,
                exc: PyExc::panic(&msg, None, None),
            })
        }
    }
}

/// Independent counting semaphores: `NetLimiter` (8 concurrent clones/downloads), `InstallLimiter` (min(N, 4); 1 when
/// the cgroup memory limit is below 4 GiB). Compile-heavy installs (rust, golang, haskell, swift, dotnet, dart, conda)
/// take every permit (`acquire_all`), so they never run beside another install (ADR-065).
///
/// A pending `acquire_all` blocks new single acquisitions (writer preference), so a compile-heavy install is never
/// starved by a stream of small ones.
#[derive(Debug)]
pub struct Limiter {
    state: Mutex<LimiterState>,
    cv: Condvar,
    permits: NonZeroUsize,
}

#[derive(Debug)]
struct LimiterState {
    available: usize,
    /// `acquire_all` callers waiting.
    exclusive_waiters: usize,
}

impl Limiter {
    /// A limiter with `n` permits.
    pub fn new(n: NonZeroUsize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(LimiterState {
                available: n.get(),
                exclusive_waiters: 0,
            }),
            cv: Condvar::new(),
            permits: n,
        })
    }

    /// The number of permits.
    pub fn permits(&self) -> NonZeroUsize {
        self.permits
    }

    /// One permit; None once `c` is cancelled.
    pub fn acquire(&self, c: &CancelToken) -> Option<LimiterPermit<'_>> {
        self.take(c, false)
    }

    /// Every permit (compile-heavy installs); None once `c` is cancelled.
    pub fn acquire_all(&self, c: &CancelToken) -> Option<LimiterPermit<'_>> {
        self.take(c, true)
    }

    fn take(&self, c: &CancelToken, all: bool) -> Option<LimiterPermit<'_>> {
        let want = if all { self.permits.get() } else { 1 };
        let mut st = sync::lock(&self.state);
        if all {
            st.exclusive_waiters = st.exclusive_waiters.saturating_add(1);
        }
        let got = loop {
            if c.is_cancelled() {
                break false;
            }
            let blocked_by_exclusive = !all && st.exclusive_waiters > 0;
            if !blocked_by_exclusive && st.available >= want {
                st.available = st.available.saturating_sub(want);
                break true;
            }
            st = match self.cv.wait_timeout(st, POLL) {
                Ok((g, _)) => g,
                Err(poisoned) => poisoned.into_inner().0,
            };
        };
        if all {
            st.exclusive_waiters = st.exclusive_waiters.saturating_sub(1);
        }
        drop(st);
        self.cv.notify_all();
        // Lazily: a permit built for a refused request would release phantom permits on drop.
        got.then(|| LimiterPermit {
            limiter: self,
            count: want,
        })
    }
}

/// Permits of a [`Limiter`]; released on drop.
#[derive(Debug)]
pub struct LimiterPermit<'a> {
    limiter: &'a Limiter,
    count: usize,
}

impl Drop for LimiterPermit<'_> {
    fn drop(&mut self) {
        let mut st = sync::lock(&self.limiter.state);
        st.available = st.available.saturating_add(self.count);
        drop(st);
        self.limiter.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use super::*;

    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    #[test]
    fn pool_size_table() {
        let avail = std::thread::available_parallelism().unwrap();
        let budget = fd_budget();
        // Explicit wins (still fd-clamped).
        assert_eq!(pool_size(Some(nz(3)), nz(64)), nz(3).min(budget));
        // Otherwise min(target, available_parallelism) — cgroup-aware.
        assert_eq!(pool_size(None, nz(1)), nz(1));
        assert_eq!(pool_size(None, nz(100_000)), avail.min(budget));
        // The fd clamp.
        assert_eq!(fd_budget_for(None), NonZeroUsize::MAX);
        assert_eq!(fd_budget_for(Some(64)), nz(1));
        assert_eq!(fd_budget_for(Some(0)), nz(1));
        assert_eq!(fd_budget_for(Some(256)), nz(24)); // macOS default soft limit
        assert_eq!(fd_budget_for(Some(1024)), nz(120));
        assert_eq!(
            fd_budget_for(Some(u64::MAX)),
            nz(usize::try_from((u64::MAX - 64) / 8).unwrap_or(usize::MAX))
        );
    }

    #[test]
    fn permits_are_bounded_and_fifo_released() {
        let pool = JobPool::new(nz(2), CancelToken::new());
        assert_eq!(pool.permits(), nz(2));
        let a = pool.acquire().unwrap();
        assert!(JobPool::holds_permit());
        // A holder gets a nested, uncounted permit at once.
        let nested = pool.acquire().unwrap();
        assert!(!nested.counted);
        drop(nested);
        drop(a);
        assert!(!JobPool::holds_permit());
        assert_eq!(sync::lock(&pool.state).available, 2);
    }

    /// Regression: a permit moved to another thread and dropped there clears the acquiring thread's flag, so that
    /// thread acquires counted permits (and is bounded by the pool) again.
    #[test]
    fn permit_dropped_on_another_thread_clears_the_owner_flag() {
        let pool = JobPool::new(nz(1), CancelToken::new());
        let p = pool.acquire().unwrap();
        assert!(JobPool::holds_permit());
        std::thread::scope(|s| {
            std::thread::Builder::new()
                .spawn_scoped(s, move || {
                    assert!(!JobPool::holds_permit());
                    drop(p);
                })
                .unwrap();
        });
        assert!(!JobPool::holds_permit());
        assert_eq!(sync::lock(&pool.state).available, 1);
        let again = pool.acquire().unwrap();
        assert!(
            again.counted,
            "the owner thread must get a counted permit again"
        );
        assert_eq!(sync::lock(&pool.state).available, 0);
        drop(again);
        assert!(!JobPool::holds_permit());
    }

    #[test]
    fn acquire_returns_none_once_cancelled() {
        let cancel = CancelToken::new();
        let pool = JobPool::new(nz(1), cancel.clone());
        let (holding_tx, holding_rx) = std::sync::mpsc::channel();
        let held = std::thread::scope(|s| {
            let h = std::thread::Builder::new()
                .spawn_scoped(s, || {
                    let p = pool.acquire();
                    holding_tx.send(()).unwrap();
                    // Keep the only permit until well after the cancellation.
                    assert!(cancel.wait_timeout(Duration::from_secs(30)));
                    std::thread::sleep(Duration::from_millis(200));
                    drop(p);
                })
                .unwrap();
            holding_rx.recv().unwrap();
            let c2 = cancel.clone();
            let t = std::thread::Builder::new()
                .spawn_scoped(s, move || {
                    std::thread::sleep(Duration::from_millis(100));
                    c2.cancel(CancelReason::Interrupt);
                })
                .unwrap();
            let start = Instant::now();
            let got = pool.acquire().is_some();
            t.join().unwrap();
            h.join().unwrap();
            (got, start.elapsed())
        });
        assert!(!held.0, "a waiter gets None once cancelled");
        assert!(held.1 < Duration::from_secs(5), "{:?}", held.1);
        assert!(pool.acquire().is_none());
    }

    #[test]
    fn map_ordered_orders_results_and_bounds_concurrency() {
        let pool = JobPool::new(nz(3), CancelToken::new());
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let out = pool.map_ordered(nz(8), 40, |i| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(u64::try_from(i % 3).unwrap() * 3));
            running.fetch_sub(1, Ordering::SeqCst);
            Ok(i * 2)
        });
        let got: Vec<usize> = out.into_iter().map(Result::unwrap).collect();
        assert_eq!(got, (0..40).map(|i| i * 2).collect::<Vec<_>>());
        assert!(peak.load(Ordering::SeqCst) <= 3);
        assert!(pool.map_ordered(nz(2), 0, |_| Ok(())).is_empty());
    }

    #[test]
    fn map_ordered_turns_panics_into_failures() {
        let pool = JobPool::new(nz(2), CancelToken::new());
        let out = pool.map_ordered(nz(2), 3, |i| {
            assert!(i != 1, "boom {i}");
            Ok(i)
        });
        assert!(matches!(out[0], Ok(0)));
        assert!(matches!(out[2], Ok(2)));
        match &out[1] {
            Err(Failure::Exc {
                kind: ExcKind::Unexpected,
                exc,
            }) => {
                assert_eq!(exc.ty, "PanicException");
                assert_eq!(exc.text, b"boom 1");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nested_map_ordered_with_one_permit_runs_inline() {
        // ADR-070: a permit holder calling map_ordered runs inline and completes (no deadlock).
        let pool = JobPool::new(nz(1), CancelToken::new());
        let out = pool.map_ordered(nz(4), 3, |i| {
            assert!(JobPool::holds_permit());
            let inner = pool.map_ordered(nz(4), 4, |j| Ok(i * 10 + j));
            Ok(inner.into_iter().map(Result::unwrap).sum::<usize>())
        });
        let sums: Vec<usize> = out.into_iter().map(Result::unwrap).collect();
        assert_eq!(sums, [6, 46, 86]);
    }

    #[test]
    fn cancelled_pool_interrupts_remaining_items() {
        let cancel = CancelToken::new();
        cancel.cancel(CancelReason::Terminate);
        let pool = JobPool::new(nz(2), cancel);
        let out = pool.map_ordered(nz(2), 3, Ok);
        assert!(
            out.iter()
                .all(|r| matches!(r, Err(Failure::Interrupted(CancelReason::Terminate))))
        );
        assert!(matches!(
            pool.map_ordered(nz(2), 1, Ok).as_slice(),
            [Err(Failure::Interrupted(CancelReason::Terminate))]
        ));
    }

    #[test]
    fn limiter_counts_and_acquire_all_is_exclusive() {
        let lim = Limiter::new(nz(3));
        assert_eq!(lim.permits(), nz(3));
        let token = CancelToken::new();
        let first = lim.acquire(&token).unwrap();
        let second = lim.acquire(&token).unwrap();
        std::thread::scope(|s| {
            let h = std::thread::Builder::new()
                .spawn_scoped(s, || {
                    let all = lim.acquire_all(&token).unwrap();
                    assert_eq!(sync::lock(&lim.state).available, 0);
                    drop(all);
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(100));
            // A pending acquire_all blocks newcomers (writer preference).
            let cancel_single = CancelToken::new();
            let c2 = cancel_single.clone();
            let t = std::thread::Builder::new()
                .spawn_scoped(s, move || {
                    std::thread::sleep(Duration::from_millis(150));
                    c2.cancel(CancelReason::Interrupt);
                })
                .unwrap();
            assert!(lim.acquire(&cancel_single).is_none());
            t.join().unwrap();
            drop(first);
            drop(second);
            h.join().unwrap();
        });
        assert_eq!(sync::lock(&lim.state).available, 3);
        let cancelled = CancelToken::new();
        cancelled.cancel(CancelReason::Interrupt);
        assert!(lim.acquire(&cancelled).is_none());
        assert!(lim.acquire_all(&cancelled).is_none());
    }
}
