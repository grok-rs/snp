//! `cargo xtask difftest [--no-build] [--debug] [COMMAND] [ARGS…]`: drive the differential harness against the
//! oracles (ARCHITECTURE §13.2, research 10 §7.4.8).
//!
//! Builds the binaries under test (`snp`, `snp-probe`) and the harness (`snp-difftest`) in the release profile
//! (`--debug`: the dev profile; `--no-build`: use what is there), then runs `snp-difftest COMMAND ARGS…` with
//! `--snp-bin <target>/<profile>/snp` added unless given. COMMAND defaults to `run`; `run` without `--suite`,
//! `--scenario` or `--file` runs the curated suite. Examples:
//!
//! ```text
//! cargo xtask difftest --suite curated
//! cargo xtask difftest --suite generated --cases 2000 --seed "$GITHUB_RUN_ID"
//! cargo xtask difftest --scenario run/modified-file-returns-zero --keep
//! cargo xtask difftest --suite seed --self-check      # oracle against itself
//! cargo xtask difftest register --strict              # the divergence-register report
//! ```
//!
//! The oracles come from `cargo xtask oracle install`; the harness never looks `pre-commit` or `snp` up on `PATH`.
//!
//! Owner: WP-35.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context as _, bail};

use crate::workspace;

const COMMANDS: [&str; 5] = ["run", "list", "register", "generate", "shrink"];

/// The cargo target directory (honours `CARGO_TARGET_DIR` and `build.target-dir` through `cargo metadata`).
pub(crate) fn target_dir(root: &std::path::Path) -> anyhow::Result<PathBuf> {
    let meta = workspace::metadata(root)?;
    meta.get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .context("`cargo metadata` has no target_directory")
}

/// Builds `packages` in `profile` (`release` / `dev`).
pub(crate) fn build(
    root: &std::path::Path,
    profile: &str,
    packages: &[&str],
) -> anyhow::Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build").current_dir(root);
    if profile == "release" {
        cmd.arg("--release");
    }
    for p in packages {
        cmd.args(["-p", p]);
    }
    let status = cmd.status().context("running cargo build")?;
    if !status.success() {
        bail!("cargo build {packages:?} failed ({status})");
    }
    Ok(())
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut profile = "release";
    let mut no_build = false;
    let mut rest: Vec<OsString> = Vec::new();
    for a in args {
        match a.to_str() {
            Some("--no-build") => no_build = true,
            Some("--debug") => profile = "dev",
            _ => rest.push(a.clone()),
        }
    }
    let first = rest
        .first()
        .and_then(|a| a.to_str())
        .unwrap_or_default()
        .to_owned();
    let (command, rest) = if COMMANDS.contains(&first.as_str()) {
        (first, rest.get(1..).unwrap_or_default().to_vec())
    } else {
        (String::from("run"), rest)
    };
    if !no_build {
        build(&root, profile, &["snp", "snp-probe", "snp-difftest"])?;
    }
    let dir = target_dir(&root)?.join(if profile == "release" {
        "release"
    } else {
        "debug"
    });
    let exe = |n: &str| dir.join(format!("{n}{}", std::env::consts::EXE_SUFFIX));
    let mut cmd = Command::new(exe("snp-difftest"));
    cmd.arg(&command).current_dir(&root);
    let given = |flag: &str| rest.iter().any(|a| a == flag);
    if matches!(command.as_str(), "run" | "shrink") && !given("--snp-bin") {
        cmd.arg("--snp-bin").arg(exe("snp"));
    }
    if command == "run" && !given("--suite") && !given("--scenario") && !given("--file") {
        cmd.args(["--suite", "curated"]);
    }
    cmd.args(&rest);
    let status = cmd
        .status()
        .with_context(|| format!("running {}", exe("snp-difftest").display()))?;
    if !status.success() {
        bail!("snp-difftest {command} failed ({status})");
    }
    Ok(())
}
