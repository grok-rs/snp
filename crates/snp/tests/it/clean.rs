//! Integration tests: `clean` (T10-GC-08, D-STORE-02).
//!
//! Owner: WP-33.

use crate::cli::{assert_code, compat, snp, store, tenv, text};

#[test]
fn t10_gc_08_clean_prints_and_is_idempotent() {
    let env = tenv();
    let st = store(&env);
    std::fs::create_dir_all(st.join("repos-v1/0123")).unwrap();
    // pre-commit's legacy store and pre-commit's own store are never touched (D-STORE-02).
    let legacy = env.root().join("home/.pre-commit");
    std::fs::create_dir_all(&legacy).unwrap();
    let pc = env.root().join("store/repoabc");
    std::fs::create_dir_all(&pc).unwrap();
    for _ in 0..2 {
        let out = compat(&env, env.root(), &["clean"]);
        assert_code(&out, 0);
        assert_eq!(text(&out.stdout), format!("Cleaned {}.\n", st.display()));
    }
    assert!(!st.join("repos-v1").exists());
    assert!(legacy.is_dir() && pc.is_dir());
}

/// `Cleaned {store}.` even when the store does not exist, and nothing is created.
#[test]
fn clean_without_a_store() {
    let env = tenv();
    let missing = env.root().join("nowhere");
    let env = env.with_env("SNP_HOME", missing.as_os_str());
    let out = snp(&env, env.root(), &["clean"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!("Cleaned {}.\n", missing.display())
    );
    assert!(!missing.exists());
}
