//! Shared steps of the git-requiring commands (`run`, `hook-impl`, `try-repo`, `install-hooks`, `list`, doctor):
//! repository discovery, `_adjust_args_and_chdir` as a pure function, config tracking, config loading with
//! buffered warnings, the overlay, and the resolution context (ARCHITECTURE §3.1 steps 3–5, 10, 11).
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_config::overlay::{self, Overlay};
use snp_config::{Config, ConfigSource, ExtMode, LoadOpts, RepoConfig};
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::{CollectWarnings, DiagSink, Level, Printer, WarnSink};
use snp_git::{Git, RepoInfo};
use snp_hooks::{Epoch, NativeSource};
use snp_lang::Host;
use snp_repo::{FastPathPolicy, Preflight, ResolveCtx, StoreTools};
use snp_store::{Store, StoreGuard};

use crate::RunCtx;

/// `GitExe::resolve` then `Git::discover` (§3.1 step 3): one combined `rev-parse`; `FatalError` texts unchanged.
///
/// `git.get_root()` (`git.py:51-73`) from the invocation cwd with the startup environment, then the version gate
/// (< 2.31 warns once on stderr, D-DIAG-01; never refuses, ADR-063).
pub(crate) fn discover(cx: &RunCtx) -> Result<(Git, RepoInfo), Failure> {
    let (git, info) = Git::discover(
        Arc::clone(&cx.git_exe),
        &cx.env,
        &cx.invocation_cwd,
        Arc::clone(&cx.git_backend),
    )?;
    cx.git_exe.version_gate(&cx.printer, &cx.env)?;
    Ok((git.with_brand(cx.brand), info))
}

/// `os.path.abspath(p)` against the invocation cwd, then `os.path.relpath(p)` against the root (`main.py:175-199`).
///
/// A cross-drive `relpath` (Windows `ValueError`, exit 3 in pre-commit) keeps the absolute path (D-PLAT-06).
pub(crate) fn rebase_path(p: &OsStr, invocation_cwd: &Path, root: &Path) -> OsString {
    let abs = snp_pycompat::ospath::abspath(p, invocation_cwd);
    snp_pycompat::ospath::relpath(abs.as_os_str(), None, root)
        .unwrap_or_else(|_| abs.into_os_string())
}

/// The config path after `_adjust_args_and_chdir`: absolute-then-relative when it exists relative to the
/// invocation cwd, else unchanged (and then interpreted relative to the root).
pub(crate) fn adjust_config(config: &OsStr, invocation_cwd: &Path, root: &Path) -> OsString {
    if invocation_cwd.join(config).exists() {
        rebase_path(config, invocation_cwd, root)
    } else {
        // `os.path.relpath(config)` after the chdir: a relative value is relative to the root
        rebase_path(config, root, root)
    }
}

/// `ConfigSource` for a root-relative (or absolute) config path.
pub(crate) fn config_source(config: &OsStr, root: &Path) -> ConfigSource {
    ConfigSource {
        path: root.join(config),
        display: config.to_os_string(),
        project_root: root.to_path_buf(),
    }
}

/// `store.mark_config_used(config)` (`main.py:384`, §3.1 step 5): best effort, never creates the store.
pub(crate) fn mark_config_used(store: &Store, cfg: &Path, root: &Path) {
    if let Ok(real) = snp_pycompat::ospath::realpath(cfg.as_os_str(), root) {
        store.mark_config_used(&real, root);
    }
}

/// `SNP_COMPAT_STRICT=1` ⇒ extension keys are unknown keys (ADR-039).
pub(crate) fn ext_mode(env: &Env) -> ExtMode {
    if env.is_truthy_flag("SNP_COMPAT_STRICT") {
        ExtMode::Strict
    } else {
        ExtMode::Enabled
    }
}

/// Warnings and diagnostics collected during a speculative load, replayed at pre-commit's position.
#[derive(Debug, Default)]
pub(crate) struct Buffered {
    /// `[WARNING] …` lines (stdout).
    pub(crate) warn: CollectWarnings,
    /// stderr diagnostics.
    pub(crate) diag: CollectWarnings,
}

impl Buffered {
    /// Prints what was collected, in order.
    pub(crate) fn replay(&self, printer: &Printer) {
        for w in self.warn.take() {
            printer.warn(&w);
        }
        for d in self.diag.take() {
            printer.diag(&d);
        }
    }
}

/// `load_config(config)` with the warnings going to `warn` / `diag`.
pub(crate) fn load_config_with(
    cx: &RunCtx,
    src: &ConfigSource,
    warn: &dyn WarnSink,
    diag: &dyn DiagSink,
) -> Result<Config, Failure> {
    let opts = LoadOpts {
        brand: cx.brand,
        ext: ext_mode(&cx.env),
        regexes: &cx.regexes,
        warn,
        diag,
    };
    snp_config::load_config(src, &opts).map_err(Failure::from)
}

