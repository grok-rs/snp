//! `install_hook_envs()` (`repository.py:25-106,214-229`, ARCHITECTURE §5.4, ADR-032, ADR-065).
//!
//! pre-commit walks the hooks in config order, keeps the first hook per `install_key` that is not installed, and —
//! under one exclusive store lock — installs them one after the other, logging the three `Installing environment`
//! lines before each. snp keeps that transcript byte for byte while installing distinct prefixes in parallel:
//!
//! 1. **Need list** (`_need_installed`, no lock): unique keys in config order (a fast-path hook's *virtual* key
//!    counts like a real one, ADR-032; a real install of the same key wins), each checked: state parses, deps and
//!    version equal, `health()` healthy (X13-ENV-01: a malformed state is "not installed").
//! 2. **Workers**, one per prefix (repository), take their keys in config order: per-repo lock → re-check (another
//!    process may have installed it) → [`snp_lang::check_support`] *before* any filesystem mutation (04b §5.1) →
//!    an `InstallLimiter` permit (every permit for the compile-heavy languages) → remove the stale env dir → install
//!    → health → wait for the coordinator's verdict → commit the marker (or remove the env dir).
//! 3. **The coordinator** (the calling thread) walks the keys in config order: it prints key *k*'s triplet only once
//!    *k − 1* is finished and *k*'s worker found it still needs installing, then commits *k*; the first failure in
//!    config order is the result, every later in-flight install is cancelled and its directory removed, so the
//!    store ends exactly as after pre-commit's sequential loop.
//!
//! The scheduling is generic over [`EnvInstaller`] so the ordering guarantees are tested with slow fake installers;
//! [`LangInstaller`] is the real one over the language backends and the store.
//!
//! Owner: WP-31.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::{ExcKind, Failure, PyExc};
use snp_core::lang::LanguageId;
use snp_core::output::{Level, Printer};
use snp_lang::{EnvCtx, Health, Host, InstallCtx, InstallState};
use snp_os::lock::FileLock;
use snp_proc::pool::Limiter;
use snp_store::{MarkerKey, RepoKey, Store, ToolRef};

use crate::tools::NoticeTools;
use crate::{HookSrc, InstallKey, ResolveCtx, ResolvedHook, messages};

/// The install-state file of an environment (ARCHITECTURE §5.4).
const STATE_FILE: &str = ".snp-install-state.json";

/// One environment to provision.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    /// The key.
    pub(crate) key: InstallKey,
    /// `hook.src`: the config's `repo` (`local`, `meta`, a URL), for the triplet.
    pub(crate) src: Arc<str>,
    /// The hook repository checkout.
    pub(crate) prefix: PathBuf,
    /// `<repo>/envs`.
    pub(crate) envs: PathBuf,
    /// The environment directory.
    pub(crate) env_dir: PathBuf,
    /// `repo: local`.
    pub(crate) is_local: bool,
}

/// One entry of the need list.
#[derive(Debug, Clone)]
pub(crate) enum Entry {
    /// A real environment.
    Real(Job),
    /// A fast-path hook's environment pre-commit would build (ADR-032): only the triplet and a marker.
    Virtual {
        /// The key.
        key: InstallKey,
        /// `hook.src`.
        src: Arc<str>,
    },
}

impl Entry {
    fn key(&self) -> &InstallKey {
        match self {
            Self::Real(j) => &j.key,
            Self::Virtual { key, .. } => key,
        }
    }

    fn src(&self) -> &str {
        match self {
            Self::Real(j) => &j.src,
            Self::Virtual { src, .. } => src,
        }
    }
}

/// A finished, not yet committed install.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    /// `InstallState::to_json`.
    pub(crate) state: Vec<u8>,
    /// The toolchains the environment links to.
    pub(crate) tools: Vec<ToolRef>,
}

