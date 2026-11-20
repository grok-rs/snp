//! The index-stat proof (ADR-134): `git diff --no-ext-diff --no-textconv --ignore-submodules` prints nothing, decided
//! in-process from the index's own cached stat data and one `lstat` per entry and per leading directory.
//!
//! git's `run_diff_files` reads a worktree file's content only for an entry whose stat data differs (or is racily
//! clean); for every other entry it prints nothing and writes nothing. The proof holds when, for the index git would
//! read (`RepoInfo::index_file`, which honours `GIT_INDEX_FILE`), with its stamp unchanged from the parse to the end
//! of the pass:
//!
//! * the reader parsed it exactly ([`IndexFile`]: v2/v3/v4, no `link`/`sdir`/`FSMN`/unknown lowercase extension);
//! * every entry is at stage 0 and none is intent-to-add; assume-unchanged, skip-worktree entries and gitlinks are
//!   skipped (git never compares them under `--ignore-submodules`);
//! * every other entry is `100644`, `100755` or `120000`, and its `lstat` matches strictly, stricter than any git
//!   build or setting: the kind and the owner-executable bit, mtime and ctime seconds and nanoseconds, inode, uid and
//!   gid truncated to 32 bits as git stores them, and the size as git stores it (a size that truncates to 0 but is not
//!   0 refuses, so git versions with and without `munge_st_size` agree); a stored size of 0 only for the empty blob
//!   over an empty file (git's racily-smudged rule); `st_dev` is not compared (git's default build does not, and
//!   btrfs and device-mapper numbers change across reboots);
//! * the entry is not racy: its mtime second is before the index file's (racily clean under neither the
//!   nanosecond nor the second build);
//! * every leading directory of a checked entry is a directory by `lstat` (git reports a path beyond a symbolic link
//!   as deleted);
//! * the configuration keeps fsmonitor, ignoreStat and sparse checkout off and names the object format
//!   ([`CheckoutConfig::index_proof`], [`CheckoutConfig::object_id_len`]), no fsmonitor test variable is set, and
//!   every config file is as it was when that was read: a changed stamp asks git again;
//! * the work tree and the git directory are on `stat_oracle_safe` filesystems (Unix only).
//!
//! Any doubt answers `false`, and the caller runs the real `git diff`.
//!
//! Owner: WP-23.

use std::collections::HashSet;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use snp_core::env::Env;
use snp_git::index_file::{IndexEntry, IndexFile};
use snp_git::{CheckoutConfig, ConfigListing, Git, RepoInfo};
use snp_os::rootstat::{RootDir, StatData};
use snp_proc::pool::JobPool;

use super::{InputKind, Kind, St};

/// Below this many items the pass runs on the calling thread.
const PARALLEL_MIN: usize = 2_048;
/// The smallest chunk a pool thread takes.
const CHUNK_MIN: usize = 512;
/// The file-type bits of `st_mode` and the regular-file and symlink types.
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;
const S_IFDIR: u32 = 0o040_000;

/// One config input and its stamp: `(lstat, stat)` and, when racy at stamping time, its content hash.
#[derive(Debug)]
struct Watched {
    path: PathBuf,
    st: (St, St),
    /// Racy at stamping time: the content is compared too.
    racy: bool,
    /// The content hash when racy (`None` also for an unreadable file, compared as such).
    hash: Option<[u8; 32]>,
}

/// What one parse of the index allows.
#[derive(Debug)]
struct Plan {
    /// The checked entries (stage 0, not skipped).
    index: IndexFile,
    checked: Vec<IndexEntry>,
    /// Every leading directory of a checked entry, each followed by a NUL.
    dirs: Vec<u8>,
    /// `(start, len)` of each directory in `dirs`.
    dir_spans: Vec<(usize, usize)>,
}

/// The parse of one index image, keyed by its stamp.
#[derive(Debug)]
struct Parsed {
    stamp: St,
    /// `None`: this index can never prove anything (refused, unmerged, intent-to-add, racy, smudged, odd mode).
    plan: Option<Plan>,
}