/// The local overlay (§8.4): located next to the config unless `--no-local-config`, strict mode or CI; a located
/// overlay is loaded (errors are the config's `InvalidConfigError` texts) and its root items applied.
pub(crate) fn apply_overlay(
    cx: &RunCtx,
    cfg: &mut Config,
    git: &Git,
    no_local_config: bool,
) -> Result<Option<Overlay>, Failure> {
    if no_local_config || ext_mode(&cx.env) == ExtMode::Strict {
        return Ok(None);
    }
    let is_tracked = |p: &Path| is_tracked(git, p);
    let path = match overlay::locate(&cfg.source.path, &cx.env, &is_tracked) {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(None),
        Err(note) => {
            cx.printer.diag(&note);
            return Ok(None);
        }
    };
    let opts = LoadOpts {
        brand: cx.brand,
        ext: ExtMode::Enabled,
        regexes: &cx.regexes,
        warn: &*cx.printer,
        diag: &*cx.printer,
    };
    let ov = overlay::load(&path, &opts).map_err(Failure::from)?;
    overlay::apply_root(cfg, &ov, &*cx.printer);
    Ok(Some(ov))
}

/// Whether `p` is tracked (`git ls-files -- :(literal)<p>` prints it): a literal pathspec, so glob characters in the
/// repository or config path (`a[12]/`, `x*`) cannot match another tracked file. `p` names an existing regular file
/// (`overlay::locate` checks `is_file`), so a literal pathspec matches at most that one entry.
fn is_tracked(git: &Git, p: &Path) -> bool {
    let mut spec = OsString::from(":(literal)");
    spec.push(p.as_os_str());
    let args = [
        OsStr::new("ls-files"),
        OsStr::new("-z"),
        OsStr::new("--"),
        spec.as_os_str(),
    ];
    git.run_raw(&args)
        .is_ok_and(|c| c.status.0 == 0 && !c.stdout.is_empty())
}

/// Whether the config names a remote repository (the shared store lock is taken before resolution, §5.5).
pub(crate) fn uses_store(cfg: &Config) -> bool {
    cfg.repos
        .iter()
        .any(|r| matches!(r, RepoConfig::Remote { .. }))
}

/// The shared store lock while remote repos are in use (§5.5), creating the store first; the brand's `Locking …`
/// line is printed when the lock is contended (`store.py:106`).
pub(crate) fn store_lock(cx: &RunCtx, store: &Store) -> Result<StoreGuard, Failure> {
    store.ensure_created(cx.brand, &*cx.printer)?;
    let printer = Arc::clone(&cx.printer);
    let brand = cx.brand;
    let mut on_wait = move || printer.log(Level::Info, snp_store::locking_message(brand));
    store.shared(&mut on_wait, &cx.cancel)
}

/// G5 (§7.2, ADR-070): `snp_hooks::native(source, id).map(|h| h.preflight(epoch, args))`; an unknown native
/// delegates.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct NativePreflight;

impl Preflight for NativePreflight {
    fn check(
        &self,
        source: NativeSource,
        id: &str,
        epoch: Epoch,
        args: &[Arc<str>],
    ) -> Result<(), String> {
        match snp_hooks::native(source, id) {
            Some(h) => h.preflight(epoch, args),
            None => Err(format!("no native implementation of {id}")),
        }
    }
}

/// The fast-path policy (§7.2 G1): `--no-fast-path` / `SNP_NO_FAST_PATH` turn it off.
pub(crate) fn fast_path_policy(
    cfg: &Config,
    env: &Env,
    no_fast_path: bool,
    with_fallback: bool,
) -> FastPathPolicy {
    let mode = if no_fast_path || env.is_truthy_flag("SNP_NO_FAST_PATH") {
        snp_config::ext::FastPath::Off
    } else {
        cfg.root_ext.fast_path
    };
    FastPathPolicy {
        mode,
        exclude: cfg.root_ext.fast_path_exclude.clone(),
        coalesce: cfg.root_ext.fast_path_coalesce,
        with_fallback,
    }
}

/// Everything a `ResolveCtx` borrows, owned for the duration of one command.
#[derive(Debug)]
pub(crate) struct ResolveParts<'a> {
    /// The command's environment (`run_env` for `run`).
    pub(crate) env: Env,
    /// The project root (`Host::cwd`).
    pub(crate) root: PathBuf,
    /// The store used for resolution (the throwaway store for try-repo).
    pub(crate) store: &'a Store,
    /// The tool directories over that store.
    pub(crate) tools: StoreTools<'a>,
    /// G5.
    pub(crate) preflight: NativePreflight,
}

impl<'a> ResolveParts<'a> {
    pub(crate) fn new(env: Env, root: PathBuf, store: &'a Store) -> Self {
        Self {
            env,
            root,
            store,
            tools: StoreTools(store),
            preflight: NativePreflight,
        }
    }

