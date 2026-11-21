//! `gc`, `clean` (`commands/gc.py`, `clean.py`; §5.6) and `snp cache`.
//!
//! Owner: WP-33.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use snp_cli::{CacheAction, CacheArgs};
use snp_config::facts::{self, RepoFacts, Value};
use snp_config::{
    ConfigSource, ExtMode, LoadOpts, check_config, check_manifest, check_manifest_bytes,
};
use snp_core::brand::Brand;
use snp_core::cancel::CancelReason;
use snp_core::failure::Failure;
use snp_core::fault::{self, Point};
use snp_core::output::{DiagSink, Level, LogRecorder, WarnSink};
use snp_pyre::RegexCache;
use snp_store::{
    MarkerKey, ProbeMemo, RepoId, RepoIndex, RepoKey, Store, StoreGuard, TrackedConfig,
    db_repo_name, marker_repo_name, tracked_order,
};

use crate::{CmdCtx, frame, messages};

/// The exclusive store lock, printing `[INFO] Locking … directory` once when it has to wait (`store.py:105-106`).
fn exclusive(cx: &CmdCtx<'_>) -> Result<StoreGuard, Failure> {
    let mut printed = false;
    let mut on_wait = || {
        if !printed {
            printed = true;
            cx.printer
                .log(Level::Info, snp_store::locking_message(cx.brand));
        }
    };
    cx.store.exclusive(&mut on_wait, cx.cancel)
}

/// What gc's marking keeps (ADR-075).
#[derive(Debug, Default, PartialEq, Eq)]
struct Used {
    /// Repository keys still in use (pre-commit's `unused.discard((repo, rev))`).
    keys: HashSet<RepoKey>,
    /// `(marker_repo_name, rev)` of every used remote key whose manifest was loadable (present or Tier A): their
    /// `keys-v1` `Initialized` markers stay.
    remote_markers: HashSet<(String, String)>,
    /// `db_repo_name` of every used local key (`local`, `local:dep,…`).
    local_markers: HashSet<String>,
}

impl Used {
    /// Adds `other`'s marks: gc's marks are set unions, so the order configs are merged in does not matter.
    fn union(&mut self, other: Self) {
        self.keys.extend(other.keys);
        self.remote_markers.extend(other.remote_markers);
        self.local_markers.extend(other.local_markers);
    }
}

/// What one live tracked config marks: its [`Used`], and the `keys-v1` file names of its remote `Initialized`
/// markers, derived where the config was loaded (ADR-150).
#[derive(Debug, Default)]
struct Marked {
    used: Used,
    marker_names: HashSet<String>,
}

/// The marking of every tracked config: the union of the live configs' marks and the dead configs, in tracked order.
#[derive(Debug, Default)]
struct Marking {
    used: Used,
    marker_names: HashSet<String>,
    dead: Vec<PathBuf>,
}

impl Marking {
    /// Adds the outcome of tracked config `t`: its marks, or `t` as dead.
    fn add(&mut self, t: &TrackedConfig, marked: Option<Marked>) {
        match marked {
            Some(m) => {
                self.used.union(m.used);
                self.marker_names.extend(m.marker_names);
            }
            None => self.dead.push(t.path.clone()),
        }
    }
}

/// The read-only state gc's config loads share (ADR-150): every load reads files and writes only to the sinks it is
/// given, so loads may run on any thread and in any order.
struct Loader<'a> {
    brand: Brand,
    /// `SNP_COMPAT_STRICT`, read once.
    ext: ExtMode,
    regexes: &'a RegexCache,
    /// gc's one `repos-v1` listing.
    index: &'a RepoIndex<'a>,
    /// The negative-prefix memo of every key derivation of this run.
    memo: &'a ProbeMemo,
}

impl Loader<'_> {
    /// The load options writing warnings to `warn` and diagnostics to `diag`.
    fn opts<'s>(&'s self, warn: &'s dyn WarnSink, diag: &'s dyn DiagSink) -> LoadOpts<'s> {
        LoadOpts {
            brand: self.brand,
            ext: self.ext,
            regexes: self.regexes,
            warn,
            diag,
        }
    }
}

/// The manifest of a remote `(repo, rev)` as gc can load it, checked but not defaulted (`snp_config::facts`): from
/// the materialised clone, else from the embedded Tier A profile of pre-commit-hooks (ADR-075). `None` = not cloned,
/// or `InvalidManifestError` (the repository stays unused and is removed, `gc.py:33-38`). The check fails and warns
/// exactly when a full `load_manifest` does.
///
/// `index` is gc's one `repos-v1` listing, taken under the exclusive lock (pre-commit's single `SELECT repo, ref,
/// path FROM repos`, `gc.py:65-70`): nothing can materialise or retire a repository while gc holds that lock, so an
/// entry loaded when first asked is the entry an up-front load would have seen.
fn gc_manifest(
    o: &LoadOpts<'_>,
    repo: &str,
    rev: &str,
    key: &RepoKey,
    index: &RepoIndex<'_>,
) -> Option<Value> {
    if let Some(h) = index.get(key) {
        let path = h.src.join(".pre-commit-hooks.yaml");
        return check_manifest(&path, path.as_os_str(), o).ok();
    }
    if snp_hooks_data::is_pre_commit_hooks_url(repo)
        && let Some(profile) = snp_hooks_data::profile_for_rev(rev)
    {
        return check_manifest_bytes(
            profile.manifest_yaml.as_bytes(),
            OsStr::new(".pre-commit-hooks.yaml"),
            o,
        )
        .ok();
    }
    None
}

