//! The command table (§10.1): one arm per [`Cli`] variant, calling the command crates' entry points with the
//! contexts the binary builds — `RunCtx` for snp-run (§3.1 step 2) and `CmdCtx` for snp-cmd — plus the
//! binary-sequenced commands of §1.4.22–§1.4.23 (`install --install-hooks`, `snp migrate lefthook --install`) and
//! the injected doctor check.
//!
//! `run_ctx` builds `RunCtx.probe` as `snp_lang::ToolProbe::new(persist_dir)` and `RunCtx.runner` as
//! `snp_lang::ProcRunner { cancel }` (the snp → snp-lang edge of §1.2, ADR-081).
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_cli::{Cli, DoctorArgs, InstallArgs, InstallHooksArgs, MigrateArgs, Shell};
use snp_cmd::{CmdCtx, DoctorCheck, DoctorLine};
use snp_core::clock::SystemClock;
use snp_core::failure::Failure;
use snp_core::output::{ColorSetting, Printer};
use snp_git::{GitBackend, GitExe, ProcBackend};
use snp_lang::{ProcRunner, ToolProbe};
use snp_proc::pool::{JobPool, Limiter};
use snp_proc::spawn::{OutputBudget, Role};
use snp_pyre::RegexCache;
use snp_run::RunCtx;
use snp_store::{Store, StoreLocation, StoreSource};

use crate::session::Session;

/// The global output budget (ADR-036): `SNP_OUTPUT_BUDGET` bytes, default 64 MiB.
const DEFAULT_OUTPUT_BUDGET: usize = 64 << 20;
/// The network limiter (§1.4.8).
const NET_PERMITS: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(n) => n,
    None => NonZeroUsize::MIN,
};
/// At most this many environment installs run at once (ADR-065).
const MAX_INSTALLS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(n) => n,
    None => NonZeroUsize::MIN,
};
/// Below this cgroup memory limit installs run one at a time (ADR-065).
const SMALL_MEMORY: u64 = 4 << 30;

/// Whether a command installs the signal handler, and with the Linux subreaper (run, hook-impl, try-repo: their
/// hook descendants are quiesced on the cancel path, ADR-065).
fn signal_policy(cli: &Cli) -> Option<bool> {
    match cli {
        Cli::Run(_) | Cli::HookImpl(_) | Cli::TryRepo(_) => Some(true),
        Cli::ValidateConfig(_)
        | Cli::ValidateManifest(_)
        | Cli::SampleConfig(_)
        | Cli::Completions(_)
        | Cli::Config(_)
        | Cli::List(_) => None,
        _ => Some(false),
    }
}

/// Installs the signal handler once per process (best effort: without it a signal keeps its default action).
fn install_signals(s: &Session, subreaper: bool) {
    if s.signals.get().is_some() {
        return;
    }
    if let Ok(guard) = snp_proc::signals::install(s.cancel.clone(), subreaper) {
        let _set: Result<(), Arc<snp_proc::signals::SignalGuard>> = s.signals.set(Arc::new(guard));
    }
}

/// What a command needs from the context builder.
#[derive(Debug, Clone, Copy)]
struct Needs {
    /// A resolved git (`git --version` once).
    git: bool,
    /// A store location that resolves (M7 refusal otherwise); commands that never touch the store run with a
    /// placeholder handle even where the location is refused (D-CRASH-17).
    store: bool,
}

const GIT_AND_STORE: Needs = Needs {
    git: true,
    store: true,
};
const STORE_ONLY: Needs = Needs {
    git: false,
    store: true,
};
const NOTHING: Needs = Needs {
    git: false,
    store: false,
};

