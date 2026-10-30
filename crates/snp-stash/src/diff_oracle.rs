//! Modification detection (§3.7, ADR-027, ADR-078).
//!
//! pre-commit's rule (`run.py:274-279`, 02 §12): a hook "modified files" when the stdout of
//! `git diff --no-ext-diff --no-textconv --ignore-submodules` differs before and after it. snp keeps the streamed
//! blake3 [`DiffFingerprint`] of that output and, in [`DiffMode::Stat`], proves "unchanged" without spawning git: a
//! snapshot of every tracked path's `(dev, ino, size, mtime_ns, ctime_ns, mode, kind)` and of every other input of
//! `git diff` (the index, every config file incl. include targets, every attributes file incl. the
//! `.gitattributes` of every directory holding a tracked path — existence counts), with racy entries (mtime or ctime
//! within 2 s of the snapshot, git's own racy rule) content-hashed. Any difference runs the real `git diff`.
//!
//! Owner: WP-23.

mod gitconfig;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_git::{DiffFingerprint, Git, IndexEntries, RepoInfo};

/// At most this many index entries for Stat mode (ADR-027).
const MAX_ENTRIES: usize = 16_384;
/// Above this many paths the re-stat runs in parallel chunks.
const PARALLEL_STAT: usize = 2_048;
/// git's racy window: an entry modified within this many nanoseconds of the snapshot is content-hashed.
const RACY_NS: i128 = 2_000_000_000;
/// The gitlink mode (`160000`): submodules are ignored (`--ignore-submodules`).
const GITLINK: u32 = 0o160_000;

/// How modifications are detected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiffMode {
    /// stat snapshots, a real `git diff` only when something changed.
    Stat,
    /// One `git diff` per hook (pre-commit's way).
    Git,
}

/// pre-commit's `_get_diff` comparison, made cheap.
#[derive(Debug)]
pub struct DiffOracle {
    git: Git,
    mode: DiffMode,
    /// The fingerprint after the last executed hook; `None` = stale (refreshed lazily by the next comparison).
    last: Option<DiffFingerprint>,
    stat: Option<Box<StatState>>,
}

/// Whether a hook changed the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Change {
    /// Same diff as before the hook.
    Unchanged,
    /// The diff changed (the hook "modified files").
    Changed,
}

/// The kind of a stat result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Kind {
    Missing,
    File,
    Dir,
    Symlink,
    Other,
    /// `lstat` failed with this raw OS error (other than ENOENT/ENOTDIR).
    Error(i32),
}

/// One stat tuple.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct St {
    kind: Kind,
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i128,
    ctime: i128,
    mode: u32,
}

impl St {
    const MISSING: Self = Self {
        kind: Kind::Missing,
        dev: 0,
        ino: 0,
        size: 0,
        mtime: 0,
        ctime: 0,
        mode: 0,
    };

    fn from_result(r: std::io::Result<std::fs::Metadata>) -> Self {
        match r {
            Ok(md) => Self::from_metadata(&md),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                Self::MISSING
            }
            Err(e) => Self {
                kind: Kind::Error(e.raw_os_error().unwrap_or(-1)),
                ..Self::MISSING
            },
        }
    }

    fn lstat(p: &Path) -> Self {
        Self::from_result(std::fs::symlink_metadata(p))
    }

    fn stat(p: &Path) -> Self {
        Self::from_result(std::fs::metadata(p))
    }

    #[cfg(unix)]
    fn from_metadata(md: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        let ft = md.file_type();
        let kind = if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_dir() {
            Kind::Dir
        } else if md.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        let ns = |s: i64, n: i64| {
            i128::from(s)
                .saturating_mul(1_000_000_000)
                .saturating_add(i128::from(n))
        };
        Self {
            kind,
            dev: md.dev(),
            ino: md.ino(),
            size: md.size(),
            mtime: ns(md.mtime(), md.mtime_nsec()),
            ctime: ns(md.ctime(), md.ctime_nsec()),
            mode: md.mode(),
        }
    }

    #[cfg(not(unix))]
    fn from_metadata(md: &std::fs::Metadata) -> Self {
        let ft = md.file_type();
        let kind = if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX));
        Self {
            kind,
            size: md.len(),
            mtime,
            ctime: mtime,
            ..Self::MISSING
        }
    }

    /// Racy (git's rule, on mtime and ctime): content must be hashed.
    fn racy(&self, taken: i128) -> bool {
        matches!(self.kind, Kind::File | Kind::Symlink)
            && self.mtime.max(self.ctime) >= taken.saturating_sub(RACY_NS)
    }
}

