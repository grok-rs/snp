//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.4 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-12 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::OsString;
    use std::num::NonZeroUsize;

    use snp_argparse::ParserSpec;
    use snp_cli::{
        AutoupdateArgs, CacheAction, CacheArgs, Cli, CliOutcome, ColorArgs, ConfigArg, ConfigArgs,
        ConfigCmd, DoctorArgs, HazmatArgs, HazmatTool, HookImplArgs, InitTemplatedirArgs,
        InstallArgs, InstallHooksArgs, ListArgs, MigrateArgs, RunArgs, RunExt, SelfArgs, Shell,
        StdinFormat, TryRepoArgs, UninstallArgs, ValidateArgs,
    };
    use snp_core::brand::Brand;
    use snp_core::env::Env;
    use snp_core::output::ColorSetting;
    use snp_core::stage::{HookType, Stage};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    fn os() -> OsString {
        OsString::from("x")
    }

    fn run_args() -> RunArgs {
        RunArgs {
            color: ColorSetting::Auto,
            config: ConfigArg::given(os()),
            hooks: vec![os()],
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
            rewrite_command: Some(os()),
            ext: RunExt {
                jobs: NonZeroUsize::new(2),
                groups: Vec::new(),
                require_groups: Vec::new(),
                no_groups: Vec::new(),
                no_fast_path: false,
                stdin_files: Some(StdinFormat::Nul),
                summary: false,
                no_local_config: false,
            },
        }
    }

    #[test]
    fn send_sync_types() {
        send_sync::<CliOutcome>();
        send_sync::<Cli>();
        send_sync::<ColorArgs>();
        send_sync::<ConfigArgs>();
        send_sync::<RunArgs>();
        send_sync::<RunExt>();
        send_sync::<StdinFormat>();
        send_sync::<TryRepoArgs>();
        send_sync::<HookImplArgs>();
        send_sync::<InstallArgs>();
        send_sync::<UninstallArgs>();
        send_sync::<InstallHooksArgs>();
        send_sync::<InitTemplatedirArgs>();
        send_sync::<AutoupdateArgs>();
        send_sync::<ValidateArgs>();
        send_sync::<HazmatArgs>();
        send_sync::<HazmatTool>();
        send_sync::<ListArgs>();
        send_sync::<ConfigCmd>();
        send_sync::<CacheArgs>();
        send_sync::<CacheAction>();
        send_sync::<MigrateArgs>();
        send_sync::<DoctorArgs>();
        send_sync::<SelfArgs>();
        send_sync::<Shell>();
    }

    #[test]
    fn parse_and_tables() {
        has::<fn(Brand, &[OsString], &Env, Option<usize>) -> CliOutcome>(snp_cli::parse);
        has::<&'static ParserSpec>(snp_cli::PRE_COMMIT_SPEC);
        has::<&'static ParserSpec>(snp_cli::SNP_SPEC);
        has::<&'static [(Shell, &'static str)]>(snp_cli::COMPLETIONS);
        has::<CliOutcome>(CliOutcome::Stdout(String::new()));
        has::<CliOutcome>(CliOutcome::Usage(String::new()));
        has::<fn(Cli) -> CliOutcome>(CliOutcome::Cli);
        has::<[Shell; 4]>([Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell]);
        has::<[StdinFormat; 2]>([StdinFormat::Nul, StdinFormat::Lines]);
        has::<RunExt>(RunExt::default());
    }

    #[test]
    fn commands() {
        let color = ColorArgs {
            color: ColorSetting::Never,
        };
        let config = ConfigArgs {
            color: ColorSetting::Always,
            config: ConfigArg::given(os()),
        };
        let validate = ValidateArgs {
            color: ColorSetting::Auto,
            filenames: vec![os()],
            strict: false,
            portable: false,
        };
        let all: Vec<Cli> = vec![
            Cli::Run(run_args()),
            Cli::TryRepo(TryRepoArgs {
                repo: os(),
                rev: None,
                run: run_args(),
            }),
            Cli::HookImpl(HookImplArgs {
                color: ColorSetting::Auto,
                config: ConfigArg::given(os()),
                hook_type: os(),
                hook_dir: None,
                skip_on_missing_config: false,
                rest: Vec::new(),
            }),
            Cli::Install(InstallArgs {
                color: ColorSetting::Auto,
                config: ConfigArg::given(os()),
                overwrite: false,
                install_hooks: false,
                hook_types: vec![HookType::PrePush],
                allow_missing_config: false,
                git_config_hooks: false,
            }),
            Cli::Uninstall(UninstallArgs {
                color: ColorSetting::Auto,
                config: ConfigArg::given(os()),
                hook_types: Vec::new(),
                git_config_hooks: true,
            }),
            Cli::InstallHooks(InstallHooksArgs {
                color: ColorSetting::Auto,
                config: ConfigArg::given(os()),
                with_fallback: false,
            }),
            Cli::InitTemplatedir(InitTemplatedirArgs {
                color: ColorSetting::Auto,
                config: ConfigArg::given(os()),
                directory: os(),
                no_allow_missing_config: false,
                hook_types: Vec::new(),
            }),
            Cli::Autoupdate(AutoupdateArgs {
                color: ColorSetting::Auto,
                config: ConfigArg::given(os()),
                bleeding_edge: false,
                freeze: false,
                repos: Vec::new(),
                jobs: 1,
            }),
            Cli::Clean(color.clone()),
            Cli::Gc(color.clone()),
            Cli::MigrateConfig(config),
            Cli::SampleConfig(color),
            Cli::ValidateConfig(validate.clone()),
            Cli::ValidateManifest(validate),
            Cli::Hazmat(HazmatArgs {
                color: ColorSetting::Auto,
                tool: HazmatTool::Cd {
                    subdir: os(),
                    cmd: vec![os()],
                },
            }),
            Cli::List(ListArgs {
                config: ConfigArg::given(os()),
                json: true,
                stage: Some(Stage::Manual),
                groups: Vec::new(),
                require_groups: Vec::new(),
                no_groups: Vec::new(),
            }),
            Cli::Config(ConfigCmd::Dump {
                config: ConfigArg::given(os()),
                json: false,
                no_local_config: false,
            }),
            Cli::Config(ConfigCmd::Schema),
            Cli::Cache(CacheArgs {
                action: CacheAction::Dir,
            }),
            Cli::Migrate(MigrateArgs::Lefthook {
                root: None,
                config: None,
                output: None,
                portable: false,
                local: false,
                strict: false,
                install: false,
                force: false,
            }),
            Cli::Migrate(MigrateArgs::Builtin {
                config: ConfigArg::given(os()),
            }),
            Cli::Migrate(MigrateArgs::Upstream {
                config: ConfigArg::given(os()),
            }),
            Cli::Doctor(DoctorArgs {
                fix: false,
                fast_path: false,
                stash: false,
                config: ConfigArg::given(os()),
            }),
            Cli::SelfCmd(SelfArgs::AliasInstall {
                dir: None,
                force: false,
            }),
            Cli::SelfCmd(SelfArgs::AliasUninstall),
            Cli::SelfCmd(SelfArgs::AliasStatus),
            Cli::SelfCmd(SelfArgs::Check),
            Cli::Completions(Shell::Zsh),
        ];
        has::<String>(format!("{all:?}"));
        has::<[HazmatTool; 2]>([
            HazmatTool::IgnoreExitCode { cmd: Vec::new() },
            HazmatTool::N1 { cmd: Vec::new() },
        ]);
        has::<[CacheAction; 3]>([CacheAction::Size, CacheAction::Gc, CacheAction::Clean]);
    }
}
