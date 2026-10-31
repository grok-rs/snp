//! The reference interpreter and pre-commit's interpreter choices (`python.py:66-165`; 04a §2.2, §11.4 step 2,
//! Q16).
//!
//! pre-commit's choices are relative to the interpreter running it (`sys.executable`, `sys.version_info`,
//! `sys.implementation`); snp has none, so a **reference interpreter** stands in for it: the first of `python3`,
//! `python` on `PATH` (Windows: the `py -3` launcher, then `python`), queried once with `-S -c`.
//!
//! Owner: WP-24.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use snp_core::env::Env;

use super::shared::stdout_text;
use crate::{Host, LangError};

/// What the reference interpreter reports about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefInterp {
    /// `sys.implementation.name` (`cpython`, `pypy`, …).
    pub(crate) implementation: String,
    /// `".".join(str(p) for p in sys.version_info)`, e.g. `3.12.3.final.0`.
    pub(crate) version_info: String,
    /// `sys.executable`.
    pub(crate) executable: PathBuf,
}

impl RefInterp {
    /// `sys.version_info[:3]` (missing or non-numeric parts are 0).
    pub(crate) fn version(&self) -> [u64; 3] {
        let mut out = [0_u64; 3];
        for (slot, part) in out.iter_mut().zip(self.version_info.split('.')) {
            *slot = part.parse().unwrap_or(0);
        }
        out
    }

    /// `{X}.{Y}`.
    pub(crate) fn minor(&self) -> String {
        let [x, y, _] = self.version();
        format!("{x}.{y}")
    }
}

/// The probe program: implementation name, `version_info` and `sys.executable`, one per line.
pub(crate) const REF_PROG: &str = "import sys;print(sys.implementation.name);print('.'.join(str(p) for p in sys.version_info));print(sys.executable)";

/// `_version_info`'s program (`python.py:27-33`).
pub(crate) const VERSION_INFO_PROG: &str =
    "import sys;print(\".\".join(str(p) for p in sys.version_info))";

/// The reference-interpreter candidates, in order: `(argv prefix)`.
fn candidates() -> &'static [&'static [&'static str]] {
    if cfg!(windows) {
        &[&["py", "-3"], &["python"]]
    } else {
        &[&["python3"], &["python"]]
    }
}

/// Parses the three lines of [`REF_PROG`].
pub(crate) fn parse_ref(stdout: &[u8]) -> Option<RefInterp> {
    let text = String::from_utf8_lossy(stdout);
    let mut lines = snp_pycompat::text::splitlines(&text);
    let implementation = snp_pycompat::text::strip(lines.next()?).to_owned();
    let version_info = snp_pycompat::text::strip(lines.next()?).to_owned();
    let executable = snp_pycompat::text::strip(lines.next()?).to_owned();
    if implementation.is_empty() || version_info.is_empty() || executable.is_empty() {
        return None;
    }
    Some(RefInterp {
        implementation,
        version_info,
        executable: PathBuf::from(executable),
    })
}

