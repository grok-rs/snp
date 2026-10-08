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
    ///
    /// The error's kind is the OS error's and its text is the complete `str(OSError)` including the file name
    /// (`[Errno 13] Permission denied: './x'`), as pre-commit's crash line shows it.
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
        crate::messages::executable(orig, what)
    }
}

impl std::fmt::Display for ExeError {
    /// [`ExeError::message`].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ExeError {}

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
///
/// The shebang line is capped at 64 KiB by file-identify (a longer line counts as no shebang, D-SEC-09).
fn parse_filename(exe: &Path, cwd: &Path) -> Result<Vec<OsString>, ExeError> {
    let full = at(cwd, exe.as_os_str());
    if std::fs::metadata(&full).is_err() {
        return Ok(Vec::new());
    }
    let os_error = |e: &io::Error| {
        // `open(path, 'rb')` names the path as Python saw it (relative to the cwd).
        ExeError::Io(io::Error::new(
            e.kind(),
            pyerr::os_error_text(e, Some(exe.as_os_str()), None),
        ))
    };
    match file_identify::parse_shebang_from_file_with(&full, ExecPolicy::PythonCompat) {
        Ok(cmd) => Ok(cmd.into_iter().map(OsString::from).collect()),
        Err(IdentifyError::PathNotFound { source, .. } | IdentifyError::IoError { source }) => {
            Err(os_error(&source))
        }
        Err(other) => Err(ExeError::Io(io::Error::other(other.to_string()))),
    }
}

/// normexe → shebang (`file_identify::parse_shebang_from_file_with`, empty for non-UTF-8 = identify parity) →
/// normexe again; Windows-only basename fallback for absolute shebang interpreters (D-PLAT-02).
///
/// `parse_shebang.normalize_cmd` (`parse_shebang.py:65-85`): exactly one level of shebang interpretation; the
/// interpreter goes through `PATH` with the same error texts. The D-PLAT-02 fallback applies only to an interpreter
/// that came from a shebang and starts with `/` (Git for Windows' `parse_interpreter`): `#!/bin/sh` resolves `sh` on
/// `PATH`; when that fails too, pre-commit's message names the original interpreter. An empty `cmd` (Python:
/// `IndexError`) is `NotFound("")`.
pub fn normalize_cmd(cmd: &[OsString], env: &Env, cwd: &Path) -> Result<Vec<OsString>, ExeError> {
    normalize_cmd_impl(cmd, env, cwd, cfg!(windows))
}

/// [`normalize_cmd`] with the D-PLAT-02 fallback switchable (tested on every OS).
fn normalize_cmd_impl(
    cmd: &[OsString],
    env: &Env,
    cwd: &Path,
    basename_fallback_on: bool,
) -> Result<Vec<OsString>, ExeError> {
    let Some((first, rest)) = cmd.split_first() else {
        return Err(ExeError::NotFound(OsString::new()));
    };
    let exe = normexe(first, env, cwd)?;
    let shebang = parse_filename(&exe, cwd)?;
    let from_shebang = !shebang.is_empty();
    let mut full: Vec<OsString> = shebang;
    full.push(exe.into_os_string());
    full.extend(rest.iter().cloned());
    let Some((head, tail)) = full.split_first() else {
        return Err(ExeError::NotFound(OsString::new()));
    };
    let exe = match normexe(head, env, cwd) {
        Ok(exe) => exe,
        Err(ExeError::NotFound(orig))
            if basename_fallback_on
                && from_shebang
                && orig.as_encoded_bytes().starts_with(b"/") =>
        {
            basename_fallback(&orig, env, cwd)?
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    out.push(exe.into_os_string());
    out.extend(tail.iter().cloned());
    Ok(out)
}

/// D-PLAT-02: `find_executable(basename(interp))`, keeping pre-commit's "not found" text for `interp` otherwise.
fn basename_fallback(orig: &OsStr, env: &Env, cwd: &Path) -> Result<PathBuf, ExeError> {
    let base = ospath::basename(orig);
    if base.is_empty() || has_sep(&base, true) {
        return Err(ExeError::NotFound(orig.to_os_string()));
    }
    find_executable(&base, env, cwd).ok_or_else(|| ExeError::NotFound(orig.to_os_string()))
}

/// `current_exe` → absolute, not canonicalised (03 H.8.1).
///
/// Computed once per process. On Linux a binary replaced while running reads as `<path> (deleted)`; then
/// `/proc/self/exe` (which still executes the running image) is used.
pub fn self_exe() -> io::Result<PathBuf> {
    static CACHE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(p) = CACHE.get() {
        return Ok(p.clone());
    }
    let exe = std::env::current_exe()?;
    let exe = if exe.is_absolute() {
        exe
    } else {
        std::path::absolute(exe)?
    };
    #[cfg(target_os = "linux")]
    let exe = if exe.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") && !exe.exists() {
        let proc_exe = PathBuf::from("/proc/self/exe");
        if proc_exe.exists() { proc_exe } else { exe }
    } else {
        exe
    };
    Ok(CACHE.get_or_init(|| exe).clone())
}

/// Windows: `PATHEXT` is tried directory-major (every extension in the first `PATH` entry before the second one),
/// extensions before the bare name, with the extension's case from `PATHEXT` (14 P-20, 04a §14.6).
#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_pathext_is_directory_major() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("tool.cmd"), b"").unwrap();
        std::fs::write(a.join("tool"), b"#!/bin/sh\n").unwrap();
        std::fs::write(b.join("tool.exe"), b"").unwrap();
        let mut path = a.clone().into_os_string();
        path.push(";");
        path.push(&b);
        let env = Env::from_pairs([("Path", path), ("PATHEXT", OsString::from(".EXE;.CMD"))]);
        // Directory-major: a\tool.CMD (second extension, first directory) beats b\tool.EXE; the extension's case
        // comes from PATHEXT; the extension-less shim is tried last.
        let got = find_executable(OsStr::new("tool"), &env, d.path()).unwrap();
        assert_eq!(got, a.join("tool.CMD"));
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
        assert_eq!(self_exe().unwrap(), self_exe().unwrap());
    }

