//! Process-level acceptance tests of WP-15: stdin kinds, merged ordering, pty facts, exit statuses, the pool and
//! limiter under stress, fd hygiene with daemonising partitions (S15), cancellation and deadlines with a descendant
//! holding the pipe, the fd-limit clamp, the output budget's resident-memory bound, descendant cleanup (`quiesce`),
//! the spawn log and the secret-free trace log. Tests that change process-wide state (rlimits, the subreaper,
//! killing every descendant) run in a driver process of their own.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use snp_core::cancel::{CancelReason, CancelToken};
use snp_proc::pool::{JobPool, Limiter};
use snp_proc::spawn::{self, Group, Output, OutputBudget, PyStatus};

use crate::common::{
    budget, bytes, driver_command, driver_param, env, hook, ms, probe, probe_argv, report, result,
    results,
};

/// Runs a driver scenario of this module; `false` when `scenario` is not one of them.
pub(crate) fn drive(scenario: &str) -> bool {
    match scenario {
        "nofile64" => nofile64(),
        "rss" => rss(),
        "quiesce" => quiesce_daemon(),
        "quiesce_hold" => quiesce_hold(),
        _ => return false,
    }
    true
}

/// Starts a driver for `scenario` and waits for it (≤ 120 s); returns its results.
fn run_driver(scenario: &str) -> Vec<(String, String)> {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("results");
    let mut child = driver_command("catalog::driver_entry", scenario)
        .env("SNP_PROC_TEST_DIR", dir.path())
        .env("SNP_PROC_TEST_OUT", &out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    let st = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(120),
            "driver {scenario} hung"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let r = results(&out);
    assert!(st.success(), "driver {scenario} failed: {st:?} {r:?}");
    r
}

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

/// ADR-019 / 14 A.4.1: a non-pty child's stdin is an empty FIFO; under a pty it is `/dev/null`.
#[cfg(unix)]
#[test]
fn stdin_is_a_fifo_without_pty_and_dev_null_under_pty() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["stdin-kind"]);
    let spec = hook(&argv, &env, d.path(), &cancel);
    let m = spawn::merged(&spec, false, &budget()).unwrap();
    assert_eq!(bytes(&m.output), b"file-type: pipe\ncrt-isatty: 0\n");
    let m = spawn::merged(&spec, true, &budget()).unwrap();
    assert_eq!(bytes(&m.output), b"file-type: char\ncrt-isatty: 0\n");
}

/// `stderr=STDOUT`: one pipe keeps the child's write order (pipe and pty).
#[test]
fn merged_output_preserves_interleaving() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["interleave", "40"]);
    let spec = hook(&argv, &env, d.path(), &cancel);
    let expected: Vec<u8> = (1..=40)
        .flat_map(|i| format!("stdout {i}\nstderr {i}\n").into_bytes())
        .collect();
    let m = spawn::merged(&spec, false, &budget()).unwrap();
    assert_eq!(m.status, PyStatus(0));
    assert_eq!(bytes(&m.output), expected);
    if cfg!(unix) {
        let m = spawn::merged(&spec, true, &budget()).unwrap();
        assert_eq!(bytes(&m.output), expected);
    }
}

/// `cmd_output_p`: the child sees a terminal on fds 1 and 2, not on 0, and `\n` is not translated.
#[cfg(unix)]
#[test]
fn pty_child_sees_a_tty_without_crlf_translation() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["tty"]);
    let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), true, &budget()).unwrap();
    assert_eq!(
        bytes(&m.output),
        b"stdin: False\nstdout: True\nstderr: True\n"
    );
    let argv = probe_argv(&["bytes", "610a620d0a63"]);
    let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), true, &budget()).unwrap();
    assert_eq!(bytes(&m.output), b"a\nb\r\nc");
}

/// Signal death → `PyStatus(-9)`; ENOEXEC → `[Errno 8] Exec format error: './x'` with no `/bin/sh` fallback.
#[cfg(unix)]
#[test]
fn signal_death_and_enoexec() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["signal", "9"]);
    let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap();
    assert_eq!(m.status, PyStatus(-9));
    std::fs::write(d.path().join("x"), b"echo would run under sh\n").unwrap();
    std::fs::set_permissions(d.path().join("x"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let argv = [OsString::from("./x")];
    let e = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap_err();
    assert_eq!(e.python_text(), "[Errno 8] Exec format error: './x'");
}

/// Windows exit codes are the unsigned 32-bit value (14 A.1 #4).
#[cfg(windows)]
#[test]
fn windows_exit_codes_are_unsigned() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["ntstatus", "C0000005"]);
    let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap();
    assert_eq!(m.status, PyStatus(3_221_225_477));
}

