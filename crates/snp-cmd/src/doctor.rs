//! `snp doctor` (§9.3, ADR-070).
//!
//! 11 §A.15: read-only by default; one line per finding, `{level:<5} {subject}: {message}` with `level` in
//! `ok|info|warn|error` (Tier B, not a stable API); exit 1 when any `error`. `--fix` performs the "doctor --fix"
//! column of 11 §A.5.2 for the current repository (replace other managers' scripts keeping their parsed options,
//! rewrite stale snp scripts, delete manager scripts found in `.legacy`) and then re-runs the checks; with `--stash`
//! it also dismisses the notices of orphaned stashes, keeping their patches (ARCHITECTURE §4.6, ADR-083).
//!
//! Owner: WP-33.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};

use snp_cli::DoctorArgs;
use snp_core::failure::Failure;
use snp_core::profile;
use snp_core::stage::HookType;
use snp_git::hookscripts::{Owner, classify};
use snp_store::StoreSource;

use crate::frame::{self, Frame};
use crate::install::{self, HooksDir, WriteOpts};
use crate::scripts::{self, ScriptArgs, parse_script_args, snp_version_of, version_probe};
use crate::{CmdCtx, messages};

/// Checks that need crates snp-cmd cannot depend on are injected by the binary: the fast-path table (wrapping
/// `snp_run::doctor_fast_path`). The stash-recovery check and `--stash` verification live in snp-cmd (snp-stash
/// edge).
pub trait DoctorCheck: Debug {
    /// The check's heading.
    fn name(&self) -> &'static str;
    /// The check's result lines.
    fn run(&self, cx: &CmdCtx<'_>) -> Vec<DoctorLine>;
}

/// One doctor result line.
#[derive(Debug, Clone)]
pub struct DoctorLine {
    /// Passed.
    pub ok: bool,
    /// What was checked.
    pub text: String,
    /// What `--fix` would do.
    pub fix: Option<String>,
}

/// The level of one finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Lvl {
    Ok,
    Info,
    Warn,
    Error,
}

impl Lvl {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// One finding.
#[derive(Debug, Clone)]
struct Finding {
    lvl: Lvl,
    subject: String,
    message: String,
    /// `--fix` can repair it.
    fixable: bool,
}

#[derive(Debug, Default)]
struct Report(Vec<Finding>);

impl Report {
    fn add(&mut self, lvl: Lvl, subject: impl Into<String>, message: impl Into<String>) {
        self.0.push(Finding {
            lvl,
            subject: subject.into(),
            message: message.into(),
            fixable: false,
        });
    }

    fn fixable(&mut self, lvl: Lvl, subject: impl Into<String>, message: impl Into<String>) {
        self.0.push(Finding {
            lvl,
            subject: subject.into(),
            message: message.into(),
            fixable: true,
        });
    }
}

/// `{level:<5} {subject}: {message}`.
fn render_line(f: &Finding) -> String {
    format!("{:<5} {}: {}", f.lvl.as_str(), f.subject, f.message)
}

fn lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `os.access(p, X_OK)` for a regular file.
#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// The first `name` on PATH that is a regular executable file (`command -v`, 11 §A.19.5).
fn on_path(cx: &CmdCtx<'_>, name: &str) -> Option<PathBuf> {
    let path = cx.env.get("PATH")?;
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path)
        .map(|d| {
            if d.as_os_str().is_empty() {
                cx.cwd.join(&file)
            } else {
                d.join(&file)
            }
        })
        .find(|c| is_executable(c))
}

