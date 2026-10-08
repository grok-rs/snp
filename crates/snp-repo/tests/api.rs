//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.20 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-31 (created by WP-00).

#[cfg(test)]
mod api {
    use std::path::PathBuf;
    use std::sync::Arc;

    use snp_config::overlay::Overlay;
    use snp_config::{Config, HookDef, ext::FastPath};
    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::lang::LanguageId;
    use snp_core::output::{Printer, WarnSink};
    use snp_git::{GitBackend, GitExe};
    use snp_hooks_data::{Epoch, NativeSource};
    use snp_lang::{Host, RunPlan, ToolDirs};
    use snp_os::lock::FileLock;
    use snp_proc::pool::{JobPool, Limiter};
    use snp_pyre::RegexCache;
    use snp_repo::{
        ExecKind, FastPathPolicy, HookSrc, InstallKey, MetaHook, Preflight, Resolve, ResolveCtx,
        ResolvedHook, StoreResolver, StoreTools,
    };
    use snp_store::{RepoHandle, RepoKey, Store};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The seams' methods, through trait objects.
    fn seams(p: &dyn Preflight, r: &dyn Resolve, cx: &ResolveCtx<'_>, h: &ResolvedHook) {
        has::<Result<(), String>>(p.check(
            NativeSource::PreCommitHooks,
            "check-json",
            Epoch(600),
            &[],
        ));
        has::<Result<Vec<ResolvedHook>, Failure>>(r.all_hooks(cx));
        has::<Result<(), Failure>>(r.install_envs(&[h], cx));
        has::<Result<Env, Failure>>(r.hook_env(h, cx));
        has::<Result<RunPlan, Failure>>(r.plan(h, cx, false));
    }

    /// `StoreTools` as the language framework's `ToolDirs`.
    fn store_tools(store: &Store, c: &CancelToken) {
        let tools = StoreTools(store);
        let StoreTools(inner): StoreTools<'_> = tools;
        has::<&Store>(inner);
        let dirs: &dyn ToolDirs = &tools;
        has::<PathBuf>(dirs.tool_dir("node", "22.11.0"));
        has::<Result<FileLock, Failure>>(dirs.lock_tool("node", c));
        has::<PathBuf>(dirs.cache_dir("uv"));
    }

    /// The public fields of `ResolveCtx`.
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per borrowed service"
    )]
    fn resolve_ctx<'a>(
        cfg: &'a Config,
        store: &'a Store,
        git_exe: &'a Arc<GitExe>,
        git_backend: Arc<dyn GitBackend>,
        env: &'a Env,
        printer: &'a Printer,
        pool: &'a JobPool,
        limiter: &'a Limiter,
        cancel: &'a CancelToken,
        host: Host<'a>,
        preflight: &'a dyn Preflight,
        overlay: &'a Overlay,
        regexes: &'a RegexCache,
        warn: &'a dyn WarnSink,
    ) -> ResolveCtx<'a> {
        ResolveCtx {
            cfg,
            store,
            git_exe,
            git_backend,
            env,
            printer,
            pool,
            net: limiter,
            installs: limiter,
            cancel,
            host,
            fast_path: FastPathPolicy {
                mode: FastPath::Auto,
                exclude: Vec::new(),
                coalesce: false,
                with_fallback: false,
            },
            preflight,
            overlay: Some(overlay),
            regexes,
            warn,
            brand: Brand::Snp,
        }
    }

    /// The public fields of `ResolvedHook`.
    fn resolved_hook(def: Arc<HookDef>, repo: RepoHandle) -> ResolvedHook {
        let key = InstallKey {
            repo: RepoKey([0; 16]),
            lang: LanguageId::Python,
            version: Arc::from("python3"),
            deps: Arc::from([]),
        };
        ResolvedHook {
            index: 0,
            src: HookSrc::Remote {
                repo: Arc::from("https://github.com/pre-commit/pre-commit-hooks"),
                rev: Arc::from("v6.0.0"),
            },
            def,
            prefix: PathBuf::from("/s/repos-v1/x/src"),
            repo: Some(repo),
            language_version: Arc::from("python3"),
            exec: ExecKind::Native {
                source: NativeSource::PreCommitHooks,
                id: Arc::from("check-json"),
                epoch: Epoch(600),
            },
            install_key: None,
            virtual_install: Some(key),
            docker_tag: None,
        }
    }

    #[test]
    fn send_sync_types() {
        send_sync::<StoreTools<'static>>();
        send_sync::<ResolveCtx<'static>>();
        send_sync::<FastPathPolicy>();
        send_sync::<HookSrc>();
        send_sync::<MetaHook>();
        send_sync::<ExecKind>();
        send_sync::<InstallKey>();
        send_sync::<ResolvedHook>();
        send_sync::<StoreResolver>();
        send_sync::<&dyn Preflight>();
        send_sync::<&dyn Resolve>();
    }

    #[test]
    fn model() {
        has::<fn(&dyn Preflight, &dyn Resolve, &ResolveCtx<'_>, &ResolvedHook)>(seams);
        has::<fn(&Store, &CancelToken)>(store_tools);
        has::<fn(Arc<HookDef>, RepoHandle) -> ResolvedHook>(resolved_hook);
        // the helper's own signature pins every field type of `ResolveCtx`
        has(resolve_ctx);
        has::<[HookSrc; 4]>([
            HookSrc::Remote {
                repo: Arc::from("r"),
                rev: Arc::from("v1"),
            },
            HookSrc::Local,
            HookSrc::Meta,
            HookSrc::Builtin,
        ]);
        has::<[MetaHook; 3]>([
            MetaHook::CheckHooksApply,
            MetaHook::CheckUselessExcludes,
            MetaHook::Identity,
        ]);
        has::<[ExecKind; 2]>([ExecKind::Language, ExecKind::Meta(MetaHook::Identity)]);
        has::<StoreResolver>(StoreResolver::default());
        has::<&dyn Resolve>(&StoreResolver::default());
        has::<fn(&Config, &ResolveCtx<'_>) -> Vec<(Arc<str>, Arc<str>, String)>>(
            snp_repo::decision_table,
        );
    }
}