/// `JobPool` never exceeds its permits under a 200-task stress test (with real children).
#[test]
fn job_pool_never_exceeds_its_permits() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let pool = JobPool::new(nz(4), cancel.clone());
    let running = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let argv = probe_argv(&["count", "x"]);
    let out = pool.map_ordered(nz(32), 200, |i| {
        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        let r = if i % 5 == 0 {
            let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap();
            bytes(&m.output)
        } else {
            std::thread::sleep(Duration::from_millis(1));
            b"1\n".to_vec()
        };
        running.fetch_sub(1, Ordering::SeqCst);
        Ok(r)
    });
    assert_eq!(out.len(), 200);
    assert!(out.into_iter().all(|r| r.unwrap() == b"1\n"));
    assert!(
        peak.load(Ordering::SeqCst) <= 4,
        "{}",
        peak.load(Ordering::SeqCst)
    );
}

/// Limiter never exceeds its permits; `acquire_all` is exclusive.
#[test]
fn limiter_never_exceeds_its_permits() {
    let lim = Limiter::new(nz(3));
    let cancel = CancelToken::new();
    let held = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let violations = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for t in 0..200_usize {
            let (lim, cancel, held, peak, violations) = (&lim, &cancel, &held, &peak, &violations);
            std::thread::Builder::new()
                .spawn_scoped(s, move || {
                    if t % 25 == 0 {
                        let all = lim.acquire_all(cancel).unwrap();
                        let before = held.fetch_add(3, Ordering::SeqCst);
                        if before != 0 {
                            violations.fetch_add(1, Ordering::SeqCst);
                        }
                        std::thread::sleep(Duration::from_millis(2));
                        held.fetch_sub(3, Ordering::SeqCst);
                        drop(all);
                    } else {
                        let p = lim.acquire(cancel).unwrap();
                        let now = held.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_micros(500));
                        held.fetch_sub(1, Ordering::SeqCst);
                        drop(p);
                    }
                })
                .unwrap();
        }
    });
    assert!(peak.load(Ordering::SeqCst) <= 3);
    assert_eq!(violations.load(Ordering::SeqCst), 0);
}

/// S15 (ADR-037): 16 concurrent partitions, one of which daemonises, all complete promptly — no pipe or pty end
/// leaks into the daemon (pipe mode and, on POSIX, pty mode).
#[test]
fn s15_concurrent_partitions_with_a_daemon_complete_promptly() {
    let modes: &[bool] = if cfg!(unix) { &[false, true] } else { &[false] };
    for &pty in modes {
        let d = tempfile::tempdir().unwrap();
        let (env, cancel) = (env(), CancelToken::new());
        let pool = JobPool::new(nz(16), cancel.clone());
        let budget = budget();
        let t0 = Instant::now();
        let out = pool.map_ordered(nz(16), 16, |i| {
            let argv = if i == 7 {
                probe_argv(&["daemonize", "8"])
            } else {
                probe_argv(&["sleep", "200", "done"])
            };
            let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), pty, &budget).unwrap();
            Ok((m.status, bytes(&m.output)))
        });
        let took = t0.elapsed();
        for (i, r) in out.into_iter().enumerate() {
            let (st, o) = r.unwrap();
            assert_eq!(st, PyStatus(0));
            if i != 7 {
                assert_eq!(o, b"done\n");
            }
        }
        assert!(took < Duration::from_secs(6), "pty={pty} took {took:?}");
    }
}