/// The proof's state for one run.
#[derive(Debug)]
pub(super) struct IndexProof {
    info: RepoInfo,
    env: Env,
    pool: Arc<JobPool>,
    hash_len: usize,
    watched: Vec<Watched>,
    /// The configuration read last allows the proof (checked again whenever a watched config file changes).
    gate_ok: bool,
    parsed: Option<Parsed>,
    /// The configuration could not be read again: never again this run.
    disabled: bool,
    /// Passes that ran, and passes that proved (tests and the verify mode read them).
    pub(super) passes: u32,
    pub(super) proven: u32,
}

impl IndexProof {
    /// The proof for this repository, or `None` when a gate fails before any work: not Unix, `SNP_DIFF_MODE=git`, a
    /// filesystem outside the allow-list, an fsmonitor test variable, or a configuration that does not allow it.
    pub(super) fn new(
        info: &RepoInfo,
        env: &Env,
        pool: Arc<JobPool>,
        config: &ConfigListing,
    ) -> Option<Self> {
        if !cfg!(unix) || env.get("SNP_DIFF_MODE").is_some_and(|v| v == "git") {
            return None;
        }
        if ["GIT_TEST_FSMONITOR", "GIT_FSMONITOR_TEST"]
            .into_iter()
            .any(|k| env.get_nonempty(k).is_some())
        {
            return None;
        }
        let safe = |p: &std::path::Path| snp_os::fsinfo::stat_oracle_safe(p).unwrap_or(false);
        if !safe(&info.root) || !safe(&info.git_dir) {
            return None;
        }
        let hash_len = gate(&config.checkout)?;
        let watched = watch(info, env, &config.files)?;
        Some(Self {
            info: info.clone(),
            env: env.clone(),
            pool,
            hash_len,
            watched,
            gate_ok: true,
            parsed: None,
            disabled: false,
            passes: 0,
            proven: 0,
        })
    }

    /// Whether `git diff` prints nothing right now (see the module documentation); `git` asks the configuration again
    /// when a config file changed.
    pub(super) fn holds(&mut self, git: &Git) -> bool {
        if self.disabled {
            return false;
        }
        if !self.config_unchanged() && !self.recheck_config(git) {
            self.disabled = true;
            return false;
        }
        if !self.gate_ok {
            return false;
        }
        let stamp = St::stat(&self.info.index_file);
        if stamp.kind != Kind::File {
            return false;
        }
        if self.parsed.as_ref().is_none_or(|p| p.stamp != stamp) {
            let plan = plan(&self.info, self.hash_len, stamp);
            // the bytes read are the stamped file's only if the stamp still holds after the read
            if St::stat(&self.info.index_file) != stamp {
                self.parsed = None;
                return false;
            }
            self.parsed = Some(Parsed { stamp, plan });
        }
        let Some(plan) = self.parsed.as_ref().and_then(|p| p.plan.as_ref()) else {
            return false;
        };
        self.passes = self.passes.saturating_add(1);
        let held =
            pass(&self.info.root, plan, &self.pool) && St::stat(&self.info.index_file) == stamp;
        if held {
            self.proven = self.proven.saturating_add(1);
        }
        held
    }

    /// Every watched config input still has its stamp (and, when it was racy, its content).
    fn config_unchanged(&self) -> bool {
        self.watched.iter().all(|w| {
            St::lstat_and_stat(&w.path) == w.st
                && (!w.racy || super::content_hash(&w.path, w.st.1.kind, true) == w.hash)
        })
    }

    /// Asks git for the configuration again (one `git config --list --show-origin -z`) and re-stamps the inputs, which
    /// were stamped before the listing so a change racing with it is seen next time; `false` when the listing fails or
    /// the inputs cannot be listed. Whether the configuration allows the proof is kept in `gate_ok`.
    fn recheck_config(&mut self, git: &Git) -> bool {
        let Some(watched) = watch(&self.info, &self.env, &[]) else {
            return false;
        };
        let Ok(listing) = git.config_listing() else {
            return false;
        };
        let Some(watched) = watch_more(watched, &self.info, &self.env, &listing.files) else {
            return false;
        };
        self.watched = watched;
        match gate(&listing.checkout) {
            Some(hash_len) => {
                if hash_len != self.hash_len {
                    self.parsed = None;
                }
                self.hash_len = hash_len;
                self.gate_ok = true;
            }
            None => self.gate_ok = false,
        }
        true
    }
}

