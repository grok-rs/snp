//! The `r` language backend (§6.2; `pre_commit/languages/r.py`).
//!
//! Owner: WP-25.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-25")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-25")]

use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: r`.
#[derive(Debug)]
pub struct R;

impl Language for R {
    fn id(&self) -> LanguageId {
        LanguageId::R
    }

    fn support(&self) -> Support {
        todo!()
    }

    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError> {
        todo!()
    }

    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        todo!()
    }

    fn health(
        &self,
        cx: &EnvCtx<'_>,
        recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        todo!()
    }

    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        todo!()
    }

    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        todo!()
    }
}