/// A cancelled hook whose daemonised descendant holds stdout returns within grace + 500 ms (the reader is
/// abandoned); the partial output is kept.
#[test]
fn cancelled_hook_with_a_descendant_holding_stdout_returns_promptly() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["daemonize", "--hold", "6"]);
    let c2 = cancel.clone();
    let canceller = std::thread::Builder::new()
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel(CancelReason::Interrupt);
            Instant::now()
        })
        .unwrap();
    let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap();
    let cancelled_at = canceller.join().unwrap();
    let after_cancel = cancelled_at.elapsed();
    assert!(m.interrupted);
    assert!(bytes(&m.output).starts_with(b"daemon "));
    // grace (2 s) + drain (500 ms) + scheduling slack.
    assert!(
        after_cancel < Duration::from_millis(3300),
        "{after_cancel:?}"
    );
}

/// A timed (isolated) hook whose descendant left the group and holds stdout: the deadline still bounds the wait.
#[test]
fn deadline_bounds_a_hook_whose_descendant_holds_stdout() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["daemonize", "--hold", "6"]);
    let mut spec = hook(&argv, &env, d.path(), &cancel);
    spec.group = Group::Isolated;
    spec.deadline = Instant::now().checked_add(Duration::from_millis(300));
    let t0 = Instant::now();
    let m = spawn::merged(&spec, false, &budget()).unwrap();
    assert!(m.timed_out && !m.interrupted);
    assert_eq!(m.status, PyStatus(0), "the direct child exited normally");
    assert!(t0.elapsed() < Duration::from_millis(3600), "{}", ms(t0));
}

/// A run under `ulimit -n 64` completes: the pool is clamped to one permit and 16 partitions run.
#[cfg(unix)]
#[test]
fn ulimit_n_64_run_completes() {
    let r = run_driver("nofile64");
    assert_eq!(result(&r, "pool"), Some("1"), "{r:?}");
    assert_eq!(result(&r, "ok"), Some("16"), "{r:?}");
}

#[cfg(unix)]
fn nofile64() {
    use rustix::process::{Resource, Rlimit};
    let hard = rustix::process::getrlimit(Resource::Nofile).maximum;
    rustix::process::setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(64),
            maximum: hard,
        },
    )
    .unwrap();
    let n = snp_proc::pool::pool_size(None, nz(16));
    report("pool", n);
    let d = PathBuf::from(driver_param("SNP_PROC_TEST_DIR").unwrap());
    let (env, cancel) = (env(), CancelToken::new());
    let pool = JobPool::new(n, cancel.clone());
    let budget = budget();
    let argv = probe_argv(&["count", "a", "b"]);
    let out = pool.map_ordered(nz(16), 16, |_| {
        let m = spawn::merged(&hook(&argv, &env, &d, &cancel), false, &budget).unwrap();
        Ok(bytes(&m.output))
    });
    let ok = out
        .into_iter()
        .filter(|r| matches!(r, Ok(o) if o == b"2\n"))
        .count();
    report("ok", ok);
}

#[cfg(not(unix))]
fn nofile64() {}

/// `OutputBudget`: 32 partitions each printing 8 MiB keep the peak RSS below 100 MB (spill used) and every output is
/// byte-identical.
#[cfg(target_os = "linux")]
#[test]
fn output_budget_bounds_resident_memory() {
    let r = run_driver("rss");
    let peak_kb: u64 = result(&r, "peak_kb").unwrap().parse().unwrap();
    assert_eq!(result(&r, "identical"), Some("32"), "{r:?}");
    assert!(
        result(&r, "spilled").unwrap().parse::<usize>().unwrap() >= 20,
        "{r:?}"
    );
    assert!(peak_kb < 100 * 1024, "peak RSS {peak_kb} kB: {r:?}");
}

