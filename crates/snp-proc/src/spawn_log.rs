//! Test instrumentation (feature `testing`, enabled only through `crates/snp`'s `testing` feature): with
//! `SNP_TEST_SPAWN_LOG=<file>` every spawn appends `<role> <group> <argv0 basename>\n` (spawn-budget tests, WP-32/WP-36).
//!
//! Owner: WP-15.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use snp_core::env::Env;

use crate::spawn::{Group, Role};

/// The armed log file.
static LOG: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Arms the log from `SNP_TEST_SPAWN_LOG` in the captured environment.
///
/// An unset or empty variable disarms it.
pub fn enable_from_env(env: &Env) {
    *snp_core::sync::lock(&LOG) = env.get_nonempty("SNP_TEST_SPAWN_LOG").map(PathBuf::from);
}

/// The logged lines of `path`.
///
/// A missing or unreadable file reads as no lines.
pub fn read(path: &Path) -> Vec<String> {
    std::fs::read(path).map_or_default(|b| {
        String::from_utf8_lossy(&b)
            .lines()
            .map(str::to_owned)
            .collect()
    })
}

/// Appends `<role> <group> <argv0 basename>` when armed (one `write` with `O_APPEND`: lines of concurrent spawns
/// never interleave).
pub(crate) fn record(role: Role, group: Group, argv0: &OsStr) {
    let Some(path) = snp_core::sync::lock(&LOG).clone() else {
        return;
    };
    let role = match role {
        Role::Hook => "hook",
        Role::Tool => "tool",
        Role::Critical => "critical",
    };
    let group = match group {
        Group::Inherit => "inherit",
        Group::Isolated => "isolated",
    };
    let base = Path::new(argv0)
        .file_name()
        .unwrap_or(argv0)
        .to_string_lossy();
    let line = format!("{role} {group} {base}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ignored = f.write_all(line.as_bytes());
    }
}
