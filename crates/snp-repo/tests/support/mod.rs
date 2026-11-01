//! Shared harness of the resolver tests: a hermetic `TestEnv`, a temp store, the real git, a captured printer and a
//! `ResolveCtx` built per call.
//!
//! Owner: WP-31.
#![allow(dead_code, reason = "each test binary uses a subset of the harness")]

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use snp_config::ext::FastPath;
use snp_config::overlay::Overlay;
use snp_config::{Config, ConfigSource, ExtMode, LoadOpts};
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::{Printer, PrinterOpts};
use snp_git::{GitBackend, GitExe, ProcBackend};
use snp_hooks_data::{Epoch, NativeSource};
use snp_lang::harness::ShimRunner;
use snp_lang::{Host, ToolProbe};
use snp_proc::pool::{JobPool, Limiter};
use snp_proc::spawn::Role;
use snp_pyre::RegexCache;
use snp_repo::{
    FastPathPolicy, Preflight, Resolve, ResolveCtx, ResolvedHook, StoreResolver, StoreTools,
};
use snp_store::{Store, StoreLocation, StoreSource};
use snp_testkit::{Bins, TestEnv};

/// A captured stream.
#[derive(Clone, Default, Debug)]
pub(crate) struct Buf(Arc<Mutex<Vec<u8>>>);

impl Buf {
    /// Everything written so far, cleared (unfiltered: a lock released on drop is released at once, so the store's
    /// `Locking … directory` notice appears only on real contention).
    pub(crate) fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

impl std::io::Write for Buf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        snp_core::sync::lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// G5 that accepts everything except the listed ids.
#[derive(Debug, Default)]
pub(crate) struct FakePreflight {
    /// Ids whose preflight fails.
    pub(crate) refuse: Vec<&'static str>,
}

impl Preflight for FakePreflight {
    fn check(&self, _s: NativeSource, id: &str, _e: Epoch, _a: &[Arc<str>]) -> Result<(), String> {
        if self.refuse.contains(&id) {
            return Err(format!("{id}: scripted refusal"));
        }
        Ok(())
    }
}

/// The harness.
#[derive(Debug)]
pub(crate) struct H {
    pub(crate) t: TestEnv,
    pub(crate) env: Env,
    pub(crate) project: PathBuf,
    pub(crate) store: Store,
    pub(crate) git_exe: Arc<GitExe>,
    pub(crate) backend: Arc<dyn GitBackend>,
    pub(crate) printer: Printer,
    pub(crate) out: Buf,
    pub(crate) err: Buf,
    pub(crate) pool: Arc<JobPool>,
    pub(crate) net: Arc<Limiter>,
    pub(crate) installs: Arc<Limiter>,
    pub(crate) cancel: CancelToken,
    pub(crate) probe: ToolProbe,
    pub(crate) runner: ShimRunner,
    pub(crate) regexes: RegexCache,
    pub(crate) preflight: FakePreflight,
    pub(crate) self_exe: PathBuf,
    pub(crate) brand: Brand,
    pub(crate) fast_path: FastPathPolicy,
    pub(crate) resolver: StoreResolver,
}

/// A path that does not exist (the harness never runs the binaries).
fn no_bins() -> Bins {
    Bins {
        snp: PathBuf::from("/nonexistent/snp"),
        probe: PathBuf::from("/nonexistent/snp-probe"),
    }
}

impl H {
    /// A fresh harness with an empty project repository.
    pub(crate) fn new() -> Self {
        Self::with(|e| e)
    }

    /// A fresh harness whose env is `f(env)`.
    pub(crate) fn with(f: impl FnOnce(Env) -> Env) -> Self {
        let t = TestEnv::new(no_bins());
        let env = f(t.env().clone());
        let project = t
            .git_repo("project")
            .file("README", b"project\n")
            .stage_all()
            .commit("init")
            .build();
        let store = Store::new(StoreLocation {
            path: t.root().join("snp-store"),
            source: StoreSource::SnpHome,
        });
        let cancel = CancelToken::new();
        let git_exe = Arc::new(GitExe::resolve(&env, &project, None).unwrap());
        let backend: Arc<dyn GitBackend> = Arc::new(ProcBackend {
            cancel: cancel.clone(),
            role: Role::Tool,
        });
        let (out, err) = (Buf::default(), Buf::default());
        let printer = Printer::new(
            Box::new(out.clone()),
            Box::new(err.clone()),
            PrinterOpts {
                brand: Brand::PreCommit,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: cancel.clone(),
            },
        );
        let nz = |n| NonZeroUsize::new(n).unwrap();
        Self {
            runner: ShimRunner::new(env.clone()),
            env,
            project,
            store,
            git_exe,
            backend,
            printer,
            out,
            err,
            pool: JobPool::new(nz(4), cancel.clone()),
            net: Limiter::new(nz(8)),
            installs: Limiter::new(nz(4)),
            cancel,
            probe: ToolProbe::new(None),
            regexes: RegexCache::new(),
            preflight: FakePreflight::default(),
            self_exe: PathBuf::from("/opt/snp/bin/snp"),
            brand: Brand::PreCommit,
            fast_path: FastPathPolicy {
                mode: FastPath::Auto,
                exclude: Vec::new(),
                coalesce: false,
                with_fallback: false,
            },
            resolver: StoreResolver::default(),
            t,
        }
    }

