//! Differential harness against pre-commit 4.6.2 (tooling, not shipped): scenarios, materialiser, normaliser,
//! `brand_map`, divergence register, shrinker (ARCHITECTURE §1.4.25, §13.2; ADR-054, ADR-058).
//!
//! Owner: WP-35.

mod normalise;
mod runner;
mod scenario;

use snp_core as _;
use snp_testkit as _;
use snp_yaml as _;

pub use normalise::{BrandMap, Tier, normalise};
pub use runner::{Runner, Verdict};
pub use scenario::{Scenario, load_scenarios};
