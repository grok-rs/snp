//! Fuzz target `git_cache_tree`: the strict cache-tree parser and the tree-object builder of the stash's in-process
//! `write-tree` and `diff-index` (ADR-153).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first input byte picks the object id length (even: 20, SHA-1; odd: 32, SHA-256) and the mode: bit 1 clear
//! parses the rest as a `TREE` extension, set parses it as an index image read with its ids and `TREE` kept. Checked:
//! nothing panics; a parsed cache-tree has its root first, every id has the chosen length, `fully_valid` agrees with
//! the ids present; built tree objects come one per node, children first, each with its node's id, and every entry
//! of a body is `<octal mode> <name>\0<id>`.
//!
//! Owner: WP-18.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_git::cache_tree::CacheTree;
use snp_git::index_file::IndexFile;

/// Every entry of a tree body is `<octal mode> <non-empty name>\0<id of hash_len bytes>`.
fn well_formed(body: &[u8], hash_len: usize) -> bool {
    let mut rest = body;
    while !rest.is_empty() {
        let Some(sp) = rest.iter().position(|&b| b == b' ') else { return false };
        if sp == 0 || !rest[..sp].iter().all(|b| (b'0'..=b'7').contains(b)) {
            return false;
        }
        let after = &rest[sp + 1..];
        let Some(nul) = after.iter().position(|&b| b == 0) else { return false };
        if nul == 0 || after.len() < nul + 1 + hash_len {
            return false;
        }
        rest = &after[nul + 1 + hash_len..];
    }
    true
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let hash_len = if pick % 2 == 0 { 20 } else { 32 };
    if pick & 2 == 0 {
        let Ok(ct) = CacheTree::parse(rest, hash_len) else {
            return;
        };
        assert!(!ct.is_empty());
        let ids: Vec<&[u8]> = ct.node_oids().collect();
        assert!(ids.iter().all(|id| id.len() == hash_len));
        assert_eq!(ct.fully_valid(), ids.len() == ct.len());
        assert!(ct.root_oid().is_none_or(|r| ids.first() == Some(&r)));
        return;
    }
    let Ok(ix) = IndexFile::parse_keep(rest, hash_len) else {
        return;
    };
    assert!(ix.entries.iter().all(|e| ix.oid(e).is_some_and(|id| id.len() == hash_len)));
    let Some(ext) = ix.tree_extension() else {
        return;
    };
    let Ok(ct) = CacheTree::parse(ext, hash_len) else {
        return;
    };
    if let Ok(objects) = ct.expected_objects(&ix) {
        assert!(ct.fully_valid());
        assert_eq!(objects.len(), ct.len());
        assert_eq!(objects.last().map(|o| o.oid.as_slice()), ct.root_oid());
        assert!(objects.iter().all(|o| o.oid.len() == hash_len && well_formed(&o.body, hash_len)));
    }
});
