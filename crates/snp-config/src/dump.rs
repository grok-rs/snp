//! `snp config dump` and `snp config schema` (§9.3).
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use crate::Config;

/// The effective config (overlay-marked fields) as YAML, or JSON with `json`.
pub fn effective_dump(cfg: &Config, json: bool) -> String {
    todo!()
}

/// The JSON schema of the config, extension keys included.
pub fn json_schema() -> &'static str {
    todo!()
}
