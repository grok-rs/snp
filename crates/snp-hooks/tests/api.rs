//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.19 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-00 / integration lead. The file spans WP-26 (framework, pygrep, fail, hazmat, P0, stubs)
//! and WP-27 (P1, P2, caveats), and lies outside their owned paths; it changes only with a
//! signature-change ADR.

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};
    use std::fmt::Debug;
    use std::path::Path;
    use std::sync::Arc;

    use snp_argparse::{Namespace, ParserSpec};
    use snp_cli::HazmatTool;
    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::{Failure, PyExc};
    use snp_core::path::RepoPath;
    use snp_git::DiffFingerprint;
    use snp_git::Git;
    use snp_git::pch_facts::PchFacts;
    use snp_hooks::content_cache::{self, Stats};
    use snp_hooks::framework::{self, FileStep, PyOut};
    use snp_hooks::pygrep::{self, Compiled};
    use snp_hooks::{
        ContentCache, Epoch, HookCx, NativeHook, NativeSource, Outcome, WriteGate, Writer, fail,
        hazmat, pch,
    };
    use snp_pyre::RegexCache;

    fn send_sync<T: Send + Sync>() {}
    fn debug<T: Debug>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// A trivial native, to check the trait's shape and its provided method.
    struct Probe;

    impl NativeHook for Probe {
        fn id(&self) -> &'static str {
            "probe"
        }
        fn prog(&self) -> &'static str {
            "probe-hook"
        }
        fn preflight(&self, _epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
            Ok(())
        }
        fn run(&self, _cx: &HookCx<'_>, _argv: &[&OsStr]) -> Outcome {
            Outcome::default()
        }
    }

    /// The trait's methods, through a trait object.
    fn native_hook(h: &dyn NativeHook, cx: &HookCx<'_>) {
        has::<&'static str>(h.id());
        has::<&'static str>(h.prog());
        has::<Result<(), String>>(h.preflight(Epoch(600), &[]));
        has::<Outcome>(h.run(cx, &[OsStr::new("a.py")]));
        has::<bool>(h.reports_modifications_exactly());
        has::<snp_git::pch_facts::FactSet>(h.facts(&[], Epoch(600)));
    }

    /// The public fields of `HookCx`.
    fn hook_cx(
        git: &Git,
        facts: &PchFacts,
        env: &Env,
        cancel: &CancelToken,
        cache: Option<&ContentCache>,
        gate: Option<&WriteGate>,
    ) {
        let cx = HookCx {
            root: Path::new("/r"),
            env,
            git,
            color: false,
            epoch: Epoch(600),
            windows: false,
            columns: 80,
            facts,
            cancel,
            cache,
            gate,
        };
        native_hook(&Probe, &cx);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<HookCx<'static>>();
        send_sync::<Outcome>();
        send_sync::<&'static dyn NativeHook>();
        send_sync::<PyOut>();
        send_sync::<FileStep>();
        send_sync::<Compiled>();
        debug::<Compiled>();
        send_sync::<ContentCache>();
        debug::<ContentCache>();
        send_sync::<WriteGate>();
        debug::<WriteGate>();
        send_sync::<Writer<'static, 'static>>();
        debug::<Writer<'static, 'static>>();
    }

    #[test]
    fn hook_contract() {
        has::<fn(&Git, &PchFacts, &Env, &CancelToken, Option<&ContentCache>, Option<&WriteGate>)>(
            hook_cx,
        );
        has::<fn(&dyn NativeHook, &HookCx<'_>)>(native_hook);
        assert!(Probe.reports_modifications_exactly());
        // ADR-102: the provided `facts` method names no fact by default
        assert!(Probe.facts(&[], Epoch(600)).is_empty());
        let outcome = Outcome {
            code: 1,
            output: b"x\n".to_vec(),
            modified: Vec::new(),
            crashed: false,
            reads_only: true,
        };
        has::<Outcome>(outcome);
        let empty = Outcome::default();
        assert_eq!(
            (empty.code, empty.crashed, empty.reads_only),
            (0, false, false)
        );
        has::<fn(NativeSource, &str) -> Option<&'static dyn NativeHook>>(snp_hooks::native);
        has::<[NativeSource; 2]>([NativeSource::PreCommitHooks, NativeSource::Builtin]);
        has::<fn(&Env, Option<usize>) -> usize>(snp_hooks::columns);
    }

    #[test]
    fn registries() {
        has::<&'static [&'static dyn NativeHook]>(pch::p0::HOOKS);
        has::<&'static [&'static dyn NativeHook]>(pch::p1::HOOKS);
        has::<&'static [&'static dyn NativeHook]>(pch::p2::HOOKS);
        has::<&'static [&'static dyn NativeHook]>(pch::caveats::HOOKS);
        has::<&'static [&'static dyn NativeHook]>(pch::stubs::HOOKS);
    }

    #[test]
    fn framework() {
        has::<fn(&'static ParserSpec, &str, &[&OsStr], usize, &Env) -> Result<Namespace, Outcome>>(
            framework::parse_args,
        );
        has::<fn() -> PyOut>(PyOut::default);
        has::<fn(&mut PyOut, &str)>(PyOut::print);
        has::<fn(&mut PyOut, &[u8])>(PyOut::print_bytes_text);
        has::<fn(&mut PyOut, &[u8])>(PyOut::write_bytes);
        has::<fn(&mut PyOut, &str)>(PyOut::eprint);
        has::<fn(PyOut) -> Vec<u8>>(PyOut::finish);
        has::<[FileStep; 4]>([
            FileStep::Unchanged,
            FileStep::Changed(RepoPath::new(b"a.py").unwrap()),
            FileStep::Failed,
            FileStep::Crash(PyExc::new("UnicodeDecodeError", "x")),
        ]);
        has::<
            fn(
                &HookCx<'_>,
                &[&OsStr],
                &mut PyOut,
                &mut dyn FnMut(&OsStr, &mut PyOut) -> FileStep,
            ) -> Outcome,
        >(framework::for_each_file);
        has::<fn(&PyExc) -> Vec<u8>>(framework::crash_line);
    }

    #[test]
    fn pygrep_fail_hazmat() {
        has::<fn(&[Arc<str>], &str, usize) -> Result<Compiled, (i32, Vec<u8>)>>(pygrep::prepare);
        has::<fn(&Compiled, &[&OsStr], &Path, &RegexCache, &CancelToken) -> Outcome>(
            pygrep::run_partition,
        );
        has::<
            fn(
                &Compiled,
                &[&OsStr],
                &Path,
                &RegexCache,
                &CancelToken,
                Option<&ContentCache>,
            ) -> Outcome,
        >(pygrep::run_partition_cached);
        has::<fn(&str, &[&[u8]]) -> Vec<u8>>(fail::output);
        has::<fn(&[OsString], &Env) -> i32>(hazmat::in_hook_main);
        has::<fn(&HazmatTool, &Env, &CancelToken) -> Result<i32, Failure>>(hazmat::cli);
        has::<&'static ParserSpec>(hazmat::HAZMAT_PY_SPEC);
    }

    /// ADR-132: the deferred baseline's write gate and the write token, the only way a native writes.
    #[test]
    fn write_gate() {
        has::<fn(&Git) -> WriteGate>(WriteGate::new);
        has::<fn(&WriteGate) -> bool>(WriteGate::fired);
        has::<fn(WriteGate) -> Option<Result<DiffFingerprint, Failure>>>(WriteGate::into_baseline);
        has::<for<'c, 'a> fn(&'c HookCx<'a>)>(writer_api);
    }

    /// `HookCx::writer` and the `Writer` methods at one pair of lifetimes.
    fn writer_api<'c, 'a>(_cx: &'c HookCx<'a>) {
        has::<fn(&'c HookCx<'a>) -> Result<Writer<'c, 'a>, PyExc>>(HookCx::writer);
        has::<fn(&Writer<'c, 'a>, &OsStr, &[u8]) -> std::io::Result<()>>(Writer::rewrite);
        has::<fn(&Writer<'c, 'a>, &OsStr) -> std::io::Result<()>>(Writer::append_newline);
        has::<fn(&Writer<'c, 'a>, &OsStr, u64) -> std::io::Result<()>>(Writer::truncate);
    }

    /// ADR-131: the content cache of a window of consecutive in-process hooks.
    #[test]
    fn content_cache() {
        has::<fn(&Path, bool) -> Option<ContentCache>>(ContentCache::new);
        has::<fn(&ContentCache) -> Stats>(ContentCache::stats);
        has::<fn(&ContentCache) -> u64>(ContentCache::mismatches);
        has::<fn(&str) -> bool>(content_cache::reads_contents);
        has::<fn() -> u64>(content_cache::process_hits);
        has::<usize>(content_cache::PER_FILE_CAP);
        has::<usize>(content_cache::TOTAL_CAP);
        has::<std::time::Duration>(content_cache::RACY_WINDOW);
        let s = Stats {
            hits: 0,
            stale: 0,
            fills: 0,
            refused: 0,
        };
        assert_eq!(s, Stats::default());
    }
}
