//! ADR-153: the stash's `git write-tree` and `git diff-index` decided in-process on the provably clean path. Any doubt
//! answers "no", and the stash runs the real command.
//!
//! **write-tree** (`write_index_as_tree`, `cache-tree.c`, the same logic in git 2.18 and 2.56) takes `<index>.lock`,
//! reads the index and, when the cache-tree is fully valid (every node valid, every node's object present), writes
//! nothing, prints the root's id and removes the lock. [`prepare`] applies that rule to the index git already read
//! successfully in this run (the caller's stamp, taken before a git command that read the index and succeeded, still
//! holds before and after the read here): the strict reader keeps the entries' ids and the `TREE` extension
//! ([`IndexFile::read_keep`]), the extension parses strictly ([`CacheTree`]), every node is valid and every node's
//! object has a loose copy (git's own existence test is an `lstat` of it, which also holds when packs exist). The
//! index must be a regular file (git locks a symbolic link's target). [`lock_probe`] then creates `<index>.lock`
//! exclusively and removes it at once, as `write-tree` does, so a held lock, a read-only directory or any other lock
//! error still runs the real `write-tree`, which fails with git's own message.
//!
//! **diff-index** `--ignore-submodules --binary --exit-code --no-color --no-ext-diff <tree> --` exits 0 with nothing
//! printed when no file pair is queued. [`diff_index_empty`] proves that for the tree `write-tree` printed or [`prepare`]
//! gave: the tree is the cache-tree's root, the tree objects git reads (the root through `parse_object`, which
//! checks its hash, and on git < 2.20 every subtree) are loose, read strictly, hash to their names and equal byte for
//! byte the objects `update_one` builds from the index entries ([`CacheTree::expected_objects`]), so the tree is
//! exactly the index's (a stale cache-tree, which `write-tree` still trusts, is refused here); git reads them from
//! those copies only ([`odb::loose_only`]: no packs, alternates or replace refs); and every entry is stat-clean by
//! ADR-134's strict match with none skipped ([`every_entry_stat_clean`]: no unmerged, intent-to-add,
//! assume-unchanged, skip-worktree or gitlink entry, nothing racy), so git compares no content and queues nothing.
//! The configuration gate is ADR-134's (step 6's `git config --list` answer: no fsmonitor, ignoreStat, sparse
//! checkout or `core.worktree`, a SHA-1 object format), on `stat_oracle_safe` filesystems, without `SNP_DIFF_MODE=git`
//! or any `GIT_TEST_*` variable, for at most [`MAX_DIFF_ENTRIES`] entries (checked on the calling thread).
//!
//! Owner: WP-23.

use std::path::Path;

use snp_core::env::Env;
use snp_git::cache_tree::CacheTree;
use snp_git::index_file::IndexFile;
use snp_git::{ConfigListing, Git, IndexStamp, ObjectId, RepoInfo, odb};

use crate::diff_oracle::every_entry_stat_clean;

/// The largest index [`prepare`] reads (the Stat mode's bound).
const MAX_ENTRIES: usize = 16_384;
/// The largest index [`diff_index_empty`] checks: one `lstat` per entry on the calling thread.
pub(crate) const MAX_DIFF_ENTRIES: usize = 2_048;

/// What [`prepare`] learned: the index as git read it and its cache-tree.
#[derive(Debug)]
pub(crate) struct Prepared {
    index: IndexFile,
    tree: CacheTree,
    stamp: IndexStamp,
    /// The index file's mtime second, as git keeps it.
    index_sec: u32,
    root: ObjectId,
}

impl Prepared {
    /// The id `write-tree` prints: the cache-tree's root, lowercase.
    pub(crate) fn root(&self) -> &ObjectId {
        &self.root
    }
}

/// The object id length when the run's configuration and environment allow the elision.
fn gate(info: &RepoInfo, env: &Env, listing: &ConfigListing) -> Option<usize> {
    if !cfg!(unix) || env.get("SNP_DIFF_MODE").is_some_and(|v| v == "git") {
        return None;
    }
    let test_vars = env.iter().any(|(k, _)| {
        k.to_str()
            .is_none_or(|k| k.starts_with("GIT_TEST_") || k == "GIT_FSMONITOR_TEST")
    });
    if test_vars || env.get("GIT_OBJECT_DIRECTORY").is_some() {
        return None;
    }
    let safe = |p: &Path| snp_os::fsinfo::stat_oracle_safe(p).unwrap_or(false);
    if !safe(&info.git_dir) {
        return None;
    }
    listing
        .checkout
        .object_id_len
        .filter(|_| listing.checkout.index_proof)
}

