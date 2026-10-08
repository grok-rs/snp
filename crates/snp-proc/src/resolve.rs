//! `parse_shebang.find_executable` / `normexe` / `normalize_cmd` (15 §A4.11, ADR-014).
//!
//! W0 working baseline (ADR-071): exact ports with an explicit `cwd` (snp never chdirs, so every relative path is
//! checked against `cwd` where Python checks against the process cwd); `os.access(X_OK)` is file-identify's
//! `ExecPolicy::PythonCompat`. WP-15 owns and completes the module without changing a signature.
//!
//! Owner: WP-15.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use file_identify::{ExecPolicy, IdentifyError};
use snp_core::env::Env;
use snp_core::failure::pyerr;
use snp_pycompat::ospath;

/// `os.pathsep`.
const PATHSEP: u8 = if cfg!(windows) { b';' } else { b':' };

/// `os.sep in s` (`altsep` too when `with_altsep`, Windows only).
fn has_sep(s: &OsStr, with_altsep: bool) -> bool {
    let b = s.as_encoded_bytes();
    if cfg!(windows) {
        b.contains(&b'\\') || (with_altsep && b.contains(&b'/'))
    } else {
        b.contains(&b'/')
    }
}

/// `s.split(os.pathsep)` (`''.split(':') == ['']`).
fn split_pathsep(s: &OsStr) -> Vec<OsString> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = s.encode_wide().collect();
        wide.split(|u| *u == u16::from(PATHSEP))
            .map(OsString::from_wide)
            .collect()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes()
            .split(|b| *b == PATHSEP)
            .map(|p| OsStr::from_bytes(p).to_os_string())
            .collect()
    }
}

/// Where a (possibly relative) path is looked up: relative paths are relative to `cwd`.
fn at(cwd: &Path, p: &OsStr) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// `os.path.isfile(p) and os.access(p, os.X_OK)`.
fn is_executable_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|md| md.is_file() && ExecPolicy::PythonCompat.is_executable(&md))
}

/// `parse_shebang.find_executable(name, env=…)` with an explicit cwd.
///
/// `exe = normpath(exe)`; a name containing `os.sep` is returned unchecked; otherwise, directory-major over
/// `PATH.split(os.pathsep)` (unset → `['']`), each `PATHEXT` extension (whenever the key exists, any OS) and then the
/// bare name are joined and the first regular, executable file is returned as written (relative stays relative).
pub fn find_executable(name: &OsStr, env: &Env, cwd: &Path) -> Option<PathBuf> {
    let exe = ospath::normpath(name);
    if has_sep(&exe, false) {
        return Some(PathBuf::from(exe));
    }
    let mut candidates: Vec<OsString> = Vec::new();
    if let Some(pathext) = env.get("PATHEXT") {
        for ext in split_pathsep(pathext) {
            let mut c = exe.clone();
            c.push(&ext);
            candidates.push(c);
        }
    }
    candidates.push(exe);
    let path = env.get("PATH").unwrap_or_else(|| OsStr::new(""));
    for dir in split_pathsep(path) {
        for cand in &candidates {
            let joined = ospath::join(&dir, &[cand]);
            if is_executable_file(&at(cwd, &joined)) {
                return Some(PathBuf::from(joined));
            }
        }
    }
    None
}

/// `parse_shebang.ExecutableNotFoundError` cases.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExeError {
    /// "Executable `x` not found".
    NotFound(OsString),
    /// "Executable `x` is a directory".
    IsDirectory(OsString),
    /// "Executable `x` is not executable".
    NotExecutable(OsString),
    /// Reading the shebang failed (identify lets the `OSError` propagate).
    Io(io::Error),
}

impl ExeError {
    /// "Executable `x` not found" (no trailing \n).
    ///
    /// `ExecutableNotFoundError.to_output()` text; `Io` renders `str(OSError)` (13 X.2.1).
    pub fn message(&self) -> String {
        let (orig, what) = match self {
            Self::NotFound(o) => (o, "not found"),
            Self::IsDirectory(o) => (o, "is a directory"),
            Self::NotExecutable(o) => (o, "is not executable"),
            Self::Io(e) => return pyerr::os_error_text(e, None, None),
        };
        format!("Executable `{}` {what}", orig.to_string_lossy())
    }
}