/// `_mark_used_repos` (`gc.py:18-58`) for the repositories of one checked config, with every key derived by
/// `RepoId::from_config_memo(repo, tracked.project_root, memo)` (ADR-075, ADR-150). `manifest(repo, rev, base key)`
/// is the checked manifest of a remote repository ([`gc_manifest`]), loaded once per entry as pre-commit does.
///
/// A config hook that ends up without `additional_dependencies` uses the base key and marker, which the
/// repository's entry has already marked: only hooks with dependencies derive more.
fn mark_used(
    repos: &[RepoFacts<'_>],
    root: &Path,
    memo: &ProbeMemo,
    used: &mut Used,
    manifest: &mut dyn FnMut(&str, &str, &RepoKey) -> Option<Value>,
) {
    for repo in repos {
        match repo {
            RepoFacts::Local { deps } => {
                for d in deps {
                    used.keys.insert(Store::local_key(d));
                    used.local_markers.insert(db_repo_name("local", d));
                }
            }
            RepoFacts::Remote { repo, rev, hooks } => {
                let id = RepoId::from_config_memo(repo, root, memo);
                let key = Store::remote_key(&id, rev, &[]);
                let Some(checked) = manifest(repo, rev, &key) else {
                    continue;
                };
                let defined = facts::manifest_hooks(&checked);
                used.keys.insert(key);
                used.remote_markers
                    .insert((marker_repo_name(&id, repo, &[]), (*rev).to_owned()));
                for h in hooks {
                    let Some(manifest_deps) = defined.deps(h.id) else {
                        continue;
                    };
                    let deps = h.deps.as_deref().unwrap_or(manifest_deps);
                    if deps.is_empty() {
                        continue;
                    }
                    used.keys.insert(Store::remote_key(&id, rev, deps));
                    used.remote_markers
                        .insert((marker_repo_name(&id, repo, deps), (*rev).to_owned()));
                }
            }
            _ => {}
        }
    }
}

/// Checks one tracked config for gc: `None` = dead (pre-commit catches `InvalidConfigError`; snp also treats the
/// configs that would crash pre-commit's loader as dead, X13-CFG-14 / X13-CMD-09). The check fails and warns
/// exactly when a full `load_config` does; gc reads its few keys from the checked value (`snp_config::facts`).
fn load_tracked(t: &TrackedConfig, o: &LoadOpts<'_>) -> Option<Value> {
    let src = ConfigSource {
        path: t.path.clone(),
        display: t.path.as_os_str().to_os_string(),
        project_root: t.project_root.clone(),
    };
    check_config(&src, o).ok()
}

/// The marks of one tracked config, its warnings and diagnostics written to `o`'s sinks in the serial order (the
/// config's, then each manifest's in `repos` order); `None` = dead. Read-only: it reads files and the store's
/// entries, never writes (ADR-150). The checked values are dropped here, on the thread that built them.
fn load_marks(l: &Loader<'_>, t: &TrackedConfig, o: &LoadOpts<'_>) -> Option<Marked> {
    let checked = load_tracked(t, o)?;
    let mut used = Used::default();
    mark_used(
        &facts::config_repos(&checked, l.ext),
        &t.project_root,
        l.memo,
        &mut used,
        &mut |repo, rev, key| gc_manifest(o, repo, rev, key, l.index),
    );
    drop(checked);
    let marker_names = used
        .remote_markers
        .iter()
        .map(|(name, rev)| {
            MarkerKey::Initialized {
                db_repo_name: Arc::from(name.as_str()),
                rev: Arc::from(rev.as_str()),
            }
            .file_name()
        })
        .collect();
    Some(Marked { used, marker_names })
}

/// Below this many tracked configs gc loads them inline: starting the pool costs more than it saves (ADR-150; a
/// count, never a timing).
const MIN_PARALLEL_CONFIGS: usize = 8;

/// gc's fan-out never uses more threads than this (ADR-150).
const MAX_WIDTH: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(n) => n,
    None => NonZeroUsize::MIN,
};

/// The number of threads gc loads `configs` tracked configs on (ADR-150): 1 (inline) on musl, whose allocator lock
/// makes the fan-out slower than one thread (measured), below [`MIN_PARALLEL_CONFIGS`] configs or with one permit;
/// else min(permits, `cores()`, [`MAX_WIDTH`]), `cores` being `available_parallelism` (cgroup-aware), asked only
/// when it matters.
fn fan_out_width(
    configs: usize,
    permits: NonZeroUsize,
    musl: bool,
    cores: impl FnOnce() -> Option<NonZeroUsize>,
) -> NonZeroUsize {
    if musl || configs < MIN_PARALLEL_CONFIGS || permits == NonZeroUsize::MIN {
        return NonZeroUsize::MIN;
    }
    permits
        .min(cores().unwrap_or(NonZeroUsize::MIN))
        .min(MAX_WIDTH)
}

/// The tracked configs read from `files`, as [`Store::tracked_configs`] returns them, with their indices in `files`.
fn read_tracked(files: &[PathBuf]) -> Vec<(usize, TrackedConfig)> {
    let mut read: Vec<Option<TrackedConfig>> =
        files.iter().map(|f| Store::read_config_entry(f)).collect();
    let decoded: Vec<(usize, &TrackedConfig)> = read
        .iter()
        .enumerate()
        .filter_map(|(i, t)| Some((i, t.as_ref()?)))
        .collect();
    tracked_order(&decoded)
        .into_iter()
        .filter_map(|i| Some((i, read.get_mut(i)?.take()?)))
        .collect()
}

/// The marking on the calling thread (width 1): each tracked config checked and marked in tracked order, its warnings
/// printed as it loads, cancellation checked before each config. `probe(i)` runs after the load of entry `i` (tests).
fn mark_inline(
    cx: &CmdCtx<'_>,
    l: &Loader<'_>,
    files: &[PathBuf],
    probe: &(dyn Fn(usize) + Sync),
) -> Result<Marking, Failure> {
    let mut marking = Marking::default();
    for (i, t) in read_tracked(files) {
        if let Some(f) = frame::interrupted(cx) {
            return Err(f);
        }
        fault::hit(Point::WorkerPartition);
        let marked = load_marks(l, &t, &l.opts(cx.printer, cx.printer));
        probe(i);
        marking.add(&t, marked);
    }
    Ok(marking)
}