/// The dispatch table: `pre_commit.main.main`'s `if args.command == …` chain plus the snp-brand commands.
pub(crate) fn run(cli: Cli, s: &Session) -> Result<i32, Failure> {
    if let Some(subreaper) = signal_policy(&cli) {
        install_signals(s, subreaper);
    }
    match cli {
        Cli::Run(a) => {
            let jobs = a.ext.jobs;
            snp_run::run(a, &run_ctx(s, jobs)?)
        }
        Cli::TryRepo(a) => {
            let jobs = a.run.ext.jobs;
            snp_run::try_repo(a, &run_ctx(s, jobs)?)
        }
        Cli::HookImpl(a) => {
            let cx = run_ctx(s, None)?;
            snp_run::hook_impl(a, &mut std::io::stdin().lock(), &cx)
        }
        Cli::InstallHooks(a) => snp_run::install_hooks(a, &run_ctx(s, None)?),
        Cli::List(a) => snp_run::list(a, &run_ctx(s, None)?),
        Cli::Install(a) => install(&a, s),
        Cli::Uninstall(a) => with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::uninstall(&a, c)),
        Cli::InitTemplatedir(a) => {
            with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::init_templatedir(&a, c))
        }
        Cli::Autoupdate(a) => with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::autoupdate(&a, c)),
        Cli::Clean(_) => with_cmd_ctx(s, STORE_ONLY, &snp_cmd::clean),
        Cli::Gc(_) => with_cmd_ctx(s, STORE_ONLY, &snp_cmd::gc),
        Cli::MigrateConfig(a) => {
            with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::migrate_config(&a, c))
        }
        Cli::SampleConfig(_) => with_cmd_ctx(s, NOTHING, &snp_cmd::sample_config),
        Cli::ValidateConfig(a) => with_cmd_ctx(s, NOTHING, &|c| snp_cmd::validate_config(&a, c)),
        Cli::ValidateManifest(a) => {
            with_cmd_ctx(s, NOTHING, &|c| snp_cmd::validate_manifest(&a, c))
        }
        Cli::Hazmat(a) => snp_hooks::hazmat::cli(&a.tool, &s.env, &s.cancel),
        Cli::Config(a) => with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::config(&a, c)),
        Cli::Cache(a) => with_cmd_ctx(s, STORE_ONLY, &|c| snp_cmd::cache(&a, c)),
        Cli::Migrate(a) => migrate(&a, s),
        Cli::Doctor(a) => doctor(&a, s),
        Cli::SelfCmd(a) => with_cmd_ctx(
            s,
            Needs {
                git: true,
                store: false,
            },
            &|c| snp_cmd::self_cmd(&a, c),
        ),
        Cli::Completions(shell) => Ok(completions(shell, &s.printer)),
        _ => Err(Failure::unexpected(
            "NotImplementedError",
            "command not wired into the dispatch table",
        )),
    }
}

/// `install` then, with `--install-hooks`, `install_hooks` (pre-commit's order; the first failure wins, ADR-070).
fn install(a: &InstallArgs, s: &Session) -> Result<i32, Failure> {
    let code = with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::install(a, c))?;
    if code != 0 || !a.install_hooks {
        return Ok(code);
    }
    let hooks = InstallHooksArgs {
        color: a.color,
        config: a.config.clone(),
        with_fallback: false,
    };
    snp_run::install_hooks(hooks, &run_ctx(s, None)?)
}

/// `completions <shell>`: the script generated by `xtask completions` (`snp_cli::COMPLETIONS`).
fn completions(shell: Shell, printer: &Printer) -> i32 {
    let wanted = std::mem::discriminant(&shell);
    match snp_cli::COMPLETIONS
        .iter()
        .find(|(s, _)| std::mem::discriminant(s) == wanted)
    {
        Some((_, script)) => {
            printer.write(script.as_bytes());
            0
        }
        None => 1,
    }
}

/// `SNP_JOBS`: a positive integer, else unset.
fn env_jobs(s: &Session) -> Option<NonZeroUsize> {
    s.env
        .get_nonempty("SNP_JOBS")
        .and_then(OsStr::to_str)
        .and_then(|v| v.trim().parse::<NonZeroUsize>().ok())
}

/// `SNP_OUTPUT_BUDGET` bytes (ADR-036), default 64 MiB.
fn output_budget(s: &Session) -> usize {
    s.env
        .get_nonempty("SNP_OUTPUT_BUDGET")
        .and_then(OsStr::to_str)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_OUTPUT_BUDGET)
}

