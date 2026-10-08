//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.10 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-00 / integration lead. The file spans the framework (WP-16) and both language groups (WP-24,
//! WP-25), and lies outside their owned paths; it changes only with a signature-change ADR.

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::env::{Env, EnvPatch};
    use snp_core::failure::Failure;
    use snp_core::lang::LanguageId;
    use snp_core::output::Printer;
    use snp_lang::common;
    use snp_lang::langs::{
        conda, coursier, dart, docker, dotnet, envless, golang, haskell, julia, lua, node, perl,
        python, r, ruby, rust, swift,
    };
    use snp_lang::{
        CmdRunner, EnvCtx, Fingerprint, Health, Host, InstallCtx, InstallOutcome, InstallState,
        LangError, Language, PlanCtx, PreStep, ProcRunner, RunPlan, Support, ToolDirs, ToolProbe,
        VersionSupport,
    };
    use snp_net::{Fetch, NetError};
    use snp_os::lock::FileLock;
    use snp_proc::spawn::Captured;

    fn send_sync<T: Send + Sync>() {}
    fn serde_type<T: serde::Serialize + serde::de::DeserializeOwned>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The seams' methods, through trait objects.
    fn seams(r: &dyn CmdRunner, t: &dyn ToolDirs, env: &Env, c: &CancelToken) {
        has::<Result<Captured, LangError>>(r.run(&[], env, Path::new("/")));
        has::<PathBuf>(t.tool_dir("node", "22.11.0"));
        has::<Result<FileLock, Failure>>(t.lock_tool("node", c));
        has::<PathBuf>(t.cache_dir("uv"));
    }

    /// The trait's methods, through a trait object.
    fn language(l: &dyn Language, h: &Host<'_>, i: &InstallCtx<'_>, p: &PlanCtx<'_>) {
        has::<LanguageId>(l.id());
        has::<Support>(l.support());
        has::<Result<Arc<str>, LangError>>(l.default_version(h));
        has::<Result<InstallOutcome, LangError>>(l.install(i));
        has::<Result<Health, LangError>>(l.health(&i.cx, None));
        has::<Result<EnvPatch, LangError>>(l.activate(&i.cx));
        has::<Result<RunPlan, LangError>>(l.plan(p));
    }

    /// Every context struct, field by field.
    #[expect(clippy::too_many_arguments, reason = "one parameter per Host service")]
    fn contexts(
        base_env: &Env,
        probe: &ToolProbe,
        fetch: &dyn Fetch,
        tools: &dyn ToolDirs,
        runner: &dyn CmdRunner,
        printer: &Printer,
        cancel: &CancelToken,
        lang: &dyn Language,
    ) {
        let host = Host {
            base_env,
            cwd: Path::new("/"),
            self_exe: Path::new("/snp"),
            probe,
            fetch: Some(fetch),
            tools,
            runner,
            printer,
            cancel,
            brand: Brand::Snp,
        };
        let deps: [Arc<str>; 1] = [Arc::from("dep")];
        let install = InstallCtx {
            cx: EnvCtx {
                host: &host,
                prefix: Path::new("/repo"),
                version: "default",
                env_dir: Path::new("/repo/py_env-default"),
            },
            deps: &deps,
            is_local: false,
        };
        let plan = PlanCtx {
            host: &host,
            prefix: Path::new("/repo"),
            version: "default",
            env_dir: None,
            env: base_env,
            entry: "x",
            args: &deps,
            is_local: true,
            color: false,
            docker_tag: Some("pre-commit-x"),
        };
        language(lang, &host, &install, &plan);
        has::<String>(format!("{install:?}{plan:?}"));
    }

    #[test]
    fn send_sync_types() {
        send_sync::<ProcRunner>();
        send_sync::<ToolProbe>();
        send_sync::<Host<'_>>();
        send_sync::<EnvCtx<'_>>();
        send_sync::<InstallCtx<'_>>();
        send_sync::<PlanCtx<'_>>();
        send_sync::<Health>();
        send_sync::<InstallOutcome>();
        send_sync::<InstallState>();
        send_sync::<Fingerprint>();
        send_sync::<RunPlan>();
        send_sync::<PreStep>();
        send_sync::<LangError>();
        send_sync::<VersionSupport>();
        send_sync::<Support>();
        send_sync::<&dyn CmdRunner>();
        send_sync::<&dyn ToolDirs>();
        send_sync::<&dyn Language>();
        send_sync::<envless::Fail>();
        send_sync::<envless::Pygrep>();
        send_sync::<envless::Unsupported>();
        send_sync::<envless::UnsupportedScript>();
        send_sync::<envless::DockerImage>();
        send_sync::<python::Python>();
        send_sync::<node::Node>();
        send_sync::<golang::Golang>();
        send_sync::<rust::Rust>();
        send_sync::<ruby::Ruby>();
        send_sync::<docker::Docker>();
        send_sync::<conda::Conda>();
        send_sync::<coursier::Coursier>();
        send_sync::<dart::Dart>();
        send_sync::<dotnet::Dotnet>();
        send_sync::<haskell::Haskell>();
        send_sync::<julia::Julia>();
        send_sync::<lua::Lua>();
        send_sync::<perl::Perl>();
        send_sync::<r::R>();
        send_sync::<swift::Swift>();
        serde_type::<InstallState>();
        serde_type::<Fingerprint>();
    }

    #[test]
    fn framework() {
        has::<fn(&dyn CmdRunner, &dyn ToolDirs, &Env, &CancelToken)>(seams);
        has::<
            fn(
                &Env,
                &ToolProbe,
                &dyn Fetch,
                &dyn ToolDirs,
                &dyn CmdRunner,
                &Printer,
                &CancelToken,
                &dyn Language,
            ),
        >(contexts);
        let runner = ProcRunner {
            cancel: CancelToken::new(),
        };
        has::<&dyn CmdRunner>(&runner);
        has::<fn(Option<PathBuf>) -> ToolProbe>(ToolProbe::new);
        has::<
            fn(
                &ToolProbe,
                LanguageId,
                &Env,
                &Path,
                &mut dyn FnMut() -> Result<Arc<str>, LangError>,
            ) -> Result<Arc<str>, LangError>,
        >(ToolProbe::default_version);
        has::<fn(&ToolProbe, &OsStr, &Env, &Path) -> Option<PathBuf>>(ToolProbe::find_exe);
        has::<fn(&ToolProbe, &Path) -> Option<Fingerprint>>(ToolProbe::fingerprint);
        has::<ToolProbe>(ToolProbe::default());
        has::<[Health; 2]>([Health::Healthy, Health::Unhealthy(String::new())]);
        let fingerprint = Fingerprint {
            exe: PathBuf::from("/usr/bin/python3"),
            len: 1,
            mtime_ns: -1,
            ino_or_index: 1,
            detail: Arc::from("3.12.3"),
        };
        has::<InstallOutcome>(InstallOutcome {
            installer: Arc::from("uv"),
            fingerprint: Some(fingerprint.clone()),
            tools: vec![(Arc::from("uv"), Arc::from("0.8.8"))],
        });
        let state = InstallState {
            v: 1,
            language: Arc::from("python"),
            language_version: Arc::from("default"),
            deps: Arc::from([]),
            installer: Arc::from("uv"),
            fingerprint: Some(fingerprint),
            snp_version: Arc::from("2.0.0"),
        };
        has::<fn(&InstallState) -> Vec<u8>>(InstallState::to_json);
        has::<fn(&[u8]) -> Option<InstallState>>(InstallState::from_json);
        has::<InstallState>(state);
        let env = Env::from_pairs::<&str, &str>([]);
        has::<[RunPlan; 4]>([
            RunPlan::Xargs {
                cmd: Arc::from([OsString::from("x")]),
                env: env.clone(),
                shuffle: true,
                serial: false,
                pre: vec![PreStep {
                    argv: Vec::new(),
                    env,
                    cwd: PathBuf::new(),
                    skip_if_ok: None,
                }],
            },
            RunPlan::Fail {
                entry: Arc::from("x"),
            },
            RunPlan::Pygrep {
                args: Arc::from([]),
                entry: Arc::from("x"),
            },
            RunPlan::Error {
                code: 1,
                output: Vec::new(),
            },
        ]);
        has::<[LangError; 7]>([
            LangError::Fatal(String::new()),
            LangError::Assertion(String::new()),
            LangError::Value(String::new()),
            LangError::CalledProcess {
                cmd: Vec::new(),
                rc: 1,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            LangError::Io {
                op: "rmtree",
                path: PathBuf::new(),
                err: io::Error::from(io::ErrorKind::NotFound),
            },
            LangError::Net(NetError::Offline),
            LangError::Interrupted,
        ]);
        has::<fn(LangError) -> Failure>(Failure::from);
        has::<[VersionSupport; 3]>([
            VersionSupport::DefaultOnly,
            VersionSupport::Any,
            VersionSupport::Ignored,
        ]);
        has::<Support>(Support {
            version: VersionSupport::Any,
            deps: true,
        });
        has::<fn(LanguageId) -> &'static dyn Language>(snp_lang::backend);
        has::<fn(LanguageId, &str, &[Arc<str>], Brand) -> Result<(), LangError>>(
            snp_lang::check_support,
        );
        has::<&[(&str, &[u8])]>(snp_lang::LOCAL_TEMPLATE);
        has::<u32>(snp_lang::LOCAL_TEMPLATE_VERSION);
    }

    #[test]
    fn common_helpers() {
        has::<fn(&OsStr, &Env, &Path) -> bool>(common::exe_exists);
        has::<fn(&dyn CmdRunner, &[OsString], &Env, &Path) -> Result<Vec<u8>, LangError>>(
            common::setup_cmd,
        );
        has::<fn(&Path) -> PathBuf>(common::bin_dir);
        has::<fn(&str) -> String>(common::docker_tag);
        has::<fn(&Path, &Path) -> io::Result<()>>(common::rmtree);
    }

    #[test]
    fn registry_is_total() {
        let backends: [&dyn Language; 21] = [
            &envless::Fail,
            &envless::Pygrep,
            &envless::Unsupported,
            &envless::UnsupportedScript,
            &envless::DockerImage,
            &python::Python,
            &node::Node,
            &golang::Golang,
            &rust::Rust,
            &ruby::Ruby,
            &docker::Docker,
            &conda::Conda,
            &coursier::Coursier,
            &dart::Dart,
            &dotnet::Dotnet,
            &haskell::Haskell,
            &julia::Julia,
            &lua::Lua,
            &perl::Perl,
            &r::R,
            &swift::Swift,
        ];
        for id in LanguageId::ALL {
            assert_eq!(snp_lang::backend(id).id(), id);
            assert_eq!(backends.iter().filter(|b| b.id() == id).count(), 1);
        }
    }

    #[cfg(feature = "testing")]
    #[test]
    fn harness() {
        use snp_lang::harness::{Recorded, ShimRunner};
        send_sync::<ShimRunner>();
        send_sync::<Recorded>();
        let shim = ShimRunner::default();
        has::<&dyn CmdRunner>(&shim);
        has::<Recorded>(Recorded {
            argv: Vec::new(),
            cwd: PathBuf::new(),
            env_delta: vec![(OsString::from("PATH"), None)],
        });
    }
}