/// `{kind}` of a `pre-commit` executable (11 §A.9.2).
fn pre_commit_kind(file: &Path, current: &Path) -> (Lvl, String) {
    if snp_os::same_file(file, current).unwrap_or(false) {
        return (Lvl::Ok, "snp alias (this snp)".to_owned());
    }
    match version_probe(file) {
        Some(line) => match snp_version_of(&line) {
            Some(v) if v == profile::SNP_VERSION => (Lvl::Ok, format!("snp alias (snp {v})")),
            Some(v) => (
                Lvl::Warn,
                format!("snp alias (snp {v}): stale; run `snp self alias install --force`"),
            ),
            None => match line.strip_prefix("pre-commit ") {
                Some(v) => (Lvl::Info, format!("Python pre-commit {v}")),
                None => (Lvl::Info, "unknown".to_owned()),
            },
        },
        None => (Lvl::Info, "unknown".to_owned()),
    }
}

/// pre-commit's own store (`store.py:25-36`): `PRE_COMMIT_HOME`, else `$XDG_CACHE_HOME/pre-commit`, else
/// `~/.cache/pre-commit`.
fn pre_commit_store(cx: &CmdCtx<'_>) -> Option<PathBuf> {
    if let Some(h) = cx.env.get_nonempty("PRE_COMMIT_HOME") {
        return Some(cx.cwd.join(h));
    }
    if let Some(x) = cx.env.get_nonempty("XDG_CACHE_HOME") {
        return Some(cx.cwd.join(x).join("pre-commit"));
    }
    snp_os::home::default_cache_root(cx.env).map(|c| c.join("pre-commit"))
}

/// The first line of the snp 1.x README (11 §A.10.3).
const V1_README: &str = "This directory is maintained by the SNP (Shell Not Pass) project.";

/// Checks 1–6 (11 §A.15): snp, the stores, the names on PATH, git.
fn global_checks(cx: &CmdCtx<'_>, r: &mut Report) {
    r.add(
        Lvl::Info,
        "snp",
        format!(
            "{} (pre-commit {} compatible), {}, invoked as {}",
            profile::SNP_VERSION,
            profile::PRE_COMMIT,
            lossy(cx.self_exe),
            cx.brand.prog()
        ),
    );
    let loc = cx.store.location();
    let source = match loc.source {
        StoreSource::SnpHome => "SNP_HOME",
        StoreSource::XdgCacheHome => "XDG_CACHE_HOME",
        _ => "default",
    };
    r.add(
        Lvl::Info,
        "store",
        format!("{} (from {source})", lossy(&loc.path)),
    );
    if cx.store.exists() {
        if tempfile::tempfile_in(&loc.path).is_err() {
            r.add(
                Lvl::Warn,
                "store",
                format!("{} is not writable", lossy(&loc.path)),
            );
        }
        let readme = snp_os::fs::read_capped(&loc.path.join("README"), 256).unwrap_or_default();
        if String::from_utf8_lossy(&readme).lines().next() == Some(V1_README) {
            r.add(
                Lvl::Info,
                "store",
                "snp 1.x cache data is present; `snp gc` removes it",
            );
        }
    }
    if let Some(pc) = pre_commit_store(cx)
        && pc.is_dir()
    {
        r.add(
            Lvl::Info,
            "pre-commit cache",
            format!(
                "{} exists; snp does not use it (`pre-commit clean` removes it)",
                lossy(&pc)
            ),
        );
    }
    match on_path(cx, "pre-commit") {
        Some(p) => {
            let (lvl, kind) = pre_commit_kind(&p, cx.self_exe);
            r.add(lvl, "pre-commit on PATH", format!("{} ({kind})", lossy(&p)));
        }
        None => r.add(Lvl::Info, "pre-commit on PATH", "not found"),
    }
    match on_path(cx, "snp") {
        Some(p) if scripts::is_snp(&p, cx.self_exe) => {
            r.add(Lvl::Ok, "snp on PATH", lossy(&p));
        }
        Some(p) => r.add(
            Lvl::Warn,
            "snp on PATH",
            format!("{} is not this snp ({})", lossy(&p), lossy(cx.self_exe)),
        ),
        None => r.add(Lvl::Info, "snp on PATH", "not found"),
    }
    match cx.git_exe {
        Some(exe) => match exe.caps.version {
            Some(v) if (v.major, v.minor) < (2, 31) => r.add(
                Lvl::Warn,
                "git",
                format!("{v} is older than 2.31; some capabilities use fallbacks"),
            ),
            Some(v) => r.add(Lvl::Ok, "git", v.to_string()),
            None => r.add(Lvl::Warn, "git", "the version could not be determined"),
        },
        None => r.add(Lvl::Error, "git", "not found"),
    }
}

