//! Change stamps of worktree files — the per-path change evidence of `stage_fixed` (ARCHITECTURE §8.3) and of
//! parallel-bucket attribution (§3.7 item 6).
//!
//! A stamp is a stat snapshot (`dev`, `ino`, `size`, `mtime`, `ctime`, `mode`, kind on Unix; `size`, `mtime`, kind
//! elsewhere). The content is hashed only where the stat cannot prove a later change: **racy** entries (git's rule:
//! `mtime`/`ctime` within 2 s of the snapshot, so a same-size rewrite inside one timestamp tick is invisible to stat)
//! and every regular file on platforms without `ctime`. Taking a snapshot therefore costs one `lstat` per path plus
//! reading only the files written in the last two seconds — never O(repository bytes) per bucket or hook.
//!
//! The comparison is conservative: any stat difference counts as a change (a touch-only rewrite included), which
//! can only fail a hook or withhold a `stage_fixed` staging, never hide a modification.
//!
//! The hash is `SipHash` with per-process random keys (`std::hash::RandomState`), streamed in 64 KiB chunks: the files
//! are the user's own worktree, never an adversary choosing collisions against an unknown key.
//!
//! Owner: WP-32.

use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use snp_core::path::RepoPath;

/// git's racy window.
const RACY: Duration = Duration::from_secs(2);

/// The stat part of a stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stat {
    len: u64,
    mtime: Option<SystemTime>,
    /// 0 = other, 1 = regular file, 2 = symlink.
    kind: u8,
    #[cfg(unix)]
    unix: (u64, u64, u32, i64, i64),
}

/// One file's stamp; `stat: None` means "absent / unreadable" (a deletion is a change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    stat: Option<Stat>,
    /// The content hash, taken only for racy entries (and everywhere without `ctime`).
    hash: Option<u64>,
}

fn keys() -> &'static RandomState {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    KEYS.get_or_init(RandomState::new)
}

fn stat_of(meta: &Metadata) -> Stat {
    let kind = if meta.file_type().is_symlink() {
        2
    } else {
        u8::from(meta.is_file())
    };
    Stat {
        len: meta.len(),
        mtime: meta.modified().ok(),
        kind,
        #[cfg(unix)]
        unix: {
            use std::os::unix::fs::MetadataExt as _;
            (
                meta.dev(),
                meta.ino(),
                meta.mode(),
                meta.ctime(),
                meta.ctime_nsec(),
            )
        },
    }
}

/// Whether the stat of `meta` cannot prove a later same-size rewrite (git's racy rule).
fn racy(meta: &Metadata, now: SystemTime) -> bool {
    if !cfg!(unix) {
        return true;
    }
    let Some(edge) = now.checked_sub(RACY) else {
        return true;
    };
    let recent = |t: Option<SystemTime>| t.is_none_or(|t| t >= edge);
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt as _;
        let secs = u64::try_from(meta.ctime()).ok();
        let nanos = u32::try_from(meta.ctime_nsec()).unwrap_or(0);
        secs.and_then(|s| SystemTime::UNIX_EPOCH.checked_add(Duration::new(s, nanos)))
    };
    #[cfg(not(unix))]
    let changed = meta.modified().ok();
    recent(meta.modified().ok()) || recent(changed)
}

fn hash_of(p: &Path, meta: &Metadata) -> Option<u64> {
    let mut hasher = keys().build_hasher();
    if meta.file_type().is_symlink() {
        return std::fs::read_link(p).ok().map(|t| {
            hasher.write(t.as_os_str().as_encoded_bytes());
            hasher.finish()
        });
    }
    if !meta.is_file() {
        return None;
    }
    let mut f = File::open(p).ok()?;
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break Some(hasher.finish()),
            Ok(n) => hasher.write(buf.get(..n).unwrap_or_default()),
            Err(_) => break None,
        }
    }
}

/// The stamp of `root/<path>` (symlinks are not followed: the link itself is the tracked object).
pub(crate) fn stamp(root: &Path, path: &RepoPath) -> Stamp {
    let p = root.join(&*path.to_os());
    let Ok(meta) = std::fs::symlink_metadata(&p) else {
        return Stamp {
            stat: None,
            hash: None,
        };
    };
    let hash = if racy(&meta, SystemTime::now()) {
        hash_of(&p, &meta)
    } else {
        None
    };
    Stamp {
        stat: Some(stat_of(&meta)),
        hash,
    }
}

/// Whether `root/<path>` differs from `before` (any stat difference; the content where `before` hashed it).
fn differs(root: &Path, path: &RepoPath, before: &Stamp) -> bool {
    let p = root.join(&*path.to_os());
    let Ok(meta) = std::fs::symlink_metadata(&p) else {
        return before.stat.is_some();
    };
    if before.stat != Some(stat_of(&meta)) {
        return true;
    }
    before.hash.is_some() && hash_of(&p, &meta) != before.hash
}

/// Stamps of several paths.
pub(crate) fn stamps<'p>(
    root: &Path,
    paths: impl IntoIterator<Item = &'p RepoPath>,
) -> BTreeMap<RepoPath, Stamp> {
    paths
        .into_iter()
        .map(|p| (p.clone(), stamp(root, p)))
        .collect()
}

/// The paths of `before` whose stamp changed.
pub(crate) fn changed(root: &Path, before: &BTreeMap<RepoPath, Stamp>) -> Vec<RepoPath> {
    before
        .iter()
        .filter(|(p, s)| differs(root, p, s))
        .map(|(p, _)| p.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_detect_same_size_rewrites() {
        let d = tempfile::tempdir().unwrap();
        let d = d.path();
        let p = RepoPath::new(b"a.txt").unwrap();
        std::fs::write(d.join("a.txt"), b"aaaa").unwrap();
        let before = stamps(d, [&p]);
        assert_eq!(changed(d, &before), Vec::<RepoPath>::new());
        // a same-size rewrite inside the racy window: the content hash catches it even with the old mtime restored
        let mtime = std::fs::metadata(d.join("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::write(d.join("a.txt"), b"bbbb").unwrap();
        File::options()
            .write(true)
            .open(d.join("a.txt"))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(changed(d, &before), vec![p.clone()]);
        std::fs::remove_file(d.join("a.txt")).unwrap();
        assert_eq!(changed(d, &before), vec![p]);
    }

    #[cfg(unix)]
    #[test]
    fn old_files_are_not_read() {
        let d = tempfile::tempdir().unwrap();
        let d = d.path();
        let p = RepoPath::new(b"old.txt").unwrap();
        std::fs::write(d.join("old.txt"), b"old").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(d.join("old.txt"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        // ctime is "now" after set_modified: still racy; a non-racy entry needs both times old
        let s = stamp(d, &p);
        let meta = std::fs::symlink_metadata(d.join("old.txt")).unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        assert!(!racy(&meta, later), "both times outside the window");
        assert_eq!(racy(&meta, SystemTime::now()), s.hash.is_some());
        // an unchanged stat with no hash is unchanged
        let cold = Stamp {
            stat: Some(stat_of(&meta)),
            hash: None,
        };
        assert!(!differs(d, &p, &cold));
        std::fs::write(d.join("old.txt"), b"new").unwrap();
        assert!(differs(d, &p, &cold), "the rewrite changed ctime");
    }
}
