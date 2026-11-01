//! `run` (`commands/run.py`; §3.1). `run()` contains no `?` and no early `return` (§4.4, `xtask lint-guards`): the
//! pipeline's single exit restores the stash explicitly before the one return path.
//!
//! Owner: WP-32.

use snp_cli::RunArgs;
use snp_core::failure::Failure;

use crate::RunCtx;
use crate::pipeline::{self, Invocation};

/// `pre-commit run`: single exit (§4.4).
///
/// The whole of ARCHITECTURE §3.1 steps 3–21: discovery and `_adjust_args_and_chdir` (pure), config tracking, the
/// next-run recovery notice, the TTY-only run hint (D-DIAG-03), the speculative config load beside the preflight
/// git calls, the preflight in pre-commit's order, `run_env`, the stash guard, the config/overlay, resolution,
/// selection, `SKIP` and the skip/only conditions, environment installs, file selection and classification, the
/// baseline digest, the hooks, `--show-diff-on-failure`, `stage_fixed` staging and the restore.
#[expect(
    clippy::needless_pass_by_value,
    reason = "frozen signature (ARCHITECTURE §1.4.21)"
)]
pub fn run(a: RunArgs, cx: &RunCtx) -> Result<i32, Failure> {
    let inv = Invocation {
        store: &cx.store,
        track_config: true,
        direct: true,
        fixed_paths: false,
        hook_stdin: None,
    };
    let outcome = pipeline::run_pipeline(&a, cx, &inv);
    cx.printer.flush();
    outcome
}
