//! Native `pygrep` (`languages/pygrep.py`; 04a §8.4/§12): compiled once per hook per run (01 Addendum §A.4.5).
//!
//! Owner: WP-26.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-26")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-26")]

use std::ffi::OsStr;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use snp_core::cancel::CancelToken;
use snp_pyre::RegexCache;

/// Parsed `args + [entry]` and the compiled `PyRegex` for that parse (manual `Debug`: shows the source and flags).
pub struct Compiled {
    /// W0 placeholder; WP-26 stores the parsed args and the `PyRegex`.
    _priv: (),
}

impl fmt::Debug for Compiled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compiled").finish_non_exhaustive()
    }
}

/// `Err((code, output))` = argparse error text (exit 2) or `re.error: …` line (exit 1) — reported once for the hook.
pub fn prepare(args: &[Arc<str>], entry: &str, columns: usize) -> Result<Compiled, (i32, Vec<u8>)> {
    todo!()
}

/// `argv` = `args + [entry] + files` of the partition, re-parsed exactly like `pygrep.py` (a file named like a flag
/// changes the parse, ADR-072); the pattern is recompiled through `cache` only when the parse differs from `c`.
pub fn run_partition(
    c: &Compiled,
    argv: &[&OsStr],
    root: &Path,
    cache: &RegexCache,
    cancel: &CancelToken,
) -> crate::Outcome {
    todo!()
}
