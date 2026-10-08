//! The local overlay `.pre-commit-config.local.y[a]ml` (§8.4, ADR-070): root items are applied to the `Config`,
//! hook patches are data merged by `merge_hook`.
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::path::{Path, PathBuf};

use snp_core::env::Env;
use snp_core::output::DiagSink;

use crate::ext::HookExt;
use crate::{Config, ConfigError, LoadOpts};

/// A loaded overlay: hooks by alias|id, `skip_groups`, `fail_on_changes`, appended `repos`.
#[derive(Debug, Clone)]
pub struct Overlay {
    /// W0 placeholder; WP-21 stores the hook patches by alias|id, `skip_groups`, `fail_on_changes` and `repos`.
    _priv: (),
}

/// One hook entry of the overlay: validated hook keys (any hook key except id/alias/repo), `entry` possibly
/// containing `{entry}`, parsed extension keys. `keys()` counts as user config for fast-path G4 (§7.2).
#[derive(Debug, Clone)]
pub struct HookPatch {
    /// The validated hook keys.
    pub raw: cfgv::Map,
    /// Parsed extension keys.
    pub ext: HookExt,
}

impl HookPatch {
    /// The keys the patch sets.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        core::iter::from_fn(|| todo!())
    }
}

/// `<config dir>/.pre-commit-config.local.y[a]ml` or `SNP_LOCAL_CONFIG`; ignored when tracked by git or when CI is
/// truthy unless named by `SNP_LOCAL_CONFIG`; `Err(note)` = ignored with a stderr note.
pub fn locate(
    cfg: &Path,
    env: &Env,
    is_tracked: &dyn Fn(&Path) -> bool,
) -> Result<Option<PathBuf>, String> {
    todo!()
}

/// Loads and validates an overlay file.
pub fn load(path: &Path, o: &LoadOpts<'_>) -> Result<Overlay, ConfigError> {
    todo!()
}

/// Root items only: `skip_groups`, `fail_on_changes`, appended repos; prints the active-overlay stderr line.
pub fn apply_root(cfg: &mut Config, ov: &Overlay, diag: &dyn DiagSink) {
    todo!()
}

impl Overlay {
    /// Keyed by alias first, else id (an id matching several hooks applies to all of them).
    pub fn hook_patch(&self, id: &str, alias: &str) -> Option<&HookPatch> {
        todo!()
    }

    /// Overlay keys that matched no resolved hook → `snp: local config overrides unknown hook '<id>'` (stderr).
    pub fn unmatched<'a>(&'a self, matched: &[&str]) -> Vec<&'a str> {
        todo!()
    }
}
