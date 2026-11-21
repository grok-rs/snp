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

mod attributes;
mod gitconfig;
mod index_proof;

pub(crate) use index_proof::every_entry_stat_clean;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_git::{
    CheckoutConfig, ConfigListing, DiffFingerprint, Git, IndexEntries, RepoInfo, SystemAttributes,
    ZList,
};
use snp_proc::pool::JobPool;

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
    /// The index-stat proof (ADR-134), Git mode only.
    proof: Option<Box<index_proof::IndexProof>>,
    /// `SNP_VERIFY_INDEX_PROOF=1`: a proven diff is still taken with the real `git diff`.
    verify_proof: bool,
    /// Proven diffs the verify mode found non-empty.
    proof_mismatches: u32,
    /// ADR-152: set when ADR-119 proved the post-checkout state, kept while every comparison since found no change.
    restore: Option<RestoreBasis>,
}

/// What ADR-152's restore proof keeps from ADR-119's (see [`DiffOracle::restore_proof`]).
#[derive(Debug, Clone, Copy)]
struct RestoreBasis {
    /// `core.fileMode` of the proven configuration.
    file_mode: bool,
    /// When the worktree was last proven equal to the snapshot (the snapshot itself, or the last comparison that
    /// found no change).
    checked_at: i128,
}

/// ADR-152: the oracle's evidence that the files the stash checked out still hold exactly what the checkout wrote,
/// with nothing in the run's configuration or attributes converting them, so `git apply` of the stash's patch would
/// leave each of them holding exactly the bytes it had before the stash (see [`DiffOracle::restore_proof`]). Only
/// the oracle makes one; the stash guard checks every file again right before it renames ([`StashGuard::prove_restore`]).
///
/// [`StashGuard::prove_restore`]: crate::StashGuard::prove_restore
#[derive(Debug, Clone)]
pub struct RestoreProof {
    files: Vec<ProofFile>,
    /// Every input of `git diff` (index, configuration and attributes files) with its snapshot `(lstat, stat)` and,
    /// when it was racy, its content hash.
    inputs: Vec<(PathBuf, St, St, Option<[u8; 32]>)>,
    checked_at: i128,
    file_mode: bool,
}

/// One checked-out file of a [`RestoreProof`].
#[derive(Debug, Clone)]
pub(crate) struct ProofFile {
    rel: Vec<u8>,
    path: PathBuf,
    st: St,
    hash: Option<[u8; 32]>,
    index_exec: bool,
}

/// What the index-stat proof did in a run (ADR-134).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProofStats {
    /// Passes that ran (a parse that could never prove runs none).
    pub passes: u32,
    /// Passes that proved `git diff` empty.
    pub proven: u32,
    /// Proven diffs `SNP_VERIFY_INDEX_PROOF`'s real `git diff` found non-empty.
    pub mismatches: u32,
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

/// The content hash of a racy path (`None` when unreadable: compared as such). A large file is hashed on several
/// threads ([`par_file_hash`], ADR-151), with the same digest.
fn content_hash(p: &Path, kind: Kind, follow: bool) -> Option<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    if kind == Kind::Symlink && !follow {
        let target = std::fs::read_link(p).ok()?;
        h.update(&crate::journal::path_bytes(&target));
    } else {
        let f = std::fs::File::open(p).ok()?;
        if let Some(digest) = par_file_hash(&f) {
            return Some(digest);
        }
        h.update_reader(f).ok()?;
    }
    Some(*h.finalize().as_bytes())
}

/// ADR-151: the smallest size [`par_hash_len`] splits (below it one thread is as fast).
#[cfg(unix)]
const PAR_HASH_MIN: u64 = 4 << 20;
/// ADR-151: files of at least this many bytes are hashed on several threads by [`par_file_hash`]. Higher than
/// [`PAR_HASH_MIN`]: each parallel hash starts its helper threads anew, and below this size the few milliseconds it
/// saves do not pay for them (B8's 10 MiB file, hashed a few times per run, added about nine threads per run on a
/// 4-CPU runner for about 3 ms).
#[cfg(unix)]
const PAR_FILE_HASH_MIN: u64 = 32 << 20;
/// ADR-151: the smallest subtree one thread of [`par_file_hash`] hashes.
#[cfg(unix)]
const PAR_HASH_SEGMENT: u64 = 1 << 20;
/// ADR-151: the most threads [`par_file_hash`] uses.
#[cfg(unix)]
const PAR_HASH_THREADS: usize = 8;

/// ADR-151: blake3 of a file of at least [`PAR_FILE_HASH_MIN`] bytes, computed on several threads (`par_hash_len`).
/// The size is checked first, so a smaller file (the common racy case) costs one `fstat` and no thread-count query;
/// the thread count is read once per process. `None` (the caller hashes sequentially) for a smaller file, one
/// available thread, a non-Unix platform or any failure of `par_hash_len`.
fn par_file_hash(f: &std::fs::File) -> Option<[u8; 32]> {
    #[cfg(unix)]
    {
        static THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let len = f.metadata().ok()?.len();
        if len < PAR_FILE_HASH_MIN {
            return None;
        }
        let threads =
            *THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, usize::from));
        par_hash_len(f, len, threads)
    }
    #[cfg(not(unix))]
    {
        let _ = f;
        None
    }
}