    /// The `ResolveCtx` (§1.4.20) for `cfg`.
    #[expect(
        clippy::too_many_arguments,
        reason = "one borrow per ResolveCtx field group"
    )]
    pub(crate) fn ctx<'b>(
        &'b self,
        cx: &'b RunCtx,
        cfg: &'b Config,
        printer: &'b Printer,
        warn: &'b dyn WarnSink,
        fast_path: FastPathPolicy,
        overlay: Option<&'b Overlay>,
        cancel: &'b snp_core::cancel::CancelToken,
    ) -> ResolveCtx<'b> {
        ResolveCtx {
            cfg,
            store: self.store,
            git_exe: &cx.git_exe,
            git_backend: Arc::clone(&cx.git_backend),
            env: &self.env,
            printer,
            pool: &cx.pool,
            net: &cx.net,
            installs: &cx.installs,
            cancel,
            host: Host {
                base_env: &self.env,
                cwd: &self.root,
                self_exe: &cx.self_exe,
                probe: &cx.probe,
                fetch: cx.fetch.as_deref(),
                tools: &self.tools,
                runner: &*cx.runner,
                printer,
                cancel,
                brand: cx.brand,
            },
            fast_path,
            preflight: &self.preflight,
            overlay,
            regexes: &cx.regexes,
            warn,
            brand: cx.brand,
        }
    }
}

/// A `Printer` whose output goes nowhere (nested resolution inside meta hooks: pre-commit's subprocess has no
/// logging handler, so INFO records such as `Initializing environment` are dropped).
pub(crate) fn null_printer(cx: &RunCtx) -> Printer {
    Printer::new(
        Box::new(std::io::sink()),
        Box::new(std::io::sink()),
        snp_core::output::PrinterOpts {
            brand: cx.brand,
            color: false,
            stdout_is_tty: false,
            windows_text_mode: cfg!(windows),
            cancel: snp_core::cancel::CancelToken::new(),
        },
    )
}

/// `tempfile.TemporaryDirectory()`: a fresh 0700 directory under `tempfile.gettempdir()` (`TMPDIR`, `TEMP`, `TMP`
/// of the startup environment, else the platform default), removed recursively on drop.
#[derive(Debug)]
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates the directory (`tmp<random>`), retrying on a name collision.
    pub(crate) fn new(env: &Env) -> Result<Self, Failure> {
        let base = ["TMPDIR", "TEMP", "TMP"]
            .iter()
            .find_map(|k| env.get_nonempty(k))
            .map_or_else(default_temp, PathBuf::from);
        let mut last = None;
        for attempt in 0_u32..100 {
            let name = format!("tmp{}", random_suffix(attempt));
            let path = base.join(name);
            match create_private_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(Failure::from_io(&e, Some(path.as_os_str()), None)),
            }
        }
        let e = last.unwrap_or_else(|| std::io::Error::other("no usable temporary directory name"));
        Err(Failure::from_io(&e, Some(base.as_os_str()), None))
    }

    /// The directory.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // best effort, like `TemporaryDirectory.cleanup` with `ignore_cleanup_errors`
        drop(std::fs::remove_dir_all(&self.path));
    }
}

fn default_temp() -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir()
    } else {
        PathBuf::from("/tmp")
    }
}

/// 8 lowercase alphanumerics from the clock, the pid and the attempt (uniqueness is enforced by `create_new`).
fn random_suffix(attempt: u32) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::hash::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u32(attempt);
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    let mut n = h.finish();
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut s = String::new();
    for _ in 0..8 {
        let i = usize::try_from(n % 37).unwrap_or(0);
        s.push(char::from(alphabet.get(i).copied().unwrap_or(b'x')));
        n /= 37;
    }
    s
}

fn create_private_dir(p: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        b.mode(0o700);
    }
    b.create(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebase_keeps_order_and_dots() {
        let root = Path::new("/r");
        let cwd = Path::new("/r/sub");
        assert_eq!(rebase_path(OsStr::new("../b.txt"), cwd, root), "b.txt");
        assert_eq!(rebase_path(OsStr::new("./../a.txt"), cwd, root), "a.txt");
        assert_eq!(rebase_path(OsStr::new("foo.py"), cwd, root), "sub/foo.py");
        assert_eq!(rebase_path(OsStr::new("/r/x"), cwd, root), "x");
        assert_eq!(
            rebase_path(OsStr::new("/elsewhere/x"), cwd, root),
            "../elsewhere/x"
        );
    }

    #[test]
    fn missing_config_stays_root_relative() {
        let root = Path::new("/nonexistent-root");
        let cwd = Path::new("/nonexistent-root/sub");
        assert_eq!(
            adjust_config(OsStr::new(".pre-commit-config.yaml"), cwd, root),
            ".pre-commit-config.yaml"
        );
    }
}
