//! Shared helpers of every backend (`lang_base` equivalents), owned by the framework.
//!
//! The public helpers are ARCHITECTURE §1.4.10's `common`; the crate-internal ones (`hook_cmd`, `prefix_path`,
//! `home_dir`, the docker run prefix of `common/docker.rs`) are shared by the env-less backends and the language
//! groups.
//!
//! Owner: WP-16.

pub(crate) mod docker;
mod rmtree;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_proc::hookcmd::HookCmdError;

use crate::{CmdRunner, LangError};

/// `lang_base.exe_exists` (`lang_base.py:63-83`): `exe` is found on `PATH` (`find_executable`, `PATHEXT` aware),
/// its path does not match `[/\\]shims[/\\]`, and either the home directory is `/` or the executable is not inside it
/// (`commonpath` raising `ValueError` — different drives, relative result — counts as "not inside").
///
/// `home` is `os.path.expanduser('~')` over `env` (`HOME`; Windows `USERPROFILE`, then `HOMEDRIVE`+`HOMEPATH`; the
/// password database when `HOME` is unset on POSIX). The node, golang and ruby default versions use it (04a §1.7).
pub fn exe_exists(name: &OsStr, env: &Env, cwd: &Path) -> bool {
    let Some(found) = snp_proc::resolve::find_executable(name, env, cwd) else {
        return false;
    };
    let home = home_dir(env);
    snp_pycompat::precommit::exe_exists_pure(found.as_os_str(), &home)
}

