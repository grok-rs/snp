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
//! On a clean `--all-files` run the oracle also guards the trusted index modes of the file table (ADR-106): it
//! clears their flag after every hook that may have changed the worktree, whatever the diff says.
//!
//! Owner: WP-23.

mod gitconfig;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_git::{CheckoutConfig, DiffFingerprint, Git, IndexEntries, RepoInfo, ZList};

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
    /// The flag of the file table's trusted index modes (ADR-106), cleared after every hook that may have changed
    /// the worktree.
    trust: Option<Arc<AtomicBool>>,
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

/// What [`DiffOracle::elide_checkout_baseline`] made of the baseline (ADR-119).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Elision {
    /// Not proven (or not applicable): the baseline stays lazy, the real `git diff` runs before the first hook.
    NotProven,
    /// Proven empty: no `git diff` runs for it.
    Elided,
    /// Proven, and `SNP_VERIFY_BASELINE`'s real `git diff` was empty too.
    Verified,
    /// Proven, but `SNP_VERIFY_BASELINE`'s real `git diff` was not empty; it is the baseline.
    Mismatch,
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
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Self::MISSING
            }
            Err(e) => Self {
                kind: Kind::Error(e.raw_os_error().unwrap_or(-1)),
                ..Self::MISSING
            },
        }
    }

    fn lstat(p: &Path) -> Self {
        Self::from_result(std::fs::symlink_metadata(p)).dir_identity()
    }

    /// A directory is compared by identity only (`kind`, `dev`, `ino`): its size and times change whenever an
    /// untracked file is created in it, which never changes `git diff`; a directory replaced by a symlink or by
    /// another directory still differs.
    const fn dir_identity(self) -> Self {
        match self.kind {
            Kind::Dir => Self {
                size: 0,
                mtime: 0,
                ctime: 0,
                mode: 0,
                ..self
            },
            _ => self,
        }
    }

    fn stat(p: &Path) -> Self {
        Self::from_result(std::fs::metadata(p))
    }

    /// `(lstat, stat)` of an input: what [`Self::lstat`] and [`Self::stat`] return, from one `lstat` call unless the
    /// path is a symlink.
    fn lstat_and_stat(p: &Path) -> (Self, Self) {
        Self::lstat_then_stat(p, Self::stat)
    }

    /// [`Self::lstat_and_stat`] with the `stat` call passed in (the tests count it).
    ///
    /// Only a symlink as the last component makes `stat` resolve a path differently from `lstat`. Otherwise both
    /// resolve the leading components alike and end at the same inode, or fail at the same component with the same
    /// error, so the raw `lstat` result (before [`Self::dir_identity`]) is the `stat` result. Off Unix (where Stat
    /// mode never runs) a reparse point that is not a symlink breaks that argument, so `stat` is always called there.
    fn lstat_then_stat(p: &Path, stat: impl FnOnce(&Path) -> Self) -> (Self, Self) {
        let raw = Self::from_result(std::fs::symlink_metadata(p));
        let follow = if cfg!(unix) && raw.kind != Kind::Symlink {
            raw
        } else {
            stat(p)
        };
        (raw.dir_identity(), follow)
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
        } else if md.is_file() {
            Kind::File
        } else {
            Kind::Other
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
    /// The index entries `paths` were built from (ADR-119 reads their modes).
    tracked: Arc<IndexEntries>,
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
        while let Some(off) = p
            .get(end..)
            .and_then(|rest| rest.iter().position(|&b| b == b'/'))
        {
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
    if facts.attr_tree || facts.overflow {
        return None;
    }
    let mut inputs: Vec<(PathBuf, InputKind)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut push = |p: PathBuf, k: InputKind| {
        if seen.insert(p.clone()) {
            inputs.push((p, k));
        }
    };
    push(info.index_file.clone(), InputKind::Index);
    // `$GIT_DIR/HEAD`: `includeIf "onbranch:…"` makes the effective config depend on it, and `git branch -m` or
    // `git symbolic-ref` change it without touching the index.
    push(info.git_dir.join("HEAD"), InputKind::Config);
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
    push(
        info.common_dir.join("info").join("attributes"),
        InputKind::Other,
    );
    push(
        info.git_dir.join("info").join("attributes"),
        InputKind::Other,
    );
    // The directories are distinct (`tracked_paths`), so only the fixed inputs above can collide with them.
    for d in dirs {
        let p = d.join(".gitattributes");
        if !seen.contains(&p) {
            inputs.push((p, InputKind::Other));
        }
    }
    Some(inputs)
}

/// `f` over every item, in parallel chunks above [`PARALLEL_STAT`] items (a refused or failed thread's chunk is done
/// here).
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() <= PARALLEL_STAT {
        return items.iter().map(&f).collect();
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .clamp(2, 8);
    let chunk = items.len().div_ceil(workers).max(1);
    let f = &f;
    let mut out: Vec<R> = Vec::new();
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        let mut start = 0_usize;
        while start < items.len() {
            let end = start.saturating_add(chunk).min(items.len());
            let part = items.get(start..end).unwrap_or_default();
            let h = std::thread::Builder::new()
                .name("snp-diff-stat".to_owned())
                .stack_size(256 * 1024)
                .spawn_scoped(s, move || part.iter().map(f).collect::<Vec<R>>());
            handles.push((start, end, h));
            start = end;
        }
        for (start, end, h) in handles {
            if let Ok(v) = h.map_err(drop).and_then(|h| h.join().map_err(drop)) {
                out.extend(v);
            } else {
                out.extend(items.get(start..end).unwrap_or_default().iter().map(f));
            }
        }
    });
    out
}