/// The content hash of a racy path (`None` when unreadable: compared as such).
fn content_hash(p: &Path, kind: Kind, follow: bool) -> Option<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    if kind == Kind::Symlink && !follow {
        let target = std::fs::read_link(p).ok()?;
        h.update(&crate::journal::path_bytes(&target));
    } else {
        let f = std::fs::File::open(p).ok()?;
        h.update_reader(f).ok()?;
    }
    Some(*h.finalize().as_bytes())
}

/// Nanoseconds since the epoch.
fn now_ns() -> i128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX))
}

/// What an input is (decides which list a change refreshes).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum InputKind {
    /// The index file: a change re-reads the tracked list.
    Index,
    /// A config file: a change re-reads the config origins.
    Config,
    /// An attributes file or a directory.
    Other,
}

/// One snapshot of every path the oracle watches.
#[derive(Debug, Clone)]
struct Snapshot {
    /// `lstat` of every tracked path, then of every watched directory.
    paths: Vec<St>,
    /// `(lstat, stat)` of every input.
    inputs: Vec<(St, St)>,
    /// Hashes of the racy paths: `(index into paths, hash)`.
    racy_paths: Vec<(usize, Option<[u8; 32]>)>,
    /// Hashes of the racy inputs.
    racy_inputs: Vec<(usize, Option<[u8; 32]>)>,
}

/// Stat-mode state.
#[derive(Debug)]
struct StatState {
    info: RepoInfo,
    env: Env,
    /// Tracked paths (gitlinks excluded) and then every directory holding one (root included).
    paths: Vec<PathBuf>,
    /// How many of `paths` are tracked paths (the rest are directories).
    n_files: usize,
    inputs: Vec<(PathBuf, InputKind)>,
    origins: Arc<[PathBuf]>,
    snapshot: Option<Snapshot>,
}

