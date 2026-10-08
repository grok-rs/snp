//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.1 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions
//! (`SpawnGuard` is `!Send` and excluded).
//!
//! Owner: WP-00.

#[cfg(test)]
mod api {
    use std::borrow::Cow;
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
    use std::time::{Duration, Instant};

    use snp_core::brand::Brand;
    use snp_core::cancel::{CancelReason, CancelToken};
    use snp_core::clock::{Clock, SystemClock};
    use snp_core::env::{Env, EnvOp, EnvPatch, Part};
    use snp_core::failure::{ExcKind, Failure, PyExc, SnpResult, pyerr};
    use snp_core::fault::{self, Point};
    use snp_core::lang::LanguageId;
    use snp_core::output::{
        CollectWarnings, Color, ColorSetting, DiagSink, Level, Printer, PrinterOpts, WarnSink,
    };
    use snp_core::path::RepoPath;
    use snp_core::profile;
    use snp_core::spawnlock::{self, SpawnGuard};
    use snp_core::stage::{ArgCount, HookType, Stage, StageSet};
    use snp_core::sync;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    // The `// exhaustive` enums of §1.4.1: each match names every variant and has no wildcard arm, so adding,
    // removing or renaming a variant breaks compilation.

    const fn hook_type(t: HookType) -> u8 {
        match t {
            HookType::CommitMsg => 0,
            HookType::PostCheckout => 1,
            HookType::PostCommit => 2,
            HookType::PostMerge => 3,
            HookType::PostRewrite => 4,
            HookType::PreCommit => 5,
            HookType::PreMergeCommit => 6,
            HookType::PrePush => 7,
            HookType::PreRebase => 8,
            HookType::PrepareCommitMsg => 9,
        }
    }

    const fn arg_count(a: ArgCount) -> (u8, u8) {
        match a {
            ArgCount::Exact(n) => (n, n),
            ArgCount::Range(lo, hi) => (lo, hi),
        }
    }

    const fn stage_hook(s: Stage) -> Option<HookType> {
        match s {
            Stage::Hook(t) => Some(t),
            Stage::Manual => None,
        }
    }

    const fn language(l: LanguageId) -> u8 {
        match l {
            LanguageId::Conda => 0,
            LanguageId::Coursier => 1,
            LanguageId::Dart => 2,
            LanguageId::Docker => 3,
            LanguageId::DockerImage => 4,
            LanguageId::Dotnet => 5,
            LanguageId::Fail => 6,
            LanguageId::Golang => 7,
            LanguageId::Haskell => 8,
            LanguageId::Julia => 9,
            LanguageId::Lua => 10,
            LanguageId::Node => 11,
            LanguageId::Perl => 12,
            LanguageId::Pygrep => 13,
            LanguageId::Python => 14,
            LanguageId::R => 15,
            LanguageId::Ruby => 16,
            LanguageId::Rust => 17,
            LanguageId::Swift => 18,
            LanguageId::Unsupported => 19,
            LanguageId::UnsupportedScript => 20,
        }
    }

    const fn brand(b: Brand) -> bool {
        match b {
            Brand::PreCommit => false,
            Brand::Snp => true,
        }
    }

    const fn exc_kind(k: ExcKind) -> bool {
        match k {
            ExcKind::Fatal => false,
            ExcKind::Unexpected => true,
        }
    }

    const fn color_setting(c: ColorSetting) -> u8 {
        match c {
            ColorSetting::Auto => 0,
            ColorSetting::Always => 1,
            ColorSetting::Never => 2,
        }
    }

    #[test]
    fn exhaustive_enums() {
        let hooks: Vec<u8> = HookType::ALL.into_iter().map(hook_type).collect();
        assert_eq!(hooks, (0..10).collect::<Vec<u8>>());
        let langs: Vec<u8> = LanguageId::ALL.into_iter().map(language).collect();
        assert_eq!(langs, (0..21).collect::<Vec<u8>>());
        assert_eq!(arg_count(ArgCount::Exact(1)), (1, 1));
        assert_eq!(arg_count(ArgCount::Range(1, 3)), (1, 3));
        assert_eq!(
            stage_hook(Stage::Hook(HookType::PrePush)),
            Some(HookType::PrePush)
        );
        assert_eq!(stage_hook(Stage::Manual), None);
        assert!(!brand(Brand::PreCommit) && brand(Brand::Snp));
        assert!(!exc_kind(ExcKind::Fatal) && exc_kind(ExcKind::Unexpected));
        assert_eq!(
            [
                ColorSetting::Auto,
                ColorSetting::Always,
                ColorSetting::Never
            ]
            .map(color_setting),
            [0, 1, 2]
        );
    }