/// The marking on the pool (ADR-150): entry `i` of `files` is read, decoded, checked and marked by job `i`
/// (`map_ordered`, `width` threads), its warnings and diagnostics recorded in `logs[i]`. A job that was refused
/// (cancellation) or panicked fails the marking before anything is replayed: the output stays a prefix of the serial
/// marking's (empty), and an incomplete marking never reaches `forget_configs` or a retirement. Otherwise, serially
/// and in tracked order: the per-config cancellation check, the replay of the config's log through the printer's
/// sinks (the calls the inline marking makes, in the same order) and the union of its marks.
fn mark_parallel(
    cx: &CmdCtx<'_>,
    l: &Loader<'_>,
    files: &[PathBuf],
    width: NonZeroUsize,
    probe: &(dyn Fn(usize) + Sync),
) -> Result<Marking, Failure> {
    let n = files.len();
    let logs: Vec<LogRecorder> = (0..n).map(|_| LogRecorder::new()).collect();
    let results = cx.pool.map_ordered(width, n, |i| {
        let (Some(file), Some(log)) = (files.get(i), logs.get(i)) else {
            return Ok(None);
        };
        let Some(t) = Store::read_config_entry(file) else {
            return Ok(None);
        };
        fault::hit(Point::WorkerPartition);
        let marked = load_marks(l, &t, &l.opts(log, log));
        probe(i);
        Ok(Some((t, marked)))
    });
    let mut outcomes: Vec<Option<(TrackedConfig, Option<Marked>)>> = Vec::new();
    let mut failure: Option<Failure> = None;
    for r in results {
        match r {
            Ok(o) => outcomes.push(o),
            Err(e) => {
                // A panic's own failure wins over the refusals it caused.
                if failure
                    .as_ref()
                    .is_none_or(|f| matches!(f, Failure::Interrupted(_)))
                {
                    failure = Some(e);
                }
                outcomes.push(None);
            }
        }
    }
    if let Some(e) = failure {
        return Err(job_failure(cx, e));
    }
    let decoded: Vec<(usize, &TrackedConfig)> = outcomes
        .iter()
        .enumerate()
        .filter_map(|(i, o)| Some((i, &o.as_ref()?.0)))
        .collect();
    let order = tracked_order(&decoded);
    let mut marking = Marking::default();
    for i in order {
        if let Some(f) = frame::interrupted(cx) {
            return Err(f);
        }
        let (Some((t, marked)), Some(log)) =
            (outcomes.get_mut(i).and_then(Option::take), logs.get(i))
        else {
            continue;
        };
        log.replay(cx.printer, cx.printer);
        marking.add(&t, marked);
    }
    Ok(marking)
}

/// What gc returns for a load job that was refused or panicked (ADR-150): the cancellation as the per-config check
/// reports it (a signal, closed stdout); else the panic, as `Interrupted(Panic)` when the panic hook recorded it (the
/// envelope then reports the recorded payload, location and backtrace, as for a panic on the main thread), else the
/// job's own `PanicException`.
fn job_failure(cx: &CmdCtx<'_>, e: Failure) -> Failure {
    if let Some(f) = frame::interrupted(cx) {
        return f;
    }
    if cx.cancel.reason() == Some(CancelReason::Panic) {
        return Failure::Interrupted(CancelReason::Panic);
    }
    e
}

/// `pre-commit gc`: `_mark_used_repos` over the tracked configs, then sweep; `{n} repo(s) removed.`.
///
/// `gc.py:61-96` under the exclusive store lock: every tracked config is loaded (its warnings print, parity); a
/// config that fails to load is forgotten and protects nothing; the used keys follow pre-commit's rules (meta
/// skipped, local keys by deps, remote keys by `(repo, rev)` and per-hook deps, only when the manifest loads);
/// every other repository is retired (ENOENT-tolerant, X13-CMD-07; other failures are the `OSError`, exit 3,
/// X13-CMD-08). Then snp's own maintenance: `sweep` (snp 1.x leftovers announced on stderr, D-DIAG-05) and the
/// `keys-v1` markers of keys that are neither present nor Tier-A-loadable (ADR-075). A store that does not exist
/// is not created: nothing is tracked, `0 repo(s) removed.`.
///
/// The configs load on the pool and their output is replayed in tracked order (ADR-150): stdout, stderr, the
/// removals and the exit are those of the serial marking.
pub fn gc(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    gc_with(cx, None, &|_| {})
}

/// [`gc`] with the fan-out width forced (`None` = [`fan_out_width`]) and `probe(i)` run after the load of
/// `configs-v1` entry `i` (tests).
fn gc_with(
    cx: &CmdCtx<'_>,
    width: Option<NonZeroUsize>,
    probe: &(dyn Fn(usize) + Sync),
) -> Result<i32, Failure> {
    if !cx.store.exists() {
        cx.printer.write_line(messages::repos_removed(0).as_bytes());
        return Ok(0);
    }
    cx.store.ensure_created(cx.brand, cx.printer)?;
    let _lock = exclusive(cx)?;
    // One listing of the repositories (ordered by key, so the retirements and the first failure are deterministic),
    // each entry loaded when first needed.
    let index = cx.store.repo_index();
    let regexes = RegexCache::default();
    let memo = ProbeMemo::new();
    let l = Loader {
        brand: cx.brand,
        ext: frame::ext_mode(cx),
        regexes: &regexes,
        index: &index,
        memo: &memo,
    };
    let files = cx.store.config_entry_files();
    let width = width.unwrap_or_else(|| {
        fan_out_width(
            files.len(),
            cx.pool.permits(),
            cfg!(target_env = "musl"),
            || std::thread::available_parallelism().ok(),
        )
    });
    let marking = if width == NonZeroUsize::MIN {
        mark_inline(cx, &l, &files, probe)?
    } else {
        mark_parallel(cx, &l, &files, width, probe)?
    };
    cx.store.forget_configs(&marking.dead);
    let used = &marking.used;
    let mut removed: usize = 0;
    for k in index.keys() {
        if used.keys.contains(k) {
            continue;
        }
        let Some(h) = index.get(k) else {
            continue;
        };
        cx.store.retire_repo(k).map_err(|e| {
            Failure::from_io(&e, Some(h.src.parent().unwrap_or(&h.src).as_os_str()), None)
        })?;
        removed = removed.saturating_add(1);
    }
    let report = cx.store.sweep(SystemTime::now());
    let mut keep_marker = |k: &MarkerKey| match k {
        MarkerKey::Initialized { db_repo_name, rev } => {
            used.remote_markers
                .contains(&(db_repo_name.to_string(), rev.to_string()))
                || used.local_markers.contains(&**db_repo_name)
        }
        // Virtual-install markers are derived from install keys gc cannot recompute; they are only consulted for
        // a repository whose `Initialized` marker (kept or not, above) decides whether anything is announced.
        _ => true,
    };
    // The `Initialized` markers of used remote keys are kept without being read (their file names are derived).
    let _stale = cx
        .store
        .retain_markers_keeping_names(&marking.marker_names, &mut keep_marker);
    if report.legacy_v1_removed {
        cx.printer
            .diag(&messages::removed_v1(cx.store.location().path.as_os_str()));
    }
    if let Some(journal) = &report.unreadable_journal {
        cx.printer
            .diag(&messages::patches_kept(journal.as_os_str()));
    }
    cx.printer
        .write_line(messages::repos_removed(removed).as_bytes());
    Ok(0)
}

