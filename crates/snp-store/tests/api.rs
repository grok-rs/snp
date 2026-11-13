//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.16 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-22 (created by WP-00).

#[cfg(test)]
mod api {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::SystemTime;

    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::lang::LanguageId;
    use snp_core::output::DiagSink;
    use snp_os::lock::FileLock;
    use snp_store::{
        MarkerKey, RepoHandle, RepoId, RepoKey, RepoKind, RepoMeta, Store, StoreGuard,
        StoreLocation, StoreSource, SweepReport, ToolRef, TrackedConfig,
    };

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<StoreSource>();
        send_sync::<StoreLocation>();
        send_sync::<RepoKey>();
        send_sync::<RepoId>();
        send_sync::<ToolRef>();
        send_sync::<TrackedConfig>();
        send_sync::<RepoKind>();
        send_sync::<RepoMeta>();
        send_sync::<RepoHandle>();
        send_sync::<MarkerKey>();
        send_sync::<StoreGuard>();
        send_sync::<Store>();
        send_sync::<SweepReport>();
    }

    #[test]
    fn location_and_keys() {
        has::<[StoreSource; 3]>([
            StoreSource::SnpHome,
            StoreSource::XdgCacheHome,
            StoreSource::PlatformDefault,
        ]);
        has::<StoreLocation>(StoreLocation {
            path: PathBuf::from("/home/u/.cache/snp"),
            source: StoreSource::PlatformDefault,
        });
        has::<fn(&Env, &Path) -> Result<StoreLocation, Failure>>(snp_store::resolve_location);
        let key = RepoKey([0; 16]);
        let RepoKey(bytes): RepoKey = key;
        has::<[u8; 16]>(bytes);
        has::<fn(&RepoKey) -> String>(RepoKey::hex);
        let id = RepoId::Url(Arc::from("https://github.com/pre-commit/pre-commit-hooks"));
        has::<[RepoId; 2]>([id.clone(), RepoId::LocalPath(PathBuf::from("/src/hooks"))]);
        has::<fn(&str, &Path) -> RepoId>(RepoId::from_config);
        has::<fn(&str, &[Arc<str>]) -> String>(snp_store::db_repo_name);
        has::<fn(&RepoId, &str, &[Arc<str>]) -> String>(snp_store::marker_repo_name);
        has::<ToolRef>(ToolRef {
            tool: Arc::from("node"),
            version: Arc::from("22.11.0"),
        });
        has::<TrackedConfig>(TrackedConfig {
            path: PathBuf::from("/r/.pre-commit-config.yaml"),
            project_root: PathBuf::from("/r"),
        });
        let meta = RepoMeta {
            kind: RepoKind::Remote {
                id,
                rev: Arc::from("v6.0.0"),
            },
            deps: Arc::from([]),
        };
        has::<RepoKind>(RepoKind::Local);
        has::<RepoHandle>(RepoHandle {
            key,
            src: PathBuf::from("/s/repos-v1/x/src"),
            envs: PathBuf::from("/s/repos-v1/x/envs"),
            meta,
        });
        has::<[MarkerKey; 2]>([
            MarkerKey::Initialized {
                db_repo_name: Arc::from("local"),
                rev: Arc::from("v1"),
            },
            MarkerKey::Installed {
                install_key: [0; 16],
            },
        ]);
    }

    #[test]
    fn store() {
        has::<fn(StoreLocation) -> Store>(Store::new);
        has::<fn(&Path, Option<&Store>) -> Store>(Store::ephemeral);
        has::<fn(&Store) -> &StoreLocation>(Store::location);
        has::<fn(&Store) -> bool>(Store::exists);
        has::<fn(&Store, Brand, &dyn DiagSink) -> Result<(), Failure>>(Store::ensure_created);
        has::<fn(&Store, &mut dyn FnMut(), &CancelToken) -> Result<StoreGuard, Failure>>(
            Store::shared,
        );
        has::<fn(&Store, &mut dyn FnMut(), &CancelToken) -> Result<StoreGuard, Failure>>(
            Store::exclusive,
        );
        has::<fn(&RepoId, &str, &[Arc<str>]) -> RepoKey>(Store::remote_key);
        has::<fn(&[Arc<str>]) -> RepoKey>(Store::local_key);
        has::<fn(&Store, &RepoKey) -> Option<RepoHandle>>(Store::lookup);
        has::<
            fn(
                &Store,
                &RepoKey,
                RepoMeta,
                &mut dyn FnMut(&Path) -> Result<(), Failure>,
                &mut dyn FnMut(),
                &CancelToken,
            ) -> Result<RepoHandle, Failure>,
        >(Store::materialize);
        has::<fn(&Store, &RepoKey, &mut dyn FnMut(), &CancelToken) -> Result<FileLock, Failure>>(
            Store::repo_lock,
        );
        has::<fn(&Store, &str, &mut dyn FnMut(), &CancelToken) -> Result<FileLock, Failure>>(
            Store::tool_lock,
        );
        has::<fn(&Store, &RepoHandle, LanguageId, &str) -> PathBuf>(Store::env_dir);
        has::<fn(&Path) -> Option<Vec<u8>>>(Store::read_install_marker);
        has::<fn(&Path, &[u8], &[ToolRef]) -> io::Result<()>>(Store::commit_install_marker);
        has::<fn(&Store, &MarkerKey) -> bool>(Store::marker_exists);
        has::<fn(&Store, &MarkerKey)>(Store::set_marker);
        has::<fn(&Store, &[MarkerKey], &mut dyn FnMut(&MarkerKey) -> bool) -> usize>(
            Store::retain_markers_keeping,
        );
        has::<fn(&Store, &Path, &Path)>(Store::mark_config_used);
        has::<fn(&Store) -> Vec<TrackedConfig>>(Store::tracked_configs);
        has::<fn(&Store, &[PathBuf])>(Store::forget_configs);
        has::<fn(&Store) -> Vec<RepoHandle>>(Store::all_repos);
        has::<fn(&Store, &RepoKey) -> io::Result<()>>(Store::retire_repo);
        has::<fn(&Store, SystemTime) -> SweepReport>(Store::sweep);
        has::<fn(&Store) -> Result<Option<PathBuf>, Failure>>(Store::clean);
        has::<fn(&Store) -> PathBuf>(Store::patches_dir);
        has::<fn(&Store) -> PathBuf>(Store::spill_dir);
        has::<fn(&Store) -> PathBuf>(Store::scratch_dir);
        has::<fn(&Store, Brand) -> PathBuf>(Store::crash_log);
        has::<fn(&Store, &str, &str) -> PathBuf>(Store::tool_dir);
        has::<fn(&Store, &str) -> PathBuf>(Store::cache_dir);
        has::<fn(&Store) -> Option<PathBuf>>(Store::relocated_from);
        has::<SweepReport>(SweepReport {
            removed_dirs: 0,
            removed_patches: 0,
            legacy_v1_removed: false,
            unreadable_journal: None,
        });
        has::<fn() -> SweepReport>(SweepReport::default);
    }
}
