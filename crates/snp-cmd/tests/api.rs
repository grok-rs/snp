//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.22 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-33 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::OsStr;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use snp_cli::{
        AutoupdateArgs, CacheArgs, ConfigArgs, ConfigCmd, DoctorArgs, InitTemplatedirArgs,
        InstallArgs, SelfArgs, UninstallArgs, ValidateArgs,
    };
    use snp_cmd::scripts::{self, Owner};
    use snp_cmd::{CmdCtx, DoctorCheck, DoctorLine};
    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::output::Printer;
    use snp_core::stage::HookType;
    use snp_git::{GitBackend, GitExe};
    use snp_proc::pool::JobPool;
    use snp_store::Store;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// An injected check (the binary wraps `snp_run::doctor_fast_path` like this).
    #[derive(Debug)]
    struct FastPathCheck;

    impl DoctorCheck for FastPathCheck {
        fn name(&self) -> &'static str {
            "fast path"
        }
        fn run(&self, _cx: &CmdCtx<'_>) -> Vec<DoctorLine> {
            vec![DoctorLine {
                ok: true,
                text: String::from("check-json: native"),
                fix: None,
            }]
        }
    }

    /// The trait's methods, through a trait object.
    fn doctor_check(c: &dyn DoctorCheck, cx: &CmdCtx<'_>) {
        has::<&'static str>(c.name());
        has::<Vec<DoctorLine>>(c.run(cx));
    }

    /// The public fields of `CmdCtx`.
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per borrowed service"
    )]
    fn cmd_ctx<'a>(
        env: &'a Env,
        printer: &'a Printer,
        store: &'a Store,
        git_exe: &'a Arc<GitExe>,
        git_backend: Arc<dyn GitBackend>,
        cancel: &'a CancelToken,
        pool: &'a JobPool,
        self_exe: &'a Path,
    ) -> CmdCtx<'a> {
        let cx = CmdCtx {
            env,
            cwd: Path::new("/r"),
            brand: Brand::PreCommit,
            printer,
            store,
            git_exe: Some(git_exe),
            git_backend,
            cancel,
            pool,
            self_exe,
            arg0: Some(OsStr::new("pre-commit")),
        };
        doctor_check(&FastPathCheck, &cx);
        cx
    }

    #[test]
    fn send_sync_types() {
        send_sync::<CmdCtx<'static>>();
        send_sync::<DoctorLine>();
    }

    #[test]
    fn commands() {
        has(cmd_ctx);
        has::<fn(&dyn DoctorCheck, &CmdCtx<'_>)>(doctor_check);
        has::<fn(&InstallArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::install);
        has::<fn(&UninstallArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::uninstall);
        has::<fn(&InitTemplatedirArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(
            snp_cmd::init_templatedir,
        );
        has::<fn(&AutoupdateArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::autoupdate);
        has::<fn(&CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::gc);
        has::<fn(&CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::clean);
        has::<fn(&ValidateArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::validate_config);
        has::<fn(&ValidateArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::validate_manifest);
        has::<fn(&ConfigArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::migrate_config);
        has::<fn(&CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::sample_config);
        has::<fn(&ConfigCmd, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::config);
        has::<fn(&CacheArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::cache);
        has::<DoctorLine>(DoctorLine {
            ok: false,
            text: String::from("stash"),
            fix: Some(String::from("git apply --3way …")),
        });
        has::<fn(&DoctorArgs, &CmdCtx<'_>, &[&dyn DoctorCheck]) -> Result<i32, Failure>>(
            snp_cmd::doctor,
        );
        has::<fn(&SelfArgs, &CmdCtx<'_>) -> Result<i32, Failure>>(snp_cmd::self_cmd);
    }

    #[test]
    fn hook_scripts() {
        has::<fn(&Path) -> io::Result<Owner>>(scripts::classify);
        has::<Owner>(Owner::Foreign);
        has::<fn(Brand, Option<&OsStr>, &Env, &Path) -> PathBuf>(scripts::select_exe);
        has::<fn(&Path, &Path) -> bool>(scripts::is_snp);
        has::<fn(&OsStr, HookType, bool, &Path) -> Vec<u8>>(scripts::render);
        has::<fn(Brand) -> &'static [&'static str]>(scripts::migration_notes);
    }
}