/// The install operations the scheduler drives (real: [`LangInstaller`]; tests: slow fakes).
pub(crate) trait EnvInstaller: Sync {
    /// `_hook_installed` (a malformed state is "not installed").
    fn installed(&self, job: &Job) -> Result<bool, Failure>;
    /// Whether a virtual key's `Installed` marker exists.
    fn virtual_installed(&self, key: &InstallKey) -> bool;
    /// Records a virtual install.
    fn mark_virtual(&self, key: &InstallKey);
    /// The per-repository lock (held for the re-check, the install and the commit).
    fn lock(&self, job: &Job) -> Result<Option<FileLock>, Failure>;
    /// `check_support` (before any filesystem mutation).
    fn check(&self, job: &Job) -> Result<(), Failure>;
    /// Compile-heavy languages take every `InstallLimiter` permit (ADR-065).
    fn heavy(&self, job: &Job) -> bool;
    /// Removes a stale env dir, installs, checks health; the env dir is removed again on failure.
    fn install(&self, job: &Job, cancel: &CancelToken) -> Result<Prepared, Failure>;
    /// Commits the marker (the env dir is removed on failure).
    fn commit(&self, job: &Job, p: &Prepared) -> Result<(), Failure>;
    /// Removes an uncommitted env dir.
    fn discard(&self, job: &Job);
}

/// The 16-byte identity of an `InstallKey` (virtual-install markers).
pub(crate) fn key_digest(k: &InstallKey) -> [u8; 16] {
    let mut h = blake3::Hasher::new();
    h.update(b"snp.install.v1\0");
    h.update(&k.repo.0);
    let mut lp = |b: &[u8]| {
        h.update(&u64::try_from(b.len()).unwrap_or(u64::MAX).to_le_bytes());
        h.update(b);
    };
    lp(k.lang.as_str().as_bytes());
    lp(k.version.as_bytes());
    for d in k.deps.iter() {
        lp(d.as_bytes());
    }
    let full = h.finalize();
    let mut out = [0_u8; 16];
    for (o, b) in out.iter_mut().zip(full.as_bytes()) {
        *o = *b;
    }
    out
}

/// `hook.src` of a resolved hook.
pub(crate) fn src_name(h: &ResolvedHook) -> Arc<str> {
    match &h.src {
        HookSrc::Remote { repo, .. } => Arc::clone(repo),
        HookSrc::Meta => Arc::from("meta"),
        HookSrc::Builtin => Arc::from("builtin"),
        _ => Arc::from("local"),
    }
}

/// The unique entries of `hooks` in config order: the first occurrence of a key fixes its position; a real
/// occurrence replaces a virtual one.
pub(crate) fn entries(hooks: &[&ResolvedHook], store: &Store) -> Result<Vec<Entry>, Failure> {
    let mut out: Vec<Entry> = Vec::new();
    for h in hooks {
        if let Some(key) = &h.install_key {
            let repo = h.repo.as_ref().ok_or_else(|| {
                Failure::unexpected(
                    "AssertionError",
                    format!(
                        "snp: hook `{}` has an environment but no repository",
                        h.def.id
                    ),
                )
            })?;
            let job = Job {
                key: key.clone(),
                src: src_name(h),
                prefix: h.prefix.clone(),
                envs: repo.envs.clone(),
                env_dir: store.env_dir(repo, key.lang, &key.version),
                is_local: matches!(h.src, HookSrc::Local),
            };
            match out.iter_mut().find(|e| e.key() == key) {
                Some(slot @ Entry::Virtual { .. }) => *slot = Entry::Real(job),
                Some(Entry::Real(_)) => {}
                None => out.push(Entry::Real(job)),
            }
        } else if let Some(key) = &h.virtual_install
            && !out.iter().any(|e| e.key() == key)
        {
            out.push(Entry::Virtual {
                key: key.clone(),
                src: src_name(h),
            });
        }
    }
    Ok(out)
}

/// The three INFO lines of `_hook_install`.
fn triplet(printer: &Printer, src: &str) {
    for line in messages::installing(src) {
        printer.log(Level::Info, &line);
    }
}