    /// Uses `backend` for every git call.
    pub(crate) fn backend(mut self, b: Arc<dyn GitBackend>) -> Self {
        self.backend = b;
        self
    }

    /// Writes `.pre-commit-config.yaml` and loads it (the load's own warnings are discarded).
    pub(crate) fn config(&self, yaml: &str) -> Config {
        let path = self.project.join(".pre-commit-config.yaml");
        std::fs::write(&path, yaml).unwrap();
        let src = ConfigSource {
            path,
            display: OsString::from(".pre-commit-config.yaml"),
            project_root: self.project.clone(),
        };
        let cfg = snp_config::load_config(
            &src,
            &LoadOpts {
                brand: self.brand,
                ext: ExtMode::Enabled,
                regexes: &self.regexes,
                warn: &self.printer,
                diag: &self.printer,
            },
        )
        .unwrap_or_else(|e| panic!("{}", e.rendered));
        self.out.take();
        self.err.take();
        cfg
    }

    /// Runs `f` with a context over `cfg`.
    pub(crate) fn with_ctx<R>(
        &self,
        cfg: &Config,
        overlay: Option<&Overlay>,
        f: impl FnOnce(&ResolveCtx<'_>) -> R,
    ) -> R {
        let tools = StoreTools(&self.store);
        let host = Host {
            base_env: &self.env,
            cwd: &self.project,
            self_exe: &self.self_exe,
            probe: &self.probe,
            fetch: None,
            tools: &tools,
            runner: &self.runner,
            printer: &self.printer,
            cancel: &self.cancel,
            brand: self.brand,
        };
        let cx = ResolveCtx {
            cfg,
            store: &self.store,
            git_exe: &self.git_exe,
            git_backend: Arc::clone(&self.backend),
            env: &self.env,
            printer: &self.printer,
            pool: &self.pool,
            net: &self.net,
            installs: &self.installs,
            cancel: &self.cancel,
            host,
            fast_path: self.fast_path.clone(),
            preflight: &self.preflight,
            overlay,
            regexes: &self.regexes,
            warn: &self.printer,
            brand: self.brand,
        };
        f(&cx)
    }

    /// `all_hooks` over `cfg`.
    pub(crate) fn resolve(&self, cfg: &Config) -> Result<Vec<ResolvedHook>, Failure> {
        self.with_ctx(cfg, None, |cx| self.resolver.all_hooks(cx))
    }

    /// `all_hooks` then `install_envs` over every hook.
    pub(crate) fn resolve_and_install(&self, cfg: &Config) -> Result<Vec<ResolvedHook>, Failure> {
        self.with_ctx(cfg, None, |cx| {
            let hooks = self.resolver.all_hooks(cx)?;
            let refs: Vec<&ResolvedHook> = hooks.iter().collect();
            self.resolver.install_envs(&refs, cx)?;
            Ok(hooks)
        })
    }

    /// A committed repository `<root>/<name>` with the given files; returns `(path, HEAD sha)`.
    pub(crate) fn hook_repo(&self, name: &str, files: &[(&str, &[u8], bool)]) -> (PathBuf, String) {
        let mut b = self.t.git_repo(name);
        for (path, bytes, exec) in files {
            b = if *exec {
                b.exec_file(path, bytes)
            } else {
                b.file(path, bytes)
            };
        }
        let dir = b.stage_all().commit("init").build();
        let sha = head(&self.env, &dir);
        (dir, sha)
    }
}

/// `git rev-parse HEAD` in `dir`.
pub(crate) fn head(env: &Env, dir: &Path) -> String {
    git(env, dir, &["rev-parse", "HEAD"]).trim().to_owned()
}

/// Runs git in `dir` and returns stdout (panics on failure).
pub(crate) fn git(env: &Env, dir: &Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args).current_dir(dir).env_clear();
    env.apply_to(&mut cmd);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `(exception type, text)` of a `Failure::Exc`.
pub(crate) fn exc(f: &Failure) -> (String, String) {
    match f {
        Failure::Exc { exc, .. } => (
            exc.ty.to_string(),
            String::from_utf8_lossy(&exc.text).into_owned(),
        ),
        other => panic!("not an exception: {other:?}"),
    }
}

/// The `Initializing` line of `name`.
pub(crate) fn initializing(name: &str) -> String {
    format!("[INFO] Initializing environment for {name}.\n")
}

/// The `Installing` triplet of `src`.
pub(crate) fn installing(src: &str) -> String {
    format!(
        "[INFO] Installing environment for {src}.\n[INFO] Once installed this environment will be reused.\n\
         [INFO] This may take a few minutes...\n"
    )
}

/// A `.pre-commit-hooks.yaml` with one hook.
pub(crate) fn manifest_one(id: &str, entry: &str, language: &str, extra: &str) -> Vec<u8> {
    format!("-   id: {id}\n    name: {id}\n    entry: {entry}\n    language: {language}\n{extra}")
        .into_bytes()
}
