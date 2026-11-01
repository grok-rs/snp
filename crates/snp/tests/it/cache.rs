//! Integration tests: `snp cache {dir,size,gc,clean}`.
//!
//! Owner: WP-33.

use crate::cli::{assert_code, snp, store, tenv, text};

#[test]
fn cache_dir_size_gc_clean() {
    let env = tenv();
    let st = store(&env);
    let out = snp(&env, env.root(), &["cache", "dir"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), format!("{}\n", st.display()));
    std::fs::create_dir_all(st.join("repos-v1")).unwrap();
    std::fs::write(st.join("repos-v1/blob"), vec![0_u8; 2048]).unwrap();
    let out = snp(&env, env.root(), &["cache", "size"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "2.0 KiB (2048 bytes)\n");
    let out = snp(&env, env.root(), &["cache", "gc"]);
    assert_eq!(text(&out.stdout), "0 repo(s) removed.\n");
    let out = snp(&env, env.root(), &["cache", "clean"]);
    assert_eq!(text(&out.stdout), format!("Cleaned {}.\n", st.display()));
}
