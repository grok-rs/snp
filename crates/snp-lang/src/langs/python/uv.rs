//! Finding uv (04a §11.4 step 3, Q5; ADR-033): `$SNP_UV` → `uv` next to the snp executable → `uv` on `PATH` when it
//! is at least [`UV_FLOOR`] and its `uv venv` knows every flag snp passes (probed once per process and binary) →
//! the pinned release ([`snp_net::tools::UV_PINNED`], sha256 embedded per target) in `tools-v1/uv`.
//!
//! Owner: WP-24.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use snp_net::tools::{self, Tool};

use super::shared::{ensure_tool, stdout_text};
use crate::{Fingerprint, Host, LangError};

/// The oldest uv on `PATH` snp uses (ADR-033: `--no-config`, `--seed`, `--no-python-downloads` and
/// `uv python find` behave as snp needs from 0.8.0 on).
pub(crate) const UV_FLOOR: [u64; 3] = [0, 8, 0];

/// The `uv venv` flags snp passes; a `PATH` uv whose help lacks one is not used.
const VENV_FLAGS: [&str; 4] = [
    "--seed",
    "--no-project",
    "--no-config",
    "--no-python-downloads",
];

/// A usable uv.
#[derive(Debug, Clone)]
pub(crate) struct Uv {
    /// The executable.
    pub(crate) exe: PathBuf,
    /// The shared toolchain it comes from (`("uv", <pinned>)`), when snp downloaded it.
    pub(crate) tool: Option<(Arc<str>, Arc<str>)>,
}

/// The executable name.
const fn exe_name() -> &'static str {
    if cfg!(windows) { "uv.exe" } else { "uv" }
}

/// One memoised probe: locked while its probe runs, so concurrent installs spawn once.
type Slot = Arc<Mutex<Option<bool>>>;

/// Probe results per binary for this process: (realpath, fingerprint) → usable. A process-wide memo (like
/// pre-commit's `lru_cache`), bounded by [`MEMO_CAP`] distinct binaries; it holds facts about files, never
/// configuration, and a changed binary (another fingerprint) is probed again. (ARCHITECTURE S6 wants this in
/// `ToolProbe`, which WP-24 does not own; see the WP-24 report.)
static PATH_UV: Mutex<Vec<(PathBuf, Fingerprint, Slot)>> = Mutex::new(Vec::new());

/// Distinct `PATH` uv binaries memoised per process; more are probed every time.
const MEMO_CAP: usize = 16;

/// The memo slot of `(real, fp)`, created when there is room.
fn slot(real: &Path, fp: &Fingerprint) -> Option<Slot> {
    let mut memo = snp_core::sync::lock(&PATH_UV);
    if let Some((_, _, s)) = memo.iter().find(|(p, f, _)| p == real && f == fp) {
        return Some(Arc::clone(s));
    }
    if memo.len() >= MEMO_CAP {
        return None;
    }
    let s: Slot = Arc::new(Mutex::new(None));
    memo.push((real.to_path_buf(), fp.clone(), Arc::clone(&s)));
    Some(s)
}

/// `uv X.Y.Z (…)` → `[X, Y, Z]`.
pub(crate) fn parse_version(stdout: &[u8]) -> Option<[u64; 3]> {
    let text = stdout_text(stdout);
    let mut words = text.split_whitespace();
    if words.next()? != "uv" {
        return None;
    }
    let v = words.next()?;
    let mut out = [0_u64; 3];
    let mut parts = v.split(['.', '-', '+']);
    for slot in &mut out {
        let p = parts.next()?;
        let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
        *slot = digits.parse().ok()?;
    }
    Some(out)
}

/// Whether the uv at `exe` (found on `PATH`) is new enough and knows snp's flags; spawns `uv --version` and
/// `uv venv --help` once per process and binary.
fn path_uv_usable(h: &Host<'_>, exe: &Path) -> Result<bool, LangError> {
    let abs = if exe.is_absolute() {
        exe.to_path_buf()
    } else {
        h.cwd.join(exe)
    };
    let real = snp_pycompat::ospath::realpath(abs.as_os_str(), h.cwd).unwrap_or(abs);
    let memo = h.probe.fingerprint(&real).and_then(|fp| slot(&real, &fp));
    let Some(memo) = memo else {
        return probe_uv(h, exe);
    };
    let mut guard = snp_core::sync::lock(&memo);
    if let Some(hit) = *guard {
        return Ok(hit);
    }
    let usable = probe_uv(h, exe)?;
    *guard = Some(usable);
    Ok(usable)
}

/// `uv --version` and, when new enough, `uv venv --help`.
fn probe_uv(h: &Host<'_>, exe: &Path) -> Result<bool, LangError> {
    let argv = |args: &[&str]| -> Vec<OsString> {
        let mut v = vec![exe.as_os_str().to_os_string()];
        v.extend(args.iter().map(OsString::from));
        v
    };
    let out = h.runner.run(&argv(&["--version"]), h.base_env, h.cwd)?;
    let new_enough = out.status.0 == 0 && parse_version(&out.stdout).is_some_and(|v| v >= UV_FLOOR);
    let usable = new_enough && {
        let help = h
            .runner
            .run(&argv(&["venv", "--help"]), h.base_env, h.cwd)?;
        let text = String::from_utf8_lossy(&help.stdout);
        help.status.0 == 0 && VENV_FLAGS.iter().all(|f| text.contains(f))
    };
    Ok(usable)
}

/// The uv to use; `Ok(None)` when none is available (no `$SNP_UV`, no sibling, no usable `PATH` uv, and the pinned
/// release is not installed and cannot be downloaded). A failed download is an error only when `required`
/// (`SNP_PYTHON_INSTALLER=uv`); otherwise the pip backend is used.
pub(crate) fn find(h: &Host<'_>, required: bool) -> Result<Option<Uv>, LangError> {
    if let Some(p) = h.base_env.get_nonempty("SNP_UV") {
        return Ok(Some(Uv {
            exe: PathBuf::from(p),
            tool: None,
        }));
    }
    if let Some(dir) = h.self_exe.parent() {
        let sibling = dir.join(exe_name());
        if sibling.is_file() {
            return Ok(Some(Uv {
                exe: sibling,
                tool: None,
            }));
        }
    }
    if let Some(found) = h.probe.find_exe(OsStr::new("uv"), h.base_env, h.cwd)
        && path_uv_usable(h, &found)?
    {
        return Ok(Some(Uv {
            exe: found,
            tool: None,
        }));
    }
    match ensure_tool(h, Tool::Uv, "uv", tools::UV_PINNED, tools::host_target()) {
        Ok(t) => Ok(Some(Uv {
            exe: t.dir.join(exe_name()),
            tool: Some((Arc::from("uv"), Arc::from(t.version))),
        })),
        Err(LangError::Interrupted) => Err(LangError::Interrupted),
        Err(e) if required => Err(e),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(
            parse_version(b"uv 0.8.8 (abc 2025-08-01)\n"),
            Some([0, 8, 8])
        );
        assert_eq!(parse_version(b"uv 0.10.2\n"), Some([0, 10, 2]));
        assert_eq!(parse_version(b"uv 0.8.0-rc.1\n"), Some([0, 8, 0]));
        assert_eq!(parse_version(b"ruff 0.8.8\n"), None);
        assert_eq!(parse_version(b"uv\n"), None);
        assert!(parse_version(b"uv 0.7.22\n").is_some_and(|v| v < UV_FLOOR));
    }
}
