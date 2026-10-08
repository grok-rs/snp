//! The `Language` framework: trait, seams, install state, plans (§1.4.10).
//!
//! Owner: WP-16.

mod context;
mod error;
mod language;
mod plan;
mod probe;
mod seams;
mod state;

use blake3 as _;

pub use context::{EnvCtx, Host, InstallCtx, PlanCtx};
pub use error::LangError;
pub use language::{LOCAL_TEMPLATE, Language, Support, VersionSupport, backend, check_support};
pub use plan::{PreStep, RunPlan};
pub use probe::ToolProbe;
pub use seams::{CmdRunner, ProcRunner, ToolDirs};
pub use state::{Fingerprint, Health, InstallOutcome, InstallState};
