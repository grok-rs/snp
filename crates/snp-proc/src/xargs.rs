//! `pre_commit.xargs`: concurrency, command-length budget, partitioning and status aggregation (02 §11.4).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-15"
)]

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::Path;

use snp_core::env::Env;

use crate::spawn::PyStatus;

/// `py_cpu_count()`: `sched_getaffinity` on Linux (14 A.4.3).
pub fn cpu_count() -> NonZeroUsize {
    todo!()
}

/// `PRE_COMMIT_NO_CONCURRENCY` present → 1; `TRAVIS` present → 2; else `cpu`; 1 when `require_serial`.
pub fn target_concurrency(env: &Env, require_serial: bool, cpu: NonZeroUsize) -> NonZeroUsize {
    todo!()
}

/// POSIX clamp(`ARG_MAX` − 2048 − env, 4096, 131072); Windows 30720.
pub fn platform_max_length(startup_env_size: usize) -> usize {
    todo!()
}

/// Windows .bat/.cmd budget (8192 − len − 4 − 1024).
pub fn batch_max_length(cmd_exe: &Path) -> usize {
    todo!()
}

/// `_command_length` unit: bytes (Unix) / UTF-16 units (Windows).
pub fn cmdlen(s: &OsStr) -> usize {
    todo!()
}

/// `xargs.ArgumentTooLongError` with the offending argument.
#[derive(Debug)]
pub struct ArgumentTooLong(pub OsString);

/// Exact port of `xargs.partition`: index ranges into `files`; ≥ 1 partition (zero files → [0..0]).
pub fn partition(
    cmd: &[OsString],
    files: &[&OsStr],
    jobs: NonZeroUsize,
    max_len: usize,
) -> Result<Vec<Range<usize>>, ArgumentTooLong> {
    todo!()
}

/// `max(abs(rc))` over the partitions, first wins on ties.
pub fn aggregate(codes: impl IntoIterator<Item = PyStatus>) -> PyStatus {
    todo!()
}