    /// D-PLAT-02 (Windows semantics, exercised on every OS): a missing absolute shebang interpreter falls back to
    /// its basename on PATH; a missing basename keeps pre-commit's message; explicit entry paths never fall back.
    #[test]
    fn d_plat_02_basename_fallback() {
        let d = fixture();
        let r = d.path();
        file(&r.join("bin1/myinterp"), 0o755, b"");
        file(&r.join("s.sh"), 0o755, b"#!/nonexistent/dir/myinterp -e\n");
        file(&r.join("t.sh"), 0o755, b"#!/nonexistent/dir/nosuch\n");
        let env = Env::from_pairs([("PATH", r.join("bin1"))]);
        let got = normalize_cmd_impl(&[OsString::from("./s.sh")], &env, r, true).unwrap();
        assert_eq!(
            got,
            [
                r.join("bin1/myinterp").into_os_string(),
                OsString::from("-e"),
                OsString::from("./s.sh"),
            ]
        );
        // pre-commit's behaviour (the POSIX default here).
        let e = normalize_cmd_impl(&[OsString::from("./s.sh")], &env, r, false).unwrap_err();
        assert_eq!(
            e.message(),
            "Executable `/nonexistent/dir/myinterp` not found"
        );
        let e = normalize_cmd_impl(&[OsString::from("./t.sh")], &env, r, true).unwrap_err();
        assert_eq!(
            e.message(),
            "Executable `/nonexistent/dir/nosuch` not found"
        );
        // An explicit absolute entry is never rewritten.
        let e = normalize_cmd_impl(
            &[OsString::from("/nonexistent/dir/myinterp")],
            &env,
            r,
            true,
        )
        .unwrap_err();
        assert_eq!(
            e.message(),
            "Executable `/nonexistent/dir/myinterp` not found"
        );
    }

    #[test]
    fn unreadable_shebang_is_an_os_error_naming_the_file() {
        if rustix::process::geteuid().is_root() {
            return; // root reads anything
        }
        let d = fixture();
        let r = d.path();
        file(&r.join("noread.sh"), 0o311, b"#!/bin/sh\n");
        let e = normalize_cmd(
            &[OsString::from("./noread.sh")],
            &Env::from_pairs([("PATH", "")]),
            r,
        )
        .unwrap_err();
        assert!(matches!(e, ExeError::Io(ref io) if io.kind() == io::ErrorKind::PermissionDenied));
        assert_eq!(e.message(), "[Errno 13] Permission denied: './noread.sh'");
    }
}