/// Whether `py -c "<find_spec(module)>"` exits 0 (10 s, inherited env, cwd = the repository root; 11 §A.19.8).
fn python_has(py: &Path, module: &str, root: &Path) -> Option<bool> {
    let code = format!(
        "import importlib.util,sys; sys.exit(0 if importlib.util.find_spec('{module}') else 1)"
    );
    let (status, _) =
        scripts::probe_output(py, &[OsStr::new("-c"), OsStr::new(&code)], Some(root))?;
    Some(status == Some(0))
}

/// The bytes of a parsed path as a path.
fn path_of(b: &[u8]) -> PathBuf {
    PathBuf::from(frame::os_from_bytes(b))
}

/// Check 8 for one hook type; `pythons` collects the distinct `INSTALL_PYTHON`s (check 13).
fn hook_check(
    cx: &CmdCtx<'_>,
    f: &Frame,
    disp: &str,
    path: &Path,
    pythons: &mut BTreeSet<PathBuf>,
    r: &mut Report,
) {
    let owner = classify(path).unwrap_or(Owner::Foreign);
    let parsed = || {
        let bytes = snp_os::fs::read_capped(path, 1 << 20).unwrap_or_default();
        parse_script_args(&owner, &bytes)
    };
    match &owner {
        Owner::Missing => {}
        Owner::NotAFile => r.add(Lvl::Error, disp, "is not a file"),
        Owner::Snp { stale } => {
            let args = parsed();
            let exe_ok = args
                .as_ref()
                .and_then(|p| p.snp_exe.as_ref())
                .is_some_and(|e| is_executable(&path_of(e)));
            if args.is_none() {
                // `--fix` would lose its `--config`: never rewritten (fix_one).
                r.add(
                    Lvl::Warn,
                    disp,
                    "an snp hook script whose options cannot be read; `--fix` leaves it alone",
                );
            } else if *stale {
                r.fixable(
                    Lvl::Warn,
                    disp,
                    "an older snp hook script; `--fix` rewrites it",
                );
            } else if !exe_ok {
                r.fixable(
                    Lvl::Warn,
                    disp,
                    "its SNP_EXE is not executable (commits fall back to snp/pre-commit on PATH)",
                );
            } else {
                r.add(Lvl::Ok, disp, "runs snp");
            }
        }
        Owner::SnpV1 => r.fixable(Lvl::Warn, disp, "runs snp 1.x, not snp"),
        Owner::Prek => match parsed() {
            Some(p) if !p.prek_only.is_empty() => r.add(
                Lvl::Warn,
                disp,
                format!(
                    "runs prek with prek-only options ({}); cannot adopt",
                    p.prek_only.join(" ")
                ),
            ),
            _ => r.fixable(Lvl::Warn, disp, "runs prek, not snp"),
        },
        Owner::PreCommit { .. } => {
            let py = parsed()
                .and_then(|p| p.install_python)
                .filter(|p| !p.is_empty())
                .map(|p| path_of(&p));
            match py {
                Some(py) if is_executable(&py) => {
                    pythons.insert(py.clone());
                    if python_has(&py, "pre_commit.__main__", f.root()) == Some(true) {
                        r.fixable(Lvl::Warn, disp, "runs Python pre-commit, not snp");
                    } else {
                        r.fixable(
                            Lvl::Error,
                            disp,
                            format!("commits fail: {} cannot import pre_commit", lossy(&py)),
                        );
                    }
                }
                _ => match on_path(cx, "pre-commit") {
                    Some(p) => {
                        let (_, kind) = pre_commit_kind(&p, cx.self_exe);
                        r.fixable(
                            Lvl::Warn,
                            disp,
                            format!("depends on the pre-commit found on PATH ({kind})"),
                        );
                    }
                    None => r.fixable(
                        Lvl::Error,
                        disp,
                        "commits fail: nothing named pre-commit on PATH",
                    ),
                },
            }
        }
        Owner::Lefthook => r.add(Lvl::Info, disp, "runs lefthook"),
        _ => r.add(Lvl::Info, disp, "a user hook script"),
    }
}

