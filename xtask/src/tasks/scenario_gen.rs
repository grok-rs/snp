//! `cargo xtask scenario-gen [--cases N] [--seed S] [--out DIR] [--no-probe]`: write scenarios of the nightly
//! generated suite as YAML files (research 10 §7.4.6 item 2), e.g. to inspect a seed or to promote a shrunk
//! failure into the curated corpus (`tests/differential/scenarios/regressions/`).
//!
//! The generator is `snp_difftest::generate` (deterministic per `(seed, case)`); this task builds `snp-difftest`
//! (dev profile) and runs `snp-difftest generate`. Defaults: 20 cases, seed 0, `target/difftest/generated`.
//!
//! Owner: WP-35.

use std::ffi::OsString;
use std::process::Command;

use anyhow::{Context as _, bail};

use crate::tasks::difftest::{build, target_dir};
use crate::workspace;

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut cases = String::from("20");
    let mut seed = String::from("0");
    let mut out = root.join("target/difftest/generated").into_os_string();
    let mut extra: Vec<OsString> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--cases") => {
                cases = it
                    .next()
                    .context("--cases needs a number")?
                    .to_string_lossy()
                    .into_owned();
            }
            Some("--seed") => {
                seed = it
                    .next()
                    .context("--seed needs a value")?
                    .to_string_lossy()
                    .into_owned();
            }
            Some("--out") => out.clone_from(it.next().context("--out needs a directory")?),
            Some("--no-probe") => extra.push(a.clone()),
            _ => bail!(
                "usage: cargo xtask scenario-gen [--cases N] [--seed S] [--out DIR] [--no-probe]"
            ),
        }
    }
    build(&root, "dev", &["snp-difftest"])?;
    let exe = target_dir(&root)?
        .join("debug")
        .join(format!("snp-difftest{}", std::env::consts::EXE_SUFFIX));
    let status = Command::new(&exe)
        .args(["generate", "--cases", &cases, "--seed", &seed, "--out"])
        .arg(&out)
        .args(&extra)
        .current_dir(&root)
        .status()
        .with_context(|| format!("running {}", exe.display()))?;
    if !status.success() {
        bail!("snp-difftest generate failed ({status})");
    }
    Ok(())
}
