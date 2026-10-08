//! `repository._hook()`: manifest ∪ config overrides, defaults, env-less guards, overlay patches (01 §6).
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use snp_core::output::WarnSink;

use crate::overlay::HookPatch;
use crate::{Config, ConfigHook, HookDef, MergeError};

/// `repository._hook()`: manifest ∪ overrides (whole-key), `language_version` via `default_language_version` (the
/// language's own default is resolved by snp-repo with probes), empty stages ⇒ `default_stages`, env-less guards,
/// W11 unknown-key warning (sorted) after merging. `overlay` (§8.4) is applied last, after manifest ∪ config:
/// `{entry}` expands against the merged entry; fast-path G4 and the `InstallKey` see the patched `HookDef`.
pub fn merge_hook(
    m: &HookDef,
    c: &ConfigHook,
    cfg: &Config,
    src: &str,
    overlay: Option<&HookPatch>,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    todo!()
}

/// The `repo: local` / `repo: meta` half of `_hook()` (no manifest: the hook dict is already defaulted).
pub fn finalize_local(
    h: &HookDef,
    cfg: &Config,
    src: &str,
    overlay: Option<&HookPatch>,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    todo!()
}
