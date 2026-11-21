//! Fuzz target `git_index_file`: the strict index reader of the index-stat proof (ADR-134).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first input byte picks the object id length (even: 20, SHA-1; odd: 32, SHA-256), the rest is the index image.
//! Checked: the reader never panics; on success the version is 2, 3 or 4, every entry's name is non-empty, holds no
//! NUL and agrees with its length field (or is at least 0xFFF long), its `CStr` form is the same bytes, every second
//! flags word holds only skip-worktree and intent-to-add, and the extended flag never appears in a v2 image; a v2/v3
//! image re-serialised from the parsed entries (same flags, ids zeroed) parses back to the same entries apart from the
//! empty-blob flag.
//!
//! Owner: WP-18.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_git::index_file::IndexFile;

/// A v2/v3 image of `ix`'s entries (ids zeroed, no extensions), as git lays it out.
fn serialise(ix: &IndexFile, hash_len: usize) -> Vec<u8> {
    let mut b = b"DIRC".to_vec();
    b.extend_from_slice(&ix.version.to_be_bytes());
    b.extend_from_slice(&u32::try_from(ix.entries.len()).unwrap().to_be_bytes());
    for e in &ix.entries {
        let start = b.len();
        for w in [e.ctime.0, e.ctime.1, e.mtime.0, e.mtime.1, e.dev, e.ino, e.mode, e.uid, e.gid, e.size] {
            b.extend_from_slice(&w.to_be_bytes());
        }
        b.extend(std::iter::repeat_n(0_u8, hash_len));
        b.extend_from_slice(&e.flags.to_be_bytes());
        if e.flags & 0x4000 != 0 {
            b.extend_from_slice(&e.flags2.to_be_bytes());
        }
        b.extend_from_slice(ix.name(e));
        let used = b.len() - start;
        b.resize(start + ((used + 8) & !7), 0);
    }
    b.extend(std::iter::repeat_n(0_u8, hash_len));
    b
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, image)) = data.split_first() else {
        return;
    };
    let hash_len = if pick % 2 == 0 { 20 } else { 32 };
    let Ok(ix) = IndexFile::parse(image, hash_len) else {
        return;
    };
    assert!((2..=4).contains(&ix.version));
    for e in &ix.entries {
        let name = ix.name(e);
        assert!(!name.is_empty() && !name.contains(&0));
        let field = usize::from(e.flags & 0x0fff);
        assert!(if field == 0x0fff { name.len() >= field } else { name.len() == field });
        assert_eq!(ix.name_cstr(e).map(std::ffi::CStr::to_bytes), Some(name));
        assert_eq!(e.flags2 & !0x6000, 0);
        assert!(ix.version >= 3 || e.flags & 0x4000 == 0);
        assert!(e.stage() <= 3);
    }
    if ix.version <= 3 {
        let again = IndexFile::parse(&serialise(&ix, hash_len), hash_len).expect("a re-serialised image parses");
        assert_eq!(again.entries.len(), ix.entries.len());
        for (a, b) in again.entries.iter().zip(&ix.entries) {
            assert_eq!(again.name(a), ix.name(b));
            assert_eq!((a.ctime, a.mtime, a.dev, a.ino, a.mode, a.uid, a.gid, a.size), (b.ctime, b.mtime, b.dev, b.ino, b.mode, b.uid, b.gid, b.size));
            assert_eq!((a.flags, a.flags2), (b.flags, b.flags2));
        }
    }
});
