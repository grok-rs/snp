//! Build-time tooling for the snp workspace (ARCHITECTURE §1.1), never shipped: `cargo xtask <task> [args…]`, one
//! file per task under `xtask/src/tasks/`.
//!
//! Owner: WP-00 (dispatcher); tasks are owned by the packages named in their files.

mod tasks;

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use snp_core as _;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(name) = args.next() else {
        return usage();
    };
    let rest: Vec<OsString> = args.collect();
    let Some(task) = tasks::find(&name) else {
        return usage();
    };
    match (task.run)(&rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            writeln!(io::stderr(), "xtask {} ({}): {e:#}", task.name, task.owner)
                .unwrap_or_default();
            ExitCode::FAILURE
        }
    }
}

/// Lists the tasks on stderr; exit 2.
fn usage() -> ExitCode {
    let mut err = io::stderr().lock();
    writeln!(err, "usage: cargo xtask <task> [args...]\n\ntasks:").unwrap_or_default();
    for t in tasks::TASKS {
        writeln!(err, "    {:<18} ({})", t.name, t.owner).unwrap_or_default();
    }
    ExitCode::from(2)
}