#[cfg(target_os = "linux")]
fn rss() {
    use std::io::Write;
    const SIZE: usize = 8 << 20;
    /// Compares the bytes written into it with the `snp-probe fill` pattern.
    struct Checker {
        pos: usize,
        ok: bool,
    }
    impl Write for Checker {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            const P: &[u8] = b"0123456789abcdef";
            for b in buf {
                if *b != P[self.pos % 16] {
                    self.ok = false;
                }
                self.pos += 1;
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let d = PathBuf::from(driver_param("SNP_PROC_TEST_DIR").unwrap());
    let (env, cancel) = (env(), CancelToken::new());
    let budget = OutputBudget::new(64 << 20, d.clone());
    let pool = JobPool::new(nz(32), cancel.clone());
    let argv = probe_argv(&["fill", &SIZE.to_string()]);
    // Like snp-run: every partition's output is held until all partitions are done.
    let outputs: Vec<Output> = pool
        .map_ordered(nz(32), 32, |_| {
            Ok(
                spawn::merged(&hook(&argv, &env, &d, &cancel), false, &budget)
                    .unwrap()
                    .output,
            )
        })
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let spilled = outputs
        .iter()
        .filter(|o| matches!(o, Output::Spilled { .. }))
        .count();
    let mut identical = 0;
    for o in &outputs {
        let mut c = Checker { pos: 0, ok: true };
        o.write_to(&mut c).unwrap();
        if c.ok && c.pos == SIZE {
            identical += 1;
        }
    }
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let peak = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .unwrap()
        .trim()
        .trim_end_matches(" kB")
        .trim()
        .to_owned();
    report("peak_kb", peak);
    report("spilled", spilled);
    report("identical", identical);
    // Nothing is left in the spill directory: the files were unnamed.
    report(
        "spill_entries",
        std::fs::read_dir(&d)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".snp-spill")
            })
            .count(),
    );
}

#[cfg(not(target_os = "linux"))]
fn rss() {}

/// Linux, subreaper: `quiesce` kills and reaps a daemonised descendant of a finished hook.
#[cfg(target_os = "linux")]
#[test]
fn quiesce_kills_a_daemonised_descendant_under_the_subreaper() {
    let r = run_driver("quiesce");
    assert_eq!(result(&r, "reparented"), Some("true"), "{r:?}");
    assert_eq!(result(&r, "alive_after"), Some("false"), "{r:?}");
}

#[cfg(target_os = "linux")]
fn quiesce_daemon() {
    let d = PathBuf::from(driver_param("SNP_PROC_TEST_DIR").unwrap());
    let cancel = CancelToken::new();
    let _guard = snp_proc::signals::install(cancel.clone(), true).unwrap();
    let env = env();
    let argv = probe_argv(&["daemonize", "30"]);
    let m = spawn::merged(&hook(&argv, &env, &d, &cancel), false, &budget()).unwrap();
    let text = String::from_utf8(bytes(&m.output)).unwrap();
    let pid: i32 = text
        .trim()
        .strip_prefix("daemon ")
        .unwrap()
        .parse()
        .unwrap();
    // The daemon's parent exited: it was reparented to this subreaper.
    let ppid = |pid: i32| -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.rsplit_once(')')?.1;
        after.split_whitespace().nth(1)?.parse().ok()
    };
    let t0 = Instant::now();
    while ppid(pid) != Some(std::process::id()) && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    report("reparented", ppid(pid) == Some(std::process::id()));
    cancel.cancel(CancelReason::Interrupt);
    snp_proc::registry::quiesce(Duration::from_millis(200));
    report("alive_after", Path::new(&format!("/proc/{pid}")).exists());
}

#[cfg(not(target_os = "linux"))]
fn quiesce_daemon() {}

/// `quiesce` ends a hook whose descendant holds its stdout (Linux: subreaper sweep; Windows: the hook's Job Object).
#[cfg(any(target_os = "linux", windows))]
#[test]
fn quiesce_releases_a_reader_blocked_by_a_descendant() {
    let r = run_driver("quiesce_hold");
    let took: u64 = result(&r, "took_ms").unwrap().parse().unwrap();
    assert!(took < 3000, "{r:?}");
    assert_eq!(result(&r, "status"), Some("0"), "{r:?}");
}

