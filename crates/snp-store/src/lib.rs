//! snp's own cache store: location, versioned layout, content-addressed keys, commit and message markers, locks,
//! config tracking, sweep/clean and the crash-log path (ARCHITECTURE §1.4.16, §5; pre-commit's `store.py`).
//!
//! Owner: WP-22.

mod keys;
mod location;
mod store;

pub use keys::{
    MarkerKey, RepoHandle, RepoId, RepoKey, RepoKind, RepoMeta, ToolRef, TrackedConfig,
    db_repo_name,
};
pub use location::{StoreLocation, StoreSource, resolve_location};
pub use store::{Store, StoreGuard, SweepReport};
