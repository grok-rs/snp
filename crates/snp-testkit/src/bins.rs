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
    /// Finds `snp` and `snp-probe` in the target dir of the running test executable. The workspace binaries are in
    /// `target/<profile>/`; a test binary is in `target/<profile>/deps/` (Cargo before 1.100) or
    /// `target/<profile>/build/<package>/<hash>/out/` (Cargo 1.100's build-dir layout), also under `--target
    /// <triple>` and a custom `CARGO_TARGET_DIR`, which keep that shape. `None` when either is missing.
    pub fn locate() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Self::locate_from(exe.parent()?)
    }

    /// [`Bins::locate`] starting from the directory that holds the test executable: it and its ancestors up to the
    /// profile directory of either layout.
    fn locate_from(dir: &Path) -> Option<Self> {
        dir.ancestors().take(5).find_map(|d| {
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
        // Cargo 1.100: `<profile>/build/<package>/<hash>/out/<test>`.
        let out = d
            .path()
            .join("build")
            .join("snp")
            .join("0123abcd")
            .join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert_eq!(Bins::locate_from(&out).unwrap().snp, bins.snp);
    }
}