    /// Calls both seam-trait methods through trait objects.
    fn sinks(w: &dyn WarnSink, d: &dyn DiagSink) {
        w.warn("x");
        d.diag("y");
    }

    /// The seam-trait methods (`WarnSink::warn`, `DiagSink::diag`) pinned through every implementor, so the
    /// trait method resolves (not `Printer`'s inherent `diag`).
    #[test]
    fn sink_methods() {
        has::<fn(&Printer, &str)>(<Printer as WarnSink>::warn);
        has::<fn(&Printer, &str)>(<Printer as DiagSink>::diag);
        has::<fn(&CollectWarnings, &str)>(<CollectWarnings as WarnSink>::warn);
        has::<fn(&CollectWarnings, &str)>(<CollectWarnings as DiagSink>::diag);
        has::<fn(&dyn WarnSink, &dyn DiagSink)>(sinks);
        let c = CollectWarnings::new();
        sinks(&c, &c);
        assert_eq!(c.take().len(), 2);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<HookType>();
        send_sync::<ArgCount>();
        send_sync::<Stage>();
        send_sync::<StageSet>();
        send_sync::<LanguageId>();
        send_sync::<Brand>();
        send_sync::<Env>();
        send_sync::<Part>();
        send_sync::<EnvOp>();
        send_sync::<EnvPatch>();
        send_sync::<CancelReason>();
        send_sync::<CancelToken>();
        send_sync::<ExcKind>();
        send_sync::<PyExc>();
        send_sync::<Failure>();
        send_sync::<ColorSetting>();
        send_sync::<Color>();
        send_sync::<Level>();
        send_sync::<PrinterOpts>();
        send_sync::<Printer>();
        send_sync::<CollectWarnings>();
        send_sync::<RepoPath>();
        send_sync::<SystemClock>();
        send_sync::<Point>();
    }

    #[test]
    fn stage() {
        has::<[HookType; 10]>(HookType::ALL);
        has::<fn(HookType) -> &'static str>(HookType::as_str);
        has::<fn(&str) -> Option<HookType>>(HookType::parse);
        has::<fn(HookType) -> ArgCount>(HookType::arg_count);
        has::<fn(HookType) -> Stage>(HookType::stage);
        has::<ArgCount>(ArgCount::Exact(1));
        has::<ArgCount>(ArgCount::Range(1, 3));
        has::<[Stage; 11]>(Stage::ALL);
        has::<fn(Stage) -> &'static str>(Stage::as_str);
        has::<fn(&str) -> Option<Stage>>(Stage::parse);
        has::<fn(&str) -> &str>(Stage::transform_legacy);
        has::<fn(&str) -> Option<(Stage, bool)>>(Stage::parse_with_legacy);
        has::<Stage>(Stage::Hook(HookType::PreCommit));
        has::<Stage>(Stage::Manual);
        has::<StageSet>(StageSet::ALL);
        has::<StageSet>(StageSet::EMPTY);
        has::<fn(StageSet, Stage) -> StageSet>(StageSet::with);
        has::<fn(StageSet, Stage) -> bool>(StageSet::contains);
        has::<fn(StageSet) -> bool>(StageSet::is_empty);
        let it = StageSet::ALL.iter();
        has::<Vec<Stage>>(it.collect());
    }

    #[test]
    fn lang_brand_profile() {
        has::<[LanguageId; 21]>(LanguageId::ALL);
        has::<fn(LanguageId) -> &'static str>(LanguageId::as_str);
        has::<fn(&str) -> Option<LanguageId>>(LanguageId::from_config);
        has::<fn(LanguageId) -> Option<&'static str>>(LanguageId::env_dir_base);
        has::<fn(Option<&OsStr>) -> Brand>(Brand::detect);
        has::<fn(Brand) -> &'static str>(Brand::prog);
        has::<fn(Brand) -> String>(Brand::version_line);
        has::<fn(Brand) -> &'static str>(Brand::crash_log_name);
        has::<[&str; 9]>([
            profile::SNP_VERSION,
            profile::PRE_COMMIT,
            profile::ARGPARSE_PY,
            profile::RE_GRAMMAR_PY,
            profile::RE_UNICODE,
            profile::TEXT_UNICODE,
            profile::PCH_PY,
            profile::IDENTIFY,
            profile::BUG_URL,
        ]);
        has::<u32>(profile::LOCAL_TEMPLATE_VERSION);
    }