/// Checks 7–13 for the repository at `f`.
fn repo_checks(cx: &CmdCtx<'_>, f: &Frame, dir: &HooksDir, stash: bool, fix: bool, r: &mut Report) {
    let hooks_path = f.git.hooks_path().ok().flatten();
    if let Some(hp) = &hooks_path {
        let v = hp.to_string_lossy();
        let husky = if v.ends_with(".husky/_") || v.ends_with(".husky") {
            " (husky)"
        } else {
            ""
        };
        r.add(
            Lvl::Warn,
            "core.hooksPath",
            format!("install refuses while core.hooksPath is set ({v}){husky}"),
        );
    }
    let mut pythons = BTreeSet::new();
    let mut manager_events: BTreeSet<&'static str> = BTreeSet::new();
    for t in HookType::ALL {
        let (disp, legacy_disp, path, legacy) = dir.paths(t);
        let disp = disp.to_string_lossy().into_owned();
        let owner = classify(&path).unwrap_or(Owner::Foreign);
        if owner.is_manager() {
            manager_events.insert(t.as_str());
        }
        hook_check(cx, f, &disp, &path, &mut pythons, r);
        let lowner = classify(&legacy).unwrap_or(Owner::Missing);
        let ldisp = legacy_disp.to_string_lossy().into_owned();
        if lowner.is_manager() {
            r.fixable(
                Lvl::Error,
                ldisp,
                format!(
                    "is a {} hook script (double run or blocked commits)",
                    lowner.owner_name().unwrap_or("snp")
                ),
            );
        } else if !matches!(lowner, Owner::Missing) {
            r.add(
                Lvl::Info,
                ldisp,
                "migration mode (the legacy hook runs first)",
            );
        }
    }
    if f.exe.caps.config_hooks
        && let Ok(cmds) = f.git.config_get_regexp(r"^hook\..*\.command$")
    {
        for (key, value) in cmds {
            let v = value.to_string_lossy();
            if !v.contains("hook-impl") {
                continue;
            }
            let key = key.to_string_lossy().into_owned();
            let name = key
                .strip_prefix("hook.")
                .and_then(|k| k.strip_suffix(".command"))
                .unwrap_or_default()
                .to_owned();
            let event = f
                .git
                .config_get(&format!("hook.{name}.event"))
                .ok()
                .flatten()
                .map_or_default(|e| e.to_string_lossy().into_owned());
            if manager_events.contains(event.as_str()) {
                r.add(
                    Lvl::Error,
                    key,
                    format!(
                        "also runs a manager for {event}, as does the hooks-dir script; hooks run twice"
                    ),
                );
            } else {
                r.add(Lvl::Info, key, v.into_owned());
            }
        }
    }
    if let Ok(Some(tmpl)) = f.git.config_get("init.templateDir") {
        let tdir = PathBuf::from(snp_os::home::expanduser(&tmpl, cx.env));
        for t in HookType::ALL {
            let p = tdir.join("hooks").join(t.as_str());
            let owner = classify(&p).unwrap_or(Owner::Foreign);
            if matches!(owner, Owner::Missing) {
                continue;
            }
            let what = owner.owner_name().map_or_else(
                || {
                    if matches!(owner, Owner::Snp { .. }) {
                        "snp".to_owned()
                    } else {
                        "a user hook script".to_owned()
                    }
                },
                str::to_owned,
            );
            r.add(Lvl::Info, format!("init.templateDir {}", lossy(&p)), what);
        }
    }
    for py in &pythons {
        if python_has(py, "pre_commit.main", f.root()) == Some(true)
            && python_has(py, "snp_pre_commit_compat", f.root()) == Some(true)
        {
            r.add(
                Lvl::Error,
                "python",
                format!(
                    "snp-pre-commit-compat and pre-commit are both installed in {}",
                    lossy(py)
                ),
            );
        }
    }
    config_check(cx, f, r);
    stash_checks(cx, f, stash, fix, r);
}

