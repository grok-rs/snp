//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.25 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-35 (created by WP-00).

#[cfg(test)]
mod api {
    use std::path::{Path, PathBuf};

    use snp_difftest::{BrandMap, Runner, Scenario, Tier, Verdict};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Scenario>();
        send_sync::<Runner>();
        send_sync::<Verdict>();
        send_sync::<Tier>();
        send_sync::<BrandMap>();
    }

    #[test]
    fn harness() {
        has::<fn(&Path) -> Result<Vec<Scenario>, String>>(snp_difftest::load_scenarios);
        has::<Runner>(Runner {
            snp_bin: PathBuf::from("target/debug/snp"),
            oracle_bin: PathBuf::from("target/oracle/pc470-py312/bin/pre-commit"),
            oracle_snp_prog: PathBuf::from("target/oracle/pc470-py312/bin/pre-commit-snp-prog"),
            root: PathBuf::from("/tmp/snp-difftest"),
        });
        has::<[Verdict; 3]>([
            Verdict::Same,
            Verdict::Diverged {
                id: Some(String::from("D-DIAG-02")),
                diff: String::new(),
            },
            Verdict::StaleDivergence(String::from("D-DIAG-02")),
        ]);
        has::<fn(&Runner, &Scenario) -> Verdict>(Runner::run);
        has::<[Tier; 3]>([Tier::A, Tier::B, Tier::C]);
        has::<fn(Tier, &[u8], &BrandMap) -> Vec<u8>>(snp_difftest::normalise);
    }
}