/// Worker → coordinator.
#[derive(Debug)]
enum Event {
    /// After the locked re-check: whether the environment still needs installing.
    Decided(bool),
    /// The install finished (not committed yet).
    Ready(Result<(), Failure>),
    /// The commit finished.
    Committed(Result<(), Failure>),
}

/// Coordinator → worker.
#[derive(Debug, Clone, Copy)]
enum Verdict {
    Commit,
}

/// A `PanicException` for a panicking installer (ADR-066).
fn panic_failure(payload: &(dyn std::any::Any + Send)) -> Failure {
    let msg = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_owned());
    Failure::Exc {
        kind: ExcKind::Unexpected,
        exc: PyExc::panic(&msg, None, None),
    }
}

/// The channels of one real job.
struct Slot {
    /// Events from the worker.
    events: mpsc::Receiver<Event>,
    /// The verdict to the worker (dropped = abort).
    verdict: Option<mpsc::SyncSender<Verdict>>,
}

/// A worker's side of one job.
struct WorkItem<'j> {
    job: &'j Job,
    events: mpsc::Sender<Event>,
    verdict: mpsc::Receiver<Verdict>,
}

/// Runs the need list (see the module docs).
pub(crate) fn run(
    inst: &dyn EnvInstaller,
    all: &[Entry],
    printer: &Printer,
    installs: &Limiter,
    cancel: &CancelToken,
) -> Result<(), Failure> {
    let mut need: Vec<&Entry> = Vec::new();
    for e in all {
        let needed = match e {
            Entry::Virtual { key, .. } => !inst.virtual_installed(key),
            Entry::Real(j) => !inst.installed(j)?,
        };
        if needed {
            need.push(e);
        }
    }
    if need.is_empty() {
        return Ok(());
    }
    let child = cancel.child();
    let abort = AtomicBool::new(false);
    // One worker per prefix, its jobs in config order.
    let mut groups: Vec<(RepoKey, Vec<WorkItem<'_>>)> = Vec::new();
    let mut slots: Vec<Option<Slot>> = Vec::new();
    for e in &need {
        let Entry::Real(job) = e else {
            slots.push(None);
            continue;
        };
        let (ev_tx, ev_rx) = mpsc::channel();
        let (v_tx, v_rx) = mpsc::sync_channel(1);
        slots.push(Some(Slot {
            events: ev_rx,
            verdict: Some(v_tx),
        }));
        let item = WorkItem {
            job,
            events: ev_tx,
            verdict: v_rx,
        };
        match groups.iter_mut().find(|(k, _)| *k == job.key.repo) {
            Some((_, items)) => items.push(item),
            None => groups.push((job.key.repo, vec![item])),
        }
    }
    std::thread::scope(|s| {
        let mut spawned = Vec::new();
        for (k, (_, items)) in groups.into_iter().enumerate() {
            let (abort, child) = (&abort, &child);
            // A refused thread drops `items` (and their senders): the coordinator reports the lost worker.
            let worker = move || work(inst, items, installs, child, abort);
            match std::thread::Builder::new()
                .name(format!("snp-install-{k}"))
                .spawn_scoped(s, worker)
            {
                Ok(h) => spawned.push(h),
                Err(e) => tracing::debug!("install worker refused: {e}"),
            }
        }
        let result = coordinate(inst, &need, &mut slots, printer);
        if result.is_err() {
            abort.store(true, Ordering::Release);
            child.cancel(CancelReason::Interrupt);
        }
        // Dropping the verdict senders releases every worker still waiting for one (they discard their install).
        drop(slots);
        for h in spawned {
            let _joined = h.join();
        }
        result
    })
}

/// The coordinator: config order, triplets, commits, first failure wins.
fn coordinate(
    inst: &dyn EnvInstaller,
    need: &[&Entry],
    slots: &mut [Option<Slot>],
    printer: &Printer,
) -> Result<(), Failure> {
    let lost = || Failure::unexpected("RuntimeError", "snp: an install worker ended unexpectedly");
    for (e, slot) in need.iter().zip(slots.iter_mut()) {
        let Some(slot) = slot else {
            if let Entry::Virtual { key, .. } = e {
                triplet(printer, e.src());
                inst.mark_virtual(key);
            }
            continue;
        };
        match slot.events.recv().map_err(|_| lost())? {
            Event::Decided(false) => continue,
            Event::Decided(true) => triplet(printer, e.src()),
            Event::Ready(r) | Event::Committed(r) => {
                r?;
                return Err(lost());
            }
        }
        match slot.events.recv().map_err(|_| lost())? {
            Event::Ready(r) => r?,
            Event::Decided(_) | Event::Committed(_) => return Err(lost()),
        }
        let sender = slot.verdict.take().ok_or_else(lost)?;
        sender.send(Verdict::Commit).map_err(|_| lost())?;
        match slot.events.recv().map_err(|_| lost())? {
            Event::Committed(r) => r?,
            Event::Decided(_) | Event::Ready(_) => return Err(lost()),
        }
    }
    Ok(())
}

/// One worker: its jobs in config order (see the module docs).
fn work(
    inst: &dyn EnvInstaller,
    items: Vec<WorkItem<'_>>,
    installs: &Limiter,
    cancel: &CancelToken,
    abort: &AtomicBool,
) {
    for item in items {
        if abort.load(Ordering::Acquire) {
            return;
        }
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| one(inst, &item, installs, cancel)));
        let ok = match r {
            Ok(ok) => ok,
            Err(payload) => {
                let _sent = item
                    .events
                    .send(Event::Ready(Err(panic_failure(&*payload))));
                false
            }
        };
        if !ok {
            return;
        }
    }
}

