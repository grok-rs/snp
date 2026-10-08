//! pre-commit's schemas reproduced item by item with cfgv 0.3 `MapItem` impls (01 §3–§5). `repo: builtin`
//! validation (no `rev`; `entry`/`additional_dependencies` forbidden; `language` absent or `system`) is part of the
//! config schema as cfgv `MapItem`s with cfgv-style contexts and messages (ADR-070); the unknown-id check stays in
//! snp-repo's merge.
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use snp_core::brand::Brand;

use crate::ExtMode;

/// `CONFIG_SCHEMA` (`clientlib.py`) plus the extension keys unless `ext` is `Strict` (§8.1).
pub fn config_schema(brand: Brand, ext: ExtMode) -> &'static cfgv::Schema {
    todo!()
}

/// `MANIFEST_SCHEMA` (`clientlib.py`).
pub fn manifest_schema(brand: Brand) -> &'static cfgv::Schema {
    todo!()
}
