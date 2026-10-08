//! Generated pre-commit-hooks data: per-tag manifests, tag → commit, epochs and classes, the verified blob table,
//! the console-script map, the `repo: builtin` manifest, parity gates and URL identity; no logic beyond lookups
//! (ARCHITECTURE §1.4.12, 09 §5–§6).
//!
//! Owner: WP-17 (`src/generated/parity_gates.rs`: WP-27 from W2 on).

mod generated;
mod lookup;
mod types;

pub use generated::{BUILTIN_MANIFEST_YAML, GENERATED_FROM};
pub use lookup::{
    import_closure, is_pre_commit_hooks_url, module_for_entry, parity_gate, profile_for_rev,
    verified_blob,
};
pub use types::{Epoch, Gate, HookClass, NativeSource, TagProfile};

use snp_core as _;