/// The cgroup memory limit of this process, when one is set (Linux: v2 `memory.max`, v1
/// `memory.limit_in_bytes`). `None` elsewhere, when unlimited or unreadable.
fn cgroup_memory_limit() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let read = |p: &Path| -> Option<u64> {
            let b = snp_os::fs::read_capped(p, 64).ok()?;
            let t = String::from_utf8_lossy(&b);
            let t = t.trim();
            // v1 reports "unlimited" as a huge page-aligned number.
            t.parse::<u64>().ok().filter(|n| *n < (1_u64 << 62))
        };
        let groups = snp_os::fs::read_capped(Path::new("/proc/self/cgroup"), 64 << 10).ok()?;
        let groups = String::from_utf8_lossy(&groups);
        let root = Path::new("/sys/fs/cgroup");
        for line in groups.lines() {
            let mut parts = line.splitn(3, ':');
            let (Some(_id), Some(ctrl), Some(path)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let rel = path.trim_start_matches('/');
            if ctrl.is_empty() {
                if let Some(n) = read(&root.join(rel).join("memory.max")) {
                    return Some(n);
                }
            } else if ctrl.split(',').any(|c| c == "memory")
                && let Some(n) = read(&root.join("memory").join(rel).join("memory.limit_in_bytes"))
            {
                return Some(n);
            }
        }
        read(&root.join("memory.max"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The git executable (with the persisted version cache when the store exists); records the version line for the
/// crash log.
fn resolve_git(s: &Session, store: Option<&Store>) -> Result<GitExe, Failure> {
    let cwd = s.cwd()?;
    let cache = store
        .filter(|st| st.exists())
        .map(|st| st.cache_dir("git-version"));
    let exe = GitExe::resolve(&s.env, cwd, cache.as_deref())?;
    if let Some(v) = exe.caps.version {
        let _set: Result<(), String> = s.git_version.set(format!("git version {v}"));
    }
    Ok(exe)
}

/// Builds the run context (§3.1 step 2): store, git, the Tool-role backend, the resolver, signals, pool
/// (`pool_size`), limiters (net 8; installs min(N, 4), 1 under a small cgroup memory limit), output budget, probe,
/// runner, fetch.
fn run_ctx(s: &Session, explicit_jobs: Option<NonZeroUsize>) -> Result<RunCtx, Failure> {
    let cwd = s.cwd()?.to_path_buf();
    let store = Store::new(snp_store::resolve_location(&s.env, &cwd)?);
    let git_exe = Arc::new(resolve_git(s, Some(&store))?);
    git_exe.version_gate(&s.printer, &s.env)?;
    let cpu = snp_proc::xargs::cpu_count();
    let target = snp_proc::xargs::target_concurrency(&s.env, false, cpu);
    let permits = snp_proc::pool::pool_size(explicit_jobs.or_else(|| env_jobs(s)), target);
    let installs = if cgroup_memory_limit().is_some_and(|m| m < SMALL_MEMORY) {
        NonZeroUsize::MIN
    } else {
        permits.min(MAX_INSTALLS)
    };
    let persist = store.exists().then(|| store.cache_dir("probes"));
    let backend: Arc<dyn GitBackend> = Arc::new(ProcBackend {
        cancel: s.cancel.clone(),
        role: Role::Tool,
    });
    let fetch: Arc<dyn snp_net::Fetch> = Arc::new(snp_net::HttpFetch::from_env(&s.env));
    Ok(RunCtx {
        env: s.env.clone(),
        brand: s.brand,
        printer: Arc::new(s.printer.clone()),
        budget: OutputBudget::new(output_budget(s), store.spill_dir()),
        store,
        git_exe,
        git_backend: backend,
        resolver: Arc::new(snp_repo::StoreResolver::default()),
        cancel: s.cancel.clone(),
        signals: s.signals.get().cloned(),
        pool: JobPool::new(permits, s.cancel.clone()),
        net: Limiter::new(NET_PERMITS),
        installs: Limiter::new(installs),
        probe: Arc::new(ToolProbe::new(persist)),
        runner: Arc::new(ProcRunner {
            cancel: s.cancel.clone(),
        }),
        invocation_cwd: cwd,
        arg0: s.arg0.clone(),
        self_exe: s.self_exe.clone(),
        clock: Arc::new(SystemClock),
        fetch: Some(fetch),
        regexes: Arc::new(RegexCache::default()),
    })
}

/// Builds the command context (store, git, pool, `self_exe`, cwd) and runs `f` with it.
fn with_cmd_ctx(
    s: &Session,
    needs: Needs,
    f: &dyn Fn(&CmdCtx<'_>) -> Result<i32, Failure>,
) -> Result<i32, Failure> {
    let cwd = s.cwd()?;
    let loc = match snp_store::resolve_location(&s.env, cwd) {
        Ok(loc) => loc,
        Err(e) if needs.store => return Err(e),
        // Never created or written by these commands: a placeholder handle.
        Err(_) => StoreLocation {
            path: cwd.join(".snp-store-unavailable"),
            source: StoreSource::PlatformDefault,
        },
    };
    let store = Store::new(loc);
    let git_exe = if needs.git {
        resolve_git(s, Some(&store)).ok().map(Arc::new)
    } else {
        None
    };
    if let Some(exe) = &git_exe {
        exe.version_gate(&s.printer, &s.env)?;
    }
    let pool = JobPool::new(snp_proc::xargs::cpu_count(), s.cancel.clone());
    let cx = CmdCtx {
        env: &s.env,
        cwd,
        brand: s.brand,
        printer: &s.printer,
        store: &store,
        git_exe: git_exe.as_ref(),
        git_backend: Arc::new(ProcBackend {
            cancel: s.cancel.clone(),
            role: Role::Tool,
        }),
        cancel: &s.cancel,
        pool: &pool,
        self_exe: &s.self_exe,
        arg0: s.arg0.as_deref(),
    };
    f(&cx)
}

/// The injected `--fast-path` check: `snp_run::doctor_fast_path` over the doctor's config (ADR-070).
#[derive(Debug)]
struct FastPathCheck<'a> {
    session: &'a Session,
    config: OsString,
}

impl DoctorCheck for FastPathCheck<'_> {
    fn name(&self) -> &'static str {
        "fast path"
    }

    fn run(&self, _cx: &CmdCtx<'_>) -> Vec<DoctorLine> {
        let rows = run_ctx(self.session, None)
            .and_then(|rcx| snp_run::doctor_fast_path(&self.config, &rcx));
        match rows {
            // A config without remote or builtin hooks has no decision to show; say so instead of printing nothing.
            Ok(rows) if rows.is_empty() => vec![DoctorLine {
                ok: true,
                text: "no remote or builtin hooks".to_owned(),
                fix: None,
            }],
            Ok(rows) => rows
                .into_iter()
                .map(|(ok, text)| DoctorLine {
                    ok,
                    text,
                    fix: None,
                })
                .collect(),
            Err(f) => vec![DoctorLine {
                ok: false,
                text: format!("unavailable: {}", failure_summary(&f)),
                fix: None,
            }],
        }
    }
}

/// One line describing a failure (for injected doctor checks).
fn failure_summary(f: &Failure) -> String {
    match f {
        Failure::Exc { exc, .. } => {
            let text = String::from_utf8_lossy(&exc.text);
            let last = text.trim().lines().last().unwrap_or_default().to_owned();
            format!("{}: {last}", exc.ty)
        }
        other => format!("{other:?}"),
    }
}

/// `snp doctor`: `snp_cmd::doctor` with the injected fast-path check wrapping `snp_run::doctor_fast_path`.
fn doctor(a: &DoctorArgs, s: &Session) -> Result<i32, Failure> {
    let fast = FastPathCheck {
        session: s,
        config: a.config.clone(),
    };
    let extra: Vec<&dyn DoctorCheck> = if a.fast_path { vec![&fast] } else { Vec::new() };
    with_cmd_ctx(s, GIT_AND_STORE, &|c| snp_cmd::doctor(a, c, &extra))
}

/// Writes `bytes` to `path` atomically, refusing an existing file without `force`.
fn write_output(path: &Path, bytes: &[u8], force: bool) -> Result<(), Failure> {
    if !force && std::fs::symlink_metadata(path).is_ok() {
        return Err(Failure::fatal(
            "FatalError",
            format!("refusing to overwrite {}; use --force", path.display()),
        ));
    }
    snp_os::fs::atomic_replace(path, bytes, None)
        .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))
}

