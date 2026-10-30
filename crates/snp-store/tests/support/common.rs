//! Shared helpers of the snp-store integration tests.
//!
//! Owner: WP-22.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::failure::Failure;
use snp_core::output::CollectWarnings;
use snp_store::{RepoId, RepoKey, RepoKind, RepoMeta, Store, StoreLocation, StoreSource};

/// A store rooted at `path` (not created).
pub(crate) fn store_at(path: &Path) -> Store {
    Store::new(StoreLocation {
        path: path.to_path_buf(),
        source: StoreSource::SnpHome,
    })
}

/// A created store under a fresh temp dir: (guard, store root, store).
pub(crate) fn new_store() -> (tempfile::TempDir, PathBuf, Store) {
    let d = tempfile::tempdir().unwrap();
    let root = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/"))
        .unwrap()
        .join("store");
    let s = store_at(&root);
    s.ensure_created(Brand::Snp, &CollectWarnings::new()).unwrap();
    (d, root, s)
}

/// A remote repository's metadata and key.
pub(crate) fn remote(url: &str, rev: &str, deps: &[&str]) -> (RepoKey, RepoMeta) {
    let id = RepoId::Url(Arc::from(url));
    let deps: Arc<[Arc<str>]> = deps.iter().map(|d| Arc::from(*d)).collect();
    let key = Store::remote_key(&id, rev, &deps);
    (
        key,
        RepoMeta {
            kind: RepoKind::Remote {
                id,
                rev: Arc::from(rev),
            },
            deps,
        },
    )
}

/// A fill that makes `src` look like a clone (`src/.git`, one file).
pub(crate) fn fake_clone(src: &Path) -> Result<(), Failure> {
    let io = |e: std::io::Error| Failure::from_io(&e, Some(src.as_os_str()), None);
    std::fs::create_dir_all(src.join(".git")).map_err(io)?;
    std::fs::write(src.join(".pre-commit-hooks.yaml"), b"[]\n").map_err(io)
}

/// Materialises `(key, meta)` with [`fake_clone`].
pub(crate) fn materialize(s: &Store, key: &RepoKey, meta: RepoMeta) -> snp_store::RepoHandle {
    s.materialize(key, meta, &mut fake_clone, &mut || {}, &CancelToken::new())
        .unwrap()
}

/// The exception text of a failure.
pub(crate) fn exc_text(f: &Failure) -> (String, String) {
    match f {
        Failure::Exc { exc, .. } => (
            exc.ty.to_string(),
            String::from_utf8_lossy(&exc.text).into_owned(),
        ),
        other => panic!("not an exception: {other:?}"),
    }
}

/// Whether the tests run as root (permission tests are meaningless then).
#[cfg(unix)]
pub(crate) fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}
