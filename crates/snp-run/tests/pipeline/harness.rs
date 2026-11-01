//! The pipeline test harness: a real git repository in a temp dir, a hermetic environment, a capturing `Printer`, and
//! a fake `Resolve` that resolves `repo: local` / `repo: meta` hooks the way pre-commit does (`Prefix(os.getcwd())`,
//! `language_version: default`, no installs) and records the install calls.

use std::ffi::OsString;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use snp_cli::{RunArgs, RunExt};
use snp_config::RepoConfig;
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::clock::{Clock, SystemClock};
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::{ColorSetting, Printer, PrinterOpts};
use snp_core::stage::{HookType, Stage};
use snp_git::{GitExe, ProcBackend};
use snp_lang::{PlanCtx, ProcRunner, RunPlan, ToolProbe};
use snp_proc::pool::{JobPool, Limiter};
use snp_proc::spawn::{OutputBudget, Role};
use snp_pyre::RegexCache;
use snp_repo::{ExecKind, HookSrc, MetaHook, Resolve, ResolveCtx, ResolvedHook};
use snp_run::RunCtx;
use snp_store::{Store, StoreLocation, StoreSource};

/// A shared byte sink.
#[derive(Clone, Default)]
pub(crate) struct Sink(pub(crate) Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sink {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

/// What the fake resolver should do at install time.
#[derive(Debug, Default)]
pub(crate) struct Installs {
    /// Hook ids passed to `install_envs`, per call.
    pub(crate) calls: Mutex<Vec<Vec<String>>>,
    /// Sleep this long in `install_envs`.
    pub(crate) delay: Mutex<Option<Duration>>,
    /// Append to this file during the install (an IDE autosave).
    pub(crate) touch: Mutex<Option<PathBuf>>,
}

/// The fake `Resolve`.
#[derive(Debug, Default)]
pub(crate) struct Fake {
    pub(crate) installs: Installs,
}

fn meta_kind(id: &str) -> ExecKind {
    match id {
        "check-hooks-apply" => ExecKind::Meta(MetaHook::CheckHooksApply),
        "check-useless-excludes" => ExecKind::Meta(MetaHook::CheckUselessExcludes),
        _ => ExecKind::Meta(MetaHook::Identity),
    }
}

impl Resolve for Fake {
    fn all_hooks(&self, cx: &ResolveCtx<'_>) -> Result<Vec<ResolvedHook>, Failure> {
        let mut out = Vec::new();
        let root = cx.host.cwd.to_path_buf();
        let mut index = 0_u32;
        for repo in &cx.cfg.repos {
            let (hooks, src, meta) = match repo {
                RepoConfig::Local { hooks } => (hooks, "local", false),
                RepoConfig::Meta { hooks } => (hooks, "meta", true),
                _ => {
                    return Err(Failure::fatal(
                        "FatalError",
                        "fake resolver: remote repos unsupported",
                    ));
                }
            };
            for h in hooks {
                let patch = cx.overlay.and_then(|o| o.hook_patch(&h.id, &h.alias));
                let def =
                    snp_config::finalize_local(h, cx.cfg, src, patch, cx.warn).map_err(|e| {
                        cx.printer
                            .log(snp_core::output::Level::Error, &e.message(cx.brand));
                        Failure::Exit(1)
                    })?;
                out.push(ResolvedHook {
                    index,
                    src: if meta { HookSrc::Meta } else { HookSrc::Local },
                    exec: if meta {
                        meta_kind(&def.id)
                    } else {
                        ExecKind::Language
                    },
                    def: Arc::new(def),
                    prefix: root.clone(),
                    repo: None,
                    language_version: Arc::from("default"),
                    install_key: None,
                    virtual_install: None,
                    docker_tag: None,
                });
                index += 1;
            }
        }
        Ok(out)
    }

    fn install_envs(&self, hooks: &[&ResolvedHook], _cx: &ResolveCtx<'_>) -> Result<(), Failure> {
        self.installs
            .calls
            .lock()
            .unwrap()
            .push(hooks.iter().map(|h| h.def.id.to_string()).collect());
        if let Some(p) = self.installs.touch.lock().unwrap().as_ref() {
            let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
            f.write_all(b"autosave\n").unwrap();
        }
        if let Some(d) = *self.installs.delay.lock().unwrap() {
            std::thread::sleep(d);
        }
        Ok(())
    }

    fn hook_env(&self, _h: &ResolvedHook, cx: &ResolveCtx<'_>) -> Result<Env, Failure> {
        Ok(cx.env.clone())
    }

    fn plan(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>, color: bool) -> Result<RunPlan, Failure> {
        let pc = PlanCtx {
            host: &cx.host,
            prefix: &h.prefix,
            version: &h.language_version,
            env_dir: None,
            env: cx.env,
            entry: &h.def.entry,
            args: &h.def.args,
            is_local: true,
            color,
            docker_tag: None,
        };
        snp_lang::backend(h.def.language)
            .plan(&pc)
            .map_err(Failure::from)
    }
}

/// A clock whose `now()` advances by a fixed step per call (deterministic durations).
#[derive(Debug)]
pub(crate) struct StepClock {
    base: Instant,
    step: Duration,
    n: Mutex<u32>,
}

impl StepClock {
    pub(crate) fn new(step: Duration) -> Self {
        Self {
            base: Instant::now(),
            step,
            n: Mutex::new(0),
        }
    }
}

impl Clock for StepClock {
    fn now(&self) -> Instant {
        let mut n = self.n.lock().unwrap();
        *n += 1;
        self.base + self.step * *n
    }
    fn unix_secs(&self) -> u64 {
        1_700_000_000
    }
}

/// The result of one command.
#[derive(Debug)]
pub(crate) struct Out {
    pub(crate) code: i32,
    pub(crate) failure: Option<Failure>,
    pub(crate) stdout: String,
    pub(crate) stdout_bytes: Vec<u8>,
    pub(crate) stderr: String,
}

/// One test repository.
pub(crate) struct H {
    _tmp: tempfile::TempDir,
    pub(crate) base: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) store: PathBuf,
    pub(crate) env: Vec<(String, OsString)>,
    pub(crate) fake: Arc<Fake>,
    pub(crate) brand: Brand,
    pub(crate) clock: Option<Arc<dyn Clock>>,
    pub(crate) color: bool,
    pub(crate) cwd: Option<PathBuf>,
}

impl H {
    pub(crate) fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = snp_pycompat::ospath::realpath(tmp.path().as_os_str(), Path::new("/")).unwrap();
        let root = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let path = OsString::from("/usr/local/bin:/usr/bin:/bin");
        let env: Vec<(String, OsString)> = vec![
            ("PATH".into(), path),
            ("HOME".into(), home.clone().into()),
            ("XDG_CONFIG_HOME".into(), home.join(".config").into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_AUTHOR_NAME".into(), "A U Thor".into()),
            ("GIT_AUTHOR_EMAIL".into(), "author@example.com".into()),
            ("GIT_COMMITTER_NAME".into(), "A U Thor".into()),
            ("GIT_COMMITTER_EMAIL".into(), "author@example.com".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("TMPDIR".into(), base.clone().into()),
        ];
        let h = Self {
            _tmp: tmp,
            store: base.join("store"),
            base,
            root,
            env,
            fake: Arc::new(Fake::default()),
            brand: Brand::PreCommit,
            clock: None,
            color: false,
            cwd: None,
        };
        h.git(&["init", "-q", "-b", "main"]);
        h.git(&["config", "commit.gpgsign", "false"]);
        h
    }

    pub(crate) fn env_set(&mut self, k: &str, v: impl Into<OsString>) {
        self.env.retain(|(x, _)| x != k);
        self.env.push((k.to_owned(), v.into()));
    }

    pub(crate) fn git(&self, args: &[&str]) -> std::process::Output {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    pub(crate) fn git_any(&self, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .output()
            .unwrap()
    }

    pub(crate) fn write(&self, path: &str, content: &[u8]) {
        let p = self.root.join(path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    pub(crate) fn exec(&self, path: &str, content: &[u8]) {
        self.write(path, content);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(self.root.join(path), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }

    pub(crate) fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path)).unwrap()
    }

    pub(crate) fn stage(&self, path: &str) {
        self.git(&["add", "--", path]);
    }

    /// Writes and commits `.pre-commit-config.yaml` (it is then not among the staged files).
    pub(crate) fn config(&self, yaml: &str) {
        self.config_staged(yaml);
        self.commit("config");
    }

    /// Writes and stages `.pre-commit-config.yaml`.
    pub(crate) fn config_staged(&self, yaml: &str) {
        self.write(".pre-commit-config.yaml", yaml.as_bytes());
        self.stage(".pre-commit-config.yaml");
    }

    pub(crate) fn commit(&self, msg: &str) {
        self.git(&["commit", "-q", "--no-verify", "-m", msg]);
    }

    fn ctx(&self, stdout: &Sink, stderr: &Sink) -> RunCtx {
        let env = Env::from_pairs(self.env.iter().cloned());
        let cancel = CancelToken::new();
        let cwd = self.cwd.clone().unwrap_or_else(|| self.root.clone());
        let printer = Arc::new(Printer::new(
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
            PrinterOpts {
                brand: self.brand,
                color: self.color,
                stdout_is_tty: false,
                windows_text_mode: cfg!(windows),
                cancel: cancel.clone(),
            },
        ));
        let git_exe = Arc::new(GitExe::resolve(&env, &cwd, None).unwrap());
        let jobs = NonZeroUsize::new(4).unwrap();
        RunCtx {
            env,
            brand: self.brand,
            printer,
            store: Store::new(StoreLocation {
                path: self.store.clone(),
                source: StoreSource::SnpHome,
            }),
            git_exe,
            git_backend: Arc::new(ProcBackend {
                cancel: cancel.clone(),
                role: Role::Tool,
            }),
            resolver: self.fake.clone(),
            cancel: cancel.clone(),
            signals: None,
            pool: JobPool::new(jobs, cancel.clone()),
            net: Limiter::new(jobs),
            installs: Limiter::new(jobs),
            probe: Arc::new(ToolProbe::default()),
            runner: Arc::new(ProcRunner {
                cancel: cancel.clone(),
            }),
            budget: OutputBudget::new(64 << 20, self.base.join("spill")),
            invocation_cwd: cwd,
            arg0: Some(OsString::from("pre-commit")),
            self_exe: PathBuf::from("/usr/bin/false"),
            clock: self.clock.clone().unwrap_or_else(|| Arc::new(SystemClock)),
            fetch: None,
            regexes: Arc::new(RegexCache::default()),
        }
    }

    fn finish(r: Result<i32, Failure>, stdout: &Sink, stderr: &Sink) -> Out {
        let (code, failure) = match r {
            Ok(c) => (c, None),
            Err(f) => (f.exit_code(), Some(f)),
        };
        Out {
            code,
            failure,
            stdout: stdout.text(),
            stdout_bytes: stdout.bytes(),
            stderr: stderr.text(),
        }
    }

    /// `run` with the given arguments.
    pub(crate) fn run(&self, a: RunArgs) -> Out {
        let (o, e) = (Sink::default(), Sink::default());
        let cx = self.ctx(&o, &e);
        let r = snp_run::run(a, &cx);
        cx.printer.finish();
        Self::finish(r, &o, &e)
    }

    /// `hook-impl` with the given arguments and stdin.
    pub(crate) fn hook_impl(&self, a: snp_cli::HookImplArgs, stdin: &[u8]) -> Out {
        let (o, e) = (Sink::default(), Sink::default());
        let cx = self.ctx(&o, &e);
        let mut input = stdin;
        let r = snp_run::hook_impl(a, &mut input, &cx);
        cx.printer.finish();
        Self::finish(r, &o, &e)
    }

    /// `install-hooks`.
    pub(crate) fn install_hooks(&self) -> Out {
        let (o, e) = (Sink::default(), Sink::default());
        let cx = self.ctx(&o, &e);
        let r = snp_run::install_hooks(
            snp_cli::InstallHooksArgs {
                color: ColorSetting::Never,
                config: ".pre-commit-config.yaml".into(),
                with_fallback: false,
            },
            &cx,
        );
        cx.printer.finish();
        Self::finish(r, &o, &e)
    }

    /// `snp list`.
    pub(crate) fn list(&self, a: snp_cli::ListArgs) -> Out {
        let (o, e) = (Sink::default(), Sink::default());
        let cx = self.ctx(&o, &e);
        let r = snp_run::list(a, &cx);
        cx.printer.finish();
        Self::finish(r, &o, &e)
    }

    pub(crate) fn installed(&self) -> Vec<Vec<String>> {
        self.fake.installs.calls.lock().unwrap().clone()
    }

    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

/// `run` defaults (`pre-commit run`).
pub(crate) fn args() -> RunArgs {
    RunArgs {
        color: ColorSetting::Never,
        config: OsString::from(".pre-commit-config.yaml"),
        hooks: Vec::new(),
        verbose: false,
        all_files: false,
        files: Vec::new(),
        show_diff_on_failure: false,
        fail_fast: false,
        hook_stage: Stage::Hook(HookType::PreCommit),
        remote_branch: None,
        local_branch: None,
        from_ref: None,
        to_ref: None,
        pre_rebase_upstream: None,
        pre_rebase_branch: None,
        commit_msg_filename: None,
        prepare_commit_message_source: None,
        commit_object_name: None,
        remote_name: None,
        remote_url: None,
        checkout_type: None,
        is_squash_merge: None,
        rewrite_command: None,
        ext: RunExt::default(),
    }
}

/// `-a`.
pub(crate) fn all() -> RunArgs {
    RunArgs {
        all_files: true,
        ..args()
    }
}

/// `-v`.
pub(crate) fn verbose(mut a: RunArgs) -> RunArgs {
    a.verbose = true;
    a
}

/// `hook-impl` arguments.
pub(crate) fn hook_impl_args(
    hook_type: &str,
    rest: &[&str],
    hook_dir: Option<&Path>,
) -> snp_cli::HookImplArgs {
    snp_cli::HookImplArgs {
        color: ColorSetting::Never,
        config: OsString::from(".pre-commit-config.yaml"),
        hook_type: OsString::from(hook_type),
        hook_dir: hook_dir.map(|p| p.as_os_str().to_os_string()),
        skip_on_missing_config: false,
        rest: rest.iter().map(OsString::from).collect(),
    }
}

/// A one-hook local config.
pub(crate) fn local(hooks: &str) -> String {
    format!("repos:\n-   repo: local\n    hooks:\n{hooks}")
}

/// A local `system` hook.
pub(crate) fn sys(id: &str, name: &str, entry: &str, extra: &str) -> String {
    let mut s = format!(
        "    -   id: {id}\n        name: {name}\n        entry: {entry}\n        language: system\n"
    );
    for l in extra.lines().filter(|l| !l.trim().is_empty()) {
        s.push_str("        ");
        s.push_str(l.trim());
        s.push('\n');
    }
    s
}

/// `{name}{dots}{status}` at 80 columns.
pub(crate) fn line(name: &str, status: &str) -> String {
    let dots = 79 - name.chars().count() - status.chars().count();
    format!("{name}{}{status}\n", ".".repeat(dots))
}
