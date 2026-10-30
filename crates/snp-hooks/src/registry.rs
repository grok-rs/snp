//! The native-hook lookup over the per-group registries.
//!
//! Owner: WP-26.

use snp_hooks_data::NativeSource;

use crate::{NativeHook, builtin, pch};

/// The pre-commit-hooks groups in lookup order. The removed-hook stubs come first: a stub entry that shares its id
/// with a real hook of an older tag (`check-byte-order-marker`, `fix-encoding-pragma`) dispatches on the epoch
/// itself (`pch::stubs`).
fn pch_groups() -> [&'static [&'static dyn NativeHook]; 5] {
    [
        pch::stubs::HOOKS,
        pch::p0::HOOKS,
        pch::p1::HOOKS,
        pch::p2::HOOKS,
        pch::caveats::HOOKS,
    ]
}

/// The first hook named `id` in `groups`.
fn find(
    groups: &[&'static [&'static dyn NativeHook]],
    id: &str,
) -> Option<&'static dyn NativeHook> {
    groups
        .iter()
        .flat_map(|g| g.iter())
        .find(|h| h.id() == id)
        .copied()
}

/// The real (non-stub) pre-commit-hooks native named `id` (the stubs delegate older epochs to it).
pub(crate) fn real_pch(id: &str) -> Option<&'static dyn NativeHook> {
    find(
        &[
            pch::p0::HOOKS,
            pch::p1::HOOKS,
            pch::p2::HOOKS,
            pch::caveats::HOOKS,
        ],
        id,
    )
}

/// Concatenates the registries of `pch::{p0, p1, p2, caveats, stubs}` (`NativeSource::PreCommitHooks`) and the
/// builtin variants (`NativeSource::Builtin`).
///
/// `repo: builtin` ids without a dedicated variant (the P1/P2/caveat hooks) resolve to the pre-commit-hooks native,
/// which the runner drives with the v6.0.0 epoch (`BUILTIN_MANIFEST_YAML` is the v6.0.0 manifest, 09 §7.1). An
/// unknown id (or a source added later) is `None`.
pub fn native(source: NativeSource, id: &str) -> Option<&'static dyn NativeHook> {
    match source {
        NativeSource::PreCommitHooks => find(&pch_groups(), id),
        NativeSource::Builtin => {
            find(&[builtin::HOOKS], id).or_else(|| real_pch(id).filter(|_| builtin::listed(id)))
        }
        _ => None,
    }
}
