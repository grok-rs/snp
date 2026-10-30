//! The binaries under test.
//!
//! Owner: WP-18.

use std::path::{Path, PathBuf};

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
    /// Finds `snp` and `snp-probe` in the target dir of the running test executable: a test binary lives in
    /// `target/<profile>/deps/`, the workspace binaries in `target/<profile>/` (also under `--target <triple>` and
    /// a custom `CARGO_TARGET_DIR`, which keep that shape). `None` when either is missing.
    pub fn locate() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Self::locate_from(exe.parent()?)
    }

    /// [`Bins::locate`] starting from the directory that holds the test executable.
    fn locate_from(dir: &Path) -> Option<Self> {
        let candidates = [Some(dir), dir.parent()];
        candidates.into_iter().flatten().find_map(|d| {
            let snp = d.join(format!("snp{}", std::env::consts::EXE_SUFFIX));
            let probe = d.join(format!("snp-probe{}", std::env::consts::EXE_SUFFIX));
            (snp.is_file() && probe.is_file()).then_some(Self { snp, probe })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_from_a_deps_dir() {
        let d = tempfile::tempdir().unwrap();
        let deps = d.path().join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        assert!(Bins::locate_from(&deps).is_none());
        let suffix = std::env::consts::EXE_SUFFIX;
        std::fs::write(d.path().join(format!("snp{suffix}")), b"").unwrap();
        assert!(
            Bins::locate_from(&deps).is_none(),
            "both binaries are needed"
        );
        std::fs::write(d.path().join(format!("snp-probe{suffix}")), b"").unwrap();
        let bins = Bins::locate_from(&deps).unwrap();
        assert_eq!(bins.snp, d.path().join(format!("snp{suffix}")));
        assert_eq!(bins.probe, d.path().join(format!("snp-probe{suffix}")));
        assert!(Bins::locate_from(d.path()).is_some());
    }
}
