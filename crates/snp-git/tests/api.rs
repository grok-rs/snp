//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.13 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-18 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::output::Printer;
    use snp_core::path::RepoPath;
    use snp_git::hookscripts::{self, Owner};
    use snp_git::pch_facts::PchFacts;
    use snp_git::{
        CloneMode, DiffFingerprint, DiffIndex, Git, GitBackend, GitCaps, GitExe, GitVersion,
        IndexEntries, MergeState, ObjectId, OwnedRepo, PrePushNs, ProcBackend, RepoInfo, RevInfo,
        ZList,
    };
    use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The seam's methods, through a trait object.
    fn backend(b: &dyn GitBackend, env: &Env) {
        let cwd = Path::new("/");
        has::<Result<Captured, SpawnError>>(b.capture(&[], env, cwd, Stdin::Null, Group::Inherit));
        has::<Result<(PyStatus, Vec<u8>), SpawnError>>(b.stream(&[], env, cwd, &mut io::sink()));
        has::<Result<PyStatus, SpawnError>>(b.inherit(&[], env, cwd, Stdin::Inherit));
    }

    /// The RPIT method of `ZList`.
    fn zlist_iter(z: &ZList) {
        has::<Vec<&[u8]>>(z.iter().collect());
    }

    #[test]
    fn send_sync_types() {
        send_sync::<GitVersion>();
        send_sync::<GitCaps>();
        send_sync::<GitExe>();
        send_sync::<ProcBackend>();
        send_sync::<RepoInfo>();
        send_sync::<ZList>();
        send_sync::<ObjectId>();
        send_sync::<DiffIndex>();
        send_sync::<DiffFingerprint>();
        send_sync::<MergeState>();
        send_sync::<PrePushNs>();
        send_sync::<IndexEntries>();
        send_sync::<Git>();
        send_sync::<OwnedRepo>();
        send_sync::<RevInfo>();
        send_sync::<CloneMode>();
        send_sync::<PchFacts>();
        send_sync::<Owner>();
        send_sync::<&dyn GitBackend>();
    }

    #[test]
    fn exe_and_types() {
        let version = GitVersion {
            major: 2,
            minor: 31,
            patch: 0,
        };
        let caps = GitCaps {
            version: Some(version),
            ls_files_dedup: true,
            path_format: true,
            config_env: true,
            init_object_format: true,
            config_hooks: false,
            pathspec_from_file: true,
            diff_no_relative: true,
        };
        has::<GitExe>(GitExe {
            argv0: vec![OsString::from("/usr/bin/git")],
            caps,
        });
        has::<fn(&Env, &Path, Option<&Path>) -> Result<GitExe, Failure>>(GitExe::resolve);
        has::<fn(&GitExe, &Printer, &Env) -> Result<(), Failure>>(GitExe::version_gate);
        has::<fn(&dyn GitBackend, &Env)>(backend);
        let proc_backend = ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        };
        has::<&dyn GitBackend>(&proc_backend);
        has::<RepoInfo>(RepoInfo {
            root: PathBuf::new(),
            git_dir: PathBuf::new(),
            common_dir: PathBuf::new(),
            index_file: PathBuf::new(),
            is_worktree: false,
        });
        has::<fn(&ZList) -> usize>(ZList::len);
        has::<fn(&ZList) -> bool>(ZList::is_empty);
        has::<fn(&ZList, usize) -> Option<&[u8]>>(ZList::get);
        has::<fn(&ZList)>(zlist_iter);
        has::<fn(ZList) -> (Vec<u8>, Vec<(u32, u32)>)>(ZList::into_parts);
        has::<ZList>(ZList::default());
        has::<[DiffIndex; 3]>([
            DiffIndex::Clean,
            DiffIndex::CrlfQuirk,
            DiffIndex::Patch { bytes_written: 1 },
        ]);
        let DiffFingerprint { digest, non_empty }: DiffFingerprint = DiffFingerprint::EMPTY;
        has::<([u8; 32], bool)>((digest, non_empty));
        assert!(!non_empty);
        has::<[MergeState; 2]>([MergeState::None, MergeState::Merging]);
        has::<PrePushNs>(PrePushNs {
            all_files: false,
            from_ref: None,
            to_ref: Some(OsString::new()),
            local_branch: OsString::new(),
            remote_branch: OsString::new(),
        });
        has::<IndexEntries>(IndexEntries {
            paths: ZList::default(),
            modes: Box::new([0o100_644]),
        });
    }

    #[test]
    fn git() {
        has::<fn(Arc<GitExe>, &Env, &Path, Arc<dyn GitBackend>) -> Result<(Git, RepoInfo), Failure>>(
            Git::discover,
        );
        has::<fn(&Git, Env) -> Git>(Git::with_env);
        has::<fn(&Git, Arc<dyn GitBackend>) -> Git>(Git::with_backend);
        has::<fn(&Git) -> &Env>(Git::env);
        has::<fn(&Git) -> &Path>(Git::root);
        has::<fn(&Git) -> Git>(Git::clone);
        has::<fn(&Git) -> Result<bool, Failure>>(Git::has_unmerged);
        has::<fn(&Git, &OsStr) -> Result<bool, Failure>>(Git::config_is_unstaged);
        has::<fn(&Git) -> Result<ZList, Failure>>(Git::intent_to_add);
        has::<fn(&Git, &ZList, Group) -> Result<(), Failure>>(Git::rm_cached);
        has::<fn(&Git, &ZList, Group) -> Result<(), Failure>>(Git::add_intent_to_add);
        has::<fn(&Git) -> Result<ObjectId, Failure>>(Git::write_tree);
        has::<fn(&Git, &ObjectId, &mut dyn io::Write) -> Result<DiffIndex, Failure>>(
            Git::diff_index_patch,
        );
        has::<fn(&Git, bool) -> Env>(Git::checkout_env);
        has::<fn(&Git, &ZList, bool, Group) -> Result<(), Failure>>(Git::checkout_paths);
        has::<fn(&Git, &Path) -> Result<ZList, Failure>>(Git::patch_paths);
        has::<fn(&Git) -> Result<ZList, Failure>>(Git::changed_paths);
        has::<fn(&Git, &Path) -> Result<ZList, Failure>>(Git::rollback_paths);
        has::<fn(&Git, &Path, bool, Group) -> Result<bool, Failure>>(Git::apply);
        has::<fn(&Git, &Path) -> Result<bool, Failure>>(Git::apply_check);
        has::<fn(&Git, &[RepoPath], Group) -> Result<(), Failure>>(Git::add_literal);
        has::<fn(&Git) -> Result<ZList, Failure>>(Git::staged_files);
        has::<fn(&Git) -> Result<ZList, Failure>>(Git::all_files);
        has::<fn(&Git) -> Result<IndexEntries, Failure>>(Git::tracked_with_modes);
        has::<fn(&Git) -> Result<IndexEntries, Failure>>(Git::staged_with_modes);
        has::<fn(&Git, &RepoInfo) -> Option<u32>>(Git::index_entry_count);
        has::<fn(&Git, &OsStr, &OsStr) -> Result<ZList, Failure>>(Git::changed_files);
        has::<fn(&Git, &RepoInfo) -> MergeState>(Git::merge_state);
        has::<fn(&Git, &RepoInfo) -> Result<ZList, Failure>>(Git::conflicted_files);
        has::<fn(&Git) -> Result<DiffFingerprint, Failure>>(Git::diff_fingerprint);
        has::<fn(&Git, bool) -> Result<(), Failure>>(Git::show_diff);
        has::<fn(&Git) -> Result<Option<OsString>, Failure>>(Git::hooks_path);
        has::<fn(&Git) -> Result<Vec<PathBuf>, Failure>>(Git::config_origins);
        has::<fn(&Git, &str) -> Result<Option<OsString>, Failure>>(Git::config_get);
        has::<fn(&Git, &str) -> Result<Vec<(OsString, OsString)>, Failure>>(Git::config_get_regexp);
        has::<fn(&Git, &[u8], &OsStr) -> Result<Option<PrePushNs>, Failure>>(Git::pre_push);
        has::<fn(&Git, &[&OsStr]) -> Result<Captured, Failure>>(Git::run_raw);
    }

    #[test]
    fn owned() {
        has::<
            fn(
                Arc<GitExe>,
                &Env,
                &Path,
                &str,
                &str,
                CloneMode,
                Arc<dyn GitBackend>,
            ) -> Result<OwnedRepo, Failure>,
        >(OwnedRepo::clone_rev);
        has::<
            fn(
                Arc<GitExe>,
                &Env,
                &Path,
                &Path,
                &str,
                Arc<dyn GitBackend>,
            ) -> Result<OwnedRepo, Failure>,
        >(OwnedRepo::clone_local);
        has::<fn(Arc<GitExe>, &Env, &Path, Arc<dyn GitBackend>) -> OwnedRepo>(OwnedRepo::open);
        has::<fn(&OwnedRepo, &str, &str) -> Result<Vec<(String, String)>, Failure>>(
            OwnedRepo::ls_tree_blobs,
        );
        has::<fn(&OwnedRepo, &str, &str) -> Result<Vec<u8>, Failure>>(OwnedRepo::read_blob);
        has::<fn(&OwnedRepo, &str, bool, bool) -> Result<RevInfo, Failure>>(OwnedRepo::rev_info);
        has::<fn(&OwnedRepo, &Git, &Path) -> Result<(PathBuf, String), Failure>>(
            OwnedRepo::shadow_try_repo,
        );
        has::<fn(&OwnedRepo) -> OwnedRepo>(OwnedRepo::clone);
        has::<RevInfo>(RevInfo {
            repo: String::new(),
            rev: String::new(),
            frozen: None,
            hook_ids: Vec::new(),
        });
        has::<[CloneMode; 2]>([CloneMode::Foreground, CloneMode::Prefetch]);
        has::<fn(&str, &Env, bool) -> bool>(snp_git::prefetch_eligible);
        has::<fn(&Env) -> Env>(snp_git::no_git_env);
    }

    #[test]
    fn pch_facts_and_hookscripts() {
        has::<fn(Git) -> PchFacts>(PchFacts::new);
        has::<fn(&PchFacts) -> Result<Arc<[Vec<u8>]>, Failure>>(PchFacts::added_files);
        has::<fn(&PchFacts) -> Result<Arc<[Vec<u8>]>, Failure>>(PchFacts::tracked_files);
        has::<fn(&PchFacts) -> Result<Arc<[(u32, Vec<u8>)]>, Failure>>(PchFacts::index_modes);
        has::<fn(&PchFacts, &[&[u8]]) -> Result<Vec<Vec<u8>>, Failure>>(PchFacts::lfs_files);
        has::<fn(&PchFacts) -> Result<Option<String>, Failure>>(PchFacts::current_branch);
        has::<fn(&PchFacts, &[&OsStr]) -> Result<Captured, Failure>>(PchFacts::raw);
        has::<[Owner; 8]>([
            Owner::Missing,
            Owner::NotAFile,
            Owner::Snp { stale: false },
            Owner::SnpV1,
            Owner::PreCommit { era: Some(1) },
            Owner::Prek,
            Owner::Lefthook,
            Owner::Foreign,
        ]);
        has::<&str>(hookscripts::SNP_SCRIPT_ID);
        has::<&[&str]>(hookscripts::SNP_PRIOR_IDS);
        has::<fn(&Path) -> io::Result<Owner>>(hookscripts::classify);
        has::<fn(&Owner) -> bool>(Owner::is_manager);
        has::<fn(&Owner) -> Option<&'static str>>(Owner::owner_name);
    }

    #[cfg(feature = "testing")]
    #[test]
    fn testing() {
        use snp_git::testing::{Response, ScriptedBackend};
        send_sync::<ScriptedBackend>();
        send_sync::<Response>();
        has::<fn() -> ScriptedBackend>(ScriptedBackend::new);
        has::<for<'a> fn(&'a ScriptedBackend, &str, Response) -> &'a ScriptedBackend>(
            ScriptedBackend::on,
        );
        has::<fn(&ScriptedBackend) -> Vec<Vec<OsString>>>(ScriptedBackend::calls);
        let scripted = ScriptedBackend::default();
        has::<&dyn GitBackend>(&scripted);
        has::<[Response; 4]>([
            Response::Real,
            Response::Exit {
                code: 1,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            Response::SpawnError(2),
            Response::Panic,
        ]);
    }
}
