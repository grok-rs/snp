//! Loading configs and manifests: cfgv `load_from_filename` over snp-yaml (`clientlib.load_config`,
//! `load_manifest`; 01 §2).
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::ffi::OsStr;
use std::path::Path;

use snp_pycompat as _;
use snp_yaml as _;

use crate::{Config, ConfigError, ConfigSource, LoadOpts, Manifest};

/// `load_config` (`clientlib.py`): read, decode, PyYAML-load, validate, default, transform (01 §2–§5).
pub fn load_config(src: &ConfigSource, o: &LoadOpts<'_>) -> Result<Config, ConfigError> {
    todo!()
}

/// `load_manifest` (`clientlib.py`). `display` is converted with `to_string_lossy` for cfgv's
/// `load_from_filename_with(…, display_filename: Option<&str>, …)`.
pub fn load_manifest(
    path: &Path,
    display: &OsStr,
    o: &LoadOpts<'_>,
) -> Result<Manifest, ConfigError> {
    todo!()
}

/// `load_manifest` over embedded bytes (Tier A per-tag manifests, builtin manifest).
pub fn load_manifest_bytes(
    bytes: &[u8],
    display: &OsStr,
    o: &LoadOpts<'_>,
) -> Result<Manifest, ConfigError> {
    todo!()
}
