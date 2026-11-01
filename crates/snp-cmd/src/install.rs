//! `install`, `uninstall`, `init-templatedir` (`commands/install_uninstall.py`, `init_templatedir.py`; 03 §2–§5;
//! 11 §A.5).
//!
//! pre-commit's algorithm and stdout lines, with the coexistence rules of 11 §A.5.2: hook scripts of other managers
//! (pre-commit, prek, snp 1.x) are replaced in place instead of being moved to `.legacy` (D-BRAND-02), a manager
//! script found in `.legacy` is deleted (D-DATA-03), a foreign `.legacy` is never clobbered without `-f`
//! (D-DATA-03), the snp notes go to stderr (D-DIAG-02), and the script is written atomically, LF, replacing a
//! symlink instead of writing through it (D-DATA-05).
//!
//! Owner: WP-33.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use snp_cli::{InitTemplatedirArgs, InstallArgs, UninstallArgs};
use snp_config::{ConfigSource, load_config};
use snp_core::failure::Failure;
use snp_core::output::Level;
use snp_core::stage::HookType;
use snp_git::hookscripts::{Owner, classify};
use snp_pyre::RegexCache;

use crate::CmdCtx;
use crate::frame::{self, Frame};
use crate::messages::{self, os_bytes};
use crate::scripts::{self, parse_script_args};

/// Where hook scripts go: `{dir}/hooks/{type}` as printed and as opened.
#[derive(Debug, Clone)]
pub(crate) struct HooksDir {
    /// The git dir as pre-commit prints it (`.git`, an absolute common dir, or `init-templatedir`'s argument).
    pub(crate) display: OsString,
    /// The same directory on disk.
    pub(crate) path: PathBuf,
}

impl HooksDir {
    /// `_hook_paths` (`install_uninstall.py:47-53`): `(hook_path, legacy_path)` as displayed and on disk.
    pub(crate) fn paths(&self, t: HookType) -> (OsString, OsString, PathBuf, PathBuf) {
        let disp = snp_pycompat::ospath::join(
            &self.display,
            &[OsStr::new("hooks"), OsStr::new(t.as_str())],
        );
        let mut legacy_disp = disp.clone();
        legacy_disp.push(".legacy");
        let path = self.path.join("hooks").join(t.as_str());
        let mut legacy = path.clone().into_os_string();
        legacy.push(".legacy");
        (disp, legacy_disp, path, PathBuf::from(legacy))
    }
}

/// How one script is written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WriteOpts<'a> {
    /// `--config=` value embedded in the script.
    pub(crate) config: &'a OsStr,
    /// `-f/--overwrite` (always for `init-templatedir`).
    pub(crate) overwrite: bool,
    /// `--skip-on-missing-config`.
    pub(crate) skip_missing: bool,
    /// `SNP_EXE`.
    pub(crate) exe: &'a Path,
    /// `doctor --fix`: M1 says when the replaced script's options could not be read.
    pub(crate) note_defaults: bool,
}

/// Writes the scripts only. `install --install-hooks` is sequenced by the binary: `snp_cmd::install(..)?` then
/// `snp_run::install_hooks(InstallHooksArgs { color, config, with_fallback: false }, &run_cx)` — pre-commit's
/// order; the first failure is returned (ADR-070).
///
/// `install()` (`install_uninstall.py:114-141`) after `_adjust_args_and_chdir` and `mark_config_used`: the
/// `core.hooksPath` refusal (`[ERROR] Cowardly refusing …`, exit 1, nothing written), then one script per hook
/// type of `-t` (in order, duplicates kept) or the config's `default_install_hook_types` (`['pre-commit']` when the
/// config cannot be loaded; its warnings still print), then M3 for every other hook type still owned by another
/// manager. `--git-config-hooks` (snp brand, git ≥ 2.54, ADR-057) writes `hook.snp-<type>.{command,event}` instead.
pub fn install(a: &InstallArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let f = frame::git_frame(cx, &a.config, false)?;
    let exe = scripts::select_exe(cx.brand, cx.arg0, cx.env, cx.self_exe);
    if a.git_config_hooks {
        let types = hook_types(cx, &f.source(), &a.hook_types)?;
        return config_hooks_install(cx, &f, &types, a.allow_missing_config, &exe);
    }
    if f.git.hooks_path()?.is_some() {
        cx.printer.log(Level::Error, messages::HOOKS_PATH_REFUSAL);
        return Ok(1);
    }
    let types = hook_types(cx, &f.source(), &a.hook_types)?;
    let dir = HooksDir {
        display: frame::common_dir_display(cx, &f)?,
        path: f.info.common_dir.clone(),
    };
    let opts = WriteOpts {
        config: &f.config_display,
        overwrite: a.overwrite,
        skip_missing: a.allow_missing_config,
        exe: &exe,
        note_defaults: false,
    };
    for &t in &types {
        install_one(cx, &dir, t, &opts, None)?;
    }
    warn_double_run(cx, &f, &types);
    note_other_managers(cx, &dir, &types);
    Ok(0)
}