/// `pre-commit clean`: `Cleaned {store}.`.
///
/// `clean.py:10-16`: pre-commit removes its store (re-created by `Store()` before dispatch, so the line always
/// prints) and the pre-1.0 `~/.pre-commit`. snp retires its own layout under the exclusive lock (recovery data,
/// `.lock` and foreign files kept, D-DATA-04), never creates the store, never touches `~/.pre-commit` or
/// pre-commit's store (D-STORE-02), and always prints `Cleaned {store}.` (ADR-030). A stash journal it cannot read
/// keeps every stash patch, with a stderr warning (D-DATA-04, D-DIAG-05).
pub fn clean(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    if cx.store.exists() {
        let _lock = exclusive(cx)?;
        if let Some(journal) = cx.store.clean()? {
            cx.printer
                .diag(&messages::patches_kept(journal.as_os_str()));
        }
    }
    cx.printer
        .write_line(&messages::cleaned(cx.store.location().path.as_os_str()));
    Ok(0)
}

/// Total size in bytes of the regular files below `dir` (symlinks are not followed; unreadable entries count 0).
fn tree_size(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else {
                continue;
            };
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    total
}

/// `12345` → `12.1 KiB` (binary units, one decimal; bytes below 1 KiB as `N B`).
pub(crate) fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut unit = 0_usize;
    let mut scaled = n;
    while scaled >= 1024 * 1024 && unit.saturating_add(1) < UNITS.len() {
        scaled /= 1024;
        unit = unit.saturating_add(1);
    }
    // `scaled` is in [1 KiB, 1 MiB) of the current unit: one decimal from the remainder.
    let whole = scaled / 1024;
    let tenth = (scaled % 1024).saturating_mul(10) / 1024;
    format!(
        "{whole}.{tenth} {}",
        UNITS.get(unit).copied().unwrap_or("PiB")
    )
}