/// `parse_shebang.normexe(orig, env=…)`.
///
/// A bare name (no `os.sep`, no `os.altsep`) goes through [`find_executable`]; a path is checked in Python's order:
/// directory → "is a directory", not a regular file → "not found", `os.access(X_OK)` false → "is not executable".
pub fn normexe(orig: &OsStr, env: &Env, cwd: &Path) -> Result<PathBuf, ExeError> {
    if !has_sep(orig, true) {
        return find_executable(orig, env, cwd)
            .ok_or_else(|| ExeError::NotFound(orig.to_os_string()));
    }
    let md = std::fs::metadata(at(cwd, orig));
    match md {
        Ok(md) if md.is_dir() => Err(ExeError::IsDirectory(orig.to_os_string())),
        Ok(md) if !md.is_file() => Err(ExeError::NotFound(orig.to_os_string())),
        Err(_) => Err(ExeError::NotFound(orig.to_os_string())),
        Ok(md) if !ExecPolicy::PythonCompat.is_executable(&md) => {
            Err(ExeError::NotExecutable(orig.to_os_string()))
        }
        Ok(_) => Ok(PathBuf::from(orig)),
    }
}

/// `parse_shebang.parse_filename`: `()` unless the file exists, else identify's `parse_shebang_from_file`.
fn parse_filename(exe: &Path, cwd: &Path) -> Result<Vec<OsString>, ExeError> {
    let full = at(cwd, exe.as_os_str());
    if std::fs::metadata(&full).is_err() {
        return Ok(Vec::new());
    }
    match file_identify::parse_shebang_from_file_with(&full, ExecPolicy::PythonCompat) {
        Ok(cmd) => Ok(cmd.into_iter().map(OsString::from).collect()),
        Err(IdentifyError::PathNotFound { source, .. } | IdentifyError::IoError { source }) => {
            Err(ExeError::Io(source))
        }
        Err(other) => Err(ExeError::Io(io::Error::other(other.to_string()))),
    }
}