/// ADR-151: blake3 of the first `len` bytes of `f` (its `fstat` size, at least [`PAR_HASH_MIN`]), computed on up to
/// `threads` threads: the input is split along blake3's own tree (`hazmat::left_subtree_len`) into subtrees of at
/// least [`PAR_HASH_SEGMENT`] bytes, each hashed with positional reads, and their chaining values are merged as blake3
/// merges them, so the digest is exactly `blake3::hash` of the bytes. Every subtree must read exactly its length and
/// the file must end at `len`, else (or for a smaller `len`, one thread or any read error) `None`, and the caller
/// hashes the file sequentially as before: the result never differs from the sequential one for a file that does not
/// change while it is read.
#[cfg(unix)]
fn par_hash_len(f: &std::fs::File, len: u64, threads: usize) -> Option<[u8; 32]> {
    use blake3::hazmat::{Mode, merge_subtrees_root};
    let threads = threads.min(PAR_HASH_THREADS);
    if len < PAR_HASH_MIN || threads < 2 {
        return None;
    }
    let target = PAR_HASH_SEGMENT.max(len.checked_div(u64::try_from(threads).ok()?)?);
    let mut segments = Vec::new();
    let root = split_subtree(0, len, target, &mut segments)?;
    let cvs: Vec<Option<blake3::hazmat::ChainingValue>> = std::thread::scope(|s| {
        // every segment's thread starts before the first join; a thread the OS refuses hashes at its join
        let mut handles = Vec::new();
        for &(off, n) in &segments {
            let h = std::thread::Builder::new()
                .name("snp-diff-hash".to_owned())
                .stack_size(256 * 1024)
                .spawn_scoped(s, move || hash_subtree(f, off, n));
            handles.push((off, n, h));
        }
        handles
            .into_iter()
            .map(|(off, n, h)| match h {
                Ok(h) => h.join().ok().flatten(),
                Err(_) => hash_subtree(f, off, n),
            })
            .collect()
    });
    // the file must end at `len`: a longer one is hashed sequentially (to its end, as before)
    let mut one = [0_u8; 1];
    if std::os::unix::fs::FileExt::read_at(f, &mut one, len).ok()? != 0 {
        return None;
    }
    let Tree::Pair(l, r) = root else {
        return None;
    };
    let (l, r) = (l.merge(&cvs)?, r.merge(&cvs)?);
    Some(*merge_subtrees_root(&l, &r, Mode::Hash).as_bytes())
}

/// The shape of a [`par_file_hash`] split: a leaf is the index of a segment.
#[cfg(unix)]
enum Tree {
    Leaf(usize),
    Pair(Box<Self>, Box<Self>),
}

#[cfg(unix)]
impl Tree {
    /// The chaining value of this subtree, from the segments' ones (`None` when one is missing).
    fn merge(
        &self,
        cvs: &[Option<blake3::hazmat::ChainingValue>],
    ) -> Option<blake3::hazmat::ChainingValue> {
        match self {
            Self::Leaf(i) => cvs.get(*i).copied().flatten(),
            Self::Pair(l, r) => Some(blake3::hazmat::merge_subtrees_non_root(
                &l.merge(cvs)?,
                &r.merge(cvs)?,
                blake3::hazmat::Mode::Hash,
            )),
        }
    }
}

/// Splits the subtree `[off, off + len)` along blake3's tree until each part is at most `target` bytes, appending the
/// parts to `segments` in order. `None` when a part would break blake3's subtree rules (never, for a split from
/// offset 0; checked rather than assumed).
#[cfg(unix)]
fn split_subtree(off: u64, len: u64, target: u64, segments: &mut Vec<(u64, u64)>) -> Option<Tree> {
    let chunk = u64::try_from(blake3::CHUNK_LEN).ok()?;
    if off.checked_rem(chunk)? != 0
        || len == 0
        || blake3::hazmat::max_subtree_len(off).is_some_and(|max| len > max)
    {
        return None;
    }
    if len <= target || len <= chunk {
        segments.push((off, len));
        return Some(Tree::Leaf(segments.len().checked_sub(1)?));
    }
    let left = blake3::hazmat::left_subtree_len(len);
    let l = split_subtree(off, left, target, segments)?;
    let r = split_subtree(
        off.checked_add(left)?,
        len.checked_sub(left)?,
        target,
        segments,
    )?;
    Some(Tree::Pair(Box::new(l), Box::new(r)))
}