/// The repository root for `snp migrate lefthook` without `--root`: the git toplevel, else the cwd.
fn migrate_root(s: &Session, root: Option<&OsStr>) -> Result<PathBuf, Failure> {
    let cwd = s.cwd()?;
    if let Some(r) = root {
        return Ok(cwd.join(r));
    }
    let Ok(exe) = resolve_git(s, None) else {
        return Ok(cwd.to_path_buf());
    };
    let backend: Arc<dyn GitBackend> = Arc::new(ProcBackend {
        cancel: s.cancel.clone(),
        role: Role::Tool,
    });
    match snp_git::Git::discover(Arc::new(exe), &s.env, cwd, backend) {
        Ok((_, info)) => Ok(info.root),
        Err(_) => Ok(cwd.to_path_buf()),
    }
}

/// `--install` step 2: remove lefthook's shims (files in the hooks dir containing `LEFTHOOK`, restoring
/// `<hook>.old`), delete `.git/info/lefthook.checksum` and `.git/info/lefthook-remotes/`. Returns whether a removed
/// or restored script called git-lfs.
fn remove_lefthook_shims(git_dir: &Path) -> Result<bool, Failure> {
    let hooks = git_dir.join("hooks");
    let mut lfs = false;
    if let Ok(rd) = std::fs::read_dir(&hooks) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "old") || !p.is_file() {
                continue;
            }
            let bytes = snp_os::fs::read_capped(&p, 1 << 20).unwrap_or_default();
            if memchr_find(&bytes, b"LEFTHOOK").is_none() {
                continue;
            }
            std::fs::remove_file(&p)
                .map_err(|err| Failure::from_io(&err, Some(p.as_os_str()), None))?;
            let mut old = p.as_os_str().to_os_string();
            old.push(".old");
            let old = PathBuf::from(old);
            if old.is_file() {
                let ob = snp_os::fs::read_capped(&old, 1 << 20).unwrap_or_default();
                lfs |= memchr_find(&ob, b"git lfs").is_some()
                    || memchr_find(&ob, b"git-lfs").is_some();
                snp_os::fs::rename_with_retry(&old, &p).map_err(|err| {
                    Failure::from_io(&err, Some(old.as_os_str()), Some(p.as_os_str()))
                })?;
            }
        }
    }
    let info = git_dir.join("info");
    let _checksum: std::io::Result<()> = std::fs::remove_file(info.join("lefthook.checksum"));
    let remotes = info.join("lefthook-remotes");
    if remotes.is_dir() {
        snp_os::fs::remove_tree_within(&remotes, &info)
            .map_err(|err| Failure::from_io(&err, Some(remotes.as_os_str()), None))?;
    }
    Ok(lfs)
}

