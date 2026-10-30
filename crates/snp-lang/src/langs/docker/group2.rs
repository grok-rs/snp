//! Helpers shared by the group-2 backends (`docker`, `conda`, `coursier`, `dart`, `dotnet`, `haskell`, `julia`, `lua`,
//! `perl`, `r`, `swift`): `lang_base.basic_run_hook` as a plan, `prefix.star`, `shutil.copy`/`copytree`,
//! `tempfile.TemporaryDirectory`, `os.path.join` over OS strings.
//!
//! They live below `langs/docker/` because that is the only directory WP-25 owns (WBS ownership); every group-2 file
//! imports them as `crate::langs::docker::group2`.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_proc::hookcmd::HookCmdError;

use crate::common::hook_cmd;
use crate::framework::messages;
use crate::{InstallCtx, InstallOutcome, LangError, PlanCtx, RunPlan, check_support};

/// `lang_base.basic_get_default_version()`.
pub(crate) fn default_version() -> Arc<str> {
    Arc::from("default")
}

/// The support assertions of `install_environment` (`assert_version_default` / `assert_no_additional_deps`), checked
/// again by the backend itself: snp-repo runs [`check_support`] before any filesystem mutation, this is the
/// defence in depth for a direct caller.
pub(crate) fn assert_support(id: LanguageId, cx: &InstallCtx<'_>) -> Result<(), LangError> {
    check_support(id, cx.cx.version, cx.deps, cx.cx.host.brand)
}

/// An install outcome without interpreter fingerprint or shared tools.
pub(crate) fn outcome(installer: &str) -> InstallOutcome {
    InstallOutcome {
        installer: Arc::from(installer),
        fingerprint: None,
        tools: Vec::new(),
    }
}

/// The C-hook plan of an `entry` that does not split into a command (X13-EXE-01/02, D-CRASH-05), the same texts as the
/// env-less languages: "Executable \`\` not found" or `ValueError: No closing quotation`.
pub(crate) fn entry_error(e: &HookCmdError) -> RunPlan {
    let text = match e {
        HookCmdError::Shlex(err) => messages::shlex_error(err.py_type(), &err.to_string()),
        _ => messages::EMPTY_ENTRY.to_owned(),
    };
    RunPlan::Error {
        code: 1,
        output: text.into_bytes(),
    }
}

/// `lang_base.run_xargs(cmd, …)`: shuffled and parallel unless the hook sets `require_serial` (the executor decides).
pub(crate) fn xargs(cmd: Vec<OsString>, env: &Env) -> RunPlan {
    RunPlan::Xargs {
        cmd: Arc::from(cmd),
        env: env.clone(),
        shuffle: true,
        serial: false,
        pre: Vec::new(),
    }
}

/// `lang_base.basic_run_hook`: `run_xargs(hook_cmd(entry, args), …)`; `cmd[0]` is resolved by the executor on the
/// hook's (activated) `PATH`.
pub(crate) fn basic_plan(cx: &PlanCtx<'_>) -> RunPlan {
    match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
        Ok(cmd) => xargs(cmd, cx.env),
        Err(e) => entry_error(&e),
    }
}

/// `os.path.join(a, *rest)` over OS strings (an absolute part discards what precedes it, an empty last part adds a
/// trailing separator).
pub(crate) fn py_join(a: &Path, rest: &[&OsStr]) -> PathBuf {
    PathBuf::from(snp_pycompat::ospath::join(a.as_os_str(), rest))
}

/// `os.path.splitext(os.path.basename(name))[0]` (`genericpath._splitext`: leading dots are not an extension).
pub(crate) fn stem(name: &OsStr) -> OsString {
    let base = snp_pycompat::ospath::basename(name);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let (root, _ext) = snp_pycompat::posixpath::splitext(base.as_bytes());
        OsString::from_vec(root.to_vec())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units: Vec<u16> = base.encode_wide().collect();
        let (root, _ext) = snp_pycompat::ntpath::splitext(units.as_slice());
        OsString::from_wide(root)
    }
    #[cfg(not(any(unix, windows)))]
    {
        base
    }
}

/// `util.win_exe(s)`: `s + '.exe'` on Windows.
pub(crate) fn win_exe(s: &str) -> String {
    if cfg!(windows) {
        format!("{s}.exe")
    } else {
        s.to_owned()
    }
}

/// An I/O error of operation `op` on `path`.
pub(crate) fn io_err<'a>(
    op: &'static str,
    path: &'a Path,
) -> impl FnOnce(io::Error) -> LangError + 'a {
    move |err| LangError::Io {
        op,
        path: path.to_path_buf(),
        err,
    }
}

/// `os.makedirs(path, exist_ok=True)`.
pub(crate) fn makedirs(path: &Path) -> Result<(), LangError> {
    std::fs::create_dir_all(path).map_err(io_err("makedirs", path))
}

