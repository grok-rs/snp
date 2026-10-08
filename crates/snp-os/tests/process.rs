//! Process-level acceptance tests of snp-os (WP-15): file locks between two processes and cancellation while
//! blocked, unnamed spill files after `kill -9`, `remove_tree_within` against a concurrent symlink swap, and the
//! `decide_color` matrix with stderr on a real terminal. A second process is this test binary re-executed with
//! `SNP_OS_TEST_DRIVER` set (only `driver_entry` runs then).
//!
//! Owner: WP-15.

#[cfg(test)]
mod process {
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use snp_core::cancel::{CancelReason, CancelToken};
    use snp_core::env::Env;
    use snp_core::output::ColorSetting;
    use snp_os::lock::{FileLock, LockMode};

    const DRIVER_VAR: &str = "SNP_OS_TEST_DRIVER";

    fn param(name: &str) -> Option<String> {
        #[expect(clippy::disallowed_methods, reason = "test driver parameters")]
        std::env::var(name).ok()
    }

    /// Re-runs this binary as a driver for `scenario` with `envs`.
    fn driver(scenario: &str, envs: &[(&str, &Path)], stderr: Stdio) -> Child {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "process::driver_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(DRIVER_VAR, scenario)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.spawn().unwrap()
    }

    fn wait_for(path: &Path, secs: u64) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn wait_exit(child: &mut Child, secs: u64) -> std::process::ExitStatus {
        let t0 = Instant::now();
        loop {
            if let Some(st) = child.try_wait().unwrap() {
                return st;
            }
            assert!(t0.elapsed() < Duration::from_secs(secs), "driver hung");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn driver_entry() {
        let Some(scenario) = param(DRIVER_VAR) else {
            return;
        };
        let path = |n: &str| PathBuf::from(param(n).unwrap());
        match scenario.as_str() {
            "lock" => {
                let mode = if param("SNP_OS_LOCK_MODE").as_deref() == Some("shared") {
                    LockMode::Shared
                } else {
                    LockMode::Exclusive
                };
                let lock = FileLock::acquire(
                    &path("SNP_OS_LOCK"),
                    mode,
                    &mut || {},
                    &CancelToken::new(),
                    None,
                )
                .unwrap();
                std::fs::write(path("SNP_OS_READY"), b"").unwrap();
                assert!(wait_for(&path("SNP_OS_RELEASE"), 30));
                drop(lock);
            }
            "spill" => {
                use std::io::Write;
                let mut f = snp_os::fs::unnamed_temp(&path("SNP_OS_SPILL_DIR")).unwrap();
                f.write_all(&vec![b's'; 1 << 20]).unwrap();
                std::fs::write(path("SNP_OS_READY"), b"").unwrap();
                std::thread::sleep(Duration::from_secs(30));
                drop(f);
            }
            "color" => {
                use std::fmt::Write as _;
                let mut out = String::new();
                for (term, setting) in [
                    (Some("xterm-256color"), ColorSetting::Auto),
                    (Some("dumb"), ColorSetting::Auto),
                    (None, ColorSetting::Auto),
                    (Some("dumb"), ColorSetting::Always),
                    (Some("xterm"), ColorSetting::Never),
                ] {
                    let env = match term {
                        Some(t) => Env::from_pairs([("TERM", t)]),
                        None => Env::from_pairs::<&str, &str>([]),
                    };
                    let got = snp_os::term::decide_color(setting, &env);
                    let _ignored: std::fmt::Result =
                        writeln!(out, "{setting:?}/{}={got}", term.unwrap_or("unset"));
                }
                std::fs::write(path("SNP_OS_OUT"), out).unwrap();
            }
            other => panic!("unknown scenario {other}"),
        }
    }

    /// Holds a lock of `mode` in another process until `release` appears.
    fn hold(mode: &str, lock: &Path, dir: &Path) -> (Child, PathBuf) {
        let ready = dir.join(format!("ready-{mode}"));
        let release = dir.join(format!("release-{mode}"));
        let mode_path = PathBuf::from(mode);
        let child = driver(
            "lock",
            &[
                ("SNP_OS_LOCK", lock),
                ("SNP_OS_READY", &ready),
                ("SNP_OS_RELEASE", &release),
                ("SNP_OS_LOCK_MODE", &mode_path),
            ],
            Stdio::null(),
        );
        assert!(wait_for(&ready, 30), "holder never locked");
        (child, release)
    }

    /// pre-commit's store lock semantics between processes: exclusive excludes everything, shared locks coexist, a
    /// blocked `acquire` calls `on_wait` once and gets the lock when the other process releases it.
    #[test]
    fn file_lock_semantics_across_two_processes() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join(".lock");
        let (mut child, release) = hold("exclusive", &lock, d.path());
        assert!(
            FileLock::try_acquire(&lock, LockMode::Exclusive)
                .unwrap()
                .is_none()
        );
        assert!(
            FileLock::try_acquire(&lock, LockMode::Shared)
                .unwrap()
                .is_none()
        );
        let rel = release;
        let t = std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::write(rel, b"").unwrap();
            })
            .unwrap();
        let mut waits = 0;
        let got = FileLock::acquire(
            &lock,
            LockMode::Exclusive,
            &mut || waits += 1,
            &CancelToken::new(),
            Some(Duration::from_secs(20)),
        );
        t.join().unwrap();
        assert!(got.is_ok());
        assert_eq!(waits, 1);
        drop(got);
        assert!(wait_exit(&mut child, 30).success());