/// The non-root chaining value of the subtree `[off, off + len)` of `f`, read with positional reads; `None` when the
/// file does not hold exactly those bytes or a read fails.
#[cfg(unix)]
fn hash_subtree(f: &std::fs::File, off: u64, len: u64) -> Option<blake3::hazmat::ChainingValue> {
    use blake3::hazmat::HasherExt;
    let mut h = blake3::Hasher::new();
    h.set_input_offset(off);
    let mut buf = vec![0_u8; 32 << 10];
    let mut done = 0_u64;
    while done < len {
        let want = usize::try_from(len.checked_sub(done)?)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let part = buf.get_mut(..want)?;
        let n = match std::os::unix::fs::FileExt::read_at(f, part, off.checked_add(done)?) {
            Ok(0) => return None,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        h.update(part.get(..n)?);
        done = done.checked_add(u64::try_from(n).ok()?)?;
    }
    Some(h.finalize_non_root())
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
    /// When it was taken (the racy rule's reference time).
    taken: i128,
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
    /// The system attributes file `git var GIT_ATTR_SYSTEM` named for this run (ADR-133), one more input.
    system_attributes: Option<PathBuf>,
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

/// Builds the list of inputs (and whether `attr.tree` forbids Stat mode); `system` is the system attributes file `git
/// var` named (ADR-133), watched beside the places ADR-119 looks in.
fn build_inputs(
    info: &RepoInfo,
    env: &Env,
    origins: &[PathBuf],
    dirs: &[PathBuf],
    system: Option<&Path>,
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
    let exe = snp_proc::resolve::find_executable(std::ffi::OsStr::new("git"), env, &info.root);
    let prefix = exe
        .as_ref()
        .and_then(|exe| snp_os::realpath::realpath_nonstrict(exe, &info.root).ok())
        .and_then(|exe| exe.parent().and_then(Path::parent).map(Path::to_path_buf));
    // the prefix of the `PATH` entry itself: a packager that links `<prefix>/bin/git` into a versioned tree builds
    // git with `sysconfdir=<prefix>/etc` (Homebrew and Linuxbrew: `Cellar/git/<version>` is not where its
    // system attributes file lives); ADR-119 needs every place git may read it from
    let link_prefix = exe
        .as_ref()
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
    for p in prefix.iter().chain(&link_prefix) {
        push(p.join("etc").join("gitattributes"), InputKind::Other);
    }
    if let Some(p) = system {
        push(p.to_path_buf(), InputKind::Other);
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
        taken,
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
        Self::with_system(info, env, tracked, origins, None)
    }

    /// [`Self::new`] watching the system attributes file `system` too (ADR-133).
    fn with_system(
        info: &RepoInfo,
        env: &Env,
        tracked: Arc<IndexEntries>,
        origins: Arc<[PathBuf]>,
        system: Option<PathBuf>,
    ) -> Option<Self> {
        let (paths, n_files) = tracked_paths(&info.root, &tracked)?;
        let dirs = paths.get(n_files..).unwrap_or_default();
        let inputs = build_inputs(info, env, &origins, dirs, system.as_deref())?;
        Some(Self {
            info: info.clone(),
            env: env.clone(),
            paths,
            n_files,
            tracked,
            inputs,
            origins,
            system_attributes: system,
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
    /// `core.autocrlf`, fsmonitor, ignoreStat, sparse checkout, configuration-based hook, `core.attributesFile` or
    /// `attr.tree`, as git itself lists them, not as the best-effort `gitconfig` reader reads them) and no fsmonitor
    /// test hook is in the environment; (b) no attributes input converts anything (ADR-133: every attributes input of
    /// the snapshot is missing or a regular file whose tokens set or value none of the converting attributes,
    /// `text=auto` allowed under [`text_auto_allowed`]; with no `core.attributesFile`, the default one is among
    /// them), the patch holds no `.gitattributes`, and every tracked `.gitattributes` is an unflagged regular file in
    /// the worktree ([`index_attributes_plain`]), so no filter, `text`/`eol`/`crlf`, `ident` or
    /// `working-tree-encoding` conversion applies and the checkout wrote each blob's bytes, which `git diff` reads
    /// back unconverted; (c) no `post-checkout` or `post-index-change` hook exists, the two hooks a path-limited
    /// checkout runs; (d) every directory holding a tracked path is a directory (no path is beyond a symbolic link);
    /// and (e) [`checked_out_files_plain`]. Anything else answers `false`: the caller takes the real baseline.
    ///
    /// `answered`: `git var GIT_ATTR_SYSTEM` named the system attributes file for this run (it is among the inputs,
    /// or there is none); without that answer only Linux builds are known to put it where [`build_inputs`] looks.
    fn checkout_left_no_diff(
        &self,
        checked_out: &ZList,
        config: &CheckoutConfig,
        answered: bool,
    ) -> bool {
        if !(answered || HEURISTIC_PLATFORM) {
            return false;
        }
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
        let text_auto = text_auto_allowed(config);
        let real_dirs = snap
            .paths
            .get(self.n_files..)
            .unwrap_or_default()
            .iter()
            .all(|s| s.kind == Kind::Dir);
        let files = snap.paths.get(..self.n_files).unwrap_or_default();
        let entries = || {
            self.tracked
                .paths
                .iter()
                .zip(self.tracked.modes.iter().copied())
        };
        real_dirs
            && checked_out.iter().all(|p| !is_attributes_file(p))
            && index_attributes_plain(entries(), &self.tracked.tags, files)
            && no_checkout_hooks(&self.info, config)
            && checked_out_files_plain(
                entries(),
                files,
                &checked_out.iter().collect(),
                config.file_mode,
            )
            && self.inputs.iter().zip(&snap.inputs).all(|((p, kind), st)| {
                *kind != InputKind::Other || attributes::input_plain(p, *st, text_auto)
            })
    }
}

/// Whether ADR-119's proof is attempted without `git var GIT_ATTR_SYSTEM`'s answer (git < 2.42, or no usable answer):
/// on Linux only, where the usual builds read `/etc/gitattributes` or `<prefix>/etc/gitattributes`, both among the
/// inputs. Elsewhere (Xcode's git, packagers' own `sysconfdir`) a missing input proves nothing; Windows has no Stat
/// mode.
const HEURISTIC_PLATFORM: bool = cfg!(target_os = "linux");

/// ADR-133: whether a `text=auto` token converts nothing in this run. With `core.autocrlf` off ([`CheckoutConfig::plain`])
/// and `core.eol` unset, `lf` or `native` off Windows, `output_eol(CRLF_AUTO)` is LF, so the checkout writes the blob's
/// bytes; `git diff` reads them back equal (no CRLF to convert, or a CR already in the index: `has_crlf_in_index`).
/// The matrix test must have agreed ([`attributes::TEXT_AUTO_MATRIX_PASSED`]).
fn text_auto_allowed(config: &CheckoutConfig) -> bool {
    attributes::TEXT_AUTO_MATRIX_PASSED && config.eol_lf && !cfg!(windows)
}

/// Whether a repository path names a `.gitattributes` file (in any directory, ASCII case ignored).
fn is_attributes_file(p: &[u8]) -> bool {
    p.iter()
        .rposition(|&b| b == b'/')
        .and_then(|slash| p.get(slash.saturating_add(1)..))
        .unwrap_or(p)
        .eq_ignore_ascii_case(b".gitattributes")
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

/// ADR-119 (b) and ADR-133 (b), the index half, over the index entries (`(path, mode)` in index order), their query F
/// `-v` tags and the snapshot's tracked-path slots (`files`, in [`tracked_paths`] order): no path has a second stage
/// (`git diff` prints an unmerged path whatever the worktree holds), and every entry named `.gitattributes` (ASCII
/// case ignored) is spelled exactly so, a regular entry (`100644` or `100755`) tagged `H` (neither assume-unchanged
/// nor skip-worktree), and its slot is a regular file. git reads a tracked `.gitattributes` from the index when its
/// worktree file is missing (first when a checkout writes a file, as a fall-back when `git diff` reads one), and an
/// assume-unchanged or skip-worktree one may differ from its worktree file arbitrarily; an unflagged one present in
/// the worktree, outside the patch and clean in `diff-index`, carries the same tokens as its index blob, which the
/// scan of the worktree file then covers. Without tags (an empty `tags`) any tracked `.gitattributes` refuses.
fn index_attributes_plain<'a>(
    entries: impl Iterator<Item = (&'a [u8], u32)>,
    tags: &[u8],
    files: &[St],
) -> bool {
    let mut last: Option<&[u8]> = None;
    let mut slot = 0_usize;
    for (i, (p, mode)) in entries.enumerate() {
        if last == Some(p) {
            return false;
        }
        last = Some(p);
        let attributes = is_attributes_file(p);
        if mode == GITLINK {
            if attributes {
                return false;
            }
            continue;
        }
        if attributes {
            let name = p
                .iter()
                .rposition(|&b| b == b'/')
                .and_then(|slash| p.get(slash.saturating_add(1)..))
                .unwrap_or(p);
            let plain = name == b".gitattributes"
                && tags.get(i) == Some(&b'H')
                && matches!(mode, 0o100_644 | 0o100_755)
                && files.get(slot).is_some_and(|st| st.kind == Kind::File);
            if !plain {
                return false;
            }
        }
        slot = slot.saturating_add(1);
    }
    true
}

/// ADR-119 (e), over the index entries (`(path, mode)` in index order) and the snapshot's tracked-path slots
/// (`files`, in [`tracked_paths`] order): every checked-out path (`want`) is a tracked entry at stage 0 (no gitlink,
/// no second stage) of mode `100644` or `100755`, its slot is a regular file whose owner-executable bit matches the
/// mode when `core.fileMode` holds (git compares exactly that bit, so a umask without it would show a mode change),
/// and no other tracked path reaches its inode (a case or Unicode normalization twin on an insensitive filesystem,
/// or a hard link: the checkout of one would change what `git diff` reads for the other). An empty list proves
/// nothing.
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
            proof: None,
            verify_proof: false,
            proof_mismatches: 0,
            restore: None,
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

    /// ADR-134: in Git mode, lets every `git diff` the oracle would take while the last fingerprint is empty (or not
    /// taken yet) be decided by the index-stat proof first: when it holds, the fingerprint is
    /// [`DiffFingerprint::EMPTY`] and nothing is spawned; on any doubt the real `git diff` runs, as before. `config`
    /// is query G's listing (the proof's configuration gate, the config files it watches). Nothing changes in Stat
    /// mode or when a gate fails (see `IndexProof::new`); the answer says whether the proof is armed.
    ///
    /// `verify` (`SNP_VERIFY_INDEX_PROOF=1`): a proven diff is still taken with the real `git diff`, which is used,
    /// and a non-empty one is counted ([`Self::proof_stats`]).
    pub fn enable_index_proof(
        &mut self,
        info: &RepoInfo,
        env: &Env,
        pool: Arc<JobPool>,
        config: &ConfigListing,
        verify: bool,
    ) -> bool {
        if self.mode != DiffMode::Git {
            return false;
        }
        self.proof = index_proof::IndexProof::new(info, env, pool, config).map(Box::new);
        self.verify_proof = verify;
        self.proof.is_some()
    }

    /// What the index-stat proof did so far (`None` when it is not armed).
    pub fn proof_stats(&self) -> Option<ProofStats> {
        self.proof.as_deref().map(|p| ProofStats {
            passes: p.passes,
            proven: p.proven,
            mismatches: self.proof_mismatches,
        })
    }

    /// The fingerprint of `git diff` now: [`DiffFingerprint::EMPTY`] without a spawn when the last fingerprint is
    /// empty or not taken yet and the index-stat proof holds (ADR-134), else the real `git diff`.
    fn fingerprint_now(&mut self) -> Result<DiffFingerprint, Failure> {
        let candidate = self.last.is_none_or(|l| l == DiffFingerprint::EMPTY);
        if candidate
            && let Some(p) = self.proof.as_deref_mut()
            && p.holds(&self.git)
        {
            if !self.verify_proof {
                return Ok(DiffFingerprint::EMPTY);
            }
            let real = self.git.diff_fingerprint()?;
            if real != DiffFingerprint::EMPTY {
                self.proof_mismatches = self.proof_mismatches.saturating_add(1);
            }
            return Ok(real);
        }
        self.git.diff_fingerprint()
    }

    /// ADR-119: right after the stash's checkout of `checked_out` (and before any hook), take the baseline as known
    /// empty when the Stat-mode snapshot proves `git diff` prints nothing (see `StatState::checkout_left_no_diff`;
    /// `config` is query G's [`CheckoutConfig`]). Only on a lazy baseline (none given to [`Self::new`]) in Stat mode;
    /// the snapshot is taken here, so a later [`Self::before_hook`] keeps it. When the proof fails nothing else
    /// changes: the baseline stays lazy and [`Self::before_hook`] runs the real `git diff` after the snapshot, as
    /// before.
    ///
    /// `system` is `git var GIT_ATTR_SYSTEM`'s answer for this run (ADR-133; `None`: not asked or no usable answer).
    /// A named file joins the inputs before the snapshot, and an answer lets the proof run off Linux.
    ///
    /// `verify` (`SNP_VERIFY_BASELINE=1`): a proven baseline is still taken with the real `git diff`, which becomes
    /// the baseline; [`Elision::Mismatch`] when it was not empty. A failing `git diff` leaves the baseline lazy.
    pub fn elide_checkout_baseline(
        &mut self,
        checked_out: &ZList,
        config: &CheckoutConfig,
        system: Option<&SystemAttributes>,
        verify: bool,
    ) -> Elision {
        if self.last.is_some() {
            return Elision::NotProven;
        }
        let Some(s) = self.stat.as_deref_mut() else {
            return Elision::NotProven;
        };
        let answered = match system {
            None => false,
            Some(SystemAttributes::File(p)) => {
                // only before the snapshot, so the file is in it; an input list that already has it keeps it
                let known = s.inputs.iter().any(|(q, _)| q == p);
                if !known && s.snapshot.is_none() {
                    s.inputs.push((p.clone(), InputKind::Other));
                    s.system_attributes = Some(p.clone());
                }
                known || s.system_attributes.as_ref() == Some(p)
            }
            Some(_) => true,
        };
        if s.snapshot.is_none() {
            s.take_snapshot();
        }
        if !s.checkout_left_no_diff(checked_out, config, answered) {
            return Elision::NotProven;
        }
        let basis = s.snapshot.as_ref().map(|snap| RestoreBasis {
            file_mode: config.file_mode,
            checked_at: snap.taken,
        });
        if !verify {
            self.last = Some(DiffFingerprint::EMPTY);
            self.restore = basis;
            return Elision::Elided;
        }
        let Ok(real) = self.git.diff_fingerprint() else {
            return Elision::NotProven;
        };
        self.last = Some(real);
        if real == DiffFingerprint::EMPTY {
            self.restore = basis;
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

    /// Whether the fingerprint the next comparison starts from is known: the baseline was supplied (ADR-078(3),
    /// ADR-106, ADR-119) or taken, and no native fixer made it stale since. ADR-132 defers the baseline only while
    /// it is not.
    pub fn baseline_known(&self) -> bool {
        self.last.is_some()
    }

    /// ADR-132: the baseline a gated hook's first writer took (the real `git diff` before its first write, so of
    /// the pre-hook tree) becomes the fingerprint the hook is compared with. The Stat-mode snapshot stays as it is
    /// (none: a gated run never called [`Self::before_hook`]); [`Self::after_native`] takes one before its real
    /// `git diff`.
    pub fn adopt_baseline(&mut self, baseline: DiffFingerprint) {
        self.last = Some(baseline);
        self.restore = None;
    }

    /// Falls back to Git mode for the rest of the run.
    fn fall_back_to_git_mode(&mut self) {
        self.mode = DiffMode::Git;
        self.stat = None;
        self.restore = None;
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
            self.restore = None;
        }
        if self.last.is_none() {
            self.last = Some(self.fingerprint_now()?);
        }
        Ok(())
    }

    /// After it: `Unchanged` without spawning when provably identical, else one real `git diff` and digest compare.
    pub fn after_hook(&mut self) -> Result<Change, Failure> {
        if self.stat.is_none() {
            self.restore = None;
        }
        self.invalidate_trust();
        if let Some(s) = self.stat.as_deref_mut() {
            let (delta, stats, taken) = s.compare();
            if !delta.any && self.last.is_some() {
                // ADR-152: the worktree is still the snapshot's, proven at `taken`
                if let Some(r) = self.restore.as_mut() {
                    r.checked_at = taken;
                }
                return Ok(Change::Unchanged);
            }
            self.restore = None;
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
        let system = s.system_attributes.clone();
        let rebuilt = if delta.index {
            StatState::with_system(&s.info, &s.env, Arc::new(tracked), origins, system)
        } else {
            let dirs = s.paths.get(s.n_files..).unwrap_or_default();
            build_inputs(&s.info, &s.env, &origins, dirs, system.as_deref()).map(|inputs| {
                StatState {
                    info: s.info.clone(),
                    env: s.env.clone(),
                    paths: s.paths.clone(),
                    n_files: s.n_files,
                    tracked: Arc::clone(&s.tracked),
                    inputs,
                    origins,
                    system_attributes: system,
                    snapshot: None,
                }
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
        let now = self.fingerprint_now()?;
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
        // ADR-152: a native fixer's report is not a comparison of the worktree with the snapshot
        self.restore = None;
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
            self.restore = None;
        }
        let d = self.fingerprint_now()?;
        self.last = Some(d);
        Ok(d.non_empty)
    }

    /// ADR-152: the evidence the stash's restore by rename needs for `checked_out` (the stash's checkout list,
    /// [`crate::StashGuard::checked_out`]), or `None`.
    ///
    /// It exists only when (a) ADR-119 proved the post-checkout state ([`Self::elide_checkout_baseline`] answered
    /// `Elided` or `Verified`): the checkout wrote each checked-out file's blob bytes and nothing in the configuration
    /// or the attributes converts any file; (b) every comparison since found no change: the stat snapshot is still
    /// the post-checkout one, for every tracked path (racy ones compared by content) and every input of `git diff`
    /// (index, configuration and attributes files), and no native fixer's report or adopted baseline replaced a
    /// comparison; (c) the last fingerprint is the empty diff; and (d) every checked-out path is a regular file of
    /// mode `100644` or `100755` in the snapshot, hashed when it was racy. The proof carries each file's snapshot
    /// stat, hash and index mode, `core.fileMode`, and when the worktree was last proven equal to the snapshot.
    pub fn restore_proof(&self, checked_out: &ZList) -> Option<RestoreProof> {
        let basis = self.restore?;
        if self.mode != DiffMode::Stat || self.last != Some(DiffFingerprint::EMPTY) {
            return None;
        }
        let s = self.stat.as_deref()?;
        let snap = s.snapshot.as_ref()?;
        let want: HashSet<&[u8]> = checked_out.iter().collect();
        if want.is_empty() || want.len() != checked_out.len() {
            return None;
        }
        let mut files: Vec<ProofFile> = Vec::new();
        files.try_reserve(want.len()).ok()?;
        let mut slot = 0_usize;
        let mut last: Option<&[u8]> = None;
        for (p, mode) in s.tracked.paths.iter().zip(s.tracked.modes.iter().copied()) {
            // the slot rule of `tracked_paths`
            if mode == GITLINK || last == Some(p) {
                if want.contains(p) {
                    return None;
                }
                continue;
            }
            last = Some(p);
            if want.contains(p) {
                let st = *snap.paths.get(slot)?;
                let path = s.paths.get(slot)?.clone();
                let hash = match snap.racy_paths.binary_search_by_key(&slot, |(i, _)| *i) {
                    Ok(k) => Some(snap.racy_paths.get(k)?.1?),
                    Err(_) => None,
                };
                if st.kind != Kind::File || !matches!(mode, 0o100_644 | 0o100_755) {
                    return None;
                }
                files.push(ProofFile {
                    rel: p.to_vec(),
                    path,
                    st,
                    hash,
                    index_exec: mode == 0o100_755,
                });
            }
            slot = slot.saturating_add(1);
        }
        if files.len() != want.len() {
            return None;
        }
        let mut inputs: Vec<(PathBuf, St, St, Option<[u8; 32]>)> = Vec::new();
        inputs.try_reserve(s.inputs.len()).ok()?;
        for (i, ((p, _), (l, st))) in s.inputs.iter().zip(&snap.inputs).enumerate() {
            let hash = match snap.racy_inputs.binary_search_by_key(&i, |(j, _)| *j) {
                Ok(k) => Some(snap.racy_inputs.get(k)?.1?),
                Err(_) => None,
            };
            inputs.push((p.clone(), *l, *st, hash));
        }
        Some(RestoreProof {
            files,
            inputs,
            checked_at: basis.checked_at,
            file_mode: basis.file_mode,
        })
    }
}

impl RestoreProof {
    /// The proof's entry for the repository path `rel`.
    pub(crate) fn file(&self, rel: &[u8]) -> Option<&ProofFile> {
        self.files.iter().find(|f| f.rel == rel)
    }

    /// `core.fileMode` of the proven configuration.
    #[cfg_attr(
        not(unix),
        expect(
            dead_code,
            reason = "ADR-152 renames on Unix builds only (Linux at run time)"
        )
    )]
    pub(crate) const fn file_mode(&self) -> bool {
        self.file_mode
    }

    /// The final check of the inputs (ADR-152): every input of `git diff` (index, configuration and attributes files)
    /// still has the snapshot's `lstat` and `stat`, and the content hash of a racy one is unchanged, so what `git apply`
    /// would read (the attributes and the configuration that decide its conversions) is still what ADR-119 proved
    /// converts nothing.
    pub(crate) fn inputs_unchanged(&self) -> bool {
        self.inputs.iter().all(|(p, l, st, hash)| {
            let (now_l, now_st) = St::lstat_and_stat(p);
            now_l == *l
                && now_st == *st
                && hash.is_none_or(|h| content_hash(p, now_st.kind, true) == Some(h))
        })
    }

    /// The final check of `f` (ADR-152): its `lstat` is exactly the snapshot's, and when that stat is racy against
    /// the last time the worktree was proven equal to the snapshot (a same-size write within the timestamp
    /// granularity would not show), its content hashes to the snapshot's hash. Returns the `lstat` result.
    #[cfg_attr(
        not(unix),
        expect(
            dead_code,
            reason = "ADR-152 renames on Unix builds only (Linux at run time)"
        )
    )]
    pub(crate) fn content_unchanged(&self, f: &ProofFile) -> Option<std::fs::Metadata> {
        let md = std::fs::symlink_metadata(&f.path).ok()?;
        if St::from_metadata(&md) != f.st {
            return None;
        }
        if f.st.racy(self.checked_at) {
            let now = content_hash(&f.path, Kind::File, false)?;
            if Some(now) != f.hash {
                return None;
            }
        }
        Some(md)
    }
}

impl ProofFile {
    /// Whether its `lstat` is still exactly the snapshot's (the stat check right before its rename).
    pub(crate) fn stat_unchanged(&self) -> bool {
        std::fs::symlink_metadata(&self.path).is_ok_and(|md| St::from_metadata(&md) == self.st)
    }

    /// The file's path in the worktree.
    #[cfg_attr(
        not(unix),
        expect(
            dead_code,
            reason = "ADR-152 renames on Unix builds only (Linux at run time)"
        )
    )]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether its index entry is `100755`.
    #[cfg_attr(
        not(unix),
        expect(
            dead_code,
            reason = "ADR-152 renames on Unix builds only (Linux at run time)"
        )
    )]
    pub(crate) const fn index_exec(&self) -> bool {
        self.index_exec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-151: the parallel hash is blake3's own digest for every size around the threshold and the segment and
    /// chunk boundaries, for every thread count; below the threshold or with one thread it declines (the caller
    /// hashes sequentially).
    #[cfg(unix)]
    #[test]
    fn par_file_hash_is_blake3_of_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let mib = 1_usize << 20;
        for len in [
            4 * mib,
            4 * mib + 1,
            4 * mib + 1023,
            5 * mib - 1,
            6 * mib + 1024 * 3 + 17,
            10 * mib + 12_345,
            16 * mib,
        ] {
            let bytes: Vec<u8> = (0..len)
                .map(|i| u8::try_from((i * 31 + i / 1021) % 253).unwrap())
                .collect();
            std::fs::write(&p, &bytes).unwrap();
            let want = *blake3::hash(&bytes).as_bytes();
            let f = std::fs::File::open(&p).unwrap();
            for threads in [2, 3, 4, 7, 8, 64] {
                assert_eq!(
                    par_hash_len(&f, u64::try_from(len).unwrap(), threads),
                    Some(want),
                    "len {len} threads {threads}"
                );
            }
            assert_eq!(par_hash_len(&f, u64::try_from(len).unwrap(), 1), None);
            assert_eq!(content_hash(&p, Kind::File, false), Some(want));
        }
        std::fs::write(&p, vec![7_u8; 4 * mib - 1]).unwrap();
        let f = std::fs::File::open(&p).unwrap();
        assert_eq!(par_file_hash(&f), None);
        assert_eq!(par_hash_len(&f, 4 * (1 << 20) - 1, 4), None);
        // the split never yields a part that breaks blake3's subtree rules, and covers the input exactly
        for len in [4_u64 << 20, (10_u64 << 20) + 1, 123_456_789] {
            for target in [1 << 20, 3 << 20, 1 << 22] {
                let mut segs = Vec::new();
                assert!(split_subtree(0, len, target, &mut segs).is_some());
                let mut at = 0;
                for (off, n) in segs {
                    assert_eq!(off, at);
                    assert!(n <= target.max(1024) && n > 0);
                    at += n;
                }
                assert_eq!(at, len);
            }
        }
        // a misaligned or oversized part is refused, never a panic
        assert!(split_subtree(1, 2048, 1024, &mut Vec::new()).is_none());
        assert!(split_subtree(1024, 4096, 1 << 20, &mut Vec::new()).is_none());
    }

    /// ADR-151 fallbacks (K5): a file longer than the size the parallel hash was given (it grew after `fstat`) is
    /// refused, never hashed to the old length, and the sequential hash covers the whole file; a size larger than the
    /// file (a subtree read hits the end early) is refused too.
    #[cfg(unix)]
    #[test]
    fn par_hash_len_refuses_a_file_that_grew_or_shrank() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let mib = 1_u64 << 20;
        let bytes: Vec<u8> = (0..(6 * mib + 100))
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        std::fs::write(&p, &bytes).unwrap();
        let f = std::fs::File::open(&p).unwrap();
        let real = u64::try_from(bytes.len()).unwrap();
        // longer than the stat: the trailing-byte probe declines, for an aligned and an unaligned stale size
        for stale in [5 * mib, real - 1, 4 * mib] {
            assert_eq!(par_hash_len(&f, stale, 4), None, "stale {stale}");
        }
        assert_eq!(
            content_hash(&p, Kind::File, false),
            Some(*blake3::hash(&bytes).as_bytes())
        );
        // shorter than the stat: the last subtree reads EOF before its length
        for past in [real + 1, real + 1024, 8 * mib] {
            assert_eq!(par_hash_len(&f, past, 4), None, "past {past}");
        }
        // and the exact size still matches blake3
        assert_eq!(
            par_hash_len(&f, real, 4),
            Some(*blake3::hash(&bytes).as_bytes())
        );
    }

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

    /// [`index_attributes_plain`] over `(path, mode)` entries with their tags, every slot a regular file.
    fn attrs_ok(entries: &[(&str, u32, u8)]) -> bool {
        let tags: Vec<u8> = entries.iter().map(|(_, _, t)| *t).collect();
        let files: Vec<St> = (1..=64).map(|i| file(i, 0o644)).collect();
        index_attributes_plain(
            entries.iter().map(|(p, m, _)| (p.as_bytes(), *m)),
            &tags,
            &files,
        )
    }

    /// ADR-119 (b) and ADR-133 (b): a tracked `.gitattributes` passes only spelled exactly, as a regular entry tagged
    /// `H` whose slot is a regular file; an unmerged path refuses.
    #[test]
    fn tracked_attributes_must_be_unflagged_regular_files() {
        const F: u32 = 0o100_644;
        assert!(attrs_ok(&[]));
        assert!(attrs_ok(&[
            ("a", F, b'H'),
            ("b/c", F, b'h'),
            ("gitattributes", F, b'S'),
            ("x.gitattributes", F, b'H'),
            (".gitattributes.d/f", F, b'H'),
        ]));
        assert!(attrs_ok(&[(".gitattributes", F, b'H')]));
        assert!(attrs_ok(&[
            ("a", F, b'H'),
            ("d/e/.gitattributes", 0o100_755, b'H')
        ]));
        for tag in *b"hSsMm" {
            assert!(
                !attrs_ok(&[(".gitattributes", F, tag)]),
                "{}",
                char::from(tag)
            );
        }
        assert!(
            !attrs_ok(&[("d/.GitAttributes", F, b'H')]),
            "another spelling"
        );
        assert!(
            !attrs_ok(&[(".gitattributes", 0o120_000, b'H')]),
            "a symlink entry"
        );
        assert!(!attrs_ok(&[(".gitattributes", GITLINK, b'H')]), "a gitlink");
        assert!(!attrs_ok(&[
            ("a", F, b'H'),
            ("m", F, b'M'),
            ("m", F, b'M'),
            ("z", F, b'H')
        ]));
        assert!(
            attrs_ok(&[("a/", F, b'H'), ("a", F, b'H')]),
            "distinct paths"
        );
        // no tags: any tracked `.gitattributes` refuses, other paths pass
        let entries = [("a", F), (".gitattributes", F)];
        let files = [file(1, 0o644), file(2, 0o644)];
        let it = || entries.iter().map(|(p, m)| (p.as_bytes(), *m));
        assert!(!index_attributes_plain(it(), &[], &files));
        assert!(index_attributes_plain(it().take(1), &[], &files));
        // the slot must be a regular file in the worktree (a gitlink takes no slot)
        let it = || {
            [("g", GITLINK), (".gitattributes", F)]
                .into_iter()
                .map(|(p, m)| (p.as_bytes(), m))
        };
        assert!(index_attributes_plain(it(), b"HH", &[file(1, 0o644)]));
        for kind in [Kind::Missing, Kind::Symlink, Kind::Dir, Kind::Error(13)] {
            let st = St {
                kind,
                ..file(1, 0o644)
            };
            assert!(!index_attributes_plain(it(), b"HH", &[st]), "{kind:?}");
        }
        assert!(!index_attributes_plain(it(), b"HH", &[]), "no slot");
    }

    /// ADR-133 (b): a `.gitattributes` in the patch (any directory, any ASCII case) is recognised.
    #[test]
    fn attributes_files_are_named_in_any_case() {
        for yes in [".gitattributes", "d/.gitattributes", "a/b/.GITATTRIBUTES"] {
            assert!(is_attributes_file(yes.as_bytes()), "{yes}");
        }
        for no in [
            "gitattributes",
            "x.gitattributes",
            ".gitattributes/f",
            "a/.gitattributes~",
        ] {
            assert!(!is_attributes_file(no.as_bytes()), "{no}");
        }
    }

    /// ADR-133: `text=auto` converts nothing only with `core.eol` unset, `lf` or `native`, off Windows.
    #[test]
    fn text_auto_needs_an_lf_eol() {
        let plain = CheckoutConfig {
            plain: true,
            eol_lf: true,
            ..CheckoutConfig::default()
        };
        assert_eq!(text_auto_allowed(&plain), !cfg!(windows));
        let crlf = CheckoutConfig {
            eol_lf: false,
            ..plain
        };
        assert!(!text_auto_allowed(&crlf));
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