/// Check 12: `minimum_pre_commit_version` above 4.6.2 and `pre-commit …` entries that need the name on PATH.
fn config_check(cx: &CmdCtx<'_>, f: &Frame, r: &mut Report) {
    let regexes = snp_pyre::RegexCache::default();
    let quiet = snp_core::output::CollectWarnings::new();
    let o = snp_config::LoadOpts {
        brand: cx.brand,
        ext: frame::ext_mode(cx),
        regexes: &regexes,
        warn: &quiet,
        diag: &quiet,
    };
    let disp = f.config_display.to_string_lossy().into_owned();
    let cfg = match snp_config::load_config(&f.source(), &o) {
        Ok(c) => c,
        Err(e) => {
            let first = e
                .rendered
                .trim()
                .lines()
                .last()
                .unwrap_or_default()
                .to_owned();
            r.add(
                Lvl::Warn,
                format!("config {disp}"),
                format!("does not load: {first}"),
            );
            return;
        }
    };
    if version_gt(&cfg.minimum_pre_commit_version, profile::PRE_COMMIT) {
        r.add(
            Lvl::Warn,
            format!("config {disp}"),
            format!(
                "minimum_pre_commit_version {} is above {}",
                cfg.minimum_pre_commit_version,
                profile::PRE_COMMIT
            ),
        );
    }
    for repo in &cfg.repos {
        if let snp_config::RepoConfig::Local { hooks } = repo {
            for h in hooks {
                let e = h.entry.trim_start();
                if (e.starts_with("pre-commit ") || &*h.entry == "pre-commit")
                    && !e.starts_with("pre-commit hazmat")
                {
                    r.add(
                        Lvl::Info,
                        format!("hook {}", h.id),
                        "its entry runs `pre-commit`, which must be on PATH",
                    );
                }
            }
        }
    }
}

/// Dotted-version comparison `a > b` (numeric components; a non-numeric component compares as 0).
fn version_gt(left: &str, right: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split('.').map(|c| c.parse().unwrap_or(0)).collect() };
    let (mut lhs, mut rhs) = (parse(left), parse(right));
    let len = lhs.len().max(rhs.len());
    lhs.resize(len, 0);
    rhs.resize(len, 0);
    lhs > rhs
}

