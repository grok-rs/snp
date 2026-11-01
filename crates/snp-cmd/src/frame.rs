//! What every git command of `main.py` does before dispatch (03 §1.2): find the repository root, re-express
//! `-c/--config` relative to it (`_adjust_args_and_chdir`, `main.py:175-199`) and track the config
//! (`store.mark_config_used`, `main.py:382-384`). snp never changes its working directory (ARCHITECTURE §1.3
//! disallows `set_current_dir`): the root is carried in [`Frame`] instead.
//!
//! Also the shared config-loading options and git helpers of the commands.
//!
//! Owner: WP-33.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_config::{ConfigSource, ExtMode, LoadOpts};
use snp_core::failure::Failure;
use snp_core::failure::pyerr;
use snp_git::{Git, GitExe, RepoInfo};
use snp_proc::spawn::{Captured, Group, Stdin};
use snp_pyre::RegexCache;

use crate::{CmdCtx, messages};

/// A git command's view of the repository after `_adjust_args_and_chdir`.
#[derive(Debug)]
pub(crate) struct Frame {
    /// The user repository (cwd = its root).
    pub(crate) git: Git,
    /// Its layout.
    pub(crate) info: RepoInfo,
    /// The resolved git executable.
    pub(crate) exe: Arc<GitExe>,
    /// `args.config` as pre-commit holds it after the chdir (relative to the root unless outside it).
    pub(crate) config_display: OsString,
    /// The config on disk (`root / config_display`).
    pub(crate) config_path: PathBuf,
}

impl Frame {
    /// The repository root (pre-commit's cwd).
    pub(crate) fn root(&self) -> &Path {
        &self.info.root
    }

    /// The config source for `load_config`.
    pub(crate) fn source(&self) -> ConfigSource {
        ConfigSource {
            path: self.config_path.clone(),
            display: self.config_display.clone(),
            project_root: self.info.root.clone(),
        }
    }
}

/// The resolved git executable, or pre-commit's first git failure (`git.get_root`, `git.py:63-67`).
pub(crate) fn git_exe(cx: &CmdCtx<'_>) -> Result<Arc<GitExe>, Failure> {
    cx.git_exe
        .cloned()
        .ok_or_else(|| Failure::fatal("FatalError", messages::GIT_FAILED))
}

/// `get_root()` + `_adjust_args_and_chdir(args)` + `store.mark_config_used(args.config)` (`main.py:175-199,
/// 382-384`).
///
/// An existing config path is made absolute against the invocation cwd and then relative to the root (a config
/// outside the repository becomes `../x.yaml`); a missing one is normalised by `relpath` against the root, which
/// re-interprets it relative to the root (03 §1.2 item 4). `relpath` across Windows drives (pre-commit's
/// `ValueError`) keeps the absolute path.
pub(crate) fn git_frame(cx: &CmdCtx<'_>, config: &OsStr) -> Result<Frame, Failure> {
    frame_impl(cx, config, true)
}

/// [`git_frame`] without `mark_config_used` (read-only commands: `doctor`).
pub(crate) fn git_frame_untracked(cx: &CmdCtx<'_>, config: &OsStr) -> Result<Frame, Failure> {
    frame_impl(cx, config, false)
}

fn frame_impl(cx: &CmdCtx<'_>, config: &OsStr, track: bool) -> Result<Frame, Failure> {
    let exe = git_exe(cx)?;
    let (git, info) = Git::discover(
        Arc::clone(&exe),
        cx.env,
        cx.cwd,
        Arc::clone(&cx.git_backend),
    )?;
    let git = git.with_brand(cx.brand);
    let config_display = adjust_config(config, cx.cwd, &info.root)?;
    let config_path = info.root.join(&config_display);
    if track && let Ok(real) = snp_os::realpath::realpath_nonstrict(&config_path, &info.root) {
        cx.store.mark_config_used(&real, &info.root);
    }
    Ok(Frame {
        git,
        info,
        exe,
        config_display,
        config_path,
    })
}

