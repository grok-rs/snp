//! Shared helpers of the snp-proc integration tests: the `snp-probe` binary, specs, and the self-re-executing
//! driver used for tests that must run in a process of their own (signals, rlimits, quiesce, RSS).
//!
//! snp-proc may not dev-depend on snp-testkit (ARCHITECTURE §1.2), so the probe is located next to the test binary
//! (`target/<profile>/snp-probe`) and built on demand.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_proc::spawn::{Group, Output, OutputBudget, Role, Spec, Stdin};

/// The `snp-probe` binary of this build (built with `cargo build -p snp-probe` when missing).
pub(crate) fn probe() -> PathBuf {
    static PROBE: OnceLock<PathBuf> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let exe = std::env::current_exe().unwrap();
            let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
            let probe = profile_dir.join(format!("snp-probe{}", std::env::consts::EXE_SUFFIX));
            if !probe.exists() {
                let profile = match profile_dir.file_name().and_then(|n| n.to_str()) {
                    Some("debug") | None => "dev".to_owned(),
                    Some(p) => p.to_owned(),
                };
                let cargo = option_env!("CARGO").unwrap_or("cargo");
                let status = Command::new(cargo)
                    .args(["build", "-p", "snp-probe", "--profile", &profile])
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .unwrap();
                assert!(status.success(), "building snp-probe failed");
            }
            probe
        })
        .clone()
}

/// `[probe, args…]`.
pub(crate) fn probe_argv(args: &[&str]) -> Vec<OsString> {
    let mut v = vec![probe().into_os_string()];
    v.extend(args.iter().map(OsString::from));
    v
}

/// A minimal hermetic child environment.
pub(crate) fn env() -> Env {
    Env::from_pairs([
        ("PATH", OsString::from("/usr/bin:/bin")),
        ("PRE_COMMIT_TEST_VAR", OsString::from("from-env")),
    ])
}

/// A Hook spec in snp's process group with an empty-pipe stdin.
pub(crate) fn hook<'a>(
    argv: &'a [OsString],
    env: &'a Env,
    cwd: &'a Path,
    cancel: &'a CancelToken,
) -> Spec<'a> {
    Spec {
        argv,
        env,
        cwd,
        stdin: Stdin::EmptyPipe,
        group: Group::Inherit,
        role: Role::Hook,
        deadline: None,
        cancel,
    }
}

/// A generous budget spilling into the OS temp dir.
pub(crate) fn budget() -> Arc<OutputBudget> {
    OutputBudget::new(64 << 20, std::env::temp_dir())
}

/// The bytes of an output.
pub(crate) fn bytes(o: &Output) -> Vec<u8> {
    let mut v = Vec::new();
    o.write_to(&mut v).unwrap();
    v
}

/// `elapsed` since `t0` in milliseconds (for messages).
pub(crate) fn ms(t0: Instant) -> u128 {
    t0.elapsed().as_millis()
}

/// The environment variable the driver mode keys on.
pub(crate) const DRIVER_VAR: &str = "SNP_PROC_TEST_DRIVER";

/// The scenario requested for this process, if it was started as a driver.
pub(crate) fn driver_scenario() -> Option<String> {
    #[expect(clippy::disallowed_methods, reason = "test driver switch")]
    std::env::var(DRIVER_VAR).ok()
}

/// A driver parameter.
pub(crate) fn driver_param(name: &str) -> Option<String> {
    #[expect(clippy::disallowed_methods, reason = "test driver parameters")]
    std::env::var(name).ok()
}

/// A command that re-runs this test binary as a driver: only the test `test_name` runs, with `scenario`.
pub(crate) fn driver_command(test_name: &str, scenario: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(DRIVER_VAR, scenario);
    cmd
}

/// Appends `key=value` to the driver's result file (`SNP_PROC_TEST_OUT`).
pub(crate) fn report(key: &str, value: impl std::fmt::Display) {
    use std::io::Write;
    let path = driver_param("SNP_PROC_TEST_OUT").expect("SNP_PROC_TEST_OUT");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    writeln!(f, "{key}={value}").unwrap();
}

/// Parses a result file into `(key, value)` pairs.
pub(crate) fn results(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .collect()
}

/// The value of `key` in `results`.
pub(crate) fn result<'a>(results: &'a [(String, String)], key: &str) -> Option<&'a str> {
    results
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Waits until `path` exists (bounded).
#[cfg(unix)]
pub(crate) fn wait_for(path: &Path, secs: u64) -> bool {
    let t0 = Instant::now();
    while t0.elapsed().as_secs() < secs {
        if path.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// The lines of a trap file (missing → none).
#[cfg(unix)]
pub(crate) fn trapped(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}
