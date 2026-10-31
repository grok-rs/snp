//! Fuzz target `stash_journal_parse`: stash journal parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is written as a store journal (`<patches>/run-1-x.snp-journal`) and as the git-dir pointer
//! (`<git_dir>/snp-stash.journal`) of a scratch worktree; `find_unrestored` parses both (JSON, the `{"hex": …}`
//! byte-string form, every field type) and `Unrestored::hint` renders whatever it found. A `{"patch": …}` naming the
//! scratch patch file makes most structurally valid inputs reach the hint. Checked: no panic, every result belongs to
//! the scratch worktree, names an existing patch and is in an unrestored state, and its hint ends with the
//! `git apply --3way` command.
//!
//! Owner: WP-23.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

struct Scratch {
    root: PathBuf,
    git_dir: PathBuf,
    patches: PathBuf,
}

/// One scratch layout per fuzz process; the patch file always exists.
fn scratch() -> &'static Scratch {
    static S: OnceLock<Scratch> = OnceLock::new();
    S.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("snp-fuzz-stash-journal-{}", std::process::id()));
        let git_dir = root.join("r/.git");
        let patches = root.join("store/patches");
        std::fs::create_dir_all(&git_dir).expect("scratch git dir");
        std::fs::create_dir_all(&patches).expect("scratch patches dir");
        std::fs::write(patches.join("patch1-1"), b"diff --git a/f b/f\n").expect("scratch patch");
        Scratch { root, git_dir, patches }
    })
}

fuzz_target!(|data: &[u8]| {
    let s = scratch();
    let worktree = s.root.join("r");
    std::fs::write(s.patches.join("run-1-x.snp-journal"), data).expect("write the journal");
    std::fs::write(s.git_dir.join("snp-stash.journal"), data).expect("write the pointer");
    for u in snp_stash::find_unrestored(&s.git_dir, &s.patches, &worktree) {
        assert_eq!(u.worktree, worktree);
        assert!(u.patch.is_file());
        assert!(matches!(&*u.state, "stashed" | "staging-fixed" | "rolling-back" | "failed"));
        let hint = u.hint();
        assert!(hint.starts_with("snp: an earlier run was interrupted before it restored your unstaged changes"));
        assert!(hint.contains(" git apply --3way "));
    }
});
