//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.15 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-21 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use file_identify::TagSet;
    use snp_config::ext::{
        self, Condition, FailOnChanges, FastPath, HookExt, OutputMode, Priority, RootExt,
        StageFixed,
    };
    use snp_config::migrate::{self, Outcome};
    use snp_config::overlay::{self, HookPatch, Overlay};
    use snp_config::revline::{self, RevUpdate, Rewrite};
    use snp_config::schema;
    use snp_config::validate::{self, FileKind};
    use snp_config::{
        Config, ConfigError, ConfigHook, ConfigSource, ExtMode, HookDef, LoadOpts, Manifest,
        MergeError, RepoConfig,
    };
    use snp_core::brand::Brand;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::lang::LanguageId;
    use snp_core::output::{CollectWarnings, DiagSink, WarnSink};
    use snp_core::path::RepoPath;
    use snp_core::stage::{HookType, StageSet};
    use snp_pyre::RegexCache;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The RPIT method of `HookPatch`.
    fn patch_keys(p: &HookPatch) {
        has::<Vec<&str>>(p.keys().collect());
    }

    fn hook_def() -> HookDef {
        HookDef {
            id: Arc::from("trailing-whitespace"),
            name: Arc::from("trim trailing whitespace"),
            entry: Arc::from("trailing-whitespace-fixer"),
            language: LanguageId::Python,
            alias: Arc::from(""),
            files: Arc::from(""),
            exclude: Arc::from("^$"),
            types: TagSet::new(),
            types_or: TagSet::new(),
            exclude_types: TagSet::new(),
            additional_dependencies: Arc::from([]),
            args: Arc::from([Arc::from("--markdown-linebreak-ext=md")]),
            always_run: false,
            fail_fast: false,
            pass_filenames: true,
            description: Arc::from(""),
            language_version: Arc::from("default"),
            log_file: Arc::from(""),
            minimum_pre_commit_version: Arc::from("0"),
            require_serial: false,
            stages: StageSet::ALL,
            stages_set: false,
            verbose: false,
            extra_keys: Vec::new(),
            ext: HookExt::default(),
        }
    }

    fn config_hook() -> ConfigHook {
        ConfigHook {
            id: Arc::from("trailing-whitespace"),
            raw: cfgv::Map::new(),
            ext: HookExt::default(),
        }
    }

    #[test]
    fn send_sync_types() {
        send_sync::<ConfigSource>();
        send_sync::<ExtMode>();
        send_sync::<LoadOpts<'static>>();
        send_sync::<ConfigError>();
        send_sync::<Config>();
        send_sync::<RepoConfig>();
        send_sync::<ConfigHook>();
        send_sync::<HookDef>();
        send_sync::<Manifest>();
        send_sync::<MergeError>();
        send_sync::<HookExt>();
        send_sync::<RootExt>();
        send_sync::<Priority>();
        send_sync::<StageFixed>();
        send_sync::<FailOnChanges>();
        send_sync::<FastPath>();
        send_sync::<OutputMode>();
        send_sync::<Condition>();
        send_sync::<Overlay>();
        send_sync::<HookPatch>();
        send_sync::<Outcome>();
        send_sync::<RevUpdate>();
        send_sync::<Rewrite>();
        send_sync::<FileKind>();
    }

    #[test]
    fn model() {
        let source = ConfigSource {
            path: PathBuf::from("/r/.pre-commit-config.yaml"),
            display: OsString::from(".pre-commit-config.yaml"),
            project_root: PathBuf::from("/r"),
        };
        let config = Config {
            source,
            repos: vec![
                RepoConfig::Remote {
                    repo: Arc::from("https://github.com/pre-commit/pre-commit-hooks"),
                    rev: Arc::from("v6.0.0"),
                    hooks: vec![config_hook()],
                },
                RepoConfig::Local {
                    hooks: vec![hook_def()],
                },
                RepoConfig::Meta { hooks: Vec::new() },
                RepoConfig::Builtin {
                    hooks: vec![config_hook()],
                },
            ],
            default_install_hook_types: vec![HookType::PreCommit],
            default_language_version: indexmap::IndexMap::new(),
            default_stages: StageSet::ALL,
            files: Arc::from(""),
            exclude: Arc::from("^$"),
            fail_fast: false,
            minimum_pre_commit_version: Arc::from("0"),
            root_ext: RootExt::default(),
            raw: cfgv::Value::Null,
        };
        has::<Config>(config);
        has::<[ExtMode; 2]>([ExtMode::Enabled, ExtMode::Strict]);
        let error = ConfigError {
            manifest: false,
            rendered: String::from("\n==> File .pre-commit-config.yaml\n=====> leaf"),
        };
        has::<ConfigError>(error);
        has::<fn(ConfigError) -> Failure>(<Failure as From<ConfigError>>::from);
        has::<Manifest>(Manifest {
            hooks: vec![hook_def()],
        });
        has::<for<'a> fn(&'a Manifest, &str) -> Option<&'a HookDef>>(Manifest::get);
        has::<[MergeError; 3]>([
            MergeError::UnknownId {
                id: Arc::from("x"),
                repo: Arc::from("local"),
            },
            MergeError::LanguageVersionOnEnvless {
                id: Arc::from("x"),
                lang: LanguageId::Unsupported,
            },
            MergeError::DepsOnEnvless {
                id: Arc::from("x"),
                lang: LanguageId::Fail,
            },
        ]);
    }

    #[test]
    fn load_and_merge() {
        let regexes = RegexCache::default();
        let sink = CollectWarnings::new();
        let opts = LoadOpts {
            brand: Brand::PreCommit,
            ext: ExtMode::Enabled,
            regexes: &regexes,
            warn: &sink,
            diag: &sink,
        };
        has::<&LoadOpts<'_>>(&opts);
        has::<fn(&ConfigSource, &LoadOpts<'_>) -> Result<Config, ConfigError>>(
            snp_config::load_config,
        );
        has::<fn(&Path, &OsStr, &LoadOpts<'_>) -> Result<Manifest, ConfigError>>(
            snp_config::load_manifest,
        );
        has::<fn(&[u8], &OsStr, &LoadOpts<'_>) -> Result<Manifest, ConfigError>>(
            snp_config::load_manifest_bytes,
        );
        has::<fn(Brand, ExtMode) -> &'static cfgv::Schema>(schema::config_schema);
        has::<fn(Brand) -> &'static cfgv::Schema>(schema::manifest_schema);
        has::<
            fn(
                &HookDef,
                &ConfigHook,
                &Config,
                &str,
                Option<&HookPatch>,
                &dyn WarnSink,
            ) -> Result<HookDef, MergeError>,
        >(snp_config::merge_hook);
        has::<
            fn(
                &HookDef,
                &Config,
                &str,
                Option<&HookPatch>,
                &dyn WarnSink,
            ) -> Result<HookDef, MergeError>,
        >(snp_config::finalize_local);
    }

    #[test]
    fn extensions() {
        let hook = HookExt {
            priority: Some(Priority::Value(1)),
            env: vec![(Arc::from("A"), Arc::from("1"))],
            groups: vec![Arc::from("lint")],
            stage_fixed: Some(StageFixed::Verify),
            skip: vec![Condition::Merge],
            only: vec![Condition::All(vec![Condition::Ci(true)])],
            timeout: Some(Duration::from_secs(1)),
            fail_text: Some(Arc::from("see docs")),
            root: RepoPath::new(b"sub"),
            use_stdin: false,
            fail_on_changes: Some(FailOnChanges::Ci),
        };
        has::<HookExt>(hook);
        let root = RootExt {
            minimum_snp_version: Some(Arc::from("2.0.0")),
            priorities: indexmap::IndexMap::from([(Arc::from("early"), 1)]),
            fail_on_changes: FailOnChanges::Always,
            skip: Vec::new(),
            only: Vec::new(),
            allow_disable: true,
            fast_path: FastPath::Auto,
            fast_path_exclude: Vec::new(),
            fast_path_coalesce: false,
            output: OutputMode::Default,
        };
        has::<RootExt>(root);
        has::<fn() -> RootExt>(RootExt::default);
        let defaults = RootExt::default();
        assert!(defaults.allow_disable);
        assert_eq!(defaults.fast_path, FastPath::Auto);
        assert_eq!(defaults.fail_on_changes, FailOnChanges::Always);
        assert_eq!(defaults.output, OutputMode::Default);
        assert!(defaults.minimum_snp_version.is_none() && defaults.priorities.is_empty());
        assert!(!defaults.fast_path_coalesce && defaults.fast_path_exclude.is_empty());
        has::<[Priority; 2]>([Priority::Value(0), Priority::Alias(Arc::from("early"))]);
        has::<[StageFixed; 2]>([StageFixed::Verify, StageFixed::Trust]);
        has::<[FailOnChanges; 4]>([
            FailOnChanges::Always,
            FailOnChanges::Never,
            FailOnChanges::Ci,
            FailOnChanges::NonCi,
        ]);
        has::<[FastPath; 3]>([FastPath::Off, FastPath::Exact, FastPath::Auto]);
        has::<[OutputMode; 3]>([OutputMode::Default, OutputMode::Summary, OutputMode::Quiet]);
        has::<[Condition; 11]>([
            Condition::Merge,
            Condition::Rebase,
            Condition::MergeCommit,
            Condition::CherryPick,
            Condition::Revert,
            Condition::Bisect,
            Condition::Ref(Arc::from("main")),
            Condition::Env(Arc::from("CI")),
            Condition::Ci(false),
            Condition::Run(Arc::from("true")),
            Condition::All(Vec::new()),
        ]);
        has::<fn(&str) -> Result<Duration, String>>(ext::parse_go_duration);
    }

    #[test]
    fn overlay() {
        has::<HookPatch>(HookPatch {
            raw: cfgv::Map::new(),
            ext: HookExt::default(),
        });
        has::<fn(&HookPatch)>(patch_keys);
        has::<fn(&Path, &Env, &dyn Fn(&Path) -> bool) -> Result<Option<PathBuf>, String>>(
            overlay::locate,
        );
        has::<fn(&Path, &LoadOpts<'_>) -> Result<Overlay, ConfigError>>(overlay::load);
        has::<fn(&mut Config, &Overlay, &dyn DiagSink)>(overlay::apply_root);
        has::<for<'a> fn(&'a Overlay, &str, &str) -> Option<&'a HookPatch>>(Overlay::hook_patch);
        has::<for<'a> fn(&'a Overlay, &[&str]) -> Vec<&'a str>>(Overlay::unmatched);
    }

    #[test]
    fn migrate_revline_validate() {
        has::<[Outcome; 2]>([Outcome::Migrated(Vec::new()), Outcome::AlreadyMigrated]);
        has::<fn(&[u8], &OsStr) -> Result<Outcome, ConfigError>>(migrate::migrate_config);
        has::<RevUpdate>(RevUpdate {
            repo_index: 0,
            new_rev: Arc::from("v6.0.0"),
            frozen_comment: None,
        });
        has::<[Rewrite; 2]>([
            Rewrite::Lines(Vec::new()),
            Rewrite::FullReformat(Vec::new()),
        ]);
        has::<fn(&[u8], &[RevUpdate]) -> Result<Rewrite, ConfigError>>(revline::rewrite);
        has::<[FileKind; 2]>([FileKind::Config, FileKind::Manifest]);
        has::<fn(FileKind, &OsStr, &LoadOpts<'_>, bool) -> Option<String>>(validate::validate_file);
    }

    /// Additive items of WP-21 (not in the frozen §1.4.15 listing; reported as contract additions).
    #[test]
    fn additive_items() {
        has::<for<'a> fn(&'a ConfigError) -> Option<(&'a str, &'a str)>>(ConfigError::uncaught);
        has::<fn(&ConfigError) -> &'static str>(ConfigError::py_type);
        has::<fn(&MergeError, Brand) -> String>(MergeError::message);
        has::<for<'a> fn(&'a Overlay) -> &'a Path>(Overlay::path);
        has::<fn(&str) -> String>(overlay::unknown_hook_note);
        has::<fn(&Path, &OsStr) -> Result<Vec<u8>, ConfigError>>(migrate::read_config);
        has::<[&str; 2]>([migrate::MIGRATED, migrate::ALREADY_MIGRATED]);
        has::<fn(FileKind, &OsStr, &LoadOpts<'_>, bool) -> Result<(), ConfigError>>(validate::validate);
        fn display<T: std::fmt::Display + std::error::Error>() {}
        display::<MergeError>();
    }

    #[test]
    fn sample_dump_schema() {
        let sample: &str = snp_config::SAMPLE_CONFIG;
        assert_eq!(sample.lines().count(), 10);
        assert!(sample.starts_with("# See https://pre-commit.com for more information\n"));
        assert!(sample.ends_with("    -   id: check-added-large-files\n"));
        has::<fn(&Config, bool) -> String>(snp_config::effective_dump);
        has::<fn() -> &'static str>(snp_config::json_schema);
    }
}
