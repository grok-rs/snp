//! Differential harness against pre-commit 4.7.0 (tooling, not shipped): scenarios, materialiser, normaliser,
//! `brand_map`, divergence register, shrinker (ARCHITECTURE §1.4.25, §13.2; ADR-054, ADR-058).
//!
//! A [`Scenario`] (research 10 §7.4.3 plus `brands`, `expect_divergence`, `strict`, `expected`) is materialised at
//! one fixed absolute root for each tool in turn (identical paths, identical commit dates and therefore identical
//! SHAs); the [`Runner`] runs its steps under the pre-commit 4.7.0 oracle and under snp, snapshots every observable
//! effect after each step (streams, exit code, worktree bytes and modes, index, refs, stash list, config bytes,
//! hook scripts, the logical store) and compares them after the normalisation tiers of COMPATIBILITY §2
//! ([`normalise()`], [`Tier`]). snp-brand runs compare against the prog-patched oracle with the [`BrandMap`] applied to
//! every non-argparse text.
//!
//! The register tooling ([`register`]) joins the scenarios with the per-ID divergence files
//! (`tests/differential/divergences/<ID>.yaml`) and COMPATIBILITY §5: an unlisted difference fails, a listed one that
//! stops reproducing fails ([`Verdict::StaleDivergence`]), unfilled stubs are reported, and class-DIAG scenarios must
//! differ on stderr only. [`generate`] produces the nightly random suite; [`shrink`] reduces a diverging scenario
//! (structural shrinking over the scenario model, then delta debugging over files, config lines and steps) and
//! writes a self-contained `repro.sh`.
//!
//! Owner: WP-35.

mod compare;
mod exec;
mod expect;
pub mod generate;
mod materialise;
mod minire;
mod normalise;
pub mod register;
mod runner;
mod scenario;
pub mod shrink;
mod snapshot;
mod text;
mod yamlv;

use std::path::PathBuf;

use snp_testkit as _;

pub use compare::Difference;
pub use exec::Captured;
pub use normalise::{
    BrandMap, DIAG_LINES, Rule, Tier, canonical_usage, mask_paths, normalise, strip_diag,
};
pub use runner::{
    Options, Outcome, Runner, SideReport, StepReport, Variant, VariantReport, Verdict,
};
pub use scenario::{
    Brand, Compare, ExitMode, Expected, Op, RepoSpec, Requires, Scenario, SignalSpec, Status, Step,
    StepExpect, StepKind, StreamMode, VersionReq, Work, load_scenarios, parse_version,
};
pub use snapshot::{Entry, State};
pub use yamlv::{YamlLoadError, Yv};

/// The workspace root of this checkout (the harness reads its committed data files from there).
pub fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(std::path::Path::parent)
        .map_or_else(|| manifest.clone(), std::path::Path::to_path_buf)
}
