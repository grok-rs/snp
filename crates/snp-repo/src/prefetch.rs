//! Background clone prefetch (ARCHITECTURE §5.3 step 3, ADR-064): an exact optimisation of pre-commit's serial
//! clones.
//!
//! At the start of `all_hooks`, every missing remote `(repo, rev)` base clone whose URL is `prefetch_eligible`
//! starts on the `NetLimiter` with `CloneMode::Prefetch` (no terminal prompt, no askpass GUI, no credential-manager
//! dialog, `BatchMode` ssh) into its own scratch directory. When resolution reaches that repository in config order
//! it prints the `Initializing` line at pre-commit's moment and takes the finished clone (waiting for it when it is
//! still running); **any** prefetch failure is a cache miss — its directory is removed, its stderr never shown —
//! and the clone is redone in the foreground exactly as pre-commit runs it. A job resolution reaches before a worker
//! picked it up is claimed and cloned in the foreground. `SNP_PREFETCH=0` disables prefetching; with fewer than two
//! clones to make there is nothing to overlap.
//!
//! Owner: WP-31.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use snp_core::sync;
use snp_git::{CloneMode, OwnedRepo};
use snp_store::RepoKey;

use crate::ResolveCtx;

/// How often a waiter re-checks the cancel token.
const POLL: Duration = Duration::from_millis(50);

/// One prefetch job.
#[derive(Debug)]
pub(crate) struct Job {
    /// The base clone's key.
    pub(crate) key: RepoKey,
    /// The URL handed to `git remote add` (a local directory already joined to the project root).
    pub(crate) url: String,
    /// The revision.
    pub(crate) rev: Arc<str>,
    state: Mutex<State>,
    cv: Condvar,
}

impl Job {
    /// A queued job.
    pub(crate) fn new(key: RepoKey, url: String, rev: Arc<str>) -> Self {
        Self {
            key,
            url,
            rev,
            state: Mutex::new(State::Queued),
            cv: Condvar::new(),
        }
    }
}

#[derive(Debug)]
enum State {
    /// Not picked up yet.
    Queued,
    /// A worker is cloning.
    Running,
    /// Finished: the prefetch directory (holding `src/`) or a cache miss.
    Done(Option<PathBuf>),
    /// Resolution owns the outcome (it took the clone or clones in the foreground).
    Claimed,
}

/// The prefetch jobs of one `all_hooks`, in config order.
#[derive(Debug)]
pub(crate) struct Prefetch {
    jobs: Vec<Job>,
    next: AtomicUsize,
    abandoned: AtomicBool,
    scratch: PathBuf,
}

impl Prefetch {
    /// Jobs whose clones go into `scratch` (the store's `scratch/`, same filesystem as `repos-v1`).
    pub(crate) fn new(jobs: Vec<Job>, scratch: PathBuf) -> Self {
        Self {
            jobs,
            next: AtomicUsize::new(0),
            abandoned: AtomicBool::new(false),
            scratch,
        }
    }

    /// The number of jobs.
    pub(crate) fn len(&self) -> usize {
        self.jobs.len()
    }

    /// A worker: clones queued jobs in config order until none is left.
    pub(crate) fn work(&self, cx: &ResolveCtx<'_>) {
        loop {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            let Some(job) = self.jobs.get(i) else {
                return;
            };
            {
                let mut st = sync::lock(&job.state);
                if !matches!(*st, State::Queued) {
                    continue;
                }
                if self.abandoned.load(Ordering::Acquire) {
                    *st = State::Claimed;
                    continue;
                }
                *st = State::Running;
            }
            let outcome = cx
                .net
                .acquire(cx.cancel)
                .and_then(|_permit| self.clone_one(job, cx));
            *sync::lock(&job.state) = State::Done(outcome);
            job.cv.notify_all();
        }
    }

    /// One background clone; `None` on any failure (its directory removed).
    fn clone_one(&self, job: &Job, cx: &ResolveCtx<'_>) -> Option<PathBuf> {
        let dir = self.scratch.join(scratch_name());
        let src = dir.join("src");
        if snp_os::fs::ensure_private_dir(&src).is_err() {
            discard(&dir, &self.scratch);
            return None;
        }
        match OwnedRepo::clone_rev(
            Arc::clone(cx.git_exe),
            cx.env,
            &src,
            &job.url,
            &job.rev,
            CloneMode::Prefetch,
            Arc::clone(&cx.git_backend),
        ) {
            Ok(_) => Some(dir),
            Err(e) => {
                tracing::debug!(url = %scrub(&job.url), "prefetch failed (cache miss): {e:?}");
                discard(&dir, &self.scratch);
                None
            }
        }
    }