/// One job; `false` stops the worker.
fn one(
    inst: &dyn EnvInstaller,
    item: &WorkItem<'_>,
    installs: &Limiter,
    cancel: &CancelToken,
) -> bool {
    let job = item.job;
    let send = |e: Event| item.events.send(e).is_ok();
    let _lock = match inst.lock(job) {
        Ok(l) => l,
        Err(e) => {
            send(Event::Ready(Err(e)));
            return false;
        }
    };
    let needed = match inst.installed(job) {
        Ok(installed) => !installed,
        Err(e) => {
            send(Event::Ready(Err(e)));
            return false;
        }
    };
    if !send(Event::Decided(needed)) {
        return false;
    }
    if !needed {
        return true;
    }
    if let Err(e) = inst.check(job) {
        send(Event::Ready(Err(e)));
        return false;
    }
    let permit = if inst.heavy(job) {
        installs.acquire_all(cancel)
    } else {
        installs.acquire(cancel)
    };
    let prepared = match permit {
        Some(p) => {
            let r = inst.install(job, cancel);
            drop(p);
            r
        }
        None => Err(Failure::Interrupted(
            cancel.reason().unwrap_or(CancelReason::Interrupt),
        )),
    };
    let prepared = match prepared {
        Ok(p) => p,
        Err(e) => {
            send(Event::Ready(Err(e)));
            return false;
        }
    };
    if !send(Event::Ready(Ok(()))) {
        inst.discard(job);
        return false;
    }
    if item.verdict.recv().is_err() {
        inst.discard(job);
        return false;
    }
    let committed = inst.commit(job, &prepared);
    let ok = committed.is_ok();
    send(Event::Committed(committed)) && ok
}

/// The real installer: the language backends over the store.
#[derive(Debug)]
pub(crate) struct LangInstaller<'c, 'a> {
    /// The context.
    pub(crate) cx: &'c ResolveCtx<'a>,
}