/// The object id length when the configuration allows the proof.
fn gate(config: &CheckoutConfig) -> Option<usize> {
    config.object_id_len.filter(|_| config.index_proof)
}

/// Stamps every config input (the files git listed, every place it reads one from, include targets, `HEAD` for
/// `onbranch:` includes), content-hashing the racy ones; `None` when the inputs cannot be listed (the `DiffOracle`'s
/// own rule: an unreadable `GIT_CONFIG_PARAMETERS`, too many includes, `attr.tree`).
fn watch(info: &RepoInfo, env: &Env, origins: &[PathBuf]) -> Option<Vec<Watched>> {
    let inputs = super::build_inputs(info, env, origins, &[], None)?;
    let taken = super::now_ns();
    let mut out: Vec<Watched> = Vec::new();
    for (path, kind) in inputs {
        if kind != InputKind::Config {
            continue;
        }
        let st = St::lstat_and_stat(&path);
        let racy = st.0.racy(taken) || st.1.racy(taken);
        let hash = racy
            .then(|| super::content_hash(&path, st.1.kind, true))
            .flatten();
        out.try_reserve(1).ok()?;
        out.push(Watched {
            path,
            st,
            racy,
            hash,
        });
    }
    Some(out)
}

/// `watched` plus the inputs `origins` adds (files git listed that the default places and include targets missed),
/// stamped now.
fn watch_more(
    mut watched: Vec<Watched>,
    info: &RepoInfo,
    env: &Env,
    origins: &[PathBuf],
) -> Option<Vec<Watched>> {
    for w in watch(info, env, origins)? {
        if !watched.iter().any(|known| known.path == w.path) {
            watched.try_reserve(1).ok()?;
            watched.push(w);
        }
    }
    Some(watched)
}

/// Parses the index and keeps what the pass checks; `None` when this index can never prove (see [`Parsed::plan`]).
fn plan(info: &RepoInfo, hash_len: usize, stamp: St) -> Option<Plan> {
    let index = IndexFile::read(&info.index_file, hash_len).ok()?;
    // the index file's mtime second as git keeps it (`istate->timestamp.sec`, an unsigned int)
    let index_sec = trunc32(i64::try_from(stamp.mtime.checked_div_euclid(1_000_000_000)?).ok()?);
    let mut checked: Vec<IndexEntry> = Vec::new();
    checked.try_reserve_exact(index.entries.len()).ok()?;
    for e in &index.entries {
        if e.stage() != 0 || e.intent_to_add() {
            return None;
        }
        if e.assume_valid() || e.skip_worktree() || e.mode == 0o160_000 {
            continue;
        }
        let smudged = e.size == 0 && !e.empty_blob;
        if !matches!(e.mode, 0o100_644 | 0o100_755 | 0o120_000) || smudged || e.mtime.0 >= index_sec
        {
            return None;
        }
        checked.push(*e);
    }
    let (dirs, dir_spans) = leading_dirs(&index, &checked)?;
    Some(Plan {
        index,
        checked,
        dirs,
        dir_spans,
    })
}

/// Directory names, each followed by a NUL, and the `(start, len)` of each.
type Dirs = (Vec<u8>, Vec<(usize, usize)>);

/// Every distinct leading directory of the checked entries (`a/b/c` gives `a` and `a/b`), each followed by a NUL.
fn leading_dirs(index: &IndexFile, checked: &[IndexEntry]) -> Option<Dirs> {
    let mut seen: HashSet<&[u8]> = HashSet::new();
    let mut dirs: Vec<u8> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for e in checked {
        let name = index.name(e);
        let mut end = 0_usize;
        while let Some(off) = name.get(end..).and_then(memchr_slash) {
            end = end.saturating_add(off);
            let dir = name.get(..end)?;
            if seen.insert(dir) {
                dirs.try_reserve(dir.len().saturating_add(1)).ok()?;
                spans.try_reserve(1).ok()?;
                spans.push((dirs.len(), dir.len()));
                dirs.extend_from_slice(dir);
                dirs.push(0);
            }
            end = end.saturating_add(1);
        }
    }
    Some((dirs, spans))
}

fn memchr_slash(b: &[u8]) -> Option<usize> {
    b.iter().position(|&c| c == b'/')
}

/// The low 32 bits of a stat field, as git stores it (`(unsigned int)st->st_…`).
fn trunc32(v: i64) -> u32 {
    u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0)
}

