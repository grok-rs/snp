//! The binaries under test.
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]

use std::path::PathBuf;

/// Binaries under test. Library code cannot read `CARGO_BIN_EXE_*` (it exists only in the owning package's tests):
/// callers pass `env!("CARGO_BIN_EXE_snp")`, or `Bins::locate()` finds them next to the test executable in the target
/// dir (CI runs `cargo build --workspace --bins` first).
#[derive(Debug, Clone)]
pub struct Bins {
    /// The `snp` binary.
    pub snp: PathBuf,
    /// The `snp-probe` binary.
    pub probe: PathBuf,
}

impl Bins {
    /// Finds `snp` and `snp-probe` in the target dir of the running test executable.
    pub fn locate() -> Option<Self> {
        todo!()
    }
}