/// `_hook_types` (`install_uninstall.py:35-44`): `-t` as given, else `default_install_hook_types`; a config that
/// fails validation falls back to `['pre-commit']` (its warnings printed). An exception pre-commit does not catch
/// (an unreadable file, X13-CFG-09) stays exit 3.
pub(crate) fn hook_types(
    cx: &CmdCtx<'_>,
    src: &ConfigSource,
    explicit: &[HookType],
) -> Result<Vec<HookType>, Failure> {
    if !explicit.is_empty() {
        return Ok(explicit.to_vec());
    }
    let regexes = RegexCache::default();
    match load_config(src, &frame::load_opts(cx, &regexes, frame::ext_mode(cx))) {
        Ok(cfg) => Ok(cfg.default_install_hook_types),
        Err(e) if e.uncaught().is_some() => Err(e.into()),
        Err(_) => Ok(vec![HookType::PreCommit]),
    }
}

/// An `OSError` for `path` (exit 3).
fn io_fail(e: &io::Error, path: &OsStr) -> Failure {
    Failure::from_io(e, Some(path), None)
}

/// `IsADirectoryError: [Errno 21] Is a directory: '<path>'`: pre-commit opens a directory found at the hook path
/// (`is_our_script`, X13-CMD-01).
fn is_a_directory(path: &OsStr) -> Failure {
    #[cfg(unix)]
    let e = io::Error::from_raw_os_error(21);
    #[cfg(not(unix))]
    let e = io::Error::from(io::ErrorKind::IsADirectory);
    io_fail(&e, path)
}

/// Classifies `path`, mapping a directory to pre-commit's crash.
pub(crate) fn classify_hook(path: &Path, display: &OsStr) -> Result<Owner, Failure> {
    let owner = classify(path).map_err(|e| io_fail(&e, display))?;
    if owner == Owner::NotAFile && path.is_dir() {
        return Err(is_a_directory(display));
    }
    Ok(owner)
}

/// `os.path.exists` (follows symlinks).
fn exists(p: &Path) -> bool {
    fs::metadata(p).is_ok()
}

/// `os.path.lexists`.
fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// The mode the new script gets: the existing regular file's mode (pre-commit truncates it in place) or
/// `0o666 & ~umask` for a new file, then `| 0o111` (`util.make_executable`). `None` on Windows.
#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature for both platforms; Windows has no mode bits"
)]
fn script_mode(hook: &Path, hooks: &Path, keep_existing: bool) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let base = if keep_existing
        && let Ok(md) = fs::metadata(hook)
        && md.is_file()
    {
        md.permissions().mode() & 0o7777
    } else {
        umask_mode(hooks)
    };
    Some(base | 0o111)
}

#[cfg(not(unix))]
fn script_mode(_hook: &Path, _hooks: &Path, _keep_existing: bool) -> Option<u32> {
    None
}

/// `0o666 & ~umask`, observed by creating (and removing) a probe file in `dir` — no `umask(2)` call, which would
/// change process-global state (03 §20.3).
#[cfg(unix)]
fn umask_mode(dir: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    let probe = dir.join(format!(
        ".snp-umask-{}-{}",
        std::process::id(),
        probe_nonce()
    ));
    let mode = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|f| f.metadata())
        .map(|m| m.permissions().mode() & 0o777);
    let _removed: io::Result<()> = fs::remove_file(&probe);
    mode.unwrap_or(0o644)
}