impl LangInstaller<'_, '_> {
    /// The `EnvCtx` of `job` on `host`.
    fn env_cx<'h>(job: &'h Job, host: &'h Host<'h>) -> EnvCtx<'h> {
        EnvCtx {
            host,
            prefix: &job.prefix,
            version: &job.key.version,
            env_dir: &job.env_dir,
        }
    }

    /// `rmtree(env_dir)` contained in `<repo>/envs` (ENV-04: the `PermissionError` of a stale dir).
    fn remove(job: &Job) -> Result<(), Failure> {
        if std::fs::symlink_metadata(&job.env_dir).is_err() {
            return Ok(());
        }
        snp_lang::common::rmtree(&job.env_dir, &job.envs)
            .map_err(|e| Failure::from_io(&e, Some(job.env_dir.as_os_str()), None))
    }
}

/// The language name pre-commit puts in the health assertion (`hook.language`).
fn lang_name(l: LanguageId) -> &'static str {
    l.as_str()
}

/// ENV-02: an install state that exists but cannot be opened is pre-commit's `PermissionError` on the state file.
fn state_unreadable(env_dir: &Path) -> Option<Failure> {
    let path = env_dir.join(STATE_FILE);
    match std::fs::File::open(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Some(Failure::from_io(&e, Some(path.as_os_str()), None))
        }
        _ => None,
    }
}

impl EnvInstaller for LangInstaller<'_, '_> {
    fn installed(&self, job: &Job) -> Result<bool, Failure> {
        let lang = job.key.lang;
        if lang.env_dir_base().is_none() {
            return Ok(true);
        }
        if let Some(f) = state_unreadable(&job.env_dir) {
            return Err(f);
        }
        let Some(state) = Store::read_install_marker(&job.env_dir)
            .as_deref()
            .and_then(InstallState::from_json)
        else {
            return Ok(false);
        };
        if *state.language != *lang.as_str()
            || *state.language_version != *job.key.version
            || *state.deps != *job.key.deps
        {
            return Ok(false);
        }
        let host = &self.cx.host;
        let health = snp_lang::backend(lang)
            .health(&Self::env_cx(job, host), Some(&state))
            .map_err(Failure::from)?;
        Ok(health == Health::Healthy)
    }

    fn virtual_installed(&self, key: &InstallKey) -> bool {
        self.cx.store.marker_exists(&MarkerKey::Installed {
            install_key: key_digest(key),
        })
    }

    fn mark_virtual(&self, key: &InstallKey) {
        self.cx.store.set_marker(&MarkerKey::Installed {
            install_key: key_digest(key),
        });
    }

    fn lock(&self, job: &Job) -> Result<Option<FileLock>, Failure> {
        let cx = self.cx;
        cx.store
            .repo_lock(
                &job.key.repo,
                &mut crate::resolve::lock_notice(cx),
                cx.cancel,
            )
            .map(Some)
    }

    fn check(&self, job: &Job) -> Result<(), Failure> {
        snp_lang::check_support(job.key.lang, &job.key.version, &job.key.deps, self.cx.brand)
            .map_err(Failure::from)
    }

    fn heavy(&self, job: &Job) -> bool {
        use LanguageId as L;
        matches!(
            job.key.lang,
            L::Rust | L::Golang | L::Haskell | L::Swift | L::Dotnet | L::Dart | L::Conda
        )
    }

    fn install(&self, job: &Job, cancel: &CancelToken) -> Result<Prepared, Failure> {
        let cx = self.cx;
        Self::remove(job)?;
        let tools = NoticeTools {
            inner: cx.host.tools,
            store: cx.store,
            printer: cx.printer,
            brand: cx.brand,
        };
        let host = Host {
            tools: &tools,
            cancel,
            ..cx.host
        };
        let lang = snp_lang::backend(job.key.lang);
        let result = (|| {
            let outcome = lang
                .install(&InstallCtx {
                    cx: Self::env_cx(job, &host),
                    deps: &job.key.deps,
                    is_local: job.is_local,
                })
                .map_err(Failure::from)?;
            let state = InstallState {
                v: InstallState::VERSION,
                language: Arc::from(job.key.lang.as_str()),
                language_version: Arc::clone(&job.key.version),
                deps: Arc::clone(&job.key.deps),
                installer: Arc::clone(&outcome.installer),
                fingerprint: outcome.fingerprint.clone(),
                snp_version: Arc::from(env!("CARGO_PKG_VERSION")),
            };
            if let Health::Unhealthy(why) = lang
                .health(&Self::env_cx(job, &host), Some(&state))
                .map_err(Failure::from)?
            {
                return Err(Failure::unexpected(
                    "AssertionError",
                    messages::unhealthy_after_install(lang_name(job.key.lang), &why),
                ));
            }
            Ok(Prepared {
                state: state.to_json(),
                tools: outcome
                    .tools
                    .iter()
                    .map(|(t, v)| ToolRef {
                        tool: Arc::clone(t),
                        version: Arc::clone(v),
                    })
                    .collect(),
            })
        })();
        if result.is_err() {
            self.discard(job);
        }
        result
    }

