//! Data types of the generated tables (ARCHITECTURE §1.4.12).
//!
//! Owner: WP-17.

/// Behaviour epoch of a pre-commit-hooks module (09 §5.2), the version of the upstream behaviour a native hook
/// reproduces (`snp_hooks::HookCx::epoch`).
///
/// `Epoch(0)` is a DELEGATE epoch (behaviour snp does not reproduce: the hook runs upstream's Python); native epochs
/// count from 1 per module in tag order, and every observable difference between two upstream versions of a module
/// (CLI flags and help texts included) starts a new one, so a module blob always determines its epoch. The
/// generated table:
#[doc = include_str!("generated/epochs.md")]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(pub u16);

/// How snp runs a pre-commit-hooks hook (09 §1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookClass {
    /// Native port, byte-exact.
    Native,
    /// Native port with documented caveats (third-party parser diagnostics: check-yaml, check-toml, check-xml,
    /// detect-aws-credentials); used in `auto` mode only once its parity gate passes.
    NativeWithCaveats,
    /// Delegated to the upstream Python hook (CPython `ast`/`tokenize` hooks and DELEGATE epochs).
    Delegate,
    /// `language: fail` in the manifest (`check-illegal-windows-names`, `forbid-submodules`): no Python at all.
    FailLanguage,
    /// A removed hook (`pre-commit-hooks-removed` entry): the stub's `SystemExit` message.
    Stub,
}

/// CI parity-gate result (09 §9.9).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Gate {
    /// Native output equals upstream on the corpus.
    Exact,
    /// Equal except documented caveats.
    Caveat,
    /// Delegate to upstream.
    Delegate,
}

/// One pre-commit-hooks tag (Tier A, 09 §6.3).
#[derive(Debug)]
pub struct TagProfile {
    /// The tag, e.g. "v6.0.0".
    pub tag: &'static str,
    /// The tag's commit (40 lowercase hex digits; every pre-commit-hooks tag is a lightweight tag on a commit).
    pub commit: &'static str,
    /// `.pre-commit-hooks.yaml` at the tag, verbatim (display names and filters differ per tag, 09 §5.3).
    pub manifest_yaml: &'static str,
    /// (hook id, class, epoch) per manifest hook, in manifest order.
    pub hooks: &'static [(&'static str, HookClass, Epoch)],
}

/// Where a native hook comes from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NativeSource {
    /// A pre-commit-hooks repository (Tier A/B).
    PreCommitHooks,
    /// `repo: builtin`.
    Builtin,
}