/// `os.mkdir(path)` / `os.makedirs(path)` without `exist_ok`: the parents are created, the leaf must not exist yet
/// (`FileExistsError` otherwise, as in pre-commit).
pub(crate) fn mkdir_new(path: &Path) -> Result<(), LangError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        makedirs(parent)?;
    }
    #[expect(
        clippy::create_dir,
        reason = "pre-commit's os.mkdir: the leaf must not exist (FileExistsError)"
    )]
    std::fs::create_dir(path).map_err(io_err("mkdir", path))
}

/// `prefix.star(end)` (`prefix.py:16-18`): the names in `dir` ending with `end` (directories and dotfiles included),
/// **sorted** by their bytes (pre-commit lists in directory order; 04b §5.5, sorted for reproducibility).
pub(crate) fn star(dir: &Path, end: &str) -> Result<Vec<OsString>, LangError> {
    let mut names = listdir(dir)?;
    names.retain(|n| n.as_encoded_bytes().ends_with(end.as_bytes()));
    Ok(names)
}

/// `os.listdir(dir)`, sorted by bytes.
pub(crate) fn listdir(dir: &Path) -> Result<Vec<OsString>, LangError> {
    let rd = std::fs::read_dir(dir).map_err(io_err("listdir", dir))?;
    let mut names = Vec::new();
    for entry in rd {
        let entry = entry.map_err(io_err("listdir", dir))?;
        names.push(entry.file_name());
    }
    names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
    Ok(names)
}

/// `os.path.isfile(p)` (follows symlinks).
pub(crate) fn is_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// `shutil.copy(src, dst_dir)`: copies the bytes and the permission bits of `src` into `dst_dir/<basename>`.
pub(crate) fn copy_into(src: &Path, dst_dir: &Path) -> Result<PathBuf, LangError> {
    let name = src.file_name().unwrap_or(src.as_os_str());
    let dst = dst_dir.join(name);
    std::fs::copy(src, &dst).map_err(io_err("copy", src))?;
    Ok(dst)
}

/// Depth bound of [`copytree`] (symlinked directories are followed, as `shutil.copytree(symlinks=False)` does; the
/// bound turns a symlink loop into an error instead of an unbounded recursion).
const MAX_COPY_DEPTH: usize = 64;

/// `shutil.copytree(src, dst)` with the defaults (`symlinks=False`): `dst` must not exist; files are copied with their
/// permission bits, symlinks are followed (their targets' contents are copied).
pub(crate) fn copytree(src: &Path, dst: &Path) -> Result<(), LangError> {
    copytree_depth(src, dst, 0)
}

fn copytree_depth(src: &Path, dst: &Path, depth: usize) -> Result<(), LangError> {
    if depth > MAX_COPY_DEPTH {
        return Err(LangError::Io {
            op: "copytree",
            path: src.to_path_buf(),
            err: io::Error::other("directory nesting too deep (symlink loop?)"),
        });
    }
    #[expect(
        clippy::create_dir,
        reason = "shutil.copytree: the destination must not exist (FileExistsError)"
    )]
    std::fs::create_dir(dst).map_err(io_err("copytree", dst))?;
    for name in listdir(src)? {
        let from = src.join(&name);
        let to = dst.join(&name);
        let md = std::fs::metadata(&from).map_err(io_err("copytree", &from))?;
        if md.is_dir() {
            copytree_depth(&from, &to, depth.saturating_add(1))?;
        } else {
            std::fs::copy(&from, &to).map_err(io_err("copytree", &from))?;
        }
    }
    Ok(())
}

/// `tempfile.TemporaryDirectory()`: a fresh `tmpXXXXXXXX` directory in `tempfile.gettempdir()` of the run's
/// environment (`TMPDIR`, `TEMP`, `TMP` — the first non-empty one naming a directory — else the OS default), removed
/// when dropped.
pub(crate) fn tempdir(env: &Env) -> Result<tempfile::TempDir, LangError> {
    let base = ["TMPDIR", "TEMP", "TMP"]
        .iter()
        .filter_map(|k| env.get_nonempty(k))
        .map(PathBuf::from)
        .find(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix("tmp")
        .rand_bytes(8)
        .tempdir_in(&base)
        .map_err(io_err("mkdtemp", &base))
}

/// `open(path, 'w').write(text)`: text mode, so `\n` becomes `\r\n` on Windows.
pub(crate) fn write_text(path: &Path, text: &str) -> Result<(), LangError> {
    let text = if cfg!(windows) {
        snp_pycompat::textio::windows_text_write(text)
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    std::fs::write(path, text.as_bytes()).map_err(io_err("write", path))
}

/// `[*names]` as argv elements.
pub(crate) fn os_args<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<OsString> {
    names.into_iter().map(OsString::from).collect()
}

/// Appends the `additional_dependencies` to `argv`.
pub(crate) fn push_deps(argv: &mut Vec<OsString>, deps: &[Arc<str>]) {
    argv.extend(deps.iter().map(|d| OsString::from(&**d)));
}