#[cfg(unix)]
fn probe_nonce() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(u64::from(std::process::id()));
    h.finish()
}

/// The first-matching option values of a replaced manager script (M1's Python interpreter, M1b's prek options).
fn replaced_notes(
    cx: &CmdCtx<'_>,
    owner: &Owner,
    hook: &Path,
    hook_disp: &OsStr,
    note_defaults: bool,
) {
    let bytes = snp_os::fs::read_capped(hook, 1 << 20).unwrap_or_default();
    let parsed = parse_script_args(owner, &bytes);
    let name = owner.owner_name().unwrap_or("snp").as_bytes();
    let mut m1 = messages::note(
        cx.brand,
        messages::M1,
        &[("owner", name), ("hook_path", os_bytes(hook_disp))],
    );
    if matches!(owner, Owner::PreCommit { .. })
        && let Some(py) = parsed.as_ref().and_then(|p| p.install_python.as_ref())
        && !py.is_empty()
    {
        m1.extend(messages::note(
            cx.brand,
            messages::M1_PYTHON,
            &[("python", py)],
        ));
    }
    if note_defaults && parsed.is_none() {
        m1.extend(messages::note(cx.brand, messages::M1_DEFAULTS, &[]));
    }
    m1.push(b'\n');
    cx.printer.write_stderr(&m1);
    if let Some(p) = &parsed
        && !p.prek_only.is_empty()
    {
        let mut m1b = messages::note(
            cx.brand,
            messages::M1B,
            &[("opts", p.prek_only.join(" ").as_bytes())],
        );
        m1b.push(b'\n');
        cx.printer.write_stderr(&m1b);
    }
}

/// M2: a manager script in `.legacy` is deleted.
fn note_m2(cx: &CmdCtx<'_>, legacy_disp: &OsStr, owner: &Owner) {
    let mut m2 = messages::note(
        cx.brand,
        messages::M2,
        &[
            ("legacy_path", os_bytes(legacy_disp)),
            ("owner", owner.owner_name().unwrap_or("snp").as_bytes()),
        ],
    );
    m2.push(b'\n');
    cx.printer.write_stderr(&m2);
}

/// `_install_hook_script` (`install_uninstall.py:64-111`) with the rules of 11 §A.5.2. `fixed_args` (doctor
/// `--fix`) overrides the embedded config and skip flag with the replaced script's own options.
pub(crate) fn install_one(
    cx: &CmdCtx<'_>,
    dir: &HooksDir,
    t: HookType,
    o: &WriteOpts<'_>,
    fixed_args: Option<(&OsStr, bool)>,
) -> Result<(), Failure> {
    let (hook_disp, legacy_disp, hook, legacy) = dir.paths(t);
    let hooks_dir = dir.path.join("hooks");
    fs::create_dir_all(&hooks_dir).map_err(|e| {
        io_fail(
            &e,
            &snp_pycompat::ospath::join(&dir.display, &[OsStr::new("hooks")]),
        )
    })?;
    let owner = classify_hook(&hook, &hook_disp)?;
    let mut keep_mode = false;
    match &owner {
        Owner::Missing => {}
        Owner::Snp { .. } => keep_mode = true,
        Owner::SnpV1 | Owner::PreCommit { .. } | Owner::Prek => {
            replaced_notes(cx, &owner, &hook, &hook_disp, o.note_defaults);
            keep_mode = true;
        }
        _ if lexists(&hook) => {
            // a foreign script (or a dangling symlink) becomes the legacy hook: pre-commit's `shutil.move`
            if lexists(&legacy) {
                let lowner = classify(&legacy).unwrap_or(Owner::Foreign);
                if lowner.is_manager() {
                    fs::remove_file(&legacy).map_err(|e| io_fail(&e, &legacy_disp))?;
                    note_m2(cx, &legacy_disp, &lowner);
                } else if !o.overwrite {
                    return Err(Failure::fatal(
                        "FatalError",
                        messages::refuse_legacy(&legacy_disp),
                    ));
                }
            }
            snp_os::fs::rename_with_retry(&hook, &legacy)
                .map_err(|e| Failure::from_io(&e, Some(&hook_disp), Some(&legacy_disp)))?;
        }
        _ => {}
    }
    if o.overwrite && exists(&legacy) {
        fs::remove_file(&legacy).map_err(|e| io_fail(&e, &legacy_disp))?;
    } else if exists(&legacy) {
        let lowner = classify(&legacy).unwrap_or(Owner::Foreign);
        if lowner.is_manager() {
            fs::remove_file(&legacy).map_err(|e| io_fail(&e, &legacy_disp))?;
            note_m2(cx, &legacy_disp, &lowner);
        } else {
            cx.printer
                .write_line(&messages::migration_mode(cx.brand, &legacy_disp));
        }
    }
    let (config, skip) = fixed_args.unwrap_or((o.config, o.skip_missing));
    let script = scripts::render(config, t, skip, o.exe);
    let mode = script_mode(&hook, &hooks_dir, keep_mode);
    snp_os::fs::atomic_replace(&hook, &script, mode).map_err(|e| io_fail(&e, &hook_disp))?;
    cx.printer
        .write_line(&messages::installed_at(cx.brand, &hook_disp));
    Ok(())
}

