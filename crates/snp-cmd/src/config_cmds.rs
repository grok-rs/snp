//! `validate-config`, `validate-manifest`, `migrate-config`, `sample-config` and `snp config dump|schema`.
//!
//! Owner: WP-33.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use snp_cli::{ConfigArg, ConfigArgs, ConfigCmd, ValidateArgs};
use snp_config::discover;
use snp_config::validate::{self, FileKind};
use snp_config::{ExtMode, SAMPLE_CONFIG, effective_dump, json_schema, load_config};
use snp_config::{migrate, overlay};
use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::DiagSink;
use snp_pyre::RegexCache;

use crate::CmdCtx;
use crate::frame;

/// `validate_config_main` / `validate_manifest_main` (`commands/validate_config.py:8-18`,
/// `validate_manifest.py:8-18`): every file in argv order; an `Invalid*Error` is `print(e)` (a blank first line,
/// the cfgv trace, a newline) and makes the exit 1; warnings are logged during the load; no file → exit 0. No git,
/// no chdir, no config tracking (`COMMANDS_NO_GIT`). An exception pre-commit does not catch (an unreadable file,
/// X13-CMD-11 / CFG-09) aborts with exit 3, later files unprocessed.
///
/// snp brand: `--strict` also reports the hook keys `run` would warn about (W11) at validation time; `--portable`
/// validates as pre-commit 4.6.2 would (`SNP_COMPAT_STRICT=1` semantics: every extension key is an unexpected key
/// and `repo: builtin` is an ordinary URL), which lists every key a pre-commit user of the same file would see
/// warned about.
fn validate_files(kind: FileKind, a: &ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let regexes = RegexCache::default();
    let ext = if a.portable {
        ExtMode::Strict
    } else {
        frame::ext_mode(cx)
    };
    let o = frame::load_opts(cx, &regexes, ext);
    let mut ret = 0;
    for filename in &a.filenames {
        if let Some(f) = frame::interrupted(cx) {
            return Err(f);
        }
        match validate::validate(kind, filename, &o, a.strict || a.portable) {
            Ok(()) => {}
            Err(e) if e.uncaught().is_some() => return Err(e.into()),
            Err(e) => {
                cx.printer.py_print(&e.rendered);
                ret = 1;
            }
        }
    }
    Ok(ret)
}

/// `pre-commit validate-config` (`clientlib.validate_config_main`).
pub fn validate_config(a: &ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    validate_files(FileKind::Config, a, cx)
}

/// `pre-commit validate-manifest` (`clientlib.validate_manifest_main`).
pub fn validate_manifest(a: &ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    validate_files(FileKind::Manifest, a, cx)
}

/// Writes a rewritten config: through a symlink to its target (pre-commit opens the path for writing), replacing
/// the regular file atomically and keeping its mode (D-DATA-09; pre-commit truncates in place).
pub(crate) fn write_config(path: &Path, cwd: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let target: PathBuf = if fs_is_symlink(path) {
        snp_os::realpath::realpath_nonstrict(path, cwd).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    snp_os::fs::atomic_replace(&target, bytes, None)
        .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))
}

fn fs_is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// `migrate_config(config_file, quiet)` (`commands/migrate_config.py:114-135`): read (a missing file is the
/// `is not a file` validation error, D-CRASH-07), migrate, write when changed and print
/// `Configuration has been migrated.`; otherwise `Configuration is already migrated.` unless `quiet` (autoupdate).
pub(crate) fn migrate_file(
    cx: &CmdCtx<'_>,
    path: &Path,
    display: &OsStr,
    root: &Path,
    quiet: bool,
) -> Result<(), Failure> {
    let bytes = migrate::read_config(path, display)?;
    match migrate::migrate_config(&bytes, display)? {
        migrate::Outcome::Migrated(new) => {
            write_config(path, root, &new)?;
            cx.printer.py_print(migrate::MIGRATED);
        }
        migrate::Outcome::AlreadyMigrated if !quiet => {
            cx.printer.py_print(migrate::ALREADY_MIGRATED);
        }
        _ => {}
    }
    Ok(())
}

/// Reads a config file to rewrite it (`snp migrate builtin|upstream`, `autoupdate`): through the loader's capped
/// read (`cfgv.load_from_filename` up to the validation context), so a non-regular file (`/dev/zero`, a FIFO) is
/// `=====> {display} is not a file` (exit 1) and a file over the YAML byte limit (16 MiB, ARCHITECTURE §11.4,
/// D-SEC-11) is `the input is N bytes long, more than the limit of M bytes` without being read.
pub fn read_config_for_rewrite(path: &Path, display: &OsStr) -> Result<Vec<u8>, Failure> {
    Ok(migrate::read_config(path, display)?)
}