    /// The finished prefetch of `key` (the directory holding `src/`), waiting while it runs; `None` when there is
    /// none (no job, a cache miss, a job no worker started, cancellation): the caller clones in the foreground.
    pub(crate) fn take(&self, key: &RepoKey, cx: &ResolveCtx<'_>) -> Option<PathBuf> {
        let job = self.jobs.iter().find(|j| j.key == *key)?;
        let mut st = sync::lock(&job.state);
        loop {
            match &mut *st {
                State::Queued => {
                    *st = State::Claimed;
                    return None;
                }
                State::Running => {
                    if cx.cancel.is_cancelled() {
                        return None;
                    }
                    st = match job.cv.wait_timeout(st, POLL) {
                        Ok((g, _)) => g,
                        Err(poisoned) => poisoned.into_inner().0,
                    };
                }
                State::Done(dir) => {
                    let dir = dir.take();
                    *st = State::Claimed;
                    return dir;
                }
                State::Claimed => return None,
            }
        }
    }

    /// Stops workers from starting further jobs.
    pub(crate) fn abandon(&self) {
        self.abandoned.store(true, Ordering::Release);
    }

    /// Removes every finished prefetch nobody took (after the workers were joined).
    pub(crate) fn cleanup(&self) {
        for job in &self.jobs {
            let mut st = sync::lock(&job.state);
            if let State::Done(Some(dir)) = &*st {
                discard(dir, &self.scratch);
            }
            *st = State::Claimed;
        }
    }
}

/// Moves a finished prefetch (`dir/src`) to `dest` (the store's empty fill directory); `false` leaves `dest` empty
/// for a foreground clone.
pub(crate) fn adopt(dir: &Path, dest: &Path, scratch: &Path) -> bool {
    let src = dir.join("src");
    let moved =
        std::fs::remove_dir(dest).is_ok() && snp_os::fs::rename_with_retry(&src, dest).is_ok();
    if !moved && std::fs::symlink_metadata(dest).is_err() {
        let _recreated: std::io::Result<_> = snp_os::fs::ensure_private_dir(dest);
    }
    discard(dir, scratch);
    moved && std::fs::symlink_metadata(dest).is_ok()
}

/// Best-effort removal of a prefetch directory inside `scratch` (sweep collects what is left).
fn discard(dir: &Path, scratch: &Path) {
    let _removed: std::io::Result<()> = snp_os::fs::remove_tree_within(dir, scratch);
}

/// `prefetch-<pid>-<16 hex>`: unique per process and call.
fn scratch_name() -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut h = blake3::Hasher::new();
    h.update(&std::process::id().to_le_bytes());
    h.update(&n.to_le_bytes());
    h.update(&now.to_le_bytes());
    let hex = h.finalize().to_hex();
    format!(
        "prefetch-{}-{}",
        std::process::id(),
        hex.get(..16).unwrap_or_default()
    )
}

/// A URL without its userinfo, for logs (tokens are never logged, 09 §6.2).
pub(crate) fn scrub(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let cut = rest.find('/').unwrap_or(rest.len());
        let (authority, path) = (
            rest.get(..cut).unwrap_or(rest),
            rest.get(cut..).unwrap_or(""),
        );
        if let Some((_, host)) = authority.rsplit_once('@') {
            return format!("{scheme}://***@{host}{path}");
        }
    }
    url.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_hides_userinfo() {
        assert_eq!(
            scrub("https://user:tok@github.com/o/r"),
            "https://***@github.com/o/r"
        );
        assert_eq!(scrub("git@github.com:o/r"), "git@github.com:o/r");
        assert_eq!(scrub("/local/path"), "/local/path");
    }

    #[test]
    fn scratch_names_are_unique() {
        assert_ne!(scratch_name(), scratch_name());
        assert!(scratch_name().starts_with("prefetch-"));
    }
}
