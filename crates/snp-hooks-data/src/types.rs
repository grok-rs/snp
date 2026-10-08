//! Data types of the generated tables.
//!
//! Owner: WP-17.

/// Behaviour epoch of a pre-commit-hooks module (09 §5.2).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(pub u16);

/// How snp runs a pre-commit-hooks hook (09 §1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookClass {
    /// Native port, byte-exact.
    Native,
    /// Native port with documented caveats.
    NativeWithCaveats,
    /// Delegated to the upstream Python hook.
    Delegate,
    /// `language: fail` in the manifest.
    FailLanguage,
    /// Not implemented upstream (stub entry).
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

/// One pre-commit-hooks tag (Tier A).
#[derive(Debug)]
pub struct TagProfile {
    /// The tag, e.g. "v6.0.0".
    pub tag: &'static str,
    /// The tag's commit (40 hex).
    pub commit: &'static str,
    /// `.pre-commit-hooks.yaml` at the tag, verbatim.
    pub manifest_yaml: &'static str,
    /// (hook id, class, epoch) per manifest hook.
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