/// `lstat` every path (parallel chunks above [`PARALLEL_STAT`]).
fn stat_all(paths: &[PathBuf]) -> Vec<St> {
    par_map(paths, |p| St::lstat(p))
}

/// `(lstat, stat)` of every input (parallel chunks above [`PARALLEL_STAT`]): one `lstat` per input, plus a `stat` for
/// each symlink ([`St::lstat_and_stat`]).
fn stat_inputs(inputs: &[(PathBuf, InputKind)]) -> Vec<(St, St)> {
    par_map(inputs, |(p, _)| St::lstat_and_stat(p))
}

fn snapshot_with(
    paths: &[PathBuf],
    inputs: &[(PathBuf, InputKind)],
    path_stats: Vec<St>,
    taken: i128,
) -> Snapshot {
    let input_stats = stat_inputs(inputs);
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
        .map(|(i, (_, s))| {
            (
                i,
                inputs
                    .get(i)
                    .and_then(|(p, _)| content_hash(p, s.kind, true)),
            )
        })
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
        tracked: Arc<IndexEntries>,
        origins: Arc<[PathBuf]>,
    ) -> Option<Self> {
        let (paths, n_files) = tracked_paths(&info.root, &tracked)?;
        let dirs = paths.get(n_files..).unwrap_or_default();
        let inputs = build_inputs(info, env, &origins, dirs)?;
        Some(Self {
            info: info.clone(),
            env: env.clone(),
            paths,
            n_files,
            tracked,
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
                let now = self.paths.get(*i).and_then(|p| {
                    content_hash(
                        p,
                        snap.paths.get(*i).map_or(Kind::Missing, |s| s.kind),
                        false,
                    )
                });
                if now != *h {
                    d.any = true;
                    break;
                }
            }
        }
        let now_inputs = stat_inputs(&self.inputs);
        for (i, (((p, kind), before), now)) in self
            .inputs
            .iter()
            .zip(&snap.inputs)
            .zip(now_inputs)
            .enumerate()
        {
            let mut changed = now != *before;
            // `racy_inputs` is in input order.
            if !changed
                && let Ok(k) = snap.racy_inputs.binary_search_by_key(&i, |(j, _)| *j)
                && let Some((_, h)) = snap.racy_inputs.get(k)
            {
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

    /// ADR-119: whether the snapshot, taken after the stash's checkout, proves that `git diff` prints nothing.
    ///
    /// The checked-out paths are exactly the files the stash's `diff-index` found different; every other tracked
    /// file was clean in that `diff-index`, which reads the worktree with the same index, configuration and
    /// attributes as `git diff`. The checkout rewrote only those paths, and wrote each from its index entry. So `git
    /// diff` prints nothing when (a) the effective configuration is plain ([`CheckoutConfig::plain`]: no
    /// `core.autocrlf`, fsmonitor, ignoreStat, sparse checkout or configuration-based hook) and no fsmonitor test
    /// hook is in the environment; (b) no attributes file exists (every attributes input of the snapshot is
    /// missing), so no filter, `text`/`eol`/`crlf`, `ident` or `working-tree-encoding` conversion applies and the
    /// checkout wrote each blob's bytes, which `git diff` reads back unconverted; (c) no `post-checkout` or
    /// `post-index-change` hook exists, the two hooks a path-limited checkout runs; (d) every directory holding a
    /// tracked path is a directory (no path is beyond a symbolic link); and (e)
    /// [`checked_out_files_plain`]. Anything else answers `false`: the caller takes the real baseline.
    fn checkout_left_no_diff(&self, checked_out: &ZList, config: &CheckoutConfig) -> bool {
        let Some(snap) = &self.snapshot else {
            return false;
        };
        if !config.plain
            || ["GIT_TEST_FSMONITOR", "GIT_FSMONITOR_TEST"]
                .into_iter()
                .any(|k| self.env.get_nonempty(k).is_some())
        {
            return false;
        }
        let no_attributes = self
            .inputs
            .iter()
            .zip(&snap.inputs)
            .all(|((_, kind), (l, s))| {
                *kind != InputKind::Other || (l.kind == Kind::Missing && s.kind == Kind::Missing)
            });
        let real_dirs = snap
            .paths
            .get(self.n_files..)
            .unwrap_or_default()
            .iter()
            .all(|s| s.kind == Kind::Dir);
        let files = snap.paths.get(..self.n_files).unwrap_or_default();
        no_attributes
            && real_dirs
            && no_checkout_hooks(&self.info, config)
            && checked_out_files_plain(
                self.tracked
                    .paths
                    .iter()
                    .zip(self.tracked.modes.iter().copied()),
                files,
                &checked_out.iter().collect(),
                config.file_mode,
            )
    }
}

/// The hooks a path-limited `git checkout` can run (ADR-119): `post-checkout` (after the checkout of paths) and
/// `post-index-change` (after every index write, git ≥ 2.22).
const CHECKOUT_HOOKS: [&str; 2] = ["post-checkout", "post-index-change"];

/// ADR-119 (c): neither of [`CHECKOUT_HOOKS`] exists, in any form (`lstat` finds nothing), in the directory git looks
/// in: `core.hooksPath` (absolute, or relative to the work-tree root, where git runs) or `<common dir>/hooks`. A
/// `core.hooksPath` git would expand (`~`, `%(prefix)`) or an empty one answers `false`.
fn no_checkout_hooks(info: &RepoInfo, config: &CheckoutConfig) -> bool {
    let dir = match config.hooks_path.as_deref() {
        None => info.common_dir.join("hooks"),
        Some(p) if p.is_empty() || p.starts_with(b"~") || p.starts_with(b"%(") => return false,
        Some(p) => info.root.join(crate::journal::bytes_path(p)),
    };
    CHECKOUT_HOOKS
        .iter()
        .all(|h| St::lstat(&dir.join(h)).kind == Kind::Missing)
}

/// ADR-119 (e), over the index entries (`(path, mode)` in index order) and the snapshot's tracked-path slots
/// (`files`, in [`tracked_paths`] order): every checked-out path (`want`) is a tracked entry at stage 0 (no gitlink, no second stage) of mode `100644` or `100755`, its slot is a regular
/// file whose owner-executable bit matches the mode when `core.fileMode` holds (git compares exactly that bit, so a
/// umask without it would show a mode change), and no other tracked path reaches its inode (a case or Unicode
/// normalization twin on an insensitive filesystem, or a hard link: the checkout of one would change what `git diff`
/// reads for the other). An empty list proves nothing.
fn checked_out_files_plain<'a>(
    tracked: impl Iterator<Item = (&'a [u8], u32)>,
    files: &[St],
    want: &HashSet<&[u8]>,
    file_mode: bool,
) -> bool {
    if want.is_empty() {
        return false;
    }
    let mut found: HashSet<&[u8]> = HashSet::new();
    let mut inodes: HashSet<(u64, u64)> = HashSet::new();
    let mut slot = 0_usize;
    let mut last: Option<&[u8]> = None;
    for (p, mode) in tracked {
        // the slot rule of `tracked_paths`
        if mode == GITLINK || last == Some(p) {
            if want.contains(p) {
                return false;
            }
            continue;
        }
        last = Some(p);
        if want.contains(p) {
            let Some(st) = files.get(slot) else {
                return false;
            };
            let exec = st.mode & 0o100 != 0;
            let regular = st.kind == Kind::File
                && match mode {
                    0o100_644 => !file_mode || !exec,
                    0o100_755 => !file_mode || exec,
                    _ => false,
                };
            if !regular || !found.insert(p) || !inodes.insert((st.dev, st.ino)) {
                return false;
            }
        }
        slot = slot.saturating_add(1);
    }
    found.len() == want.len()
        && files
            .iter()
            .filter(|s| {
                !matches!(s.kind, Kind::Missing | Kind::Error(_))
                    && inodes.contains(&(s.dev, s.ino))
            })
            .count()
            == inodes.len()
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
            trust: None,
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
        StatState::new(info, env, tracked, origins)
    }

    /// The chosen mode.
    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    /// ADR-119: right after the stash's checkout of `checked_out` (and before any hook), take the baseline as known
    /// empty when the Stat-mode snapshot proves `git diff` prints nothing (see `StatState::checkout_left_no_diff`;
    /// `config` is query G's [`CheckoutConfig`]). Only on a lazy baseline (none given to [`Self::new`]) in Stat mode;
    /// the snapshot is taken here, so a later [`Self::before_hook`] keeps it. When the proof fails nothing else
    /// changes: the baseline stays lazy and [`Self::before_hook`] runs the real `git diff` after the snapshot, as
    /// before.
    ///
    /// `verify` (`SNP_VERIFY_BASELINE=1`): a proven baseline is still taken with the real `git diff`, which becomes
    /// the baseline; [`Elision::Mismatch`] when it was not empty. A failing `git diff` leaves the baseline lazy.
    pub fn elide_checkout_baseline(
        &mut self,
        checked_out: &ZList,
        config: &CheckoutConfig,
        verify: bool,
    ) -> Elision {
        if self.last.is_some() {
            return Elision::NotProven;
        }
        let Some(s) = self.stat.as_deref_mut() else {
            return Elision::NotProven;
        };
        if s.snapshot.is_none() {
            s.take_snapshot();
        }
        if !s.checkout_left_no_diff(checked_out, config) {
            return Elision::NotProven;
        }
        if !verify {
            self.last = Some(DiffFingerprint::EMPTY);
            return Elision::Elided;
        }
        let Ok(real) = self.git.diff_fingerprint() else {
            return Elision::NotProven;
        };
        self.last = Some(real);
        if real == DiffFingerprint::EMPTY {
            Elision::Verified
        } else {
            Elision::Mismatch
        }
    }

    /// Guards the flag of the file table's trusted index modes (ADR-106): from now on every [`Self::after_hook`] and
    /// [`Self::after_native`] clears it before it decides anything. That covers every kind of change (a real `git
    /// diff` that differs, a Stat-mode delta, a native fixer's report) before `stage_fixed` may rewrite the result
    /// to `Unchanged`, and every hook whose effect the diff cannot show: one that runs `git rm` or `git add` leaves
    /// `git diff` empty while files appear or disappear (D-CRASH-08 promises deletions are seen). Hooks that never
    /// reach the oracle (`reads_only` natives, meta hooks) cannot change the worktree.
    pub fn guard_index_trust(&mut self, valid: Arc<AtomicBool>) {
        self.trust = Some(valid);
    }

    /// Clears the trusted index modes' flag (ADR-106).
    fn invalidate_trust(&self) {
        if let Some(t) = &self.trust {
            t.store(false, Ordering::Release);
        }
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
        self.invalidate_trust();
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
            StatState::new(&s.info, &s.env, Arc::new(tracked), origins)
        } else {
            let dirs = s.paths.get(s.n_files..).unwrap_or_default();
            build_inputs(&s.info, &s.env, &origins, dirs).map(|inputs| StatState {
                info: s.info.clone(),
                env: s.env.clone(),
                paths: s.paths.clone(),
                n_files: s.n_files,
                tracked: Arc::clone(&s.tracked),
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
        self.invalidate_trust();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A regular file's stat tuple: inode `ino`, permission bits `perm`.
    fn file(ino: u64, perm: u32) -> St {
        St {
            kind: Kind::File,
            dev: 1,
            ino,
            size: 3,
            mtime: 1,
            ctime: 1,
            mode: 0o100_000 | perm,
        }
    }

    /// [`checked_out_files_plain`] over `(path, mode)` entries, slot stats and the checked-out list.
    fn plain(entries: &[(&str, u32)], files: &[St], want: &[&str], file_mode: bool) -> bool {
        let want: HashSet<&[u8]> = want.iter().map(|p| p.as_bytes()).collect();
        checked_out_files_plain(
            entries.iter().map(|(p, m)| (p.as_bytes(), *m)),
            files,
            &want,
            file_mode,
        )
    }

    /// ADR-119 (e): the per-path half of the proof, case by case.
    #[test]
    fn checked_out_files_must_be_plain_regular_files_of_their_own() {
        let entries = [("a", 0o100_644), ("b", 0o100_755), ("c", 0o100_644)];
        let files = [file(1, 0o644), file(2, 0o755), file(3, 0o600)];
        assert!(plain(&entries, &files, &["a"], true));
        assert!(plain(&entries, &files, &["a", "b", "c"], true));
        assert!(
            !plain(&entries, &files, &[], true),
            "an empty list proves nothing"
        );
        assert!(!plain(&entries, &files, &["z"], true), "not tracked");
        // the owner-executable bit, under `core.fileMode` only (a umask without `u+x`)
        let no_x = [file(1, 0o644), file(2, 0o644), file(3, 0o600)];
        assert!(!plain(&entries, &no_x, &["b"], true));
        assert!(plain(&entries, &no_x, &["b"], false));
        let x = [file(1, 0o744), file(2, 0o755), file(3, 0o600)];
        assert!(!plain(&entries, &x, &["a"], true));
        assert!(plain(&entries, &x, &["a"], false));
        // not a regular file now, or not a regular entry
        for kind in [
            Kind::Symlink,
            Kind::Dir,
            Kind::Missing,
            Kind::Other,
            Kind::Error(13),
        ] {
            let st = St {
                kind,
                ..file(1, 0o644)
            };
            assert!(
                !plain(
                    &entries,
                    &[st, file(2, 0o755), file(3, 0o600)],
                    &["a"],
                    true
                ),
                "{kind:?}"
            );
        }
        assert!(
            !plain(&[("a", 0o120_000)], &[file(1, 0o644)], &["a"], true),
            "a symlink entry"
        );
        assert!(
            !plain(&[("a", 0o100_664)], &[file(1, 0o664)], &["a"], true),
            "an unusual mode"
        );
        // a gitlink has no slot; a second stage is refused
        let staged = [
            ("g", GITLINK),
            ("m", 0o100_644),
            ("m", 0o100_644),
            ("x", 0o100_644),
        ];
        let slots = [file(5, 0o644), file(6, 0o644)];
        assert!(plain(&staged, &slots, &["x"], true), "x is the second slot");
        assert!(!plain(&staged, &slots, &["m"], true));
        assert!(!plain(&staged, &slots, &["g"], true));
        let shifted = [file(5, 0o644), St::MISSING];
        assert!(
            !plain(&staged, &shifted, &["x"], true),
            "x's slot follows the rule of tracked_paths"
        );
        // an inode reached by another tracked path: a case twin on an insensitive filesystem, a hard link
        let twins = [file(1, 0o644), file(2, 0o755), file(1, 0o644)];
        assert!(!plain(&entries, &twins, &["a"], true));
        assert!(!plain(&entries, &twins, &["c"], true));
        assert!(plain(&entries, &twins, &["b"], true));
        assert!(!plain(&entries, &twins, &["a", "c"], true));
        // missing slots and unreadable ones share no inode
        let holes = [
            file(1, 0o644),
            St::MISSING,
            St {
                kind: Kind::Error(13),
                ..St::MISSING
            },
        ];
        assert!(plain(&entries, &holes, &["a"], true));
        // fewer slots than entries: refused, never read out of bounds
        assert!(!plain(&entries, &files[..1], &["c"], true));
    }

    /// The reference: an `lstat` and a `stat` call per input, as `stat_inputs` took them before.
    fn two_calls(p: &Path) -> (St, St) {
        (St::lstat(p), St::stat(p))
    }

    /// [`St::lstat_then_stat`] and how many times it called `stat`.
    fn counted(p: &Path) -> ((St, St), usize) {
        let mut calls = 0_usize;
        let got = St::lstat_then_stat(p, |p| {
            calls += 1;
            St::stat(p)
        });
        (got, calls)
    }

    /// A regular file and a directory take one `lstat` and no `stat` on Unix (off Unix `stat` is always called); the
    /// `stat` half of a directory keeps the size and times its `lstat` half drops.
    #[test]
    fn a_file_or_a_directory_takes_one_call_on_unix() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("file");
        let dir = d.path().join("dir");
        std::fs::write(&file, "x = 1\n").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("entry"), "e").unwrap();
        for (p, kind) in [(&file, Kind::File), (&dir, Kind::Dir)] {
            let ((l, s), calls) = counted(p);
            assert_eq!((l, s), two_calls(p), "{}", p.display());
            assert_eq!((l.kind, s.kind), (kind, kind), "{}", p.display());
            assert_eq!(calls, usize::from(!cfg!(unix)), "{}", p.display());
            assert_eq!(l, s.dir_identity(), "{}", p.display());
        }
        let ((l, s), _) = counted(&dir);
        assert_ne!(l, s, "the stat half of a directory is its full stat");
        assert_eq!(St::lstat_and_stat(&file), counted(&file).0);
    }

    /// Every other shape an input can take: the pair equals the two-call result, and `stat` is called exactly when
    /// `lstat` saw a symlink (to a file or a directory, dangling, a loop, into an unsearchable directory, a chain).
    #[cfg(unix)]
    #[test]
    fn one_lstat_per_input_unless_it_is_a_symlink() {
        use rustix::io::Errno;
        use std::os::unix::fs::{PermissionsExt, symlink};
        let d = tempfile::tempdir().unwrap();
        let at = |rel: &str| d.path().join(rel);
        std::fs::write(at("file"), "x = 1\n").unwrap();
        std::fs::write(at("empty"), "").unwrap();
        std::fs::create_dir_all(at("dir")).unwrap();
        std::fs::write(at("dir/entry"), "e").unwrap();
        std::fs::create_dir_all(at("locked")).unwrap();
        std::fs::write(at("locked/f"), "f").unwrap();
        for (target, link) in [
            ("file", "to-file"),
            ("dir", "to-dir"),
            ("nowhere", "dangling"),
            ("self", "self"),
            ("loop-b", "loop-a"),
            ("loop-a", "loop-b"),
            ("to-file", "chain"),
            ("locked/f", "into-locked"),
        ] {
            symlink(target, at(link)).unwrap();
        }
        let socket = std::os::unix::net::UnixListener::bind(at("sock")).unwrap();
        std::fs::set_permissions(at("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = |e: Errno| Kind::Error(e.raw_os_error());
        let denied = if rustix::process::geteuid().is_root() {
            Kind::File
        } else {
            err(Errno::ACCESS)
        };
        let long: String = std::iter::repeat_n('n', 300).collect();
        // (path, kinds of the lstat and stat halves); `None`: platform-defined (whether `lstat` follows a symlink
        // before a trailing slash).
        let cases: [(&str, Option<(Kind, Kind)>); 22] = [
            ("file", Some((Kind::File, Kind::File))),
            ("empty", Some((Kind::File, Kind::File))),
            ("dir", Some((Kind::Dir, Kind::Dir))),
            ("dir/entry", Some((Kind::File, Kind::File))),
            ("sock", Some((Kind::Other, Kind::Other))),
            ("missing", Some((Kind::Missing, Kind::Missing))),
            ("file/x", Some((Kind::Missing, Kind::Missing))),
            ("dangling/x", Some((Kind::Missing, Kind::Missing))),
            // a trailing slash after a symlink to a file: ENOTDIR on Linux, macOS resolves the link
            (
                "to-file/",
                cfg!(target_os = "linux").then_some((Kind::Missing, Kind::Missing)),
            ),
            ("to-dir/entry", Some((Kind::File, Kind::File))),
            ("self/x", Some((err(Errno::LOOP), err(Errno::LOOP)))),
            (
                &long,
                Some((err(Errno::NAMETOOLONG), err(Errno::NAMETOOLONG))),
            ),
            ("locked", Some((Kind::Dir, Kind::Dir))),
            ("locked/f", Some((denied, denied))),
            ("to-file", Some((Kind::Symlink, Kind::File))),
            ("to-dir", Some((Kind::Symlink, Kind::Dir))),
            ("dangling", Some((Kind::Symlink, Kind::Missing))),
            ("self", Some((Kind::Symlink, err(Errno::LOOP)))),
            ("loop-a", Some((Kind::Symlink, err(Errno::LOOP)))),
            ("chain", Some((Kind::Symlink, Kind::File))),
            ("into-locked", Some((Kind::Symlink, denied))),
            ("to-dir/", None),
        ];
        let got: Vec<_> = cases
            .iter()
            .map(|(rel, _)| (counted(&at(rel)), two_calls(&at(rel))))
            .collect();
        std::fs::set_permissions(at("locked"), std::fs::Permissions::from_mode(0o700)).unwrap();
        drop(socket);
        for ((rel, want), (((l, s), calls), reference)) in cases.iter().zip(got) {
            assert_eq!((l, s), reference, "{rel:?}");
            assert_eq!(calls, usize::from(l.kind == Kind::Symlink), "{rel:?}");
            if let Some(kinds) = want {
                assert_eq!((l.kind, s.kind), *kinds, "{rel:?}");
            }
        }
    }
}
