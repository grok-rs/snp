//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.18 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-23 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use snp_core::cancel::CancelToken;
    use snp_core::clock::Clock;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::output::Printer;
    use snp_core::path::RepoPath;
    use snp_git::{DiffFingerprint, Git, IndexEntries, RepoInfo};
    use snp_proc::signals::SignalGuard;
    use snp_stash::diff_oracle::{Change, DiffMode, DiffOracle};
    use snp_stash::{StashCtx, StashGuard, Stashed, Unrestored};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The public fields of `StashCtx`.
    fn stash_ctx(
        git: Git,
        info: RepoInfo,
        printer: Arc<Printer>,
        clock: Arc<dyn Clock>,
        signals: Option<Arc<SignalGuard>>,
    ) -> StashCtx {
        StashCtx {
            git,
            info,
            patches_dir: PathBuf::from("/s/patches"),
            printer,
            cancel: CancelToken::new(),
            clock,
            signals,
            lock_timeout: Duration::from_secs(60),
            verify: false,
            old_git: false,
            report_recovery: true,
        }
    }

    #[test]
    fn send_sync_types() {
        send_sync::<StashCtx>();
        send_sync::<Stashed>();
        send_sync::<StashGuard>();
        send_sync::<Unrestored>();
        send_sync::<DiffMode>();
        send_sync::<DiffOracle>();
        send_sync::<Change>();
    }

    #[test]
    fn guard() {
        has::<fn(Git, RepoInfo, Arc<Printer>, Arc<dyn Clock>, Option<Arc<SignalGuard>>) -> StashCtx>(
            stash_ctx,
        );
        has::<[Stashed; 2]>([
            Stashed::Nothing,
            Stashed::Patch(PathBuf::from("/s/patches/p")),
        ]);
        has::<fn(StashCtx) -> Result<StashGuard, Failure>>(snp_stash::enter);
        has::<fn(&StashGuard) -> &Stashed>(StashGuard::stashed);
        has::<fn(&StashGuard) -> bool>(StashGuard::diff_index_clean);
        has::<fn(&StashGuard) -> Option<(&'static str, OsString)>>(StashGuard::child_env_token);
        has::<fn(&mut StashGuard, &[RepoPath]) -> Result<bool, Failure>>(StashGuard::stage_fixed);
        has::<fn(StashGuard) -> Result<(), Failure>>(StashGuard::restore);
    }

    #[test]
    fn recovery() {
        has::<Unrestored>(Unrestored {
            patch: PathBuf::from("/s/patches/p"),
            tree: Some(String::from("4b825dc642cb6eb9a060e54bf8d69288fbee4904")),
            worktree: PathBuf::from("/r"),
            pid: 1,
            state: Box::from("stashed"),
            staged_fixed: Vec::new(),
        });
        has::<fn(&Unrestored) -> String>(Unrestored::hint);
        has::<fn(&Path, &Path, &Path) -> Vec<Unrestored>>(snp_stash::find_unrestored);
        has::<fn(&Path, &Path, &Path) -> Vec<Unrestored>>(snp_stash::recovery_scan);
        has::<fn(&Git, &Path) -> Result<bool, Failure>>(snp_stash::verify_restored);
    }

    #[test]
    fn diff_oracle() {
        has::<[DiffMode; 2]>([DiffMode::Stat, DiffMode::Git]);
        has::<[Change; 2]>([Change::Unchanged, Change::Changed]);
        has::<
            fn(
                &Git,
                &RepoInfo,
                &Env,
                Option<DiffFingerprint>,
                Option<Arc<IndexEntries>>,
                Option<Arc<[PathBuf]>>,
            ) -> Result<DiffOracle, Failure>,
        >(DiffOracle::new);
        has::<fn(&DiffOracle) -> DiffMode>(DiffOracle::mode);
        has::<fn(&mut DiffOracle) -> Result<(), Failure>>(DiffOracle::before_hook);
        has::<fn(&mut DiffOracle) -> Result<Change, Failure>>(DiffOracle::after_hook);
        has::<fn(&mut DiffOracle, bool, bool) -> Result<Change, Failure>>(DiffOracle::after_native);
        has::<fn(&mut DiffOracle) -> Result<bool, Failure>>(DiffOracle::non_empty);
    }
}
