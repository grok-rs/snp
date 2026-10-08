//! `Language` trait, registry, shared helpers, `CmdRunner` seam and shim harness, and the 21 language backends
//! (planning, install, health, activation; never runs hooks) (ARCHITECTURE §1.4.10, §6).
//!
//! pre-commit's languages are Python modules with `ENVIRONMENT_DIR`, `get_default_version`, `health_check`,
//! `install_environment`, `in_env` and `run_hook` (`lang_base.py:27-60`). snp turns them into the [`Language`] trait
//! over zero-sized backends ([`backend`]): activation is data ([`snp_core::env::EnvPatch`], never `os.environ`),
//! running is a [`RunPlan`] the executor (snp-run) carries out, setup commands go through the [`CmdRunner`] seam,
//! probes are memoised by [`ToolProbe`], and the support checks ([`check_support`]) run before any filesystem
//! mutation (04b §5.1).
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