/// The built-in stash-recovery check: journaled runs of this worktree that never restored; with `--stash`, each
/// one's patch is also verified against the worktree (`git apply --check --reverse`). With `--fix --stash` every
/// such run is dismissed first (ARCHITECTURE §4.6, ADR-083): its journal is retired so the recovery notice stops,
/// and the patch is kept and named.
fn stash_checks(cx: &CmdCtx<'_>, f: &Frame, verify: bool, fix: bool, r: &mut Report) {
    let patches = cx.store.patches_dir();
    if fix && verify {
        for d in snp_stash::dismiss_unrestored(&f.git, &f.info.git_dir, &patches, f.root()) {
            let state = if d.present {
                "its changes are in the worktree"
            } else {
                "its changes are NOT in the worktree"
            };
            r.add(
                Lvl::Ok,
                format!("stash {}", lossy(&d.patch)),
                format!(
                    "recovery notice dismissed ({state}); the patch is kept: git apply --whitespace=nowarn {} still applies \
                     it",
                    lossy(&d.patch)
                ),
            );
            if let Some(ita) = d.ita_notice {
                r.add(Lvl::Info, "stash intent-to-add", ita);
            }
        }
    }
    let found = snp_stash::recovery_scan(&f.info.git_dir, &patches, f.root());
    if found.is_empty() {
        r.add(Lvl::Ok, "stash", "no unrestored stash");
        return;
    }
    for u in &found {
        let subject = format!("stash {}", lossy(&u.patch));
        if verify && snp_stash::verify_restored(&f.git, &u.patch).is_ok_and(|present| present) {
            r.add(
                Lvl::Warn,
                subject,
                "its changes are present in the worktree; the next run retires the journal (or `snp doctor --fix \
                 --stash` now)",
            );
            continue;
        }
        r.add(
            Lvl::Error,
            subject,
            format!(
                "unrestored changes (state {}): {}; `snp doctor --fix --stash` dismisses the notice and keeps the \
                 patch",
                u.state,
                u.hint().trim_end()
            ),
        );
    }
}

/// `--fix` for one hook type (11 §A.5.2 "doctor --fix"): another manager's script (or a stale snp script) is
/// rewritten keeping its parsed `--config` / skip flag (M1, with ` (could not read its options; defaults used)`
/// when they cannot be read); a manager `.legacy` is deleted (M2, by `install_one`).
fn fix_one(cx: &CmdCtx<'_>, dir: &HooksDir, t: HookType, exe: &Path) -> Result<(), Failure> {
    let (_, _, path, legacy) = dir.paths(t);
    let owner = classify(&path).unwrap_or(Owner::Foreign);
    let bytes = snp_os::fs::read_capped(&path, 1 << 20).unwrap_or_default();
    let parsed: Option<ScriptArgs> = parse_script_args(&owner, &bytes);
    let rewrite = match &owner {
        // An snp script whose options cannot be read is left alone: rewriting it would drop its `--config`.
        Owner::Snp { stale } => parsed.as_ref().is_some_and(|p| {
            *stale
                || !p
                    .snp_exe
                    .as_ref()
                    .is_some_and(|e| is_executable(&path_of(e)))
        }),
        Owner::SnpV1 | Owner::PreCommit { .. } => true,
        Owner::Prek => parsed.as_ref().is_none_or(|p| p.prek_only.is_empty()),
        _ => false,
    };
    let legacy_manager = classify(&legacy).is_ok_and(|o| o.is_manager());
    if !rewrite && !legacy_manager {
        return Ok(());
    }
    if !rewrite {
        // Only the `.legacy` is a manager script: delete it (M2), leave the hook alone.
        let lowner = classify(&legacy).unwrap_or(Owner::Foreign);
        fs::remove_file(&legacy)
            .map_err(|e| Failure::from_io(&e, Some(legacy.as_os_str()), None))?;
        let (_, legacy_disp, _, _) = dir.paths(t);
        let mut m2 = messages::note(
            cx.brand,
            messages::M2,
            &[
                ("legacy_path", messages::os_bytes(&legacy_disp)),
                ("owner", lowner.owner_name().unwrap_or("snp").as_bytes()),
            ],
        );
        m2.push(b'\n');
        cx.printer.write_stderr(&m2);
        return Ok(());
    }
    let default_cfg = OsString::from(".pre-commit-config.yaml");
    let config: OsString = parsed
        .as_ref()
        .and_then(|p| p.config.as_ref())
        .map_or(default_cfg, |c| frame::os_from_bytes(c));
    let skip = parsed.as_ref().is_some_and(|p| p.skip_on_missing_config);
    let opts = WriteOpts {
        config: &config,
        overwrite: false,
        skip_missing: skip,
        exe,
        note_defaults: true,
    };
    install::install_one(cx, dir, t, &opts, Some((&config, skip)))
}