/// The first position of `needle` in `hay`.
fn memchr_find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

/// Whether the repository uses git-lfs (`filter=lfs` in its `.gitattributes`).
fn uses_lfs(root: &Path) -> bool {
    snp_os::fs::read_capped(&root.join(".gitattributes"), 1 << 20)
        .is_ok_and(|b| memchr_find(&b, b"filter=lfs").is_some())
}

/// `snp migrate …` (§1.4.23): convert, then for `lefthook --install` write atomically, remove lefthook's shims,
/// `git lfs update --force` when LFS is detected and `snp_cmd::install` every emitted stage.
fn migrate(a: &MigrateArgs, s: &Session) -> Result<i32, Failure> {
    match a {
        MigrateArgs::Lefthook {
            root,
            config,
            output,
            portable,
            local,
            strict,
            install,
            force,
        } => {
            let root = migrate_root(s, root.as_deref())?;
            let cwd = s.cwd()?;
            let to_stdout = output.as_deref().is_some_and(|o| o == "-");
            let out_path = match output.as_deref() {
                Some(o) if o != "-" => cwd.join(o),
                _ => root.join(".pre-commit-config.yaml"),
            };
            let opts = snp_migrate::LefthookOpts {
                root: root.clone(),
                config: config.as_deref().map(|c| cwd.join(c)),
                output: (!to_stdout).then(|| out_path.clone()),
                portable: *portable,
                local: *local,
                strict: *strict,
            };
            let converted = snp_migrate::lefthook(&opts)?;
            for line in &converted.report {
                s.printer.diag(line);
            }
            if to_stdout {
                s.printer.write(converted.config_yaml.as_bytes());
            } else {
                write_output(&out_path, converted.config_yaml.as_bytes(), *force)?;
                if let Some(ov) = &converted.overlay_yaml {
                    let ov_path = out_path
                        .parent()
                        .unwrap_or(&root)
                        .join(".pre-commit-config.local.yaml");
                    write_output(&ov_path, ov.as_bytes(), *force)?;
                }
            }
            if *install && !to_stdout {
                let code = migrate_install(s, &root, &out_path)?;
                if code != 0 {
                    return Ok(code);
                }
            }
            Ok(if *strict && converted.lossy { 2 } else { 0 })
        }
        MigrateArgs::Builtin { config } => rewrite_config(
            s,
            config,
            snp_migrate::to_builtin,
            Some(snp_migrate::builtin_not_converted),
        ),
        MigrateArgs::Upstream { config } => {
            rewrite_config(s, config, snp_migrate::to_upstream, None)
        }
        _ => Err(Failure::unexpected(
            "NotImplementedError",
            "migrate: subcommand not supported by this build",
        )),
    }
}