/// `lang_base.setup_cmd`: runs `argv`, returns stdout, `CalledProcessError` (format A) on a non-zero exit.
///
/// `cmd_output_b(*cmd, cwd=prefix, check=True)` (`lang_base.py:86-87`, `util.py:90-113`): the error carries the
/// normalised argv when `argv[0]` resolves against `env` (shebang interpreter prepended, as pre-commit's
/// `CalledProcessError.cmd`), the argv as given otherwise, and both output streams as raw bytes (format A renders
/// non-UTF-8 output without decoding it, T10-XRG-21).
pub fn setup_cmd(
    r: &dyn CmdRunner,
    argv: &[OsString],
    env: &Env,
    cwd: &Path,
) -> Result<Vec<u8>, LangError> {
    let out = r.run(argv, env, cwd)?;
    if out.status.0 == 0 {
        return Ok(out.stdout);
    }
    let cmd = snp_proc::resolve::normalize_cmd(argv, env, cwd).unwrap_or_else(|_| argv.to_vec());
    Err(LangError::CalledProcess {
        cmd,
        rc: out.status.0,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// `bin` | `Scripts`.
///
/// `python.bin_dir` (`python.py:47-49`): `Scripts` on Windows, `bin` elsewhere (venv layouts).
pub fn bin_dir(env_dir: &Path) -> PathBuf {
    env_dir.join(if cfg!(windows) { "Scripts" } else { "bin" })
}

/// "pre-commit-" + md5(hex) (ADR-035).
///
/// pre-commit tags `'pre-commit-' + md5(basename(prefix_dir)).hexdigest().lower()` (`docker.py:73-75`); snp's
/// prefixes are all named `src`, so the store's repo key (hex) replaces the basename: two repos never share a tag, and
/// the `PRE_COMMIT` label keeps `docker image prune --filter label=PRE_COMMIT` working.
pub fn docker_tag(repo_key_hex: &str) -> String {
    use md5::Digest as _;
    use std::fmt::Write as _;
    let digest = md5::Md5::digest(repo_key_hex.as_bytes());
    let mut s = String::from("pre-commit-");
    for b in digest {
        // Writing into a String cannot fail.
        let _ignored = write!(s, "{b:02x}");
    }
    s
}

/// Readonly-aware, contained `rmtree`.
///
/// `util.rmtree` (`util.py:205-235`): removal that fixes permissions on `EACCES`/`EPERM` (Go's read-only module
/// cache, read-only git objects on Windows) — here without ever following a symlink (an entry that is a link is
/// unlinked, never traversed; Unix walks descriptor-relative from `root`, so a directory swapped for a link
/// mid-walk is not followed). Refused (`PermissionDenied`) unless `path` lies strictly inside `root` (lexically, no
/// `..`); a missing `path` is `Ok` (pre-commit raises `FileNotFoundError`, which only ever crashed it).
pub fn rmtree(path: &Path, root: &Path) -> io::Result<()> {
    rmtree::remove(path, root)
}

/// `util.clean_path_on_failure(path)` (`util.py:27-35`): runs `f`; when it fails — an `Err` or a panic unwinding
/// through (pre-commit catches every `BaseException`, `SystemExit` and `KeyboardInterrupt` included) — `path` is
/// removed with [`rmtree`] (contained in `root`, best effort) before the failure propagates; on success it is kept.
///
/// The environment install of snp-repo wraps `Language::install` + the health check in it (`repository.py:83-104`).
pub fn clean_path_on_failure<T, E>(
    path: &Path,
    root: &Path,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    /// Removes the path when dropped armed (normal failure or unwinding).
    struct Guard<'a> {
        path: &'a Path,
        root: &'a Path,
        armed: bool,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            if self.armed {
                let _ignored = rmtree(self.path, self.root);
            }
        }
    }
    let mut guard = Guard {
        path,
        root,
        armed: true,
    };
    let result = f();
    guard.armed = result.is_err();
    drop(guard);
    result
}

/// `os.path.expanduser('~')` over the captured environment (`HomeLookup` of `snp_pycompat::ospath`), with the
/// password database (`pwd.getpwuid(os.getuid()).pw_dir`) as the POSIX fallback when `HOME` is unset.
pub(crate) fn home_dir(env: &Env) -> OsString {
    let lookup = snp_pycompat::ospath::HomeLookup {
        env: &|k: &str| env.get(k).map(OsStr::to_os_string),
        pw_current: &pw_current_home,
        pw_named: &|_: &OsStr| None,
    };
    snp_pycompat::ospath::expanduser(OsStr::new("~"), &lookup).into_owned()
}

/// The home directory of the real uid from `/etc/passwd` (POSIX; `None` elsewhere or when not listed).
fn pw_current_home() -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::io::Read as _;
        let uid = rustix::process::getuid().as_raw();
        let mut bytes = Vec::new();
        std::fs::File::open("/etc/passwd")
            .and_then(|f| f.take(16 << 20).read_to_end(&mut bytes))
            .ok()?;
        bytes.split(|b| *b == b'\n').find_map(|line| {
            let mut fields = line.split(|b| *b == b':');
            let (_name, _pw, id, _gid, _gecos, home) = (
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
            );
            let id: u32 = std::str::from_utf8(id).ok()?.parse().ok()?;
            (id == uid).then(|| {
                use std::os::unix::ffi::OsStrExt;
                OsStr::from_bytes(home).to_os_string()
            })
        })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `lang_base.hook_cmd(entry, args)` (`lang_base.py:174-178`) with the hazmat rewrite of ADR-006 / 03 §H.8.1:
/// `shlex.split(entry)` (POSIX, no comments); a leading `pre-commit hazmat` or `snp hazmat` (exact tokens, any
/// quoting) becomes `[self_exe, "__hazmat"]`; then `args`.
///
/// The same contract as `snp_proc::hookcmd::hook_cmd` (WP-15), on the W0 `snp_pycompat::shlex` baseline. `Empty`:
/// the entry and the args produce no token at all. Once WP-15 has merged this body becomes a call of
/// `snp_proc::hookcmd::hook_cmd` (one implementation, so `plan` and the hook runner cannot drift);
/// `tests/envless/plans.rs` stays the regression check.
pub(crate) fn hook_cmd(
    entry: &str,
    args: &[Arc<str>],
    self_exe: &Path,
) -> Result<Vec<OsString>, HookCmdError> {
    let tokens = snp_pycompat::shlex::split(entry).map_err(HookCmdError::Shlex)?;
    let mut cmd: Vec<OsString> = Vec::new();
    let rest: &[String] = match tokens.as_slice() {
        [tool, verb, rest @ ..] if (tool == "pre-commit" || tool == "snp") && verb == "hazmat" => {
            cmd.push(self_exe.as_os_str().to_os_string());
            cmd.push(OsString::from("__hazmat"));
            rest
        }
        all => all,
    };
    cmd.extend(rest.iter().map(OsString::from));
    cmd.extend(args.iter().map(|a| OsString::from(&**a)));
    if cmd.is_empty() {
        return Err(HookCmdError::Empty);
    }
    Ok(cmd)
}

/// `Prefix(prefix).path(part)` (`prefix.py:13-14`): `os.path.normpath(os.path.join(prefix, part))` — lexical; an
/// absolute `part` discards the prefix.
pub(crate) fn prefix_path(prefix: &Path, part: &OsStr) -> OsString {
    snp_pycompat::ospath::normpath(&snp_pycompat::ospath::join(prefix.as_os_str(), &[part]))
}