/// M3: after the requested types, every other hook type whose script still runs pre-commit, prek or snp 1.x.
pub(crate) fn note_other_managers(cx: &CmdCtx<'_>, dir: &HooksDir, done: &[HookType]) {
    for t in HookType::ALL {
        if done.contains(&t) {
            continue;
        }
        let (disp, _, path, _) = dir.paths(t);
        let owner = classify(&path).unwrap_or(Owner::Foreign);
        if matches!(owner, Owner::PreCommit { .. } | Owner::Prek | Owner::SnpV1) {
            let mut m3 = messages::note(
                cx.brand,
                messages::M3,
                &[
                    ("hook_path", os_bytes(&disp)),
                    ("owner", owner.owner_name().unwrap_or_default().as_bytes()),
                    ("T", t.as_str().as_bytes()),
                ],
            );
            m3.push(b'\n');
            cx.printer.write_stderr(&m3);
        }
    }
}

/// 02 G.5 item 3: a traditional install while a git ≥ 2.54 config hook also runs a manager for the same event.
fn warn_double_run(cx: &CmdCtx<'_>, f: &Frame, types: &[HookType]) {
    if !f.exe.caps.config_hooks {
        return;
    }
    let Ok(events) = f.git.config_get_regexp(r"^hook\..*\.event$") else {
        return;
    };
    for &t in types {
        for (key, value) in &events {
            if value.as_os_str() != t.as_str() {
                continue;
            }
            let key = key.to_string_lossy();
            let Some(name) = key
                .strip_prefix("hook.")
                .and_then(|k| k.strip_suffix(".event"))
            else {
                continue;
            };
            let cmd_key = format!("hook.{name}.command");
            let runs_manager = f
                .git
                .config_get(&cmd_key)
                .ok()
                .flatten()
                .is_some_and(|c| c.to_string_lossy().contains("hook-impl"));
            if runs_manager {
                cx.printer.diag(&format!(
                    "{} git config also runs a config hook (hook.{name}) for {t}; hooks will run twice.",
                    cx.printer.paint("[WARNING]", snp_core::output::Color::Yellow),
                    t = t.as_str()
                ));
            }
        }
    }
}