/// Queries the reference interpreter (no memo: [`crate::ToolProbe::default_version`] memoises the default version,
/// and installs query it once each). `None` when no candidate exists or none answers.
pub(crate) fn reference(h: &Host<'_>) -> Result<Option<RefInterp>, LangError> {
    for cand in candidates() {
        let Some(first) = cand.first() else {
            continue;
        };
        if h.probe
            .find_exe(OsStr::new(first), h.base_env, h.cwd)
            .is_none()
        {
            continue;
        }
        let mut argv: Vec<OsString> = cand.iter().map(OsString::from).collect();
        argv.extend(["-S", "-c", REF_PROG].map(OsString::from));
        let out = h.runner.run(&argv, h.base_env, h.cwd)?;
        if out.status.0 != 0 {
            continue;
        }
        if let Some(r) = parse_ref(&out.stdout) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// `_impl_exe_name()` (`python.py:76-80`): `python` for CPython, else the implementation name.
pub(crate) fn impl_exe_name(implementation: &str) -> &str {
    if implementation == "cpython" {
        "python"
    } else {
        implementation
    }
}

/// `_find_by_sys_executable()` (`python.py:83-105`) for `exe` and its realpath: the lower-cased basename with
/// everything from `.exe` dropped, unless it is `python`/`pythonw`, when `find(name)` succeeds.
pub(crate) fn find_by_sys_executable(
    exe: &OsStr,
    realpath: &OsStr,
    find: &mut dyn FnMut(&str) -> bool,
) -> Option<String> {
    for path in [exe, realpath] {
        let lower = path.to_string_lossy().to_lowercase();
        let base = snp_pycompat::ospath::basename(OsStr::new(&lower));
        let base = base.to_string_lossy();
        let (name, _, _) = snp_pycompat::text::partition(&base, ".exe");
        if name != "python" && name != "pythonw" && find(name) {
            return Some(name.to_owned());
        }
    }
    None
}

/// `get_default_version()` (`python.py:108-131`) with the facts injected: `impl{X.Y}`, `impl{X}` on `PATH`, then the
/// `sys.executable` names, then (Windows) `python{X.Y}` through the `py` launcher, else `default`.
pub(crate) fn default_from(
    r: &RefInterp,
    realpath: &OsStr,
    find: &mut dyn FnMut(&str) -> bool,
    py_launcher: &mut dyn FnMut(&str) -> bool,
) -> String {
    let [major, _, _] = r.version();
    let minor = r.minor();
    let name = impl_exe_name(&r.implementation);
    for potential in [minor.clone(), major.to_string()] {
        let exe = format!("{name}{potential}");
        if find(&exe) {
            return exe;
        }
    }
    if let Some(exe) = find_by_sys_executable(r.executable.as_os_str(), realpath, find) {
        return exe;
    }
    if cfg!(windows) {
        let exe = format!("python{minor}");
        if py_launcher(&exe) {
            return exe;
        }
    }
    "default".to_owned()
}

/// `_sys_executable_matches(version)` (`python.py:134-145`) against the reference interpreter's `version_info`:
/// `python` always matches; otherwise `python` followed by dot-separated ints (Python `int()` rules) that equal the
/// first components of `version_info` — at most three, since the fourth component is the release level string.
pub(crate) fn sys_executable_matches(version: &str, version_info: &str) -> bool {
    if version == "python" {
        return true;
    }
    let Some(rest) = version.strip_prefix("python") else {
        return false;
    };
    let Ok(info) = snp_pycompat::int::parse_version(rest) else {
        return false;
    };
    let actual: Vec<i128> = version_info
        .split('.')
        .take(3)
        .map(|p| p.parse::<i128>().unwrap_or(-1))
        .collect();
    info.len() <= 3 && actual.get(..info.len()) == Some(info.as_slice())
}

/// `_find_by_py_launcher(version)` (`python.py:64-73`, Windows): `py -{X.Y} -c 'import sys; print(sys.executable)'`
/// with `PYTHONIOENCODING=UTF-8`; the printed path on success.
pub(crate) fn py_launcher(h: &Host<'_>, version: &str) -> Result<Option<PathBuf>, LangError> {
    let Some(num) = version.strip_prefix("python") else {
        return Ok(None);
    };
    let argv: Vec<OsString> = [
        format!("-{num}"),
        "-c".to_owned(),
        "import sys; print(sys.executable)".to_owned(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let mut full = vec![OsString::from("py")];
    full.extend(argv);
    let env = super::shared::with_var(h.base_env, "PYTHONIOENCODING", "UTF-8");
    let out = h.runner.run(&full, &env, h.cwd)?;
    if out.status.0 != 0 {
        return Ok(None);
    }
    let text = stdout_text(&out.stdout);
    Ok((!text.is_empty()).then(|| PathBuf::from(text)))
}

/// The `uv python` request for a `language_version` that names no interpreter on this host (04a §11.4 step 2):
/// `pythonX.Y` → `X.Y`, `pypyX.Y` → `pypy@X.Y`, anything else verbatim (`3.11`, `cpython3.11`, `3.11.4`).
pub(crate) fn uv_request(version: &str) -> String {
    let numeric = |s: &str| {
        !s.is_empty()
            && s.split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    if let Some(rest) = version.strip_prefix("python")
        && numeric(rest)
    {
        return rest.to_owned();
    }
    if let Some(rest) = version.strip_prefix("pypy")
        && numeric(rest)
    {
        return format!("pypy@{rest}");
    }
    version.to_owned()
}

/// An absolute form of `p` (relative to `cwd`).
pub(crate) fn absolute(p: &Path, cwd: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// `os.path.expanduser(version)` over the run's environment (`norm_version`'s last step).
pub(crate) fn expanduser(version: &str, env: &Env) -> PathBuf {
    PathBuf::from(snp_os::home::expanduser(OsStr::new(version), env))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(implementation: &str, version_info: &str, exe: &str) -> RefInterp {
        RefInterp {
            implementation: implementation.to_owned(),
            version_info: version_info.to_owned(),
            executable: PathBuf::from(exe),
        }
    }

    /// `python_test.py:40-63` (`_get_default_version`): `find_executable` answers for the names in `found`.
    fn default_with(implementation: &str, exe: &str, found: &[&str], v: (u64, u64)) -> String {
        let rr = r(
            implementation,
            &format!("{}.{}.0.final.0", v.0, v.1),
            &format!("/fake/path/{exe}"),
        );
        let mut find = |s: &str| found.contains(&s);
        default_from(
            &rr,
            OsStr::new(&format!("/fake/path/{exe}")),
            &mut find,
            &mut |_| false,
        )
    }

    #[test]
    fn lang_python_default_version_sys_executable_found() {
        assert_eq!(
            default_with("cpython", "python3.12", &["python3.12"], (3, 12)),
            "python3.12"
        );
    }

    #[test]
    fn lang_python_default_version_picks_specific_when_found() {
        assert_eq!(
            default_with("cpython", "python3", &["python3", "python3.12"], (3, 12)),
            "python3.12"
        );
    }

    #[test]
    fn lang_python_default_version_picks_pypy_versioned_exe() {
        assert_eq!(
            default_with("pypy", "python", &["pypy3.12", "python3"], (3, 12)),
            "pypy3.12"
        );
    }

    #[test]
    fn lang_python_default_version_picks_pypy_unversioned_exe() {
        assert_eq!(
            default_with("pypy", "python", &["pypy3", "python3"], (3, 12)),
            "pypy3"
        );
    }

    #[test]
    fn lang_python_default_version_nothing_found() {
        let d = default_with("cpython", "python", &[], (3, 12));
        assert_eq!(d, "default");
    }

    /// `python_test.py:121-131`.
    #[test]
    fn lang_python_sys_executable_matches() {
        for v in [
            "python3.9",
            "python3",
            "python",
            "python3.9.10",
            "python 3.9",
        ] {
            assert!(sys_executable_matches(v, "3.9.10.final.0"), "{v}");
        }
        for v in [
            "notpython",
            "python3.x",
            "python3.9.10.0",
            "python3.8",
            "python.",
            "python3.9.11",
        ] {
            assert!(!sys_executable_matches(v, "3.9.10.final.0"), "{v}");
        }
    }

    /// `python_test.py:134-148` (`test_find_by_sys_executable`).
    #[test]
    fn lang_python_find_by_sys_executable() {
        for (exe, real, expected) in [
            ("/usr/bin/python3", "/usr/bin/python3.7", Some("python3")),
            ("/usr/bin/python", "/usr/bin/python3.7", Some("python3.7")),
            ("/usr/bin/python", "/usr/bin/python", None),
            (
                "/usr/bin/python3.7m",
                "/usr/bin/python3.7m",
                Some("python3.7m"),
            ),
            ("v/bin/python", "v/bin/pypy", Some("pypy")),
        ] {
            let got = find_by_sys_executable(OsStr::new(exe), OsStr::new(real), &mut |_| true);
            assert_eq!(got.as_deref(), expected, "{exe}");
        }
    }

    #[test]
    fn ref_probe_output() {
        let got = parse_ref(b"cpython\n3.12.3.final.0\n/usr/bin/python3\n").unwrap();
        assert_eq!(got, r("cpython", "3.12.3.final.0", "/usr/bin/python3"));
        assert_eq!(got.version(), [3, 12, 3]);
        assert_eq!(got.minor(), "3.12");
        assert!(parse_ref(b"cpython\n").is_none());
        assert!(parse_ref(b"").is_none());
    }

    #[test]
    fn uv_requests() {
        assert_eq!(uv_request("python3.11"), "3.11");
        assert_eq!(uv_request("pypy3.10"), "pypy@3.10");
        assert_eq!(uv_request("3.11"), "3.11");
        assert_eq!(uv_request("cpython3.11"), "cpython3.11");
        assert_eq!(uv_request("python"), "python");
    }
}