/// The index file's mtime second as git keeps it (`(unsigned int)st_mtime`), when it is a regular file.
#[cfg(unix)]
#[expect(
    clippy::filetype_is_file,
    reason = "only a regular file: git locks a symbolic link's target, and anything else is a doubt"
)]
fn regular_index_sec(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    let m = std::fs::symlink_metadata(path).ok()?;
    if !m.file_type().is_file() {
        return None;
    }
    u32::try_from(m.mtime() & 0xFFFF_FFFF).ok()
}

#[cfg(not(unix))]
fn regular_index_sec(_path: &Path) -> Option<u32> {
    None
}

/// `write-tree` would print the cache-tree's root and write nothing (see the module documentation); `read` is the
/// stamp of the index under which a git command of this run read it and succeeded.
pub(crate) fn prepare(
    git: &Git,
    info: &RepoInfo,
    env: &Env,
    listing: &ConfigListing,
    read: IndexStamp,
) -> Option<Prepared> {
    let hash_len = gate(info, env, listing)?;
    let index_sec = regular_index_sec(&info.index_file)?;
    if git.index_stamp(info) != Some(read) {
        return None;
    }
    let index = IndexFile::read_keep(&info.index_file, hash_len).ok()?;
    // the bytes read are the stamped file's only if the stamp still holds after the read
    if index.entries.len() > MAX_ENTRIES
        || git.index_stamp(info) != Some(read)
        || regular_index_sec(&info.index_file) != Some(index_sec)
    {
        return None;
    }
    let tree = CacheTree::parse(index.tree_extension()?, hash_len).ok()?;
    if !tree.fully_valid() {
        return None;
    }
    let objects = info.common_dir.join("objects");
    if !tree.node_oids().all(|oid| odb::loose_exists(&objects, oid)) {
        return None;
    }
    let root = ObjectId::parse(&odb::hex(tree.root_oid()?))?;
    Some(Prepared {
        index,
        tree,
        stamp: read,
        index_sec,
        root,
    })
}

/// `write-tree`'s lock, taken and given back: `<index>.lock` created exclusively (`O_CREAT|O_EXCL`, mode 0666 less
/// the umask, as git's lockfile) and removed at once. `false` on any error (the real `write-tree` then reports it in
/// git's words); a lock that was created but could not be removed is left to git's own message as well.
pub(crate) fn lock_probe(index_file: &Path) -> bool {
    let mut name = index_file.as_os_str().to_os_string();
    name.push(".lock");
    let lock = Path::new(&name);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock)
    {
        Ok(f) => {
            drop(f);
            std::fs::remove_file(lock).is_ok()
        }
        Err(_) => false,
    }
}

/// `diff-index <tree>` would exit 0 with nothing printed (see the module documentation).
pub(crate) fn diff_index_empty(
    git: &Git,
    info: &RepoInfo,
    env: &Env,
    p: &Prepared,
    tree: &ObjectId,
) -> bool {
    let safe = |dir: &Path| snp_os::fsinfo::stat_oracle_safe(dir).unwrap_or(false);
    if p.index.hash_len() != 20
        || tree != &p.root
        || p.index.entries.len() > MAX_DIFF_ENTRIES
        || !safe(&info.root)
        || !odb::loose_only(&info.common_dir, env)
        || !every_entry_stat_clean(&info.root, &p.index, p.index_sec)
    {
        return false;
    }
    let Ok(expected) = p.tree.expected_objects(&p.index) else {
        return false;
    };
    let objects = info.common_dir.join("objects");
    let stored = expected
        .iter()
        .all(|o| odb::read_loose(&objects, &o.oid, b"tree").is_ok_and(|body| body == o.body));
    stored && git.index_stamp(info) == Some(p.stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_probe_creates_and_removes_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("index");
        std::fs::write(&index, b"x").unwrap();
        assert!(lock_probe(&index));
        assert!(!dir.path().join("index.lock").exists());
        std::fs::write(dir.path().join("index.lock"), b"").unwrap();
        assert!(!lock_probe(&index), "a held lock");
        assert!(dir.path().join("index.lock").exists(), "left alone");
        assert!(!lock_probe(&dir.path().join("missing").join("index")));
    }
}
