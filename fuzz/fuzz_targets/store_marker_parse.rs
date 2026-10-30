//! Fuzz target `store_marker_parse`: store marker parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input becomes the contents of every small file the store parses — a repository's `.snp-repo.json` commit
//! marker, a `configs-v1` entry, a `keys-v1` marker, an environment's `.snp-install-state.json` and
//! `.snp-tools.json`, `store-v1.json` and the README — in a per-process scratch store. Checked: no panic; a marker
//! is accepted only when it derives its directory's key; an install state is returned only when it is one JSON
//! value; every tracked config has non-empty halves; sweep and marker retention never fail.
//!
//! Owner: WP-22.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;

use libfuzzer_sys::fuzz_target;
use snp_core::brand::Brand;
use snp_core::output::CollectWarnings;
use snp_store::{RepoKey, Store, StoreLocation, StoreSource};

/// One scratch store per fuzz process.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::temp_dir().join(format!("snp-fuzz-store-marker-{}", std::process::id()))
    })
}

const KEY: &str = "00112233445566778899aabbccddeeff";

fuzz_target!(|data: &[u8]| {
    let root = scratch();
    let _ = std::fs::remove_dir_all(root);
    let store = Store::new(StoreLocation {
        path: root.clone(),
        source: StoreSource::SnpHome,
    });
    store
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .expect("scratch store");
    let repo = root.join("repos-v1").join(KEY);
    let env = repo.join("envs").join("py_env-3.12");
    std::fs::create_dir_all(repo.join("src/.git")).expect("src");
    std::fs::create_dir_all(&env).expect("env");
    std::fs::write(repo.join(".snp-repo.json"), data).expect("marker");
    std::fs::write(root.join("configs-v1").join(KEY), data).expect("config entry");
    std::fs::write(root.join("keys-v1").join(KEY), data).expect("keys marker");
    std::fs::write(env.join(".snp-install-state.json"), data).expect("state");
    std::fs::write(env.join(".snp-tools.json"), data).expect("tools");
    std::fs::write(root.join("store-v1.json"), data).expect("store file");
    std::fs::write(root.join("README"), data).expect("readme");

    let key = RepoKey::from_hex_for_fuzz();
    if let Some(h) = store.lookup(&key) {
        let derived = match &h.meta.kind {
            snp_store::RepoKind::Remote { id, rev } => Store::remote_key(id, rev, &h.meta.deps),
            _ => Store::local_key(&h.meta.deps),
        };
        assert_eq!(derived, key);
    }
    for h in store.all_repos() {
        assert!(h.src.starts_with(root));
    }
    if let Some(state) = Store::read_install_marker(&env) {
        assert_eq!(state, data);
    }
    for t in store.tracked_configs() {
        assert!(!t.path.as_os_str().is_empty() && !t.project_root.as_os_str().is_empty());
    }
    let _ = store.relocated_from();
    let _kept = store.retain_markers(&mut |_| true);
    let _report = store.sweep(SystemTime::now());
});

/// The fixed key of the scratch entry.
trait FromHexForFuzz {
    fn from_hex_for_fuzz() -> Self;
}

impl FromHexForFuzz for RepoKey {
    fn from_hex_for_fuzz() -> Self {
        let mut out = [0_u8; 16];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&KEY[i * 2..i * 2 + 2], 16).expect("hex");
        }
        Self(out)
    }
}