/// Whether the `lstat` of a checked entry matches it strictly (see the module documentation).
fn entry_matches(e: &IndexEntry, st: &StatData) -> bool {
    let kind = st.mode & S_IFMT;
    let kind_ok = match e.mode {
        0o100_644 => kind == S_IFREG && st.mode & 0o100 == 0,
        0o100_755 => kind == S_IFREG && st.mode & 0o100 != 0,
        0o120_000 => kind == S_IFLNK,
        _ => false,
    };
    let time = |t: (i64, i64)| (trunc32(t.0), u32::try_from(t.1).unwrap_or(u32::MAX));
    let low = st.size & 0xFFFF_FFFF;
    let size_ok = if st.size == 0 {
        e.size == 0 && e.empty_blob
    } else {
        low != 0 && u64::from(e.size) == low
    };
    kind_ok
        && size_ok
        && e.mtime == time(st.mtime)
        && e.ctime == time(st.ctime)
        && u64::from(e.ino) == st.ino & 0xFFFF_FFFF
        && e.uid == st.uid
        && e.gid == st.gid
}

/// One `lstat` per checked entry and per leading directory, relative to the root's descriptor, on the pool (at most
/// its cores, ADR-101); the first doubt stops every thread.
fn pass(root: &std::path::Path, plan: &Plan, pool: &JobPool) -> bool {
    let Ok(dir) = RootDir::open(root) else {
        return false;
    };
    let n_files = plan.checked.len();
    let n = n_files.saturating_add(plan.dir_spans.len());
    let failed = AtomicBool::new(false);
    let item = |i: usize| -> bool {
        if let Some(e) = plan.checked.get(i) {
            plan.index
                .name_cstr(e)
                .and_then(|name| dir.lstat(name).ok())
                .is_some_and(|st| entry_matches(e, &st))
        } else {
            plan.dir_spans
                .get(i.saturating_sub(n_files))
                .and_then(|&(s, len)| plan.dirs.get(s..s.saturating_add(len).saturating_add(1)))
                .and_then(|b| CStr::from_bytes_with_nul(b).ok())
                .and_then(|name| dir.lstat(name).ok())
                .is_some_and(|st| st.mode & S_IFMT == S_IFDIR)
        }
    };
    let range = |start: usize, end: usize| -> bool {
        for i in start..end {
            if failed.load(Ordering::Relaxed) {
                return false;
            }
            if !item(i) {
                failed.store(true, Ordering::Relaxed);
                return false;
            }
        }
        true
    };
    let threads = pool.permits().get().min(pool.cores().get());
    if n < PARALLEL_MIN || threads <= 1 || JobPool::holds_permit() {
        return range(0, n);
    }
    let per = n.div_ceil(threads.saturating_mul(4)).max(CHUNK_MIN);
    let chunks = n.div_ceil(per);
    let limit = std::num::NonZeroUsize::new(threads).unwrap_or(std::num::NonZeroUsize::MIN);
    let parts = pool.map_ordered(limit, chunks, |k| {
        let start = k.saturating_mul(per);
        Ok(range(start, start.saturating_add(per).min(n)))
    });
    parts.into_iter().all(|r| r.is_ok_and(|ok| ok)) && !failed.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(mode: u32, size: u32, empty_blob: bool) -> IndexEntry {
        let mut img = b"DIRC".to_vec();
        img.extend_from_slice(&2_u32.to_be_bytes());
        img.extend_from_slice(&1_u32.to_be_bytes());
        for w in [10_u32, 11, 20, 21, 99, 7, mode, 1000, 1001, size] {
            img.extend_from_slice(&w.to_be_bytes());
        }
        if empty_blob {
            img.extend_from_slice(&[
                0xe6, 0x9d, 0xe2, 0x9b, 0xb2, 0xd1, 0xd6, 0x43, 0x4b, 0x8b, 0x29, 0xae, 0x77, 0x5a,
                0xd8, 0xc2, 0xe4, 0x8c, 0x53, 0x91,
            ]);
        } else {
            img.extend_from_slice(&[1; 20]);
        }
        img.extend_from_slice(&1_u16.to_be_bytes());
        img.extend_from_slice(b"a\0");
        img.extend_from_slice(&[0; 20]);
        IndexFile::parse(&img, 20).unwrap().entries[0]
    }

    fn st(mode: u32, size: u64) -> StatData {
        StatData {
            mode,
            ino: 7 | (5 << 32),
            uid: 1000,
            gid: 1001,
            size,
            mtime: (0x14 | (3 << 32), 21),
            ctime: (10, 11),
        }
    }

    /// The strict per-entry match: kind and owner-x bit, truncated fields, the size rules.
    #[test]
    fn entry_match_is_strict() {
        let f = e(0o100_644, 3, false);
        assert!(entry_matches(&f, &st(0o100_644, 3)));
        assert!(
            entry_matches(&f, &st(0o100_664, 3)),
            "group bits are not git's"
        );
        assert!(!entry_matches(&f, &st(0o100_744, 3)), "owner x bit");
        assert!(!entry_matches(&f, &st(0o120_777, 3)), "kind");
        assert!(!entry_matches(&f, &st(0o040_755, 3)), "kind");
        assert!(!entry_matches(&f, &st(0o100_644, 4)), "size");
        assert!(
            entry_matches(&f, &st(0o100_644, 3 | (1 << 32))),
            "size truncated"
        );
        let x = e(0o100_755, 3, false);
        assert!(entry_matches(&x, &st(0o100_755, 3)));
        assert!(!entry_matches(&x, &st(0o100_655, 3)));
        let l = e(0o120_000, 3, false);
        assert!(entry_matches(&l, &st(0o120_777, 3)));
        assert!(!entry_matches(&l, &st(0o100_644, 3)));
        let mut changed = st(0o100_644, 3);
        changed.mtime.1 = 22;
        assert!(!entry_matches(&f, &changed), "mtime ns");
        let mut changed = st(0o100_644, 3);
        changed.ctime.0 = 9;
        assert!(!entry_matches(&f, &changed), "ctime s");
        let mut changed = st(0o100_644, 3);
        changed.ino = 8;
        assert!(!entry_matches(&f, &changed), "ino");
        let mut changed = st(0o100_644, 3);
        changed.uid = 0;
        assert!(!entry_matches(&f, &changed), "uid");
        // empty files: only the empty blob over an empty file
        assert!(entry_matches(&e(0o100_644, 0, true), &st(0o100_644, 0)));
        assert!(!entry_matches(&e(0o100_644, 0, false), &st(0o100_644, 0)));
        assert!(!entry_matches(&e(0o100_644, 0, true), &st(0o100_644, 1)));
        // a size that truncates to 0 (a multiple of 4 GiB) refuses whatever git stored
        assert!(!entry_matches(
            &e(0o100_644, 0, false),
            &st(0o100_644, 1 << 32)
        ));
        assert!(!entry_matches(
            &e(0o100_644, 0x8000_0000, false),
            &st(0o100_644, 1 << 32)
        ));
        assert!(!entry_matches(&e(0o160_000, 3, false), &st(0o040_755, 3)));
    }

    #[test]
    fn leading_dirs_are_distinct() {
        let img = {
            let mut b = b"DIRC".to_vec();
            b.extend_from_slice(&2_u32.to_be_bytes());
            b.extend_from_slice(&3_u32.to_be_bytes());
            for name in [&b"a/b/c"[..], b"a/b/d", b"a/e/f"] {
                let mut ent = Vec::new();
                for w in [0_u32, 0, 0, 0, 0, 0, 0o100_644, 0, 0, 1] {
                    ent.extend_from_slice(&w.to_be_bytes());
                }
                ent.extend_from_slice(&[1; 20]);
                ent.extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
                ent.extend_from_slice(name);
                let padded = (ent.len() + 8) & !7;
                ent.resize(padded, 0);
                b.extend_from_slice(&ent);
            }
            b.extend_from_slice(&[0; 20]);
            b
        };
        let ix = IndexFile::parse(&img, 20).unwrap();
        let (dirs, spans) = leading_dirs(&ix, &ix.entries).unwrap();
        let got: Vec<&[u8]> = spans.iter().map(|&(s, n)| &dirs[s..s + n]).collect();
        assert_eq!(got, [&b"a"[..], b"a/b", b"a/e"]);
        assert_eq!(dirs, b"a\0a/b\0a/e\0");
    }
}