/// `--install`: shims → `git lfs update --force` (when LFS is used) → `snp install` for the emitted stages (the
/// written config's `default_install_hook_types`; the git-lfs hooks are chained as `.legacy`).
fn migrate_install(s: &Session, root: &Path, config: &Path) -> Result<i32, Failure> {
    let config = config.as_os_str().to_os_string();
    with_cmd_ctx(s, GIT_AND_STORE, &|c| {
        let Some(exe) = c.git_exe else {
            return Err(Failure::fatal(
                "FatalError",
                "git failed. Is it installed, and are you in a Git repository directory?",
            ));
        };
        let (git, info) =
            snp_git::Git::discover(Arc::clone(exe), c.env, root, Arc::clone(&c.git_backend))?;
        let shim_lfs = remove_lefthook_shims(&info.common_dir)?;
        if shim_lfs || uses_lfs(root) {
            let out = git.run_raw(&[
                OsStr::new("lfs"),
                OsStr::new("update"),
                OsStr::new("--force"),
            ])?;
            if out.status.0 != 0 {
                c.printer.diag(&format!(
                    "warning: `git lfs update --force` failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
        let args = InstallArgs {
            color: ColorSetting::Auto,
            config: config.clone(),
            overwrite: false,
            install_hooks: false,
            hook_types: Vec::new(),
            allow_missing_config: false,
            git_config_hooks: false,
        };
        let cx = CmdCtx {
            cwd: root,
            ..clone_cx(c)
        };
        snp_cmd::install(&args, &cx)
    })
}

/// A field-by-field copy of a command context.
fn clone_cx<'a>(c: &CmdCtx<'a>) -> CmdCtx<'a> {
    CmdCtx {
        env: c.env,
        cwd: c.cwd,
        brand: c.brand,
        printer: c.printer,
        store: c.store,
        git_exe: c.git_exe,
        git_backend: Arc::clone(&c.git_backend),
        cancel: c.cancel,
        pool: c.pool,
        self_exe: c.self_exe,
        arg0: c.arg0,
    }
}

/// Explains, one line each, what a migration leaves as it is.
type Explain = fn(&[u8]) -> Result<Vec<String>, Failure>;

/// `snp migrate builtin|upstream`: rewrite the config in place (atomically, mode kept).
fn rewrite_config(
    s: &Session,
    config: &OsStr,
    convert: fn(&[u8]) -> Result<Vec<u8>, Failure>,
    explain: Option<Explain>,
) -> Result<i32, Failure> {
    let path = s.cwd()?.join(config);
    // Never `fs::read`: `/dev/zero` or a FIFO named as the config would grow without bound (D-SEC-11).
    let bytes = snp_cmd::read_config_for_rewrite(&path, config)?;
    let new = convert(&bytes)?;
    if let Some(explain) = explain {
        // Say why a block stays as it is instead of only "already migrated" (ADR-084).
        for why in explain(&bytes)? {
            s.printer.py_print(&why);
        }
    }
    if new == bytes {
        s.printer.py_print("Configuration is already migrated.");
        return Ok(0);
    }
    snp_os::fs::atomic_replace(&path, &new, None)
        .map_err(|e| Failure::from_io(&e, Some(config), None))?;
    s.printer.py_print("Configuration has been migrated.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_bytes() {
        assert_eq!(memchr_find(b"abcLEFTHOOK", b"LEFTHOOK"), Some(3));
        assert_eq!(memchr_find(b"abc", b"LEFTHOOK"), None);
        assert_eq!(memchr_find(b"", b""), Some(0));
    }

    #[test]
    fn signal_policies() {
        assert_eq!(signal_policy(&Cli::Completions(Shell::Bash)), None);
    }
}