/// The tracked paths (gitlinks and duplicate stages excluded) followed by every directory holding one (root
/// included); the second value is the number of tracked paths. `None` above [`MAX_ENTRIES`].
fn tracked_paths(root: &Path, tracked: &IndexEntries) -> Option<(Vec<PathBuf>, usize)> {
    let n = tracked.paths.len();
    if n > MAX_ENTRIES {
        return None;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut seen_dirs: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
    let mut last: Option<&[u8]> = None;
    for (i, p) in tracked.paths.iter().enumerate() {
        if tracked.modes.get(i).copied() == Some(GITLINK) || last == Some(p) {
            continue;
        }
        last = Some(p);
        files.push(root.join(crate::journal::bytes_path(p)));
        // Every ancestor directory (`a/b/c` → `a`, `a/b`).
        let mut end = 0_usize;
        while let Some(off) = p.get(end..).and_then(|rest| rest.iter().position(|&b| b == b'/')) {
            end = end.saturating_add(off);
            if let Some(dir) = p.get(..end)
                && seen_dirs.insert(dir.to_vec())
            {
                dirs.push(root.join(crate::journal::bytes_path(dir)));
            }
            end = end.saturating_add(1);
        }
    }
    let n_files = files.len();
    files.append(&mut dirs);
    Some((files, n_files))
}

/// Builds the list of inputs (and whether `attr.tree` forbids Stat mode).
fn build_inputs(
    info: &RepoInfo,
    env: &Env,
    origins: &[PathBuf],
    dirs: &[PathBuf],
) -> Option<Vec<(PathBuf, InputKind)>> {
    let env_facts = gitconfig::from_env(env, &info.root)?;
    if env.is_present("GIT_ATTR_SOURCE") || env_facts.attr_tree {
        return None;
    }
    let home = env.get_nonempty("HOME").map(PathBuf::from);
    let xdg = env
        .get_nonempty("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    let mut seeds: Vec<PathBuf> = origins.to_vec();
    seeds.extend([
        info.git_dir.join("config"),
        info.common_dir.join("config"),
        info.git_dir.join("config.worktree"),
        info.common_dir.join("config.worktree"),
    ]);
    match env.get_nonempty("GIT_CONFIG_GLOBAL") {
        Some(g) => seeds.push(PathBuf::from(g)),
        None => seeds.extend(home.as_ref().map(|h| h.join(".gitconfig"))),
    }
    seeds.extend(xdg.as_ref().map(|x| x.join("git").join("config")));
    let prefix = snp_proc::resolve::find_executable(std::ffi::OsStr::new("git"), env, &info.root)
        .and_then(|exe| snp_os::realpath::realpath_nonstrict(&exe, &info.root).ok())
        .and_then(|exe| exe.parent().and_then(Path::parent).map(Path::to_path_buf));
    if let Some(s) = env.get_nonempty("GIT_CONFIG_SYSTEM") {
        seeds.push(PathBuf::from(s));
    } else {
        seeds.push(PathBuf::from("/etc/gitconfig"));
        seeds.extend(prefix.as_ref().map(|p| p.join("etc").join("gitconfig")));
    }
    seeds.extend(env_facts.config_files.iter().cloned());
    let facts = gitconfig::collect(&seeds, &info.root, env);
    if facts.attr_tree {
        return None;
    }
    let mut inputs: Vec<(PathBuf, InputKind)> = Vec::new();
    let mut push = |p: PathBuf, k: InputKind| {
        if !inputs.iter().any(|(q, _)| *q == p) {
            inputs.push((p, k));
        }
    };
    push(info.index_file.clone(), InputKind::Index);
    for c in facts.config_files {
        push(c, InputKind::Config);
    }
    for a in facts
        .attributes_files
        .into_iter()
        .chain(env_facts.attributes_files)
    {
        push(a, InputKind::Other);
    }
    if let Some(x) = &xdg {
        push(x.join("git").join("attributes"), InputKind::Other);
    }
    push(PathBuf::from("/etc/gitattributes"), InputKind::Other);
    if let Some(p) = &prefix {
        push(p.join("etc").join("gitattributes"), InputKind::Other);
    }
    push(info.common_dir.join("info").join("attributes"), InputKind::Other);
    push(info.git_dir.join("info").join("attributes"), InputKind::Other);
    for d in dirs {
        push(d.join(".gitattributes"), InputKind::Other);
    }
    Some(inputs)
}

/// `lstat` every path (parallel chunks above [`PARALLEL_STAT`]).
fn stat_all(paths: &[PathBuf]) -> Vec<St> {
    if paths.len() <= PARALLEL_STAT {
        return paths.iter().map(|p| St::lstat(p)).collect();
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .clamp(2, 8);
    let chunk = paths.len().div_ceil(workers).max(1);
    let mut out: Vec<St> = Vec::new();
    let joined = std::thread::scope(|s| {
        let mut handles = Vec::new();
        let mut start = 0_usize;
        while start < paths.len() {
            let end = start.saturating_add(chunk).min(paths.len());
            let part = paths.get(start..end).unwrap_or_default();
            let h = std::thread::Builder::new()
                .name("snp-diff-stat".to_owned())
                .stack_size(256 * 1024)
                .spawn_scoped(s, move || part.iter().map(|p| St::lstat(p)).collect::<Vec<St>>());
            handles.push((start, end, h));
            start = end;
        }
        let mut ok = true;
        for (start, end, h) in handles {
            if let Ok(v) = h.map_err(drop).and_then(|h| h.join().map_err(drop)) {
                out.extend(v);
            } else {
                // A refused or failed thread: stat that chunk here.
                ok = false;
                out.extend(paths.get(start..end).unwrap_or_default().iter().map(|p| St::lstat(p)));
            }
        }
        ok
    });
    let _: bool = joined;
    out
}

fn snapshot_with(paths: &[PathBuf], inputs: &[(PathBuf, InputKind)], path_stats: Vec<St>, taken: i128) -> Snapshot {
    let input_stats: Vec<(St, St)> = inputs
        .iter()
        .map(|(p, _)| (St::lstat(p), St::stat(p)))
        .collect();
    let racy_paths = path_stats
        .iter()
        .enumerate()
        .filter(|(_, s)| s.racy(taken))
        .map(|(i, s)| (i, paths.get(i).and_then(|p| content_hash(p, s.kind, false))))
        .collect();
    let racy_inputs = input_stats
        .iter()
        .enumerate()
        .filter(|(_, (l, s))| l.racy(taken) || s.racy(taken))
        .map(|(i, (_, s))| (i, inputs.get(i).and_then(|(p, _)| content_hash(p, s.kind, true))))
        .collect();
    Snapshot {
        paths: path_stats,
        inputs: input_stats,
        racy_paths,
        racy_inputs,
    }
}

/// What differs between a snapshot and the current state.
#[derive(Debug, Default)]
struct Delta {
    any: bool,
    index: bool,
    config: bool,
}

impl StatState {
    fn new(
        info: &RepoInfo,
        env: &Env,
        tracked: &IndexEntries,
        origins: Arc<[PathBuf]>,
    ) -> Option<Self> {
        let (paths, n_files) = tracked_paths(&info.root, tracked)?;
        let dirs = paths.get(n_files..).unwrap_or_default();
        let inputs = build_inputs(info, env, &origins, dirs)?;
        Some(Self {
            info: info.clone(),
            env: env.clone(),
            paths,
            n_files,
            inputs,
            origins,
            snapshot: None,
        })
    }

    fn take_snapshot(&mut self) {
        let taken = now_ns();
        let stats = stat_all(&self.paths);
        self.snapshot = Some(snapshot_with(&self.paths, &self.inputs, stats, taken));
    }

    /// Compares the current state with the snapshot; returns the delta and the fresh path stats.
    fn compare(&self) -> (Delta, Vec<St>, i128) {
        let taken = now_ns();
        let stats = stat_all(&self.paths);
        let Some(snap) = &self.snapshot else {
            return (
                Delta {
                    any: true,
                    index: true,
                    config: true,
                },
                stats,
                taken,
            );
        };
        let mut d = Delta::default();
        if stats != snap.paths {
            d.any = true;
        }
        if !d.any {
            for (i, h) in &snap.racy_paths {
                let now = self.paths.get(*i).and_then(|p| content_hash(p, snap.paths.get(*i).map_or(Kind::Missing, |s| s.kind), false));
                if now != *h {
                    d.any = true;
                    break;
                }
            }
        }
        for (i, ((p, kind), before)) in self.inputs.iter().zip(&snap.inputs).enumerate() {
            let now = (St::lstat(p), St::stat(p));
            let mut changed = now != *before;
            if !changed && let Some((_, h)) = snap.racy_inputs.iter().find(|(j, _)| *j == i) {
                changed = content_hash(p, now.1.kind, true) != *h;
            }
            if changed {
                d.any = true;
                match kind {
                    InputKind::Index => d.index = true,
                    InputKind::Config => d.config = true,
                    InputKind::Other => {}
                }
            }
        }
        (d, stats, taken)
    }
}

impl DiffOracle {
    /// Stat mode only on Unix, `stat_oracle_safe` filesystems, ≤ 16 384 index entries, no `attr.tree` /
    /// `GIT_ATTR_SOURCE`, `SNP_DIFF_MODE != "git"`, and `tracked` + `origins` supplied by the step-6 fan-out (no
    /// extra spawn). `None` for either ⇒ Git mode.
    ///
    /// `baseline` is the fingerprint of the diff before the first hook (`run.py:430` `_get_diff()`), or `None` to take
    /// it lazily before the first comparison. In Stat mode with a baseline, the first snapshot is taken here.
    pub fn new(
        git: &Git,
        info: &RepoInfo,
        env: &Env,
        baseline: Option<DiffFingerprint>,
        tracked: Option<Arc<IndexEntries>>,
        origins: Option<Arc<[PathBuf]>>,
    ) -> Result<Self, Failure> {
        let stat = Self::stat_state(info, env, tracked, origins);
        let mut oracle = Self {
            git: git.clone(),
            mode: if stat.is_some() {
                DiffMode::Stat
            } else {
                DiffMode::Git
            },
            last: baseline,
            stat: stat.map(Box::new),
        };
        if oracle.last.is_some()
            && let Some(s) = oracle.stat.as_deref_mut()
        {
            s.take_snapshot();
        }
        Ok(oracle)
    }

    fn stat_state(
        info: &RepoInfo,
        env: &Env,
        tracked: Option<Arc<IndexEntries>>,
        origins: Option<Arc<[PathBuf]>>,
    ) -> Option<StatState> {
        if !cfg!(unix) || env.get("SNP_DIFF_MODE").is_some_and(|v| v == "git") {
            return None;
        }
        let (tracked, origins) = (tracked?, origins?);
        if tracked.paths.len() > MAX_ENTRIES {
            return None;
        }
        let safe = |p: &Path| snp_os::fsinfo::stat_oracle_safe(p).unwrap_or(false);
        if !safe(&info.root) || !safe(&info.git_dir) {
            return None;
        }
        StatState::new(info, env, &tracked, origins)
    }

    /// The chosen mode.
    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    /// Falls back to Git mode for the rest of the run.
    fn fall_back_to_git_mode(&mut self) {
        self.mode = DiffMode::Git;
        self.stat = None;
    }

    /// Before a hook that may modify files (stat snapshot, or nothing in Git mode).
    ///
    /// A stale fingerprint (after a native fixer reported changes) is refreshed here — after the snapshot, so a
    /// change racing with the refresh is seen by the next comparison.
    pub fn before_hook(&mut self) -> Result<(), Failure> {
        if let Some(s) = self.stat.as_deref_mut()
            && s.snapshot.is_none()
        {
            s.take_snapshot();
        }
        if self.last.is_none() {
            self.last = Some(self.git.diff_fingerprint()?);
        }
        Ok(())
    }

    /// After it: `Unchanged` without spawning when provably identical, else one real `git diff` and digest compare.
    pub fn after_hook(&mut self) -> Result<Change, Failure> {
        if let Some(s) = self.stat.as_deref_mut() {
            let (delta, stats, taken) = s.compare();
            if !delta.any && self.last.is_some() {
                return Ok(Change::Unchanged);
            }
            self.refresh_lists(&delta)?;
            match self.stat.as_deref_mut() {
                Some(s) if !delta.index && !delta.config => {
                    s.snapshot = Some(snapshot_with(&s.paths, &s.inputs, stats, taken));
                }
                Some(s) => s.take_snapshot(),
                None => {}
            }
        }
        self.compare_real()
    }

    /// Re-reads the tracked list (index changed) and the config origins (config changed); Git mode when Stat mode
    /// stops being possible.
    fn refresh_lists(&mut self, delta: &Delta) -> Result<(), Failure> {
        if !delta.index && !delta.config {
            return Ok(());
        }
        let Some(s) = self.stat.as_deref() else {
            return Ok(());
        };
        let tracked = if delta.index {
            self.git.tracked_with_modes()?
        } else {
            IndexEntries::default()
        };
        let origins: Arc<[PathBuf]> = if delta.config {
            self.git.config_origins()?.into()
        } else {
            Arc::clone(&s.origins)
        };
        let rebuilt = if delta.index {
            StatState::new(&s.info, &s.env, &tracked, origins)
        } else {
            let dirs = s.paths.get(s.n_files..).unwrap_or_default();
            build_inputs(&s.info, &s.env, &origins, dirs).map(|inputs| StatState {
                info: s.info.clone(),
                env: s.env.clone(),
                paths: s.paths.clone(),
                n_files: s.n_files,
                inputs,
                origins,
                snapshot: None,
            })
        };
        match rebuilt {
            Some(n) => self.stat = Some(Box::new(n)),
            None => self.fall_back_to_git_mode(),
        }
        Ok(())
    }

    /// One real `git diff`, compared with the last fingerprint (a stale one counts as changed: fail closed).
    fn compare_real(&mut self) -> Result<Change, Failure> {
        let now = self.git.diff_fingerprint()?;
        let change = if self.last == Some(now) {
            Change::Unchanged
        } else {
            Change::Changed
        };
        self.last = Some(now);
        Ok(change)
    }

    /// Native fixer reported exact modified paths (09 §6.7 caveat rules evaluated by the caller).
    ///
    /// `needs_real_diff` (a caveat applies): one real `git diff` compared with the fingerprint refreshed by
    /// [`DiffOracle::before_hook`]. Otherwise `modified_tracked` decides without a spawn; the fingerprint and the
    /// snapshot become stale and are refreshed lazily by the next comparison.
    pub fn after_native(
        &mut self,
        modified_tracked: bool,
        needs_real_diff: bool,
    ) -> Result<Change, Failure> {
        if needs_real_diff {
            if let Some(s) = self.stat.as_deref_mut() {
                s.take_snapshot();
            }
            return self.compare_real();
        }
        if modified_tracked {
            self.last = None;
            if let Some(s) = self.stat.as_deref_mut() {
                s.snapshot = None;
            }
            return Ok(Change::Changed);
        }
        Ok(Change::Unchanged)
    }

    /// Show-diff gate (last digest).
    ///
    /// `retval and args.show_diff_on_failure and prior_diff` (`run.py:304`): whether the diff after the last executed
    /// hook is non-empty (refreshed when stale).
    pub fn non_empty(&mut self) -> Result<bool, Failure> {
        if let Some(d) = self.last {
            return Ok(d.non_empty);
        }
        if let Some(s) = self.stat.as_deref_mut()
            && s.snapshot.is_none()
        {
            s.take_snapshot();
        }
        let d = self.git.diff_fingerprint()?;
        self.last = Some(d);
        Ok(d.non_empty)
    }
}