/// `snp doctor [--fix] [--fast-path] [--stash]`.
///
/// The checks of 11 §A.15 (snp, the stores, `pre-commit`/`snp` on PATH, git, `core.hooksPath`, every hook type and
/// its `.legacy`, git config hooks, `init.templateDir`, the config, the Python compat collision), the built-in
/// stash-recovery check (`--stash` verifies each unrestored patch), then every injected check (the binary passes the
/// fast-path table for `--fast-path`; a failing injected line is a `warn`). The last line is
/// `{n} problem(s): {e} error(s), {w} warning(s).`, plus ``Run `snp doctor --fix` …`` when `--fix` could repair
/// something. Exit 1 when any `error` remains.
pub fn doctor(a: &DoctorArgs, cx: &CmdCtx<'_>, extra: &[&dyn DoctorCheck]) -> Result<i32, Failure> {
    let frame = cx
        .git_exe
        .is_some()
        .then(|| frame::git_frame_untracked(cx, &a.config).ok())
        .flatten();
    let dir = match &frame {
        Some(f) => Some(HooksDir {
            display: frame::common_dir_display(cx, f)?,
            path: f.info.common_dir.clone(),
        }),
        None => None,
    };
    if a.fix
        && let (Some(f), Some(d)) = (&frame, &dir)
    {
        if f.git.hooks_path()?.is_some() {
            cx.printer.write_line(
                b"warn  core.hooksPath: --fix skipped: install refuses while core.hooksPath is set",
            );
        } else {
            let exe = scripts::select_exe(cx.brand, cx.arg0, cx.env, cx.self_exe);
            for t in HookType::ALL {
                fix_one(cx, d, t, &exe)?;
            }
        }
    }
    let mut r = Report::default();
    global_checks(cx, &mut r);
    match (&frame, &dir) {
        (Some(f), Some(d)) => repo_checks(cx, f, d, a.stash, a.fix, &mut r),
        _ => r.add(Lvl::Info, "repository", "not inside a git repository"),
    }
    for c in extra {
        for line in c.run(cx) {
            let lvl = if line.ok { Lvl::Ok } else { Lvl::Warn };
            match line.fix {
                Some(fix) => r.fixable(lvl, c.name(), format!("{} ({fix})", line.text)),
                None => r.add(lvl, c.name(), line.text),
            }
        }
        if let Some(f) = frame::interrupted(cx) {
            return Err(f);
        }
    }
    let (mut errors, mut warnings) = (0_usize, 0_usize);
    let mut fixable = false;
    for f in &r.0 {
        cx.printer.write_line(render_line(f).as_bytes());
        match f.lvl {
            Lvl::Error => errors = errors.saturating_add(1),
            Lvl::Warn => warnings = warnings.saturating_add(1),
            _ => {}
        }
        fixable |= f.fixable && f.lvl >= Lvl::Warn;
    }
    let mut summary = messages::doctor_summary(errors, warnings);
    if fixable && !a.fix {
        summary.push(' ');
        summary.push_str(&messages::doctor_fix_hint(cx.brand));
    }
    cx.printer.write_line(summary.as_bytes());
    Ok(i32::from(errors != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_shape() {
        let f = Finding {
            lvl: Lvl::Ok,
            subject: "git".into(),
            message: "2.43.0".into(),
            fixable: false,
        };
        assert_eq!(render_line(&f), "ok    git: 2.43.0");
        let f = Finding {
            lvl: Lvl::Error,
            subject: ".git/hooks/pre-commit".into(),
            message: "x".into(),
            fixable: true,
        };
        assert_eq!(render_line(&f), "error .git/hooks/pre-commit: x");
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_gt("5", "4.6.2"));
        assert!(version_gt("4.10.0", "4.6.2"));
        assert!(!version_gt("4.6.2", "4.6.2"));
        assert!(!version_gt("0", "4.6.2"));
        assert!(!version_gt("4.6", "4.6.2"));
    }
}
