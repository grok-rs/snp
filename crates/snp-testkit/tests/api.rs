//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.14 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-18 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use snp_core::env::Env;
    use snp_testkit::{Bins, Cmd, Fixture, Oracle, Output, RepoBuilder, TestEnv};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The generic and `impl Trait`-argument methods, called with concrete argument types.
    fn generic_methods(env: &TestEnv, cmd: Cmd) {
        has::<TestEnv>(env.with_env("K", OsString::from("v")));
        has::<TestEnv>(env.with_env("K", "v"));
        let built = cmd
            .arg("a")
            .arg(OsString::from("b"))
            .args(["c", "d"])
            .args(vec![PathBuf::from("e")])
            .env("K", "v")
            .env_remove("K")
            .stdin(b"in")
            .tty();
        let Output {
            code,
            stdout,
            stderr,
        } = Cmd::output(built);
        has::<(i32, Vec<u8>, Vec<u8>)>((code, stdout, stderr));
    }

    /// The exported macro, with its argument types.
    fn snapshot(env: &TestEnv, out: &Output) {
        snp_testkit::snp_snapshot!(env, out);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Bins>();
        send_sync::<TestEnv>();
        send_sync::<Oracle>();
        send_sync::<Fixture>();
        send_sync::<RepoBuilder>();
        send_sync::<Cmd>();
        send_sync::<Output>();
    }

    #[test]
    fn types_and_functions() {
        has::<Bins>(Bins {
            snp: PathBuf::from("snp"),
            probe: PathBuf::from("snp-probe"),
        });
        has::<fn() -> Option<Bins>>(Bins::locate);
        has::<fn(Bins) -> TestEnv>(TestEnv::new);
        has::<fn(&TestEnv) -> &Path>(TestEnv::root);
        has::<fn(&TestEnv) -> &Env>(TestEnv::env);
        has::<fn(&TestEnv, Cmd)>(generic_methods);
        has::<fn(&TestEnv, &Output)>(snapshot);
        has::<fn(&TestEnv, &str) -> RepoBuilder>(TestEnv::git_repo);
        has::<fn(&TestEnv, Fixture) -> PathBuf>(TestEnv::fixture_repo);
        has::<fn(&TestEnv, &Path) -> Cmd>(TestEnv::snp);
        has::<fn(&TestEnv, &Path) -> Cmd>(TestEnv::snp_as_pre_commit);
        has::<fn(&TestEnv, Oracle, &Path) -> Option<Cmd>>(TestEnv::oracle);
        has::<fn(&TestEnv) -> Vec<(String, String)>>(TestEnv::filters);
        has::<fn(&TestEnv) -> &Path>(TestEnv::probe);
        has::<[Oracle; 2]>([Oracle::Pc470Py312, Oracle::Pc470Py313]);
        has::<[Fixture; 11]>([
            Fixture::Resource("script_hooks_repo"),
            Fixture::MergeConflict,
            Fixture::ConflictingSubmodule,
            Fixture::CommitMsgRepo,
            Fixture::PrepareCommitMsgRepo { failing: true },
            Fixture::PushExample,
            Fixture::ConsumingRepo("python_hooks_repo"),
            Fixture::OutOfDate { tagged: false },
            Fixture::HookDisappearing,
            Fixture::ImgStaged,
            Fixture::SubStaged,
        ]);
        has::<fn(RepoBuilder, &str, &[u8]) -> RepoBuilder>(RepoBuilder::file);
        has::<fn(RepoBuilder, &str, &[u8]) -> RepoBuilder>(RepoBuilder::exec_file);
        has::<fn(RepoBuilder, &str, &str) -> RepoBuilder>(RepoBuilder::symlink);
        has::<fn(RepoBuilder, &str) -> RepoBuilder>(RepoBuilder::stage);
        has::<fn(RepoBuilder) -> RepoBuilder>(RepoBuilder::stage_all);
        has::<fn(RepoBuilder, &str) -> RepoBuilder>(RepoBuilder::commit);
        has::<fn(RepoBuilder, &str) -> RepoBuilder>(RepoBuilder::tag);
        has::<fn(RepoBuilder, &str, &str) -> RepoBuilder>(RepoBuilder::config);
        has::<fn(RepoBuilder) -> PathBuf>(RepoBuilder::build);
        has::<fn(Cmd, &str) -> Cmd>(Cmd::env_remove);
        has::<fn(Cmd, &[u8]) -> Cmd>(Cmd::stdin);
        has::<fn(Cmd) -> Cmd>(Cmd::tty);
        has::<fn(Cmd) -> Output>(Cmd::output);
    }
}
