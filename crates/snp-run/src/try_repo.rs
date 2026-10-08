//! `try-repo` (`commands/try_repo.py`; 03 §12, ADR-059).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-32"
)]

use snp_cli::TryRepoArgs;
use snp_core::failure::Failure;

use crate::RunCtx;

/// pre-commit's throwaway `Store(tempdir)` (03 §12): runs against `Store::ephemeral(tmp, Some(&cx.store))`, so the
/// `Initializing environment` line and the `Installing environment` triplet print on every try-repo; the temp
/// config is never tracked (the user's `-c` is tracked in the main store as for every git command); ADR-059.
pub fn try_repo(a: TryRepoArgs, cx: &RunCtx) -> Result<i32, Failure> {
    todo!()
}