/// `snp migrate lefthook`'s default output name: `.snp.yaml`, or `.pre-commit-config.yaml` with `--portable`
/// (pre-commit must find it; ADR-087).
pub fn migrate_output_name(portable: bool) -> &'static str {
    if portable {
        discover::PRE_COMMIT_CONFIG
    } else {
        ".snp.yaml"
    }
}

/// The local overlay written next to `config` (`snp migrate lefthook --local`): `.snp.local.yaml` beside an
/// `.snp.y[a]ml`, else `.pre-commit-config.local.yaml` (ADR-087).
pub fn overlay_name(config: &Path) -> &'static str {
    let [first, _] = discover::overlay_names(config);
    first
}

/// The config `snp migrate builtin|upstream` rewrites: `-c` relative to `cwd`, else the config discovered in
/// `root` (ADR-087; its note printed on `diag`), else `.pre-commit-config.yaml` in `cwd`. Returns the path and its
/// display (relative to `cwd` when inside it).
///
/// # Errors
///
/// `FatalError` when both `.snp.yaml` and `.snp.yml` exist.
pub fn rewrite_target(
    config: &ConfigArg,
    cwd: &Path,
    root: &Path,
    brand: Brand,
    env: &Env,
    diag: &dyn DiagSink,
) -> Result<(PathBuf, OsString), Failure> {
    if let Some(c) = config.explicit() {
        return Ok((cwd.join(c), c.to_os_string()));
    }
    let found = discover::discover(None, cwd, root, brand, env)?;
    found.note(diag, true);
    let path = cwd.join(&found.config);
    let display = match path.strip_prefix(cwd) {
        Ok(rel) if found.chosen.is_some() => rel.as_os_str().to_os_string(),
        _ => found.config,
    };
    Ok((path, display))
}

/// `pre-commit migrate-config`.
///
/// A git command (`_adjust_args_and_chdir`, config tracking), then `migrate_config(args.config)` (not quiet);
/// always 0 unless the config is invalid YAML (`InvalidConfigError`, exit 1).
pub fn migrate_config(a: &ConfigArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let f = frame::git_frame(cx, &a.config, true)?;
    migrate_file(cx, &f.config_path, &f.config_display, f.root(), false)?;
    Ok(0)
}

/// `pre-commit sample-config`.
///
/// `print(SAMPLE_CONFIG, end='')` (`commands/sample_config.py`): the text layer, exactly 10 lines, exit 0. No git.
pub fn sample_config(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    match SAMPLE_CONFIG.strip_suffix('\n') {
        Some(body) => cx.printer.py_print(body),
        None => cx.printer.py_print(SAMPLE_CONFIG),
    }
    Ok(0)
}

/// Whether git tracks `path` (`git ls-files -- <path>` prints it), for the overlay's "ignored when tracked" rule.
fn is_tracked(cx: &CmdCtx<'_>, f: &frame::Frame, path: &Path) -> bool {
    frame::git_checked(
        cx,
        &f.exe,
        &[
            OsStr::new("ls-files"),
            OsStr::new("--error-unmatch"),
            OsStr::new("--"),
            path.as_os_str(),
        ],
        f.root(),
    )
    .is_ok()
}

/// `snp config dump|schema`.
///
/// `dump`: the effective config (the validated, defaulted config with the local overlay of ARCHITECTURE §8.4
/// applied unless `--no-local-config`) as pre-commit's `yaml_dump`, or JSON with `--json`; a git command like `run`
/// (the config path is re-expressed relative to the repository root and tracked). `schema`: the JSON schema of
/// the config, extension keys included.
pub fn config(a: &ConfigCmd, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    match a {
        ConfigCmd::Schema => {
            write_text(cx, json_schema());
            Ok(0)
        }
        ConfigCmd::Dump {
            config,
            json,
            no_local_config,
        } => {
            let f = frame::git_frame(cx, config, true)?;
            let regexes = RegexCache::default();
            let ext = frame::ext_mode(cx);
            let o = frame::load_opts(cx, &regexes, ext);
            let src = f.source();
            let mut cfg = load_config(&src, &o)?;
            if !no_local_config {
                let tracked = |p: &Path| is_tracked(cx, &f, p);
                match overlay::locate(&f.config_path, cx.env, &tracked) {
                    Ok(Some(path)) => {
                        let ov = overlay::load(&path, &o)?;
                        overlay::apply_root(&mut cfg, &ov, cx.printer);
                    }
                    Ok(None) => {}
                    Err(note) => cx.printer.diag(&note),
                }
            }
            write_text(cx, &effective_dump(&cfg, *json));
            Ok(0)
        }
        _ => Err(frame::unsupported("config")),
    }
}

/// Writes `s` with exactly one trailing newline (text layer).
fn write_text(cx: &CmdCtx<'_>, s: &str) {
    cx.printer.py_print(s.strip_suffix('\n').unwrap_or(s));
}
