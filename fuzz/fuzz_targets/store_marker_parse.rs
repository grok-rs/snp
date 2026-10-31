//! Fuzz target `store_marker_parse`: store marker parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is split at the first `0xff` byte. The head describes a repository (`url \0 rev \0 dep \0 dep …`, lossy
//! UTF-8; an empty url selects the local template), which is materialised through the store's own encoder so the
//! entry's directory is the key that metadata derives; the tail is then XOR-ed into its `.snp-repo.json` commit
//! marker (an empty tail leaves it intact), which makes the accept path of `lookup`/`all_repos` reachable. The whole
//! input also becomes the contents of every other small file the store parses — a fixed-key marker, a `configs-v1`
//! entry, a `keys-v1` marker, an environment's `.snp-install-state.json` and `.snp-tools.json`, `store-v1.json` and
//! the README — in one per-process scratch store that is created once and overwritten in place. Checked: no panic;
//! a marker is accepted only when it derives its directory's key (an intact marker is always accepted); an install
//! state is returned only when it is one JSON value; every tracked config has non-empty halves; sweep and marker
//! retention never fail.
//!
//! Owner: WP-22.
#![no_main]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use libfuzzer_sys::fuzz_target;
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::failure::Failure;
use snp_core::output::CollectWarnings;
use snp_store::{RepoId, RepoKey, RepoKind, RepoMeta, Store, StoreLocation, StoreSource};

const KEY: &str = "00112233445566778899aabbccddeeff";

/// One scratch store per fuzz process, created once.
fn scratch() -> &'static (PathBuf, Store) {
    static S: OnceLock<(PathBuf, Store)> = OnceLock::new();
    S.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("snp-fuzz-store-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(StoreLocation {
            path: root.clone(),
            source: StoreSource::SnpHome,
        });
        store
            .ensure_created(Brand::Snp, &CollectWarnings::new())
            .expect("scratch store");
        (root, store)
    })
}

/// The key a handle's metadata derives.
fn derived(meta: &RepoMeta) -> RepoKey {
    match &meta.kind {
        RepoKind::Remote { id, rev } => Store::remote_key(id, rev, &meta.deps),
        _ => Store::local_key(&meta.deps),
    }
}

/// The metadata described by `head`.
fn meta_from(head: &[u8]) -> RepoMeta {
    let mut parts = head.split(|&b| b == 0).map(String::from_utf8_lossy);
    let url = parts.next().unwrap_or_default().into_owned();
    let rev = parts.next().unwrap_or_default().into_owned();
    let deps: Arc<[Arc<str>]> = parts.take(8).map(|d| Arc::from(&*d)).collect();
    let kind = if url.is_empty() {
        RepoKind::Local
    } else {
        RepoKind::Remote {
            id: RepoId::Url(Arc::from(url)),
            rev: Arc::from(rev),
        }
    };
    RepoMeta { kind, deps }
}

fn fake_clone(src: &Path) -> Result<(), Failure> {
    std::fs::create_dir_all(src.join(".git"))
        .map_err(|e| Failure::from_io(&e, Some(src.as_os_str()), None))
}

fuzz_target!(|data: &[u8]| {
    let (root, store) = scratch();
    let (head, tail) = match data.iter().position(|&b| b == 0xff) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &[][..]),
    };

    // 1. A real entry, then the tail XOR-ed into its marker.
    let meta = meta_from(head);
    let key = derived(&meta);
    let h = store
        .materialize(&key, meta, &mut fake_clone, &mut || {}, &CancelToken::new())
        .expect("materialize");
    assert_eq!(derived(&h.meta), key);
    let marker = root.join("repos-v1").join(key.hex()).join(".snp-repo.json");
    let mut bytes = std::fs::read(&marker).expect("marker");
    let len = bytes.len().max(1);
    for (i, b) in tail.iter().enumerate() {
        if let Some(slot) = bytes.get_mut(i % len) {
            *slot ^= b;
        }
    }
    let intact = tail.iter().all(|&b| b == 0);
    std::fs::write(&marker, &bytes).expect("rewrite marker");
    match store.lookup(&key) {
        Some(h) => assert_eq!(derived(&h.meta), key),
        None => assert!(!intact, "an intact marker is accepted"),
    }

    // 2. The raw input as every other parsed file.
    let fixed = root.join("repos-v1").join(KEY);
    let env = fixed.join("envs").join("py_env-3.12");
    std::fs::create_dir_all(fixed.join("src/.git")).expect("src");
    std::fs::create_dir_all(&env).expect("env");
    std::fs::write(fixed.join(".snp-repo.json"), data).expect("fixed marker");
    std::fs::write(root.join("configs-v1").join(KEY), data).expect("config entry");
    std::fs::write(root.join("keys-v1").join(KEY), data).expect("keys marker");
    std::fs::write(env.join(".snp-install-state.json"), data).expect("state");
    std::fs::write(env.join(".snp-tools.json"), data).expect("tools");
    std::fs::write(root.join("store-v1.json"), data).expect("store file");
    std::fs::write(root.join("README"), data).expect("readme");

    for h in store.all_repos() {
        assert!(h.src.starts_with(root));
        assert_eq!(derived(&h.meta), h.key, "accepted only under its own key");
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

    // Keep the scratch store small: drop this iteration's entry (retired to trash/, emptied here).
    let _ = store.retire_repo(&key);
    let _ = std::fs::remove_dir_all(root.join("trash"));
});
