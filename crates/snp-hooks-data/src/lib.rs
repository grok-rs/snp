//! Generated pre-commit-hooks data: per-tag manifests, tag → commit, epochs and classes, the verified blob table,
//! the console-script map, the `repo: builtin` manifest, parity gates and URL identity; no logic beyond lookups
//! (ARCHITECTURE §1.4.12, §7.2; 09 §5–§6; ADR-031).
//!
//! Every table under `src/generated/` is produced by `cargo xtask gen-pch-data <clone>` from the tags
//! v3.0.0…v6.0.0 of `github.com/pre-commit/pre-commit-hooks` and checked for drift in CI
//! (`cargo xtask gen-pch-data --check <clone>`); nothing is read at run time.
//!
//! Owner: WP-17 (`src/generated/parity_gates.rs`: WP-27 from W2 on).

#[rustfmt::skip] // `cargo xtask gen-pch-data` output, byte-checked by `--check`
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
