//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.8 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-15 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::num::NonZeroUsize;
    use std::ops::Range;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_proc::hookcmd::{self, HookCmdError};
    use snp_proc::pool::{self, JobPool, Limiter, LimiterPermit, Permit};
    use snp_proc::registry::{self, Origin, Sig};
    use snp_proc::resolve::{self, ExeError};
    use snp_proc::signals::{self, RestoreShield, SignalGuard};
    use snp_proc::spawn::{
        self, Captured, Group, Merged, Output, OutputBudget, PyStatus, Role, SpawnError, Spec,
        Stdin,
    };
    use snp_proc::xargs::{self, ArgumentTooLong};
    use snp_pycompat::shlex::ShlexError;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    fn aggregate(v: Vec<PyStatus>) -> PyStatus {
        xargs::aggregate(v)
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Stdin>();
        send_sync::<Group>();
        send_sync::<Role>();
        send_sync::<Spec<'_>>();
        send_sync::<PyStatus>();
        send_sync::<OutputBudget>();
        send_sync::<Output>();
        send_sync::<Captured>();
        send_sync::<Merged>();
        send_sync::<SpawnError>();
        send_sync::<JobPool>();
        send_sync::<Permit<'_>>();
        send_sync::<Limiter>();
        send_sync::<LimiterPermit<'_>>();
        send_sync::<SignalGuard>();
        send_sync::<RestoreShield>();
        send_sync::<Sig>();
        send_sync::<Origin>();
        send_sync::<ExeError>();
        send_sync::<ArgumentTooLong>();
        send_sync::<HookCmdError>();
    }

    #[test]
    fn spawn() {
        has::<[Stdin; 4]>([
            Stdin::EmptyPipe,
            Stdin::Null,
            Stdin::Bytes(Arc::from(&b"x"[..])),
            Stdin::Inherit,
        ]);
        has::<[Group; 2]>([Group::Inherit, Group::Isolated]);
        has::<[Role; 3]>([Role::Hook, Role::Tool, Role::Critical]);
        let env = Env::from_pairs::<&str, &str>([]);
        let cancel = CancelToken::new();
        let argv = [OsString::from("/bin/true")];
        let spec = Spec {
            argv: &argv,
            env: &env,
            cwd: Path::new("/"),
            stdin: Stdin::Null,
            group: Group::Inherit,
            role: Role::Tool,
            deadline: Some(Instant::now()),
            cancel: &cancel,
        };
        has::<Spec<'_>>(spec.clone());
        has::<i64>(PyStatus(-9).0);
        has::<fn(usize, PathBuf) -> Arc<OutputBudget>>(OutputBudget::new);
        has::<Output>(Output::Memory(Vec::new()));
        has::<fn(File, u64) -> Output>(|file, len| Output::Spilled { file, len });
        has::<fn(&Output, &mut dyn io::Write) -> io::Result<()>>(Output::write_to);
        has::<fn(&Output) -> bool>(Output::is_blank);
        has::<Captured>(Captured {
            status: PyStatus(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            stopped_by_tty: false,
        });
        has::<Merged>(Merged {
            status: PyStatus(1),
            output: Output::Memory(Vec::new()),
            timed_out: false,
            interrupted: false,
        });
        has::<SpawnError>(SpawnError {
            program: OsString::new(),
            io: io::Error::from(io::ErrorKind::NotFound),
        });
        has::<fn(&SpawnError) -> String>(SpawnError::python_text);
        has::<fn(&Spec<'_>) -> Result<Captured, SpawnError>>(spawn::capture);
        has::<fn(&Spec<'_>, bool, &OutputBudget) -> Result<Merged, SpawnError>>(spawn::merged);
        has::<fn(&Spec<'_>, &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError>>(
            spawn::stream,
        );
        has::<fn(&Spec<'_>) -> Result<PyStatus, SpawnError>>(spawn::inherit);
    }

    #[test]
    fn pool_signals_registry() {
        has::<fn(Option<NonZeroUsize>, NonZeroUsize) -> NonZeroUsize>(pool::pool_size);
        has::<fn(NonZeroUsize, CancelToken) -> Arc<JobPool>>(JobPool::new);
        has::<fn(&JobPool) -> NonZeroUsize>(JobPool::permits);
        has::<for<'a> fn(&'a JobPool) -> Option<Permit<'a>>>(JobPool::acquire);
        has::<
            fn(
                &JobPool,
                NonZeroUsize,
                usize,
                fn(usize) -> Result<u8, Failure>,
            ) -> Vec<Result<u8, Failure>>,
        >(JobPool::map_ordered::<u8, fn(usize) -> Result<u8, Failure>>);
        has::<fn() -> bool>(JobPool::holds_permit);
        has::<fn(NonZeroUsize) -> Arc<Limiter>>(Limiter::new);
        has::<fn(&Limiter) -> NonZeroUsize>(Limiter::permits);
        has::<for<'a> fn(&'a Limiter, &CancelToken) -> Option<LimiterPermit<'a>>>(Limiter::acquire);
        has::<for<'a> fn(&'a Limiter, &CancelToken) -> Option<LimiterPermit<'a>>>(
            Limiter::acquire_all,
        );
        has::<fn(CancelToken, bool) -> io::Result<SignalGuard>>(signals::install);
        has::<fn(&SignalGuard) -> RestoreShield>(SignalGuard::shield);
        has::<fn(&SignalGuard) -> Option<i32>>(SignalGuard::die_by_signal_if_terminated);
        let sig = Sig::Int;
        match sig {
            Sig::Int | Sig::Term | Sig::Hup => {}
        }
        has::<[Origin; 3]>([Origin::Kernel, Origin::Sent, Origin::Unknown]);
        has::<fn(Sig, Origin)>(registry::forward);
        has::<fn()>(registry::escalate);
        has::<fn(Duration)>(registry::quiesce);
    }

    #[cfg(feature = "testing")]
    #[test]
    fn spawn_log() {
        use snp_proc::spawn_log;
        has::<fn(&Env)>(spawn_log::enable_from_env);
        has::<fn(&Path) -> Vec<String>>(spawn_log::read);
    }

    #[test]
    fn resolve_xargs_hookcmd() {
        has::<fn(&OsStr, &Env, &Path) -> Option<PathBuf>>(resolve::find_executable);
        has::<[ExeError; 4]>([
            ExeError::NotFound(OsString::new()),
            ExeError::IsDirectory(OsString::new()),
            ExeError::NotExecutable(OsString::new()),
            ExeError::Io(io::Error::from(io::ErrorKind::NotFound)),
        ]);
        has::<fn(&ExeError) -> String>(ExeError::message);
        has::<fn(&OsStr, &Env, &Path) -> Result<PathBuf, ExeError>>(resolve::normexe);
        has::<fn(&[OsString], &Env, &Path) -> Result<Vec<OsString>, ExeError>>(
            resolve::normalize_cmd,
        );
        has::<fn() -> io::Result<PathBuf>>(resolve::self_exe);
        has::<fn() -> NonZeroUsize>(xargs::cpu_count);
        has::<fn(&Env, bool, NonZeroUsize) -> NonZeroUsize>(xargs::target_concurrency);
        has::<fn(usize) -> usize>(xargs::platform_max_length);
        has::<fn(&Path) -> usize>(xargs::batch_max_length);
        has::<fn(&OsStr) -> usize>(xargs::cmdlen);
        has::<OsString>(ArgumentTooLong(OsString::new()).0);
        has::<
            fn(
                &[OsString],
                &[&OsStr],
                NonZeroUsize,
                usize,
            ) -> Result<Vec<Range<usize>>, ArgumentTooLong>,
        >(xargs::partition);
        has::<fn(Vec<PyStatus>) -> PyStatus>(aggregate);
        has::<[HookCmdError; 2]>([
            HookCmdError::Shlex(ShlexError::NoClosingQuotation),
            HookCmdError::Empty,
        ]);
        has::<fn(&str, &[Arc<str>], &Path) -> Result<Vec<OsString>, HookCmdError>>(
            hookcmd::hook_cmd,
        );
    }
}