/// `_adjust_args_and_chdir`'s treatment of `args.config` (`main.py:175-199`): an existing path is made absolute
/// against the invocation cwd; then the path is always re-expressed with `os.path.relpath` against the new cwd (the
/// root), so a missing `./x.yaml` becomes `x.yaml` and `-c ''` raises `ValueError: no path specified` (03 §1.2
/// item 4). `relpath` across Windows drives (pre-commit's `ValueError`) keeps the absolute path.
pub(crate) fn adjust_config(config: &OsStr, cwd: &Path, root: &Path) -> Result<OsString, Failure> {
    let exists = !config.is_empty() && cwd.join(config).exists();
    let p = if exists {
        snp_pycompat::ospath::abspath(config, cwd).into_os_string()
    } else {
        config.to_os_string()
    };
    match snp_pycompat::ospath::relpath(&p, None, root) {
        Ok(rel) => Ok(rel),
        Err(e @ snp_pycompat::PathError::NoPathSpecified) => Err(Failure::Exc {
            kind: snp_core::failure::ExcKind::Unexpected,
            exc: snp_core::failure::PyExc::new(e.py_type(), e.to_string()),
        }),
        Err(_) => Ok(snp_pycompat::ospath::abspath(&p, root).into_os_string()),
    }
}

/// `SNP_COMPAT_STRICT=1`: extension keys are unknown keys, as for pre-commit (ADR-039).
pub(crate) fn ext_mode(cx: &CmdCtx<'_>) -> ExtMode {
    if cx.env.is_truthy_flag("SNP_COMPAT_STRICT") {
        ExtMode::Strict
    } else {
        ExtMode::Enabled
    }
}

/// `LoadOpts` writing warnings (`[WARNING] …`, stdout) and diagnostics (stderr) through the printer.
pub(crate) fn load_opts<'a>(
    cx: &'a CmdCtx<'_>,
    regexes: &'a RegexCache,
    ext: ExtMode,
) -> LoadOpts<'a> {
    LoadOpts {
        brand: cx.brand,
        ext,
        regexes,
        warn: cx.printer,
        diag: cx.printer,
    }
}

/// `git <args>` (the resolved argv0) in `cwd` with the startup environment, stdin `/dev/null`, unchecked.
pub(crate) fn git_raw(
    cx: &CmdCtx<'_>,
    exe: &GitExe,
    args: &[&OsStr],
    cwd: &Path,
) -> Result<(Vec<OsString>, Captured), Failure> {
    let mut argv = exe.argv0.clone();
    argv.extend(args.iter().map(|a| a.to_os_string()));
    let out = cx
        .git_backend
        .capture(&argv, cx.env, cwd, Stdin::Null, Group::Inherit)
        .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))?;
    Ok((argv, out))
}

/// `cmd_output('git', …)` (checked): stdout, or pre-commit's `CalledProcessError` (format A, exit 3).
pub(crate) fn git_checked(
    cx: &CmdCtx<'_>,
    exe: &GitExe,
    args: &[&OsStr],
    cwd: &Path,
) -> Result<Vec<u8>, Failure> {
    let (argv, out) = git_raw(cx, exe, args, cwd)?;
    if out.status.0 == 0 {
        return Ok(out.stdout);
    }
    Err(Failure::unexpected(
        "CalledProcessError",
        pyerr::called_process_a(&argv, out.status.0, Some(&out.stdout), Some(&out.stderr)),
    ))
}

/// The bytes of a git output line, stripped (`out.strip()`).
pub(crate) fn stripped(b: &[u8]) -> OsString {
    os_from_bytes(snp_pycompat::text::bytes_strip(b))
}