/// `snp install --git-config-hooks` (ADR-057, ARCHITECTURE §9.7): `hook.snp-<type>.command` = the `select_exe`
/// path + the `hook-impl` tokens + ` --`, `hook.snp-<type>.event` = `<type>` (`--replace-all`, so stale duplicates
/// go), in the repository's local config.
fn config_hooks_install(
    cx: &CmdCtx<'_>,
    f: &Frame,
    types: &[HookType],
    skip_missing: bool,
    exe: &Path,
) -> Result<i32, Failure> {
    if !f.exe.caps.config_hooks {
        let v = f
            .exe
            .caps
            .version
            .map_or_else(|| "an unknown version".to_owned(), |v| v.to_string());
        return Err(Failure::fatal(
            "FatalError",
            messages::config_hooks_need_git(cx.brand, &v),
        ));
    }
    for &t in types {
        let name = format!("snp-{}", t.as_str());
        let command = scripts::config_hook_command(&f.config_display, t, skip_missing, exe);
        let command = frame::os_from_bytes(&command);
        let cmd_key = format!("hook.{name}.command");
        let ev_key = format!("hook.{name}.event");
        frame::git_checked(
            cx,
            &f.exe,
            &[OsStr::new("config"), OsStr::new(&cmd_key), &command],
            f.root(),
        )?;
        frame::git_checked(
            cx,
            &f.exe,
            &[
                OsStr::new("config"),
                OsStr::new("--replace-all"),
                OsStr::new(&ev_key),
                OsStr::new(t.as_str()),
            ],
            f.root(),
        )?;
        cx.printer
            .write_line(messages::config_hook_installed(cx.brand, &name).as_bytes());
    }
    Ok(0)
}

/// `snp uninstall --git-config-hooks`: `git config --remove-section hook.snp-<type>` (silently skipped when absent).
fn config_hooks_uninstall(cx: &CmdCtx<'_>, f: &Frame, types: &[HookType]) -> Result<i32, Failure> {
    for &t in types {
        let name = format!("snp-{}", t.as_str());
        let section = format!("hook.{name}");
        let (_, out) = frame::git_raw(
            cx,
            &f.exe,
            &[
                OsStr::new("config"),
                OsStr::new("--remove-section"),
                OsStr::new(&section),
            ],
            f.root(),
        )?;
        if out.status.0 == 0 {
            cx.printer
                .write_line(messages::config_hook_uninstalled(&name).as_bytes());
        }
    }
    Ok(0)
}

/// `pre-commit uninstall`.
///
/// `uninstall()` (`install_uninstall.py:149-167`): for each hook type (as for `install`), a script that is snp's or
/// pre-commit's (snp *is* a pre-commit implementation; snp 1.x scripts too) is removed (`{type} uninstalled`) and
/// a `.legacy` restored (`Restored previous hooks to …`) — except a `.legacy` that is snp's own script, which is
/// deleted with M12 (11 §A.5.2). Other scripts (prek, foreign) are left alone silently. No `core.hooksPath` check.
/// Always 0.
pub fn uninstall(a: &UninstallArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let f = frame::git_frame(cx, &a.config, false)?;
    let types = hook_types(cx, &f.source(), &a.hook_types)?;
    if a.git_config_hooks {
        return config_hooks_uninstall(cx, &f, &types);
    }
    let dir = HooksDir {
        display: frame::common_dir_display(cx, &f)?,
        path: f.info.common_dir.clone(),
    };
    for t in types {
        uninstall_one(cx, &dir, t)?;
    }
    Ok(0)
}

/// `_uninstall_hook_script` (`install_uninstall.py:149-161`).
fn uninstall_one(cx: &CmdCtx<'_>, dir: &HooksDir, t: HookType) -> Result<(), Failure> {
    let (hook_disp, legacy_disp, hook, legacy) = dir.paths(t);
    let owner = classify_hook(&hook, &hook_disp)?;
    if !matches!(
        owner,
        Owner::Snp { .. } | Owner::SnpV1 | Owner::PreCommit { .. }
    ) {
        return Ok(());
    }
    fs::remove_file(&hook).map_err(|e| io_fail(&e, &hook_disp))?;
    cx.printer
        .write_line(&messages::uninstalled(cx.brand, t.as_str()));
    if exists(&legacy) {
        let lowner = classify(&legacy).unwrap_or(Owner::Foreign);
        match lowner {
            Owner::Snp { .. } => {
                fs::remove_file(&legacy).map_err(|e| io_fail(&e, &legacy_disp))?;
                let mut m12 = messages::note(
                    cx.brand,
                    messages::M12,
                    &[("legacy_path", os_bytes(&legacy_disp))],
                );
                m12.push(b'\n');
                cx.printer.write_stderr(&m12);
            }
            Owner::SnpV1 => {
                fs::remove_file(&legacy).map_err(|e| io_fail(&e, &legacy_disp))?;
                note_m2(cx, &legacy_disp, &lowner);
            }
            _ => {
                snp_os::fs::rename_with_retry(&legacy, &hook)
                    .map_err(|e| Failure::from_io(&e, Some(&legacy_disp), Some(&hook_disp)))?;
                cx.printer.write_line(&messages::restored(&hook_disp));
            }
        }
    }
    Ok(())
}

