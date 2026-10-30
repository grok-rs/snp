//! The `dont_write_to_home_directory` guard (10 §2.2, §7.3.2): the real `~/.cache/pre-commit` and `~/.cache/snp`
//! (and their `$XDG_CACHE_HOME` forms) are untouched while the kit runs git fixtures, the oracle and snp; every store
//! write lands under the test root.
//!
//! Owner: WP-18.

#[cfg(test)]
mod guard {
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    use snp_core::env::Env;
    use snp_testkit::{Bins, Fixture, Oracle, TestEnv};

    /// What would change if anything wrote there: existence, mtime and the entry list.
    fn stamp(p: &Path) -> Option<(SystemTime, Vec<PathBuf>)> {
        let md = std::fs::metadata(p).ok()?;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(p)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        entries.sort();
        Some((md.modified().ok()?, entries))
    }

    fn real_caches() -> Vec<PathBuf> {
        let process = Env::capture_process();
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(home) = process.get("HOME").or_else(|| process.get("USERPROFILE")) {
            roots.push(PathBuf::from(home).join(".cache"));
        }
        if let Some(xdg) = process.get_nonempty("XDG_CACHE_HOME") {
            roots.push(PathBuf::from(xdg));
        }
        let mut out: Vec<PathBuf> = roots
            .iter()
            .flat_map(|r| [r.join("pre-commit"), r.join("snp")])
            .collect();
        for k in ["PRE_COMMIT_HOME", "SNP_HOME"] {
            if let Some(v) = process.get_nonempty(k) {
                out.push(PathBuf::from(v));
            }
        }
        out
    }

    #[test]
    fn real_caches_are_untouched_by_the_suite() {
        let watched = real_caches();
        let before: Vec<_> = watched.iter().map(|p| stamp(p)).collect();

        let env = TestEnv::new(Bins::locate().unwrap_or_else(|| Bins {
            snp: PathBuf::from("/nonexistent/snp"),
            probe: PathBuf::from("/nonexistent/snp-probe"),
        }));
        let repo = env.fixture_repo(Fixture::ConsumingRepo("script_hooks_repo"));
        std::fs::write(repo.join("foo.py"), b"").unwrap();
        // the reference implementation run in the kit's env writes its store under the root only
        // the oracle half runs whenever the venv exists; `SNP_REQUIRE_ORACLE=1` (CI jobs that install it) turns a
        // missing venv into a failure instead of a reported skip
        let oracle = env.oracle(Oracle::Pc462Py312, &repo);
        if oracle.is_none() {
            let required = Env::capture_process()
                .get_nonempty("SNP_REQUIRE_ORACLE")
                .is_some();
            assert!(
                !required,
                "SNP_REQUIRE_ORACLE is set but the pc462-py312 oracle is not installed (cargo xtask oracle install)"
            );
            eprintln!(
                "guard: oracle half SKIPPED (pc462-py312 not installed; set SNP_REQUIRE_ORACLE=1 to require it)"
            );
        }
        if let Some(cmd) = oracle {
            let out = cmd.args(["run", "--all-files"]).output();
            assert!(
                out.code == 0 || out.code == 1,
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(
                env.root()
                    .join("store")
                    .read_dir()
                    .unwrap()
                    .next()
                    .is_some(),
                "PRE_COMMIT_HOME was used"
            );
        }
        if let Some(bins) = Bins::locate() {
            drop(env.snp(&repo).arg("--version").output());
            drop(
                env.snp_as_pre_commit(&repo)
                    .args(["run", "--all-files"])
                    .output(),
            );
            assert!(bins.snp.is_file());
        }
        let after: Vec<_> = watched.iter().map(|p| stamp(p)).collect();
        for ((p, b), a) in watched.iter().zip(&before).zip(&after) {
            assert_eq!(b, a, "{} changed during the test run", p.display());
        }
        // and the kit's own env points every store and HOME inside the root
        for k in [
            "HOME",
            "XDG_CACHE_HOME",
            "PRE_COMMIT_HOME",
            "SNP_HOME",
            "TMPDIR",
        ] {
            let v = PathBuf::from(env.env().get(k).unwrap());
            assert!(
                v.starts_with(env.root()),
                "{k}={} escapes the root",
                v.display()
            );
        }
    }
}