fn quiesce_hold() {
    let d = PathBuf::from(driver_param("SNP_PROC_TEST_DIR").unwrap());
    let cancel = CancelToken::new();
    let _guard = snp_proc::signals::install(CancelToken::new(), true).unwrap();
    let env = env();
    let argv = probe_argv(&["daemonize", "--hold", "20"]);
    let t0 = Instant::now();
    std::thread::scope(|s| {
        let h = std::thread::Builder::new()
            .spawn_scoped(s, || {
                spawn::merged(&hook(&argv, &env, &d, &cancel), false, &budget()).unwrap()
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        snp_proc::registry::quiesce(Duration::from_millis(100));
        let m = h.join().unwrap();
        report("status", m.status.0);
    });
    report("took_ms", t0.elapsed().as_millis());
}

/// `SNP_LOG=trace`: the spawn event shows no secret value (allow-list, ADR-077).
#[test]
fn trace_log_shows_no_secrets() {
    use std::sync::Mutex;
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buf = Buf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let d = tempfile::tempdir().unwrap();
    let env = snp_core::env::Env::from_pairs([
        ("PATH", "/usr/bin:/bin"),
        ("NPM_AUTH", "npm-token-SECRET1"),
        ("MYSQL_PWD", "mysql-SECRET2"),
        ("OPENAI_API_KEY", "sk-SECRET3"),
        ("SNP_JOBS", "4"),
    ]);
    let cancel = CancelToken::new();
    let argv = probe_argv(&["args", "https://user:SECRET4@example.com/repo.git"]);
    tracing::subscriber::with_default(subscriber, || {
        let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap();
        assert_eq!(m.status, PyStatus(0));
    });
    let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("spawn"), "{log}");
    assert!(
        log.contains("SNP_JOBS=\\\"4\\\"") || log.contains("SNP_JOBS=\"4\""),
        "{log}"
    );
    assert!(log.contains("NPM_AUTH=<redacted, 17 bytes>"), "{log}");
    for secret in ["SECRET1", "SECRET2", "SECRET3", "SECRET4"] {
        assert!(!log.contains(secret), "{secret} leaked: {log}");
    }
}

/// `SNP_TEST_SPAWN_LOG`: every spawn appends `<role> <group> <argv0 basename>`.
#[cfg(feature = "testing")]
#[test]
fn spawn_log_records_role_group_and_basename() {
    use snp_proc::spawn::Role;
    use snp_proc::spawn_log;
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("spawns.log");
    spawn_log::enable_from_env(&snp_core::env::Env::from_pairs([(
        "SNP_TEST_SPAWN_LOG",
        &log,
    )]));
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["count"]);
    let mut spec = hook(&argv, &env, d.path(), &cancel);
    spec.role = Role::Critical;
    spec.group = Group::Isolated;
    spawn::capture(&spec).unwrap();
    spawn_log::enable_from_env(&snp_core::env::Env::from_pairs::<&str, &str>([]));
    let lines = spawn_log::read(&log);
    let exe = format!(
        "critical isolated snp-probe{}",
        std::env::consts::EXE_SUFFIX
    );
    assert!(lines.contains(&exe), "{lines:?}");
    assert_eq!(
        spawn_log::read(&d.path().join("missing")),
        Vec::<String>::new()
    );
}

/// The probe used by every test exists and runs.
#[test]
fn probe_is_available() {
    assert!(probe().exists());
}

/// perf-smoke (`#[ignore = "perf"]`, release, 3× margin, ADR-071): a cancelled hook whose descendant holds stdout
/// returns within grace (2 s) + drain (500 ms); 3 × 2.5 s = 7.5 s.
#[test]
#[ignore = "perf"]
fn perf_cancelled_hook_returns_within_grace_plus_drain() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let argv = probe_argv(&["daemonize", "--hold", "12"]);
    let c2 = cancel.clone();
    let canceller = std::thread::Builder::new()
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            c2.cancel(CancelReason::Interrupt);
            Instant::now()
        })
        .unwrap();
    let m = spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget()).unwrap();
    let after_cancel = canceller.join().unwrap().elapsed();
    assert!(m.interrupted);
    assert!(
        after_cancel < Duration::from_millis(7500),
        "{after_cancel:?}"
    );
}

/// perf-smoke: T10-XRG-11's five concurrent 0.5 s partitions finish within 2.5 s; 3 × 2.5 s = 7.5 s.
#[test]
#[ignore = "perf"]
fn perf_concurrent_partitions_overlap() {
    let d = tempfile::tempdir().unwrap();
    let (env, cancel) = (env(), CancelToken::new());
    let pool = JobPool::new(nz(5), cancel.clone());
    let argv = probe_argv(&["sleep", "500"]);
    let t0 = Instant::now();
    let out = pool.map_ordered(nz(5), 5, |_| {
        Ok(
            spawn::merged(&hook(&argv, &env, d.path(), &cancel), false, &budget())
                .unwrap()
                .status,
        )
    });
    assert!(out.into_iter().all(|r| r.unwrap() == PyStatus(0)));
    assert!(t0.elapsed() < Duration::from_millis(7500), "{}", ms(t0));
}
