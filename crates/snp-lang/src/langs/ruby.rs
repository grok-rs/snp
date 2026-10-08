//! The `ruby` language backend (§6.2; `pre_commit/languages/ruby.py`).
//!
//! Owner: WP-24.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-24")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-24")]

use std::sync::Arc;

use flate2 as _;
use sha2 as _;
use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;
use tar as _;

use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: ruby`.
#[derive(Debug)]
pub struct Ruby;

impl Language for Ruby {
    fn id(&self) -> LanguageId {
        LanguageId::Ruby
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
