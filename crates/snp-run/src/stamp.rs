//! Content stamps of worktree files: (`len`, `mtime`, keyed content hash) — the per-path change evidence of
//! `stage_fixed` (ARCHITECTURE §8.3) and of parallel-bucket attribution (§3.7 item 6).
//!
//! The hash is `SipHash` with per-process random keys (`std::hash::RandomState`), streamed in 64 KiB chunks: the files
//! are the user's own worktree, never an adversary choosing collisions against an unknown key.
//!
//! Owner: WP-32.

use std::collections::BTreeMap;
use std::fs::File;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use std::time::SystemTime;

use snp_core::path::RepoPath;

/// One file's stamp; `None` fields mean "absent / unreadable" (a deletion is a change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: Option<u64>,
    mtime: Option<SystemTime>,
    hash: Option<u64>,
}

fn keys() -> &'static RandomState {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    KEYS.get_or_init(RandomState::new)
}

/// The stamp of `root/<path>` (symlinks are not followed: the link itself is the tracked object).
pub(crate) fn stamp(root: &Path, path: &RepoPath) -> Stamp {
    let p = root.join(&*path.to_os());
    let Ok(meta) = std::fs::symlink_metadata(&p) else {
        return Stamp {
            len: None,
            mtime: None,
            hash: None,
        };
    };
    let mut hasher = keys().build_hasher();
    let hash = if meta.file_type().is_symlink() {
        std::fs::read_link(&p).ok().map(|t| {
            hasher.write(t.as_os_str().as_encoded_bytes());
            hasher.finish()
        })
    } else if meta.is_file() {
        File::open(&p).ok().and_then(|mut f| {
            let mut buf = vec![0_u8; 64 * 1024];
            loop {
                match f.read(&mut buf) {
                    Ok(0) => break Some(hasher.finish()),
                    Ok(n) => hasher.write(buf.get(..n).unwrap_or_default()),
                    Err(_) => break None,
                }
            }
        })
    } else {
        None
    };
    Stamp {
        len: Some(meta.len()),
        mtime: meta.modified().ok(),
        hash,
    }
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
        .filter(|(p, s)| stamp(root, p) != **s)
        .map(|(p, _)| p.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_detect_same_size_rewrites() {
        let d = std::env::temp_dir().join(format!("snp-run-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = RepoPath::new(b"a.txt").unwrap();
        std::fs::write(d.join("a.txt"), b"aaaa").unwrap();
        let before = stamps(&d, [&p]);
        assert_eq!(changed(&d, &before), Vec::<RepoPath>::new());
        std::fs::write(d.join("a.txt"), b"bbbb").unwrap();
        assert_eq!(changed(&d, &before), vec![p.clone()]);
        std::fs::remove_file(d.join("a.txt")).unwrap();
        assert_eq!(changed(&d, &before), vec![p]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