    #[test]
    fn env() {
        has::<fn() -> Env>(Env::capture_process);
        let e: Env = Env::from_pairs::<&str, OsString>([("A", OsString::from("1"))]);
        has::<for<'a> fn(&'a Env, &str) -> Option<&'a OsStr>>(Env::get);
        has::<for<'a> fn(&'a Env, &str) -> Option<&'a OsStr>>(Env::get_nonempty);
        has::<fn(&Env, &str) -> bool>(Env::is_present);
        has::<fn(&Env, &str) -> bool>(Env::is_truthy_flag);
        has::<fn(&Env, &EnvPatch) -> Env>(Env::patched);
        has::<fn(&Env, &[&str]) -> Env>(Env::without);
        has::<Vec<(&OsStr, &OsStr)>>(e.iter().collect());
        has::<fn(&Env) -> usize>(Env::python_environ_size);
        has::<fn(&Env, &mut std::process::Command)>(Env::apply_to);
        has::<Part>(Part::Lit(OsString::new()));
        has::<Part>(Part::Var {
            name: Cow::Borrowed("PATH"),
            default: Cow::Borrowed(""),
        });
        has::<EnvOp>(EnvOp::Set(OsString::new()));
        has::<EnvOp>(EnvOp::Unset);
        has::<EnvOp>(EnvOp::Template(Box::new([])));
        let p = EnvPatch::default()
            .set("K", "v")
            .unset(String::from("U"))
            .prepend_path("PATH", [PathBuf::from("/bin")])
            .template("T", vec![Part::Lit("x".into())])
            .extend(EnvPatch::default());
        has::<fn(&EnvPatch) -> bool>(EnvPatch::is_empty);
        has::<Env>(e.patched(&p));
        has::<EnvPatch>(p);
        has::<fn(&Env) -> Env>(Env::clone);
    }

    #[test]
    fn cancel() {
        has::<fn() -> CancelToken>(CancelToken::new);
        has::<fn(&CancelToken, CancelReason)>(CancelToken::cancel);
        has::<fn(&CancelToken) -> Option<CancelReason>>(CancelToken::reason);
        has::<fn(&CancelToken) -> bool>(CancelToken::is_cancelled);
        has::<fn(&CancelToken, Duration) -> bool>(CancelToken::wait_timeout);
        has::<fn(&CancelToken) -> CancelToken>(CancelToken::child);
        has::<CancelToken>(CancelToken::default());
        has::<[CancelReason; 5]>([
            CancelReason::Interrupt,
            CancelReason::Terminate,
            CancelReason::Hangup,
            CancelReason::StdoutClosed,
            CancelReason::Panic,
        ]);
    }

    #[test]
    fn failures() {
        has::<[ExcKind; 2]>([ExcKind::Fatal, ExcKind::Unexpected]);
        let exc: PyExc = PyExc::new("FatalError", b"x".to_vec()).with_hint(String::from("h"));
        let PyExc {
            ty: _,
            text: _,
            hint: _,
            chain: _,
            backtrace: _,
        }: &PyExc = &exc;
        has::<Cow<'static, str>>(exc.ty.clone());
        has::<fn(&str, Option<&str>, Option<String>) -> PyExc>(PyExc::panic);
        has::<Failure>(Failure::Exc {
            kind: ExcKind::Fatal,
            exc,
        });
        has::<Failure>(Failure::Interrupted(CancelReason::Interrupt));
        has::<Failure>(Failure::StdoutClosed);
        has::<Failure>(Failure::SystemExit {
            code: 1,
            stderr: Vec::new(),
        });
        has::<Failure>(Failure::Usage {
            stderr: String::new(),
        });
        has::<Failure>(Failure::Exit(1));
        has::<Failure>(Failure::fatal("FatalError", "t"));
        has::<Failure>(Failure::unexpected(String::from("OSError"), b"t".to_vec()));
        has::<fn(&io::Error, Option<&OsStr>, Option<&OsStr>) -> Failure>(Failure::from_io);
        has::<fn(&Failure) -> i32>(Failure::exit_code);
        has::<fn(&Failure) -> bool>(Failure::writes_crash_log);
        has::<SnpResult<()>>(Ok(()));
        has::<fn(&io::Error) -> &'static str>(pyerr::os_error_class);
        has::<fn(&io::Error, Option<&OsStr>, Option<&OsStr>) -> String>(pyerr::os_error_text);
        has::<fn(&[OsString], i64, Option<&[u8]>, Option<&[u8]>) -> Vec<u8>>(
            pyerr::called_process_a,
        );
        has::<fn(&[OsString], i64) -> String>(pyerr::called_process_b);
    }

    #[test]
    fn output() {
        has::<fn(&OsStr) -> Option<ColorSetting>>(ColorSetting::parse);
        has::<[ColorSetting; 3]>([
            ColorSetting::Auto,
            ColorSetting::Always,
            ColorSetting::Never,
        ]);
        has::<[Color; 5]>([
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Turquoise,
            Color::Subtle,
        ]);
        has::<[Level; 3]>([Level::Info, Level::Warning, Level::Error]);
        let o = PrinterOpts {
            brand: Brand::Snp,
            color: false,
            stdout_is_tty: false,
            windows_text_mode: false,
            cancel: CancelToken::new(),
        };
        has::<fn(Box<dyn io::Write + Send>, Box<dyn io::Write + Send>, PrinterOpts) -> Printer>(
            Printer::new,
        );
        has::<fn(PrinterOpts) -> Printer>(Printer::stdio);
        let p = Printer::new(Box::new(io::sink()), Box::new(io::sink()), o);
        has::<fn(&Printer) -> Brand>(Printer::brand);
        has::<fn(&Printer) -> bool>(Printer::color);
        has::<fn(&Printer, &[u8])>(Printer::write);
        has::<fn(&Printer, &[u8])>(Printer::write_line);
        has::<fn(&Printer, Level, &str)>(Printer::log);
        has::<fn(&Printer, &str)>(Printer::py_print);
        has::<fn(&Printer, &str)>(Printer::py_eprint);
        has::<fn(&Printer, &[u8])>(Printer::write_stderr);
        has::<fn(&Printer, &str)>(Printer::diag);
        has::<Cow<'_, str>>(p.paint("x", Color::Green));
        has::<fn(&Printer)>(Printer::flush);
        has::<fn(&Printer)>(Printer::finish);
        has::<fn(&Printer) -> bool>(Printer::is_closed);
        has::<&dyn WarnSink>(&p);
        has::<&dyn DiagSink>(&p);
        has::<fn() -> CollectWarnings>(CollectWarnings::new);
        has::<fn(&CollectWarnings) -> Vec<String>>(CollectWarnings::take);
        let c = CollectWarnings::default();
        has::<&dyn WarnSink>(&c);
        has::<&dyn DiagSink>(&c);
        has::<String>(format!("{p:?}{c:?}"));
    }

    #[test]
    fn misc() {
        has::<fn(&[u8]) -> Option<RepoPath>>(RepoPath::new);
        let r = RepoPath::new(b"a").unwrap();
        has::<&[u8]>(r.as_bytes());
        has::<Cow<'_, OsStr>>(r.to_os());
        has::<Cow<'_, str>>(r.lossy());
        has::<fn(&RepoPath) -> bool>(RepoPath::is_contained);
        let clock: &dyn Clock = &SystemClock;
        has::<Instant>(clock.now());
        has::<u64>(clock.unix_secs());
        let m = Mutex::new(0);
        has::<MutexGuard<'_, i32>>(sync::lock(&m));
        let l = RwLock::new(0);
        has::<RwLockReadGuard<'_, i32>>(sync::read(&l));
        drop(sync::read(&l));
        has::<RwLockWriteGuard<'_, i32>>(sync::write(&l));
        has::<fn() -> SpawnGuard>(spawnlock::guard);
        has::<[Point; 6]>([
            Point::PrinterWrite,
            Point::WorkerPartition,
            Point::StashState(1),
            Point::RestoreCommand(1),
            Point::InstallCommit,
            Point::StoreMaterialize,
        ]);
        has::<fn(&Env)>(fault::arm_from_env);
        has::<fn(Point)>(fault::hit);
    }
}