/// `pre-commit init-templatedir`.
///
/// `init_templatedir()` (`init_templatedir.py:14-39`): no chdir and no config tracking (`COMMANDS_NO_GIT`); the
/// hook types come from `-c` read relative to the invocation cwd; scripts go to `{directory}/hooks/{type}` with
/// `overwrite=True` (every `.legacy` deleted) and `--skip-on-missing-config` unless `--no-allow-missing-config`;
/// no `core.hooksPath` check. Then the two `init.templateDir` warnings when `git config init.templateDir`
/// (`~`-expanded, realpath'd) is not `realpath(directory)`. Always 0.
pub fn init_templatedir(a: &InitTemplatedirArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let src = ConfigSource {
        path: cx.cwd.join(&a.config.path),
        display: a.config.path.clone(),
        project_root: cx.cwd.to_path_buf(),
    };
    let types = hook_types(cx, &src, &a.hook_types)?;
    let exe = scripts::select_exe(cx.brand, cx.arg0, cx.env, cx.self_exe);
    let dir = HooksDir {
        display: a.directory.clone(),
        path: cx.cwd.join(&a.directory),
    };
    let opts = WriteOpts {
        config: &a.config.path,
        overwrite: true,
        skip_missing: !a.no_allow_missing_config,
        exe: &exe,
        note_defaults: false,
    };
    for &t in &types {
        install_one(cx, &dir, t, &opts, None)?;
    }
    let configured = configured_template_dir(cx);
    let dest = snp_os::realpath::realpath_nonstrict(Path::new(&a.directory), cx.cwd)
        .unwrap_or_else(|_| cx.cwd.join(&a.directory));
    if configured.as_deref() != Some(dest.as_path()) {
        cx.printer
            .log(Level::Warning, messages::TEMPLATEDIR_NOT_SET);
        cx.printer.log(
            Level::Warning,
            &messages::templatedir_maybe(dest.as_os_str()),
        );
    }
    Ok(0)
}

/// `git config init.templateDir` in the invocation cwd (`init_templatedir.py:28-33`): a failing (unset) key or a
/// missing git is `None`; else `realpath(expanduser(out.strip()))`.
fn configured_template_dir(cx: &CmdCtx<'_>) -> Option<PathBuf> {
    let exe = cx.git_exe?;
    let (_, out) = frame::git_raw(
        cx,
        exe,
        &[OsStr::new("config"), OsStr::new("init.templateDir")],
        cx.cwd,
    )
    .ok()?;
    if out.status.0 != 0 {
        return None;
    }
    let value = frame::stripped(&out.stdout);
    let expanded = snp_os::home::expanduser(&value, cx.env);
    snp_os::realpath::realpath_nonstrict(Path::new(&expanded), cx.cwd).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_paths_join_like_python() {
        let d = HooksDir {
            display: OsString::from("./tmpl/"),
            path: PathBuf::from("/x/tmpl"),
        };
        let (disp, legacy, path, lpath) = d.paths(HookType::PrePush);
        assert_eq!(disp, "./tmpl/hooks/pre-push");
        assert_eq!(legacy, "./tmpl/hooks/pre-push.legacy");
        assert_eq!(path, Path::new("/x/tmpl/hooks/pre-push"));
        assert_eq!(lpath, Path::new("/x/tmpl/hooks/pre-push.legacy"));
    }

    #[cfg(unix)]
    #[test]
    fn umask_probe_is_a_plain_file_mode() {
        let d = tempfile::tempdir().unwrap();
        let m = umask_mode(d.path());
        assert_eq!(m & !0o666, 0);
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    }
}