/// Raw bytes as an `OsString` (Unix: verbatim; Windows: lossy UTF-8).
pub(crate) fn os_from_bytes(b: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(b).to_os_string()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// `git.get_git_common_dir()` (`git.py:85-93`): `os.path.normpath(os.path.join('.', <--git-common-dir>))`, run in
/// the root — relative (`.git`) in the main worktree, absolute in linked worktrees and submodules (03 §2.3).
pub(crate) fn common_dir_display(cx: &CmdCtx<'_>, f: &Frame) -> Result<OsString, Failure> {
    let out = git_checked(
        cx,
        &f.exe,
        &[OsStr::new("rev-parse"), OsStr::new("--git-common-dir")],
        f.root(),
    )?;
    let dir = stripped(&out);
    if dir == "--git-common-dir" {
        // git < 2.5: get_git_dir
        let out = git_checked(
            cx,
            &f.exe,
            &[OsStr::new("rev-parse"), OsStr::new("--git-dir")],
            f.root(),
        )?;
        return Ok(normjoin_dot(&stripped(&out)));
    }
    Ok(normjoin_dot(&dir))
}

fn normjoin_dot(p: &OsStr) -> OsString {
    snp_pycompat::ospath::normpath(&snp_pycompat::ospath::join(OsStr::new("."), &[p]))
}

/// A subcommand variant this build does not know (the CLI enums are `#[non_exhaustive]`).
pub(crate) fn unsupported(what: &str) -> Failure {
    Failure::unexpected(
        "NotImplementedError",
        format!("{what}: subcommand not supported by this build"),
    )
}

/// Whether the token is cancelled by a signal (the command stops and the envelope reports it).
pub(crate) fn interrupted(cx: &CmdCtx<'_>) -> Option<Failure> {
    use snp_core::cancel::CancelReason;
    match cx.cancel.reason() {
        Some(r @ (CancelReason::Interrupt | CancelReason::Terminate | CancelReason::Hangup)) => {
            Some(Failure::Interrupted(r))
        }
        Some(CancelReason::StdoutClosed) => Some(Failure::StdoutClosed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adj(c: impl AsRef<OsStr>, cwd: &Path, root: &Path) -> OsString {
        adjust_config(c.as_ref(), cwd, root).unwrap()
    }

    /// `s` with the separators `os.path` writes on this platform (`ntpath` writes `\`).
    fn sep(s: &str) -> String {
        if cfg!(windows) {
            s.replace('/', "\\")
        } else {
            s.to_owned()
        }
    }

    #[test]
    fn existing_config_is_re_expressed_against_the_root() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            adj(".pre-commit-config.yaml", &sub, root),
            ".pre-commit-config.yaml"
        );
        std::fs::write(root.join(".pre-commit-config.yaml"), b"").unwrap();
        assert_eq!(
            adj("../../.pre-commit-config.yaml", &sub, root),
            ".pre-commit-config.yaml"
        );
        std::fs::write(sub.join("c.yaml"), b"").unwrap();
        assert_eq!(adj("c.yaml", &sub, root), *sep("a/b/c.yaml"));
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("o.yaml"), b"").unwrap();
        let rel = adj(out.path().join("o.yaml"), &sub, root);
        assert!(rel.to_string_lossy().starts_with(".."), "{rel:?}");
    }

    /// Oracle (pc462): `os.path.relpath` normalises a missing config after the chdir to the root.
    #[test]
    fn missing_config_is_normalised_by_relpath() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        assert_eq!(adj("./missing.yaml", root, root), "missing.yaml");
        assert_eq!(adj("a/../missing.yaml", root, root), "missing.yaml");
        assert_eq!(adj(root.join("missing.yaml"), root, root), "missing.yaml");
        assert_eq!(adj("a//b.yaml", root, root), *sep("a/b.yaml"));
        assert_eq!(
            adj("sub/../../missing.yaml", root, root),
            *sep("../missing.yaml")
        );
    }

    #[test]
    fn empty_config_is_value_error() {
        let d = tempfile::tempdir().unwrap();
        let Err(Failure::Exc { kind, exc }) = adjust_config(OsStr::new(""), d.path(), d.path())
        else {
            panic!("expected ValueError");
        };
        assert_eq!(kind, snp_core::failure::ExcKind::Unexpected);
        assert_eq!(exc.ty, "ValueError");
        assert_eq!(exc.text, b"no path specified");
    }

    #[test]
    fn dot_join_normalises() {
        assert_eq!(normjoin_dot(OsStr::new(".git")), ".git");
        assert_eq!(normjoin_dot(OsStr::new("/abs/.git")), *sep("/abs/.git"));
    }
}
