//! The native-hook lookup over the per-group registries.
//!
//! Owner: WP-26.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-26")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-26")]

use snp_hooks_data::NativeSource;

use crate::NativeHook;

/// Concatenates the registries of `pch::{p0, p1, p2, caveats, stubs}` (`NativeSource::PreCommitHooks`) and the
/// builtin variants (`NativeSource::Builtin`).
pub fn native(source: NativeSource, id: &str) -> Option<&'static dyn NativeHook> {
    todo!()
}