    fn commit(&self, job: &Job, p: &Prepared) -> Result<(), Failure> {
        Store::commit_install_marker(&job.env_dir, &p.state, &p.tools).map_err(|e| {
            self.discard(job);
            Failure::from_io(&e, Some(job.env_dir.join(STATE_FILE).as_os_str()), None)
        })
    }

    fn discard(&self, job: &Job) {
        let _removed: Result<(), Failure> = Self::remove(job);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use snp_core::output::PrinterOpts;
    use snp_core::sync;

    use super::*;

    /// A captured stdout.
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            sync::lock(&self.0).extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn printer() -> (Printer, Buf) {
        let out = Buf::default();
        let p = Printer::new(
            Box::new(out.clone()),
            Box::new(std::io::sink()),
            PrinterOpts {
                brand: snp_core::brand::Brand::PreCommit,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: CancelToken::new(),
            },
        );
        (p, out)
    }

    /// Slow fake installs: `delay(job)` per install, failures by src, a log of events.
    #[derive(Default)]
    struct Fake {
        installed: Mutex<Vec<String>>,
        virtuals: Mutex<Vec<[u8; 16]>>,
        fail: Vec<&'static str>,
        unsupported: Vec<&'static str>,
        heavy: Vec<&'static str>,
        delays: Vec<(&'static str, u64)>,
        log: Mutex<Vec<String>>,
        running: AtomicUsize,
        max_running: AtomicUsize,
        heavy_overlap: AtomicBool,
        heavy_running: AtomicUsize,
        discarded: Mutex<Vec<String>>,
    }

    impl Fake {
        fn name(job: &Job) -> String {
            format!("{}#{}", job.src, job.key.deps.join(","))
        }
    }

    impl EnvInstaller for Fake {
        fn installed(&self, job: &Job) -> Result<bool, Failure> {
            Ok(sync::lock(&self.installed).contains(&Self::name(job)))
        }
        fn virtual_installed(&self, key: &InstallKey) -> bool {
            self.virtuals.lock().unwrap().contains(&key_digest(key))
        }
        fn mark_virtual(&self, key: &InstallKey) {
            self.virtuals.lock().unwrap().push(key_digest(key));
        }
        fn lock(&self, _job: &Job) -> Result<Option<FileLock>, Failure> {
            Ok(None)
        }
        fn check(&self, job: &Job) -> Result<(), Failure> {
            if self.unsupported.contains(&&*job.src) {
                return Err(Failure::unexpected("AssertionError", "unsupported"));
            }
            Ok(())
        }
        fn heavy(&self, job: &Job) -> bool {
            self.heavy.contains(&&*job.src)
        }
        fn install(&self, job: &Job, cancel: &CancelToken) -> Result<Prepared, Failure> {
            let heavy = self.heavy(job);
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_running.fetch_max(now, Ordering::SeqCst);
            if heavy {
                if self.heavy_running.fetch_add(1, Ordering::SeqCst) > 0 || now > 1 {
                    self.heavy_overlap.store(true, Ordering::SeqCst);
                }
            } else if self.heavy_running.load(Ordering::SeqCst) > 0 {
                self.heavy_overlap.store(true, Ordering::SeqCst);
            }
            sync::lock(&self.log).push(format!("start {}", job.src));
            let ms = self
                .delays
                .iter()
                .find(|(s, _)| *s == &*job.src)
                .map_or(0, |d| d.1);
            let cancelled = cancel.wait_timeout(Duration::from_millis(ms));
            if heavy {
                self.heavy_running.fetch_sub(1, Ordering::SeqCst);
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            if cancelled {
                return Err(Failure::Interrupted(CancelReason::Interrupt));
            }
            if self.fail.contains(&&*job.src) {
                return Err(Failure::unexpected(
                    "CalledProcessError",
                    format!("{} failed", job.src),
                ));
            }
            Ok(Prepared {
                state: Vec::new(),
                tools: Vec::new(),
            })
        }
        fn commit(&self, job: &Job, _p: &Prepared) -> Result<(), Failure> {
            sync::lock(&self.installed).push(Self::name(job));
            Ok(())
        }
        fn discard(&self, job: &Job) {
            self.discarded.lock().unwrap().push(job.src.to_string());
        }
    }

    fn key(repo: u8, deps: &[&str]) -> InstallKey {
        InstallKey {
            repo: RepoKey([repo; 16]),
            lang: LanguageId::Python,
            version: Arc::from("python3"),
            deps: deps.iter().map(|d| Arc::from(*d)).collect(),
        }
    }

    fn real(src: &str, repo: u8) -> Entry {
        Entry::Real(Job {
            key: key(repo, &[]),
            src: Arc::from(src),
            prefix: PathBuf::from("/p"),
            envs: PathBuf::from("/p/envs"),
            env_dir: PathBuf::from("/p/envs/py_env-python3"),
            is_local: false,
        })
    }

    fn transcript(out: &Buf) -> String {
        String::from_utf8(out.0.lock().unwrap().clone()).unwrap()
    }

    fn expected(srcs: &[&str]) -> String {
        let mut out = String::new();
        for s in srcs {
            out.push_str("[INFO] Installing environment for ");
            out.push_str(s);
            out.push_str(
                ".\n[INFO] Once installed this environment will be reused.\n[INFO] This may take a few minutes...\n",
            );
        }
        out
    }

    fn limiter(n: usize) -> Arc<Limiter> {
        Limiter::new(std::num::NonZeroUsize::new(n).unwrap())
    }

    #[test]
    fn slow_parallel_installs_print_the_sequential_transcript() {
        let fake = Fake {
            delays: vec![("r1", 150), ("r2", 10), ("r3", 60)],
            ..Fake::default()
        };
        let all = [
            real("r1", 1),
            real("r2", 2),
            Entry::Virtual {
                key: key(9, &[]),
                src: Arc::from("v"),
            },
            real("r3", 3),
        ];
        let (p, out) = printer();
        run(&fake, &all, &p, &limiter(4), &CancelToken::new()).unwrap();
        assert_eq!(transcript(&out), expected(&["r1", "r2", "v", "r3"]));
        assert!(
            fake.max_running.load(Ordering::SeqCst) >= 2,
            "installs overlapped"
        );
        // Warm: nothing printed.
        let (p, out) = printer();
        run(&fake, &all, &p, &limiter(4), &CancelToken::new()).unwrap();
        assert_eq!(transcript(&out), "");
    }

    #[test]
    fn first_failure_in_config_order_wins_and_later_installs_are_discarded() {
        let fake = Fake {
            delays: vec![("r1", 100), ("r2", 0), ("r3", 0)],
            fail: vec!["r1", "r2"],
            ..Fake::default()
        };
        let all = [real("r1", 1), real("r2", 2), real("r3", 3)];
        let (p, out) = printer();
        let err = run(&fake, &all, &p, &limiter(4), &CancelToken::new()).unwrap_err();
        let Failure::Exc { exc, .. } = err else {
            panic!("{err:?}")
        };
        assert_eq!(exc.text, b"r1 failed");
        assert_eq!(transcript(&out), expected(&["r1"]));
        // r3 finished in parallel but must not be committed.
        assert!(
            !fake
                .installed
                .lock()
                .unwrap()
                .iter()
                .any(|n| n.starts_with("r3"))
        );
    }

    #[test]
    fn check_support_failure_prints_its_triplet_first() {
        let fake = Fake {
            unsupported: vec!["r2"],
            ..Fake::default()
        };
        let all = [real("r1", 1), real("r2", 2)];
        let (p, out) = printer();
        let err = run(&fake, &all, &p, &limiter(2), &CancelToken::new()).unwrap_err();
        assert!(matches!(err, Failure::Exc { .. }));
        assert_eq!(transcript(&out), expected(&["r1", "r2"]));
        assert!(fake.log.lock().unwrap().iter().all(|l| l != "start r2"));
    }

    #[test]
    fn compile_heavy_installs_never_overlap() {
        let fake = Fake {
            heavy: vec!["h1", "h2"],
            delays: vec![("h1", 60), ("h2", 60), ("p1", 60), ("p2", 60)],
            ..Fake::default()
        };
        let all = [real("h1", 1), real("p1", 2), real("h2", 3), real("p2", 4)];
        let (p, out) = printer();
        run(&fake, &all, &p, &limiter(4), &CancelToken::new()).unwrap();
        assert_eq!(transcript(&out), expected(&["h1", "p1", "h2", "p2"]));
        assert!(!fake.heavy_overlap.load(Ordering::SeqCst));
    }

    #[test]
    fn same_prefix_keys_install_serially_in_order() {
        let fake = Fake {
            delays: vec![("a", 30)],
            ..Fake::default()
        };
        let mut b = real("a", 1);
        if let Entry::Real(j) = &mut b {
            j.key = key(1, &["x"]);
        }
        let all = [real("a", 1), b];
        let (p, out) = printer();
        run(&fake, &all, &p, &limiter(4), &CancelToken::new()).unwrap();
        assert_eq!(transcript(&out), expected(&["a", "a"]));
        assert_eq!(fake.max_running.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn virtual_keys_print_once() {
        let fake = Fake::default();
        let all = [Entry::Virtual {
            key: key(5, &[]),
            src: Arc::from("v"),
        }];
        let (p, out) = printer();
        run(&fake, &all, &p, &limiter(1), &CancelToken::new()).unwrap();
        run(&fake, &all, &p, &limiter(1), &CancelToken::new()).unwrap();
        assert_eq!(transcript(&out), expected(&["v"]));
    }

    #[test]
    fn x13_env_04_a_stale_env_dir_that_cannot_be_removed_is_an_os_error() {
        // `rmtree` repairs permissions like pre-commit's `util.rmtree`, so an unremovable directory needs a foreign
        // owner; the containment refusal takes the same path (the install stops before touching anything else).
        let tmp = tempfile::tempdir().unwrap();
        let envs = tmp.path().join("envs");
        std::fs::create_dir_all(&envs).unwrap();
        let outside = tmp.path().join("py_env-x");
        std::fs::create_dir_all(&outside).unwrap();
        let job = Job {
            key: key(1, &[]),
            src: Arc::from("local"),
            prefix: tmp.path().to_path_buf(),
            envs,
            env_dir: outside.clone(),
            is_local: true,
        };
        let err = LangInstaller::remove(&job).unwrap_err();
        let Failure::Exc { exc, .. } = err else {
            panic!("{err:?}")
        };
        assert_eq!(exc.ty, "PermissionError");
        assert!(String::from_utf8_lossy(&exc.text).contains(&*outside.to_string_lossy()));
        assert!(outside.is_dir());
    }

    #[test]
    fn digests_distinguish_keys() {
        assert_ne!(key_digest(&key(1, &[])), key_digest(&key(1, &["a"])));
        assert_ne!(
            key_digest(&key(1, &["a", "b"])),
            key_digest(&key(1, &["ab"]))
        );
        assert_eq!(key_digest(&key(2, &["a"])), key_digest(&key(2, &["a"])));
    }
}