/// normexe → shebang (`file_identify::parse_shebang_from_file_with`, empty for non-UTF-8 = identify parity) →
/// normexe again; Windows-only basename fallback for absolute shebang interpreters (D-PLAT-02).
///
/// An empty `cmd` (Python: `IndexError`) is `NotFound("")`.
pub fn normalize_cmd(cmd: &[OsString], env: &Env, cwd: &Path) -> Result<Vec<OsString>, ExeError> {
    let Some((first, rest)) = cmd.split_first() else {
        return Err(ExeError::NotFound(OsString::new()));
    };
    let exe = normexe(first, env, cwd)?;
    let shebang = parse_filename(&exe, cwd)?;
    let mut full: Vec<OsString> = shebang;
    full.push(exe.into_os_string());
    full.extend(rest.iter().cloned());
    let Some((head, tail)) = full.split_first() else {
        return Err(ExeError::NotFound(OsString::new()));
    };
    let exe = match normexe(head, env, cwd) {
        Ok(exe) => exe,
        Err(ExeError::NotFound(orig)) if cfg!(windows) && has_sep(&orig, true) => {
            // D-PLAT-02: `#!/bin/sh` on Windows falls back to `sh` on PATH (Git for Windows semantics).
            let base = ospath::basename(&orig);
            if base.is_empty() {
                return Err(ExeError::NotFound(orig));
            }
            normexe(&base, env, cwd).map_err(|_| ExeError::NotFound(orig))?
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    out.push(exe.into_os_string());
    out.extend(tail.iter().cloned());
    Ok(out)
}

/// `current_exe` → absolute, not canonicalised (03 H.8.1).
pub fn self_exe() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if exe.is_absolute() {
        Ok(exe)
    } else {
        std::path::absolute(exe)
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn file(p: &Path, mode: u32, content: &[u8]) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, content).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The V-FE fixture of 15 §A4.11.
    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        file(&r.join("bin1/tool"), 0o755, b"");
        file(&r.join("bin2/tool"), 0o644, b"");
        file(&r.join("rel/tool2"), 0o755, b"");
        file(&r.join("tool3"), 0o755, b"");
        file(&r.join("extd/tool.sh"), 0o755, b"");
        file(&r.join("extd/tool"), 0o755, b"");
        file(&r.join("bin1/both"), 0o755, b"");
        std::fs::create_dir_all(r.join("bin3/tool")).unwrap();
        d
    }

    fn fe(root: &Path, exe: &str, pairs: &[(&str, &str)]) -> Result<String, ()> {
        let rs = root.to_str().unwrap();
        let env = Env::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.replace("$ROOT", rs))),
        );
        find_executable(OsStr::new(exe), &env, root)
            .map(|p| p.to_str().unwrap().replace(rs, "$ROOT"))
            .ok_or(())
    }

    #[test]
    fn v_fe_find_executable_vectors() {
        let d = fixture();
        let r = d.path();
        let some = |s: &str| Ok(s.to_owned());
        assert_eq!(
            fe(r, "tool", &[("PATH", "$ROOT/bin2:$ROOT/bin1")]),
            some("$ROOT/bin1/tool")
        );
        assert_eq!(
            fe(r, "tool", &[("PATH", "$ROOT/bin3:$ROOT/bin1")]),
            some("$ROOT/bin1/tool")
        );
        assert_eq!(fe(r, "tool", &[("PATH", "$ROOT/bin2")]), Err(()));
        assert_eq!(fe(r, "tool2", &[("PATH", "rel")]), some("rel/tool2"));
        assert_eq!(fe(r, "tool2", &[("PATH", "./rel/")]), some("./rel/tool2"));
        assert_eq!(fe(r, "tool3", &[("PATH", "")]), some("tool3"));
        assert_eq!(fe(r, "tool3", &[]), some("tool3"));
        assert_eq!(fe(r, "tool3", &[("PATH", ":")]), some("tool3"));
        assert_eq!(fe(r, "tool3", &[("PATH", "$ROOT/bin1:")]), some("tool3"));
        assert_eq!(fe(r, "tool3", &[("PATH", "$ROOT/bin1")]), Err(()));
        assert_eq!(
            fe(r, "tool", &[("PATH", "$ROOT/extd"), ("PATHEXT", ".sh:.py")]),
            some("$ROOT/extd/tool.sh")
        );
        assert_eq!(
            fe(r, "tool", &[("PATH", "$ROOT/extd"), ("PATHEXT", "")]),
            some("$ROOT/extd/tool")
        );
        assert_eq!(
            fe(
                r,
                "tool",
                &[("PATH", "$ROOT/bin1:$ROOT/extd"), ("PATHEXT", ".sh")]
            ),
            some("$ROOT/bin1/tool")
        );
        assert_eq!(fe(r, "a/b", &[("PATH", "$ROOT/bin1")]), some("a/b"));
        assert_eq!(fe(r, "./tool3", &[("PATH", "$ROOT/bin1")]), Err(()));
        assert_eq!(fe(r, "bin1/../tool3", &[("PATH", "")]), some("tool3"));
        assert_eq!(fe(r, "tool/..", &[("PATH", "$ROOT/bin1")]), Err(()));
        assert_eq!(fe(r, "", &[("PATH", "$ROOT/bin1")]), Err(()));
        assert_eq!(fe(r, "nonexistent", &[("PATH", "$ROOT/bin1")]), Err(()));
    }

    #[test]
    fn normexe_messages() {
        let d = fixture();
        let r = d.path();
        let env = Env::from_pairs([("PATH", r.join("bin1"))]);
        let msg = |s: &str| normexe(OsStr::new(s), &env, r).unwrap_err().message();
        assert_eq!(msg("nope"), "Executable `nope` not found");
        assert_eq!(msg("./bin3"), "Executable `./bin3` is a directory");
        assert_eq!(msg("./missing"), "Executable `./missing` not found");
        assert_eq!(msg("bin2/tool"), "Executable `bin2/tool` is not executable");
        assert_eq!(
            normexe(OsStr::new("both"), &env, r).unwrap(),
            r.join("bin1/both")
        );
        assert_eq!(
            normexe(OsStr::new("./tool3"), &env, r).unwrap(),
            Path::new("./tool3")
        );
    }

    #[test]
    fn normalize_cmd_resolves_a_shebang_script() {
        let d = fixture();
        let r = d.path();
        file(&r.join("bin1/interp"), 0o755, b"");
        file(
            &r.join("scripts/hook.sh"),
            0o755,
            b"#!/usr/bin/env interp -x\necho hi\n",
        );
        let env = Env::from_pairs([("PATH", r.join("bin1"))]);
        let cmd = [OsString::from("scripts/hook.sh"), OsString::from("--flag")];
        let got = normalize_cmd(&cmd, &env, r).unwrap();
        assert_eq!(
            got,
            [
                r.join("bin1/interp").into_os_string(),
                OsString::from("-x"),
                OsString::from("scripts/hook.sh"),
                OsString::from("--flag"),
            ]
        );
        // A missing interpreter is reported by name.
        file(&r.join("scripts/bad.sh"), 0o755, b"#!/usr/bin/env nosuch\n");
        let e = normalize_cmd(&[OsString::from("scripts/bad.sh")], &env, r).unwrap_err();
        assert_eq!(e.message(), "Executable `nosuch` not found");
        // An executable without a shebang is its own program.
        assert!(
            normalize_cmd(
                &[OsString::from("tool3")],
                &Env::from_pairs([("PATH", "")]),
                r
            )
            .is_ok()
        );
        assert_eq!(
            normalize_cmd(&[], &env, r).unwrap_err().message(),
            "Executable `` not found"
        );
    }

    #[test]
    fn self_exe_is_absolute() {
        assert!(self_exe().unwrap().is_absolute());
    }
}