/// `snp cache {dir,size,gc,clean}`.
///
/// `dir` prints the store path (whether or not it exists yet); `size` the total size of its files
/// (`{human} ({bytes} bytes)`, `0 B (0 bytes)` for a missing store); `gc` and `clean` are [`gc`] and [`clean`].
pub fn cache(a: &CacheArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let path = &cx.store.location().path;
    match a.action {
        CacheAction::Dir => {
            cx.printer.write_line(path.as_os_str().as_encoded_bytes());
            Ok(0)
        }
        CacheAction::Size => {
            let n = if cx.store.exists() {
                tree_size(path)
            } else {
                0
            };
            cx.printer
                .write_line(format!("{} ({n} bytes)", human_size(n)).as_bytes());
            Ok(0)
        }
        CacheAction::Gc => gc(cx),
        CacheAction::Clean => clean(cx),
        _ => Err(frame::unsupported("cache")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 * 1024 * 1024 + 512 * 1024), "5.5 MiB");
        assert_eq!(human_size(u64::MAX), "16383.9 PiB");
    }

    /// The width rule (ADR-150): inline on musl, below 8 configs and with one permit; else min(permits, cores, 8),
    /// the cores asked only when they matter.
    #[test]
    fn fan_out_width_rule() {
        let nz = |n: usize| NonZeroUsize::new(n).unwrap();
        let never = || -> Option<NonZeroUsize> { panic!("cores asked") };
        assert_eq!(fan_out_width(200, nz(4), true, never), nz(1));
        assert_eq!(fan_out_width(7, nz(4), false, never), nz(1));
        assert_eq!(fan_out_width(0, nz(4), false, never), nz(1));
        assert_eq!(fan_out_width(200, nz(1), false, never), nz(1));
        assert_eq!(fan_out_width(8, nz(4), false, || Some(nz(16))), nz(4));
        assert_eq!(fan_out_width(200, nz(32), false, || Some(nz(16))), nz(8));
        assert_eq!(fan_out_width(200, nz(32), false, || Some(nz(2))), nz(2));
        assert_eq!(fan_out_width(200, nz(32), false, || None), nz(1));
        // This build's gate: musl targets always load inline.
        let musl = cfg!(target_env = "musl");
        let this = fan_out_width(200, nz(4), musl, || Some(nz(4)));
        assert_eq!(this, if musl { nz(1) } else { nz(4) });
    }

    /// Serial (width 1) and parallel (width 4 on a 4-permit pool, whatever the CPU count) gc over the same generated
    /// stores: byte-identical stdout and stderr, in the same write calls and interleaving, the same result, and the
    /// same repositories, tracked configs and markers left (ADR-150).
    mod parallel {
        use std::collections::BTreeSet;
        use std::fmt::Write as _;
        use std::io::Write;
        use std::sync::Mutex;

        use proptest::prelude::*;
        use snp_core::cancel::CancelToken;
        use snp_core::env::Env;
        use snp_core::output::{Printer, PrinterOpts};
        use snp_git::ProcBackend;
        use snp_proc::pool::JobPool;
        use snp_proc::spawn::Role;
        use snp_store::{RepoKind, RepoMeta, StoreLocation, StoreSource};

        use super::super::*;

        /// Every write of the printer, tagged `o` (stdout) or `e` (stderr), in order.
        #[derive(Clone, Default)]
        struct Events(Arc<Mutex<Vec<Write1>>>);

        /// One write: the stream tag and the bytes.
        type Write1 = (u8, Vec<u8>);

        struct Capture(Events, u8);

        impl Write for Capture {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                snp_core::sync::lock(&(self.0).0).push((self.1, buf.to_vec()));
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        /// What a gc run did.
        #[derive(Debug, PartialEq, Eq)]
        struct Outcome {
            events: Vec<(u8, Vec<u8>)>,
            result: String,
            repos: BTreeSet<String>,
            configs: BTreeSet<String>,
            keys: BTreeSet<String>,
        }

        impl Outcome {
            fn stream(&self, tag: u8) -> Vec<u8> {
                self.events
                    .iter()
                    .filter(|(t, _)| *t == tag)
                    .flat_map(|(_, b)| b.clone())
                    .collect()
            }
        }

        fn names(dir: &Path) -> BTreeSet<String> {
            fs::read_dir(dir).map_or_else(
                |_| BTreeSet::new(),
                |rd| {
                    rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                },
            )
        }

        fn store_at(base: &Path) -> Store {
            Store::new(StoreLocation {
                path: base.join("store"),
                source: StoreSource::SnpHome,
            })
        }

        /// Runs gc on the store under `base` with the width forced, on a pool of `permits`.
        fn run_gc(
            base: &Path,
            width: usize,
            permits: usize,
            cancel: &CancelToken,
            probe: &(dyn Fn(usize) + Sync),
        ) -> Outcome {
            let store = store_at(base);
            let events = Events::default();
            let printer = Printer::new(
                Box::new(Capture(events.clone(), b'o')),
                Box::new(Capture(events.clone(), b'e')),
                PrinterOpts {
                    brand: Brand::Snp,
                    color: false,
                    stdout_is_tty: false,
                    windows_text_mode: false,
                    cancel: cancel.clone(),
                },
            );
            let env = Env::from_pairs([("SNP_TEST", "1")]);
            let pool = JobPool::new(NonZeroUsize::new(permits).unwrap(), cancel.clone());
            let cx = CmdCtx {
                env: &env,
                cwd: base,
                brand: Brand::Snp,
                printer: &printer,
                store: &store,
                git_exe: None,
                git_backend: Arc::new(ProcBackend {
                    cancel: cancel.clone(),
                    role: Role::Tool,
                }),
                cancel,
                pool: &pool,
                self_exe: Path::new("snp"),
                arg0: None,
            };
            let r = gc_with(&cx, NonZeroUsize::new(width), probe);
            printer.finish();
            let root = base.join("store");
            let events = std::mem::take(&mut *events.0.lock().unwrap());
            Outcome {
                events,
                result: format!("{r:?}"),
                repos: names(&root.join("repos-v1")),
                configs: names(&root.join("configs-v1")),
                keys: names(&root.join("keys-v1")),
            }
        }

        const IDS: [&str; 3] = ["a", "b", "c"];

        /// A repository entry's manifest: 0 valid (hooks a, b with deps [x], c), 1 invalid, 2 no manifest file, 3
        /// valid with a `FutureWarning` (`files: '[[m]'`).
        type Entry = (usize, usize, u8, bool);

        /// A config's `repos` item: 0 remote (n, rev, hooks: (id, deps 0 none / 1 [] / 2 [y])), 1 local with deps,
        /// 2 meta, 3 the Tier A pre-commit-hooks, 4 a local path (an existing directory).
        type Item = (u8, usize, usize, Vec<(usize, u8)>);

        /// A tracked config: 0 valid, 1 invalid (`repos: 5`), 2 unreadable, 3 deleted after tracking, 4 a YAML
        /// error; its items; an unknown root key (warning); a root `files` `FutureWarning`; a duplicated entry.
        type Cfg = (u8, Vec<Item>, bool, bool, bool);

        #[derive(Debug, Clone)]
        struct Scenario {
            entries: Vec<Entry>,
            configs: Vec<Cfg>,
            markers: Vec<(usize, usize)>,
        }

        fn url(n: usize) -> String {
            format!("https://example.invalid/r{n}")
        }

        fn rev(r: usize) -> String {
            format!("v{r}")
        }

        fn manifest(kind: u8) -> Option<String> {
            let mut s = String::new();
            match kind {
                1 => s.push_str("-   id: a\n"),
                2 => return None,
                _ => {
                    for (i, id) in IDS.iter().enumerate() {
                        writeln!(
                            s,
                            "-   id: {id}\n    name: {id}\n    entry: e\n    language: system"
                        )
                        .unwrap();
                        if i == 1 {
                            s.push_str("    additional_dependencies: [x]\n");
                        }
                        if kind == 3 {
                            s.push_str("    files: '[[m]'\n");
                        }
                    }
                }
            }
            Some(s)
        }

        fn materialize(store: &Store, (n, r, kind, deps): Entry) {
            let id = RepoId::Url(Arc::from(url(n).as_str()));
            let deps: Arc<[Arc<str>]> = if deps {
                Arc::from([Arc::from("x")])
            } else {
                Arc::from([])
            };
            let rev = rev(r);
            let key = Store::remote_key(&id, &rev, &deps);
            let meta = RepoMeta {
                kind: RepoKind::Remote {
                    id,
                    rev: Arc::from(rev.as_str()),
                },
                deps,
            };
            let text = manifest(kind);
            store
                .materialize(
                    &key,
                    meta,
                    &mut |dir: &Path| {
                        fs::create_dir_all(dir.join(".git")).unwrap();
                        if let Some(m) = &text {
                            fs::write(dir.join(".pre-commit-hooks.yaml"), m).unwrap();
                        }
                        Ok(())
                    },
                    &mut || {},
                    &CancelToken::new(),
                )
                .unwrap();
        }

        fn render(cfg: &Cfg) -> String {
            let (kind, items, unknown, future, _) = cfg;
            match kind {
                1 => return "repos: 5\n".to_owned(),
                4 => return "repos: [\n".to_owned(),
                _ => {}
            }
            let mut s = String::new();
            if *unknown {
                s.push_str("foo: 1\n");
            }
            if *future {
                s.push_str("files: '[[z]'\n");
            }
            s.push_str(if items.is_empty() {
                "repos: []\n"
            } else {
                "repos:\n"
            });
            for (k, n, r, hooks) in items {
                match k {
                    0 => writeln!(s, "-   repo: {}\n    rev: {}", url(*n), rev(*r)),
                    1 => writeln!(s, "-   repo: local"),
                    2 => writeln!(s, "-   repo: meta"),
                    3 => writeln!(
                        s,
                        "-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0"
                    ),
                    _ => writeln!(s, "-   repo: ./d{n}\n    rev: {}", rev(*r)),
                }
                .unwrap();
                s.push_str("    hooks:\n");
                match k {
                    1 => {
                        s.push_str(
                            "    -   id: l\n        name: l\n        entry: e\n        language: system\n",
                        );
                        if hooks.first().is_some_and(|h| h.1 == 2) {
                            s.push_str("        additional_dependencies: [y]\n");
                        }
                    }
                    2 => s.push_str("    -   id: identity\n"),
                    3 => s.push_str("    -   id: trailing-whitespace\n"),
                    _ => {
                        for (id, deps) in hooks {
                            writeln!(s, "    -   id: {}", IDS[*id % IDS.len()]).unwrap();
                            match deps {
                                1 => s.push_str("        additional_dependencies: []\n"),
                                2 => s.push_str("        additional_dependencies: [y]\n"),
                                _ => {}
                            }
                        }
                        if hooks.is_empty() {
                            s.push_str("    -   id: a\n");
                        }
                    }
                }
            }
            s
        }

        /// Builds the scenario's store and projects under `base` (removed first), deterministically.
        fn build(base: &Path, sc: &Scenario) {
            let _removed: std::io::Result<()> = fs::remove_dir_all(base);
            fs::create_dir_all(base).unwrap();
            let store = store_at(base);
            store
                .ensure_created(Brand::Snp, &snp_core::output::CollectWarnings::new())
                .unwrap();
            for e in &sc.entries {
                materialize(&store, *e);
            }
            let configs = base.join("store/configs-v1");
            for (i, cfg) in sc.configs.iter().enumerate() {
                let proj = base.join(format!("p{i}"));
                for n in 0..3 {
                    fs::create_dir_all(proj.join(format!("d{n}"))).unwrap();
                }
                let path = proj.join(".pre-commit-config.yaml");
                fs::write(&path, render(cfg)).unwrap();
                store.mark_config_used(&path, &proj);
                if cfg.4 {
                    // A hand-made duplicate: the same pair under another entry name.
                    let before = names(&configs);
                    let entry = configs.join(store_entry_for(&before, &configs, &path));
                    fs::copy(&entry, configs.join(format!("{:032x}", i + 1))).unwrap();
                }
                match cfg.0 {
                    2 => {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
                        }
                    }
                    3 => fs::remove_file(&path).unwrap(),
                    _ => {}
                }
            }
            for (n, r) in &sc.markers {
                let id = RepoId::Url(Arc::from(url(*n).as_str()));
                store.set_marker(&MarkerKey::Initialized {
                    db_repo_name: Arc::from(marker_repo_name(&id, &url(*n), &[]).as_str()),
                    rev: Arc::from(rev(*r).as_str()),
                });
            }
        }

        /// The configs-v1 entry recording `path`.
        fn store_entry_for(names: &BTreeSet<String>, dir: &Path, path: &Path) -> String {
            names
                .iter()
                .find(|n| Store::read_config_entry(&dir.join(n)).is_some_and(|t| t.path == path))
                .cloned()
                .unwrap()
        }

        /// Restores permissions so the temp dir can be removed.
        fn unlock(base: &Path, sc: &Scenario) {
            #[cfg(unix)]
            for (i, cfg) in sc.configs.iter().enumerate() {
                use std::os::unix::fs::PermissionsExt;
                if cfg.0 == 2 {
                    let path = base.join(format!("p{i}/.pre-commit-config.yaml"));
                    let _restored: std::io::Result<()> =
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
                }
            }
            #[cfg(not(unix))]
            let _ = (base, sc);
        }

        fn item() -> impl Strategy<Value = Item> {
            (
                prop::sample::select(vec![0u8, 0, 0, 0, 1, 2, 3, 4]),
                0usize..5,
                0usize..2,
                prop::collection::vec((0usize..3, 0u8..3), 0..3),
            )
        }

        fn cfg() -> impl Strategy<Value = Cfg> {
            (
                prop::sample::select(vec![0u8, 0, 0, 0, 0, 0, 1, 2, 3, 4]),
                prop::collection::vec(item(), 0..4),
                prop::bool::weighted(0.3),
                prop::bool::weighted(0.2),
                prop::bool::weighted(0.15),
            )
        }

        fn scenario() -> impl Strategy<Value = Scenario> {
            (
                prop::collection::vec(
                    (0usize..6, 0usize..2, 0u8..4, prop::bool::weighted(0.3)),
                    0..8,
                ),
                prop::collection::vec(cfg(), 0..14),
                prop::collection::vec((0usize..6, 0usize..2), 0..4),
            )
                .prop_map(|(entries, configs, markers)| Scenario {
                    entries,
                    configs,
                    markers,
                })
        }

        /// (serial, parallel) outcomes of the scenario, each on a fresh build at the same path.
        fn both(base: &Path, sc: &Scenario) -> (Outcome, Outcome) {
            build(base, sc);
            let serial = run_gc(base, 1, 1, &CancelToken::new(), &|_| {});
            unlock(base, sc);
            build(base, sc);
            let parallel = run_gc(base, 4, 4, &CancelToken::new(), &|_| {});
            unlock(base, sc);
            (serial, parallel)
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(48))]

            #[test]
            fn parallel_gc_equals_serial_gc(sc in scenario()) {
                let d = tempfile::tempdir().unwrap();
                let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap().join("b");
                let (serial, parallel) = both(&base, &sc);
                prop_assert_eq!(serial, parallel, "{:?}", sc);
            }
        }

        /// The generator reaches warnings (stdout), `FutureWarning`s (stderr), dead configs, removed repositories,
        /// removed markers, duplicates and the Tier A profile.
        #[test]
        fn generator_is_not_vacuous() {
            use proptest::strategy::ValueTree as _;
            let mut runner = proptest::test_runner::TestRunner::deterministic();
            let d = tempfile::tempdir().unwrap();
            let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/"))
                .unwrap()
                .join("b");
            let (mut warn, mut future, mut forgot, mut removed, mut dups) = (0, 0, 0, 0, 0);
            for _ in 0..40 {
                let sc = scenario().new_tree(&mut runner).unwrap().current();
                build(&base, &sc);
                let configs_before = names(&base.join("store/configs-v1")).len();
                let repos_before = names(&base.join("store/repos-v1")).len();
                unlock(&base, &sc);
                let (serial, parallel) = both(&base, &sc);
                assert_eq!(serial, parallel, "{sc:?}");
                let out = String::from_utf8_lossy(&serial.stream(b'o')).into_owned();
                let err = String::from_utf8_lossy(&serial.stream(b'e')).into_owned();
                warn += usize::from(out.contains("[WARNING] Unexpected key"));
                future += usize::from(err.contains("FutureWarning"));
                forgot += usize::from(serial.configs.len() < configs_before);
                removed += usize::from(serial.repos.len() < repos_before);
                dups += usize::from(sc.configs.iter().any(|c| c.4));
            }
            assert!(
                warn >= 5 && future >= 5 && forgot >= 5 && removed >= 5 && dups >= 5,
                "warn {warn}, future {future}, forgot {forgot}, removed {removed}, dups {dups}"
            );
        }

        /// A scenario of `n` valid configs, each with a warning, all repositories materialised, plus one unused.
        fn warned(n: usize) -> Scenario {
            Scenario {
                entries: (0..5)
                    .map(|k| (k, 0, 0, false))
                    .chain([(5, 1, 0, false)])
                    .collect(),
                configs: (0..n)
                    .map(|i| {
                        (
                            0,
                            vec![(0, i % 5, 0, vec![(0, 0)])],
                            true,
                            i % 3 == 0,
                            false,
                        )
                    })
                    .collect(),
                markers: vec![(5, 1)],
            }
        }

        fn is_prefix(short: &[u8], long: &[u8]) -> bool {
            long.starts_with(short)
        }

        /// Ctrl-C during the fan-out (ADR-150): the interrupted exit, nothing forgotten, retired or unmarked, and
        /// stdout and stderr a prefix of the uninterrupted run's; the same as the serial marking does.
        #[test]
        fn cancellation_stops_before_any_side_effect() {
            let d = tempfile::tempdir().unwrap();
            let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/"))
                .unwrap()
                .join("b");
            let sc = warned(12);
            build(&base, &sc);
            let before = run_gc(&base, 1, 1, &CancelToken::new(), &|_| {});
            for (width, permits) in [(1, 1), (4, 4), (2, 4)] {
                build(&base, &sc);
                let (repos, configs, keys) = (
                    names(&base.join("store/repos-v1")),
                    names(&base.join("store/configs-v1")),
                    names(&base.join("store/keys-v1")),
                );
                let cancel = CancelToken::new();
                let out = run_gc(&base, width, permits, &cancel, &|i| {
                    if i == 3 {
                        cancel.cancel(CancelReason::Interrupt);
                    }
                });
                assert_eq!(out.result, "Err(Interrupted(Interrupt))", "width {width}");
                assert_eq!(
                    (&out.repos, &out.configs, &out.keys),
                    (&repos, &configs, &keys)
                );
                assert!(
                    is_prefix(&out.stream(b'o'), &before.stream(b'o')),
                    "width {width}"
                );
                assert!(
                    is_prefix(&out.stream(b'e'), &before.stream(b'e')),
                    "width {width}"
                );
                assert!(out.stream(b'o').len() < before.stream(b'o').len());
            }
            assert_eq!(before.result, "Ok(0)");
            assert!(before.repos.len() < names(&base.join("store/repos-v1")).len());
        }

        /// A panic in a load job (ADR-066, ADR-150): `PanicException` (exit 3), or the hook's recorded panic when the
        /// hook cancelled the token; nothing forgotten, retired or unmarked; the output a prefix of the serial run's.
        #[test]
        fn panic_in_a_job_stops_before_any_side_effect() {
            let d = tempfile::tempdir().unwrap();
            let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/"))
                .unwrap()
                .join("b");
            let mut sc = warned(12);
            sc.configs[0].0 = 1; // one dead config, which a completed gc would forget
            build(&base, &sc);
            let before = run_gc(&base, 1, 1, &CancelToken::new(), &|_| {});
            for hook in [false, true] {
                build(&base, &sc);
                let (repos, configs, keys) = (
                    names(&base.join("store/repos-v1")),
                    names(&base.join("store/configs-v1")),
                    names(&base.join("store/keys-v1")),
                );
                let cancel = CancelToken::new();
                let out = run_gc(&base, 4, 4, &cancel, &|i| {
                    if i == 5 {
                        if hook {
                            cancel.cancel(CancelReason::Panic);
                        }
                        panic!("boom in job {i}");
                    }
                });
                if hook {
                    assert_eq!(out.result, "Err(Interrupted(Panic))");
                } else {
                    assert!(out.result.contains("PanicException"), "{}", out.result);
                    let text = format!("{:?}", b"boom in job 5".to_vec());
                    assert!(out.result.contains(&text), "{}", out.result);
                }
                assert_eq!(
                    (&out.repos, &out.configs, &out.keys),
                    (&repos, &configs, &keys)
                );
                assert!(is_prefix(&out.stream(b'o'), &before.stream(b'o')));
                assert!(is_prefix(&out.stream(b'e'), &before.stream(b'e')));
            }
            assert!(before.configs.len() < names(&base.join("store/configs-v1")).len());
        }
    }

    mod marks {
        use std::fmt::Write as _;
        use std::path::Path;

        use proptest::prelude::*;
        use snp_config::{
            Config, ConfigSource, ExtMode, LoadOpts, Manifest, RepoConfig, load_config,
            load_manifest_bytes,
        };
        use snp_core::brand::Brand;
        use snp_core::output::CollectWarnings;

        use super::*;

        /// The marking before the check-only loads (the oracle): the fully loaded config and manifests, every
        /// manifest hook re-deriving its key and marker even without dependencies.
        fn mark_used_full(
            cfg: &Config,
            root: &Path,
            used: &mut Used,
            manifest: &mut dyn FnMut(&str, &str, &RepoKey) -> Option<Manifest>,
        ) {
            for repo in &cfg.repos {
                match repo {
                    RepoConfig::Local { hooks } => {
                        for h in hooks {
                            used.keys
                                .insert(Store::local_key(&h.additional_dependencies));
                            used.local_markers
                                .insert(db_repo_name("local", &h.additional_dependencies));
                        }
                    }
                    RepoConfig::Remote { repo, rev, hooks } => {
                        let id = RepoId::from_config(repo, root);
                        let key = Store::remote_key(&id, rev, &[]);
                        let Some(manifest) = manifest(repo, rev, &key) else {
                            continue;
                        };
                        used.keys.insert(key);
                        used.remote_markers
                            .insert((marker_repo_name(&id, repo, &[]), rev.to_string()));
                        for h in hooks {
                            let Some(m) = manifest.get(&h.id) else {
                                continue;
                            };
                            let raw_deps = h
                                .raw
                                .iter()
                                .find(|(k, _)| k.as_str() == Some("additional_dependencies"))
                                .and_then(|(_, v)| v.as_array())
                                .map(|deps| {
                                    deps.iter()
                                        .filter_map(|d| d.as_str().map(Arc::from))
                                        .collect::<Vec<Arc<str>>>()
                                });
                            let deps =
                                raw_deps.unwrap_or_else(|| m.additional_dependencies.to_vec());
                            used.keys.insert(Store::remote_key(&id, rev, &deps));
                            used.remote_markers
                                .insert((marker_repo_name(&id, repo, &deps), rev.to_string()));
                        }
                    }
                    _ => {}
                }
            }
        }

        const IDS: [&str; 3] = ["a", "b", "c"];
        const DEPS: [&str; 4] = [
            "",
            "additional_dependencies: []",
            "additional_dependencies: [x]",
            "additional_dependencies: [x, 'y==1']",
        ];

        /// A config hook or manifest hook: id, dependencies line, whether the required keys are present.
        type Hook = (usize, usize, bool);

        /// A `repos` entry: 0 URL, 1 local path (an existing directory), 2 local, 3 meta, 4 builtin; the
        /// repository number (repeats share a manifest); its hooks.
        type Repo = (u8, usize, Vec<Hook>);

        /// A manifest per repository number: absent, invalid, or the hooks.
        type Man = Option<Option<Vec<Hook>>>;

        fn hook_lines(out: &mut String, (id, deps, full): &Hook, indent: &str) {
            let id = IDS.get(*id).copied().unwrap_or("a");
            writeln!(out, "{indent}-   id: {id}").unwrap();
            if *full {
                writeln!(
                    out,
                    "{indent}    name: n\n{indent}    entry: e\n{indent}    language: system"
                )
                .unwrap();
            }
            if let Some(d) = DEPS.get(*deps).filter(|d| !d.is_empty()) {
                writeln!(out, "{indent}    {d}").unwrap();
            }
        }

        fn render_config(repos: &[Repo]) -> String {
            let mut s = String::from(if repos.is_empty() {
                "repos: []\n"
            } else {
                "repos:\n"
            });
            for (kind, n, hooks) in repos {
                match kind {
                    0 => writeln!(s, "-   repo: https://example.com/r{n}\n    rev: v{n}"),
                    1 => writeln!(s, "-   repo: ./d{n}\n    rev: v{n}"),
                    2 => writeln!(s, "-   repo: local"),
                    3 => writeln!(s, "-   repo: meta"),
                    _ => writeln!(s, "-   repo: builtin"),
                }
                .unwrap();
                s.push_str(if hooks.is_empty() {
                    "    hooks: []\n"
                } else {
                    "    hooks:\n"
                });
                for h in hooks {
                    if *kind == 3 {
                        s.push_str("    -   id: identity\n");
                    } else {
                        // local hooks need the required keys; builtin hooks take ids only
                        let h = if *kind == 2 {
                            (h.0, h.1, true)
                        } else if *kind == 4 {
                            (h.0, 0, false)
                        } else {
                            (h.0, h.1, false)
                        };
                        hook_lines(&mut s, &h, "    ");
                    }
                }
            }
            s
        }

        fn render_manifest(m: Option<&Vec<Hook>>) -> String {
            match m {
                None => "-   id: a\n".to_owned(),
                Some(hooks) if hooks.is_empty() => "[]\n".to_owned(),
                Some(hooks) => {
                    let mut s = String::new();
                    for h in hooks {
                        hook_lines(&mut s, &(h.0, h.1, true), "");
                    }
                    s
                }
            }
        }

        fn hook() -> impl Strategy<Value = Hook> {
            (0..IDS.len(), 0..DEPS.len(), Just(true))
        }

        fn repo() -> impl Strategy<Value = Repo> {
            (
                prop::sample::select(vec![0u8, 0, 1, 1, 2, 3, 4]),
                0usize..4,
                prop::collection::vec(hook(), 0..4),
            )
        }

        fn man() -> impl Strategy<Value = Man> {
            prop::option::weighted(
                0.85,
                prop::option::weighted(0.85, prop::collection::vec(hook(), 0..5)),
            )
        }

        /// Marks of `config` with `manifests` (by repository number) both ways: (old, new); `None` = the config
        /// does not load (both ways).
        fn marks(config: &str, manifests: &[Man], ext: ExtMode) -> Option<(Used, Used)> {
            let dir = tempfile::tempdir().unwrap();
            for n in 0..4 {
                fs::create_dir_all(dir.path().join(format!("d{n}"))).unwrap();
            }
            let path = dir.path().join("cfg.yaml");
            fs::write(&path, config).unwrap();
            let src = ConfigSource {
                path,
                display: "cfg.yaml".into(),
                project_root: dir.path().to_path_buf(),
            };
            let regexes = RegexCache::default();
            let (warn, diag) = (CollectWarnings::new(), CollectWarnings::new());
            let o = LoadOpts {
                brand: Brand::Snp,
                ext,
                regexes: &regexes,
                warn: &warn,
                diag: &diag,
            };
            let text = |repo: &str| -> Option<String> {
                let n: usize = repo.rsplit(['r', 'd']).next()?.parse().ok()?;
                manifests
                    .get(n)?
                    .as_ref()
                    .map(|m| render_manifest(m.as_ref()))
            };
            let display = OsStr::new(".pre-commit-hooks.yaml");
            let full = load_config(&src, &o);
            let checked = check_config(&src, &o);
            assert_eq!(full.is_ok(), checked.is_ok(), "{config}");
            let (cfg, checked) = (full.ok()?, checked.ok()?);
            let mut old = Used::default();
            mark_used_full(&cfg, &src.project_root, &mut old, &mut |repo, _, _| {
                load_manifest_bytes(text(repo)?.as_bytes(), display, &o).ok()
            });
            let mut new = Used::default();
            mark_used(
                &facts::config_repos(&checked, ext),
                &src.project_root,
                &ProbeMemo::new(),
                &mut new,
                &mut |repo, _, _| check_manifest_bytes(text(repo)?.as_bytes(), display, &o).ok(),
            );
            Some((old, new))
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(128))]

            /// The marks gc derives from the check-only loads equal the old marking's over the full loads.
            #[test]
            fn check_only_marks_equal_full_load_marks(
                repos in prop::collection::vec(repo(), 0..5),
                manifests in prop::collection::vec(man(), 4),
                strict in prop::bool::weighted(0.2),
            ) {
                let ext = if strict { ExtMode::Strict } else { ExtMode::Enabled };
                let config = render_config(&repos);
                if let Some((old, new)) = marks(&config, &manifests, ext) {
                    prop_assert_eq!(old, new, "{}", config);
                }
            }
        }

        /// The generators reach configs that load with remote keys per dependency set, local-path ids, local
        /// markers and repositories left unmarked by an invalid or absent manifest.
        #[test]
        fn marks_generator_is_not_vacuous() {
            use proptest::strategy::ValueTree as _;
            let mut runner = proptest::test_runner::TestRunner::deterministic();
            let strategy = (
                prop::collection::vec(repo(), 0..5),
                prop::collection::vec(man(), 4),
            );
            let (mut loaded, mut dep_keys, mut abs, mut local) = (0, 0, 0, 0);
            for _ in 0..200 {
                let (repos, manifests) = strategy.new_tree(&mut runner).unwrap().current();
                let Some((old, new)) = marks(&render_config(&repos), &manifests, ExtMode::Enabled)
                else {
                    continue;
                };
                assert_eq!(old, new);
                loaded += 1;
                dep_keys += usize::from(new.remote_markers.iter().any(|(n, _)| n.contains(":x")));
                abs += usize::from(
                    new.remote_markers
                        .iter()
                        .any(|(n, _)| n.starts_with("abs:")),
                );
                local += usize::from(new.local_markers.iter().any(|n| n.starts_with("local:")));
            }
            assert!(
                loaded >= 100 && dep_keys >= 20 && abs >= 20 && local >= 20,
                "loaded {loaded}, dep keys {dep_keys}, abs {abs}, local {local}"
            );
        }
    }
}