        let (mut child, release) = hold("shared", &lock, d.path());
        let shared = FileLock::try_acquire(&lock, LockMode::Shared).unwrap();
        assert!(shared.is_some(), "shared locks coexist");
        assert!(
            FileLock::try_acquire(&lock, LockMode::Exclusive)
                .unwrap()
                .is_none()
        );
        drop(shared);
        std::fs::write(&release, b"").unwrap();
        assert!(wait_exit(&mut child, 30).success());
    }

    /// A blocked `acquire` observes cancellation (Err(Interrupted)) and its timeout (Err(TimedOut)).
    #[test]
    fn file_lock_wait_is_cancellable_and_bounded() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join(".lock");
        let (mut child, release) = hold("exclusive", &lock, d.path());
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let t = std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                c2.cancel(CancelReason::Interrupt);
            })
            .unwrap();
        let t0 = Instant::now();
        let e = FileLock::acquire(&lock, LockMode::Shared, &mut || {}, &cancel, None).unwrap_err();
        t.join().unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::Interrupted);
        assert!(t0.elapsed() < Duration::from_secs(5));
        let e = FileLock::acquire(
            &lock,
            LockMode::Exclusive,
            &mut || {},
            &CancelToken::new(),
            Some(Duration::from_millis(150)),
        )
        .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
        std::fs::write(&release, b"").unwrap();
        assert!(wait_exit(&mut child, 30).success());
    }

    /// ADR-077: an unnamed spill file leaves nothing behind when its process is killed (`kill -9`).
    #[test]
    fn unnamed_temp_leaves_nothing_after_kill_9() {
        let d = tempfile::tempdir().unwrap();
        let spill = d.path().join("spill");
        std::fs::create_dir_all(&spill).unwrap();
        let ready = d.path().join("ready");
        let mut child = driver(
            "spill",
            &[("SNP_OS_SPILL_DIR", &spill), ("SNP_OS_READY", &ready)],
            Stdio::null(),
        );
        assert!(wait_for(&ready, 30));
        if cfg!(unix) {
            // Unnamed while alive, too.
            assert_eq!(std::fs::read_dir(&spill).unwrap().count(), 0);
        }
        child.kill().unwrap();
        let _status = child.wait().unwrap();
        let t0 = Instant::now();
        while std::fs::read_dir(&spill).unwrap().count() != 0
            && t0.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_dir(&spill).unwrap().count(), 0);
    }

    /// CVE-2022-21658 class: a directory swapped for a symlink to an outside directory while the tree is being
    /// removed is never followed; nothing outside `root` is touched.
    #[cfg(unix)]
    #[test]
    fn remove_tree_within_survives_a_concurrent_symlink_swap() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for round in 0..20 {
            let d = tempfile::tempdir().unwrap();
            let root = d.path().join("root");
            let outside = d.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            for i in 0..50 {
                std::fs::write(outside.join(format!("precious{i}")), b"keep").unwrap();
            }
            let victim = root.join("victim");
            for i in 0..40 {
                let sub = victim.join(format!("d{i}"));
                std::fs::create_dir_all(sub.join("deeper")).unwrap();
                for j in 0..10 {
                    std::fs::write(sub.join("deeper").join(format!("f{j}")), b"x").unwrap();
                }
            }
            let stop = AtomicBool::new(false);
            std::thread::scope(|s| {
                let swapper = std::thread::Builder::new()
                    .spawn_scoped(s, || {
                        let mut k = 0;
                        while !stop.load(Ordering::Relaxed) {
                            let target = victim.join(format!("d{}", k % 40));
                            let parked = root.join(format!("parked{round}-{k}"));
                            if std::fs::rename(&target, &parked).is_ok() {
                                let _ignored: std::io::Result<()> =
                                    std::os::unix::fs::symlink(&outside, &target);
                            }
                            k += 1;
                        }
                    })
                    .unwrap();
                let _result = snp_os::fs::remove_tree_within(&victim, &root);
                stop.store(true, Ordering::Relaxed);
                swapper.join().unwrap();
            });
            for i in 0..50 {
                assert!(
                    outside.join(format!("precious{i}")).exists(),
                    "round {round}: outside file {i} deleted"
                );
            }
            // Whatever the race left behind is still removable and stays inside root.
            let _again = snp_os::fs::remove_tree_within(&victim, &root);
            assert!(outside.exists());
        }
    }

    /// `decide_color` matrix: always/never/auto × tty/non-tty × TERM (stderr on a real pty vs a pipe).
    #[cfg(unix)]
    #[test]
    fn decide_color_matrix() {
        use rustix::fs::{Mode, OFlags};
        use rustix::pty::OpenptFlags;

        let run = |stderr: Stdio| -> String {
            let d = tempfile::tempdir().unwrap();
            let out = d.path().join("out");
            let mut child = driver("color", &[("SNP_OS_OUT", &out)], stderr);
            assert!(wait_exit(&mut child, 60).success());
            std::fs::read_to_string(out).unwrap()
        };
        // Non-tty stderr: auto never colours.
        let piped = run(Stdio::piped());
        assert_eq!(
            piped,
            "Auto/xterm-256color=false\nAuto/dumb=false\nAuto/unset=false\nAlways/dumb=true\nNever/xterm=false\n"
        );
        // A terminal on stderr: auto colours unless TERM=dumb (an unset TERM is not dumb).
        let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        rustix::pty::grantpt(&master).unwrap();
        rustix::pty::unlockpt(&master).unwrap();
        let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
        let slave = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY,
            Mode::empty(),
        )
        .unwrap();
        let tty = run(Stdio::from(slave));
        assert_eq!(
            tty,
            "Auto/xterm-256color=true\nAuto/dumb=false\nAuto/unset=true\nAlways/dumb=true\nNever/xterm=false\n"
        );
        drop(master);
    }
}
