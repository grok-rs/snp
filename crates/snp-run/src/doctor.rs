//! `snp doctor --fast-path` rows (ADR-070).
//!
//! Owner: WP-32.

use std::ffi::OsStr;

use snp_core::failure::Failure;
use snp_core::output::CollectWarnings;

use crate::RunCtx;
use crate::common::{self, ResolveParts};

/// `snp doctor --fast-path`: loads the config and returns `snp_repo::decision_table` rows as (ok, text); the binary
/// wraps it in a `snp_cmd::DoctorCheck` (no snp → snp-config edge needed; ADR-070).
///
/// One row per hook: `{id}: {decision} ({reason})`; a row is `ok` unless the decision is an error. Config warnings
/// are not printed (doctor is read-only and quiet about the config).
pub fn doctor_fast_path(config: &OsStr, cx: &RunCtx) -> Result<Vec<(bool, String)>, Failure> {
    let (_git, info) = common::discover(cx)?;
    let root = info.root;
    let config = common::adjust_config(config, &cx.invocation_cwd, &root);
    let src = common::config_source(&config, &root);
    let sink = CollectWarnings::new();
    let cfg = common::load_config_with(cx, &src, &sink, &sink)?;
    let printer = common::null_printer(cx);
    let parts = ResolveParts::new(cx.env.clone(), root, &cx.store);
    let policy = common::fast_path_policy(&cfg, &cx.env, false, false);
    let rcx = parts.ctx(cx, &cfg, &printer, &sink, policy, None, &cx.cancel);
    Ok(snp_repo::decision_table(&cfg, &rcx)
        .into_iter()
        .map(|(id, decision, reason)| row(&id, &decision, &reason))
        .collect())
}

/// One doctor row.
fn row(id: &str, decision: &str, reason: &str) -> (bool, String) {
    let ok = !decision.starts_with("error");
    let text = if reason.is_empty() {
        format!("{id}: {decision}")
    } else {
        format!("{id}: {decision} ({reason})")
    };
    (ok, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows() {
        assert_eq!(
            row("check-yaml", "native", "Tier A v6.0.0"),
            (true, String::from("check-yaml: native (Tier A v6.0.0)"))
        );
        assert_eq!(
            row("x", "delegate", ""),
            (true, String::from("x: delegate"))
        );
        assert!(!row("y", "error", "unknown id").0);
    }
}
