//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.21 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-32 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::OsString;
    use std::io;
    use std::path::PathBuf;
    use std::sync::Arc;

    use snp_cli::{HookImplArgs, InstallHooksArgs, ListArgs, RunArgs, TryRepoArgs};
    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::clock::Clock;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::output::Printer;
    use snp_git::{GitBackend, GitExe};
    use snp_lang::{CmdRunner, ToolProbe};
    use snp_net::Fetch;
    use snp_proc::pool::{JobPool, Limiter};
    use snp_proc::signals::SignalGuard;
    use snp_proc::spawn::OutputBudget;
    use snp_pyre::RegexCache;
    use snp_repo::Resolve;
    use snp_run::RunCtx;
    use snp_run::render::{self, Details, Status};
    use snp_store::Store;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The services of a `RunCtx` that W0 cannot construct (their constructors are stubs).
    struct Services {
        printer: Arc<Printer>,
        store: Store,
        git_exe: Arc<GitExe>,
        git_backend: Arc<dyn GitBackend>,
        resolver: Arc<dyn Resolve>,
        signals: Option<Arc<SignalGuard>>,
        pool: Arc<JobPool>,
        limiter: Arc<Limiter>,
        runner: Arc<dyn CmdRunner>,
        budget: Arc<OutputBudget>,
        clock: Arc<dyn Clock>,
        fetch: Option<Arc<dyn Fetch>>,
    }

    /// The public fields of `RunCtx`.
    fn run_ctx(s: Services) -> RunCtx {
        RunCtx {
            env: Env::from_pairs([("PATH", "/usr/bin")]),
            brand: Brand::PreCommit,
            printer: s.printer,
            store: s.store,
            git_exe: s.git_exe,
            git_backend: s.git_backend,
            resolver: s.resolver,
            cancel: CancelToken::new(),
            signals: s.signals,
            pool: s.pool,
            net: Arc::clone(&s.limiter),
            installs: s.limiter,
            probe: Arc::new(ToolProbe::default()),
            runner: s.runner,
            budget: s.budget,
            invocation_cwd: PathBuf::from("/r"),
            arg0: Some(OsString::from("pre-commit")),
            self_exe: PathBuf::from("/usr/bin/snp"),
            clock: s.clock,
            fetch: s.fetch,
            regexes: Arc::new(RegexCache::default()),
        }
    }

    #[test]
    fn send_sync_types() {
        send_sync::<RunCtx>();
        send_sync::<Status>();
        send_sync::<Details<'static>>();
    }

    #[test]
    fn commands() {
        has::<fn(Services) -> RunCtx>(run_ctx);
        has::<fn(RunArgs, &RunCtx) -> Result<i32, Failure>>(snp_run::run);
        has::<fn(HookImplArgs, &mut dyn io::Read, &RunCtx) -> Result<i32, Failure>>(
            snp_run::hook_impl,
        );
        has::<fn(TryRepoArgs, &RunCtx) -> Result<i32, Failure>>(snp_run::try_repo);
        has::<fn(InstallHooksArgs, &RunCtx) -> Result<i32, Failure>>(snp_run::install_hooks);
        has::<fn(ListArgs, &RunCtx) -> Result<i32, Failure>>(snp_run::list);
        has::<fn(&snp_cli::ConfigArg, &RunCtx) -> Result<Vec<(bool, String)>, Failure>>(
            snp_run::doctor_fast_path,
        );
    }

    #[test]
    fn renderer() {
        has::<fn(&[&str]) -> usize>(render::cols);
        has::<fn(&str, usize) -> String>(render::start_msg);
        has::<fn(Status, bool) -> String>(render::end_msg);
        has::<[Status; 4]>([
            Status::Passed,
            Status::Failed,
            Status::Skipped,
            Status::NoFiles,
        ]);
        let ext_lines = vec![String::from("- hook timed out after 1s")];
        has::<Details<'_>>(Details {
            id: "check-json",
            duration: Some(0.01),
            exit_code: Some(1),
            modified: false,
            ext_lines: &ext_lines,
            output: b"x.json: Failed to json decode\n",
            color: false,
        });
        has::<fn(&Details<'_>) -> Vec<u8>>(render::details);
    }
}
