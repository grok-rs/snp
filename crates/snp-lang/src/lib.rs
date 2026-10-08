//! `Language` trait, registry, shared helpers, `CmdRunner` seam and shim harness, and the 21 language backends
//! (planning, install, health, activation; never runs hooks) (ARCHITECTURE §1.4.10, §6).
//!
//! Owner: WP-16 (`lib.rs`, `framework/`, `common/`, `harness/`, `langs/mod.rs`, `langs/envless.rs`); the
//! environment languages under `langs/` belong to WP-24 (group 1) and WP-25 (group 2).

pub mod common;
mod framework;
#[cfg(feature = "testing")]
pub mod harness;
pub mod langs;

pub use framework::{
    CmdRunner, EnvCtx, Fingerprint, Health, Host, InstallCtx, InstallOutcome, InstallState,
    LOCAL_TEMPLATE, LangError, Language, PlanCtx, PreStep, ProcRunner, RunPlan, Support, ToolDirs,
    ToolProbe, VersionSupport, backend, check_support,
};
/// Single definition in `snp_core::profile` (snp-store keys use it).
pub use snp_core::profile::LOCAL_TEMPLATE_VERSION;
