//! W0 binary-skeleton checks (WBS WP-00 items 6 and the panic-hook acceptance): `--version` in both brands, panics
//! become a `PanicException` envelope (never exit 101), and a panic while the Printer lock is held (fault point
//! `PrinterWrite`, feature `testing`) does not hang.
//!
//! Owner: WP-33 (created by WP-00).

#[cfg(test)]
mod skeleton {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    const SNP: &str = env!("CARGO_BIN_EXE_snp");

    /// Runs `exe args` with a hermetic environment plus `extra`, killing it after 60 s.
    fn run(exe: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let home = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .env_clear()
            .env("HOME", home.path())
            .env("PRE_COMMIT_HOME", home.path().join("store"))
            .envs(extra.iter().copied())
            .current_dir(home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("`snp {args:?}` hung");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The binary under the compat name `pre-commit` (brand detection uses argv[0], ADR-023).
    fn compat_alias(dir: &Path) -> PathBuf {
        let alias = dir.join(if cfg!(windows) {
            "pre-commit.exe"
        } else {
            "pre-commit"
        });
        #[cfg(unix)]
        std::os::unix::fs::symlink(SNP, &alias).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(SNP, &alias).unwrap();
        alias
    }

    #[test]
    fn version_snp_brand() {
        for flag in ["--version", "-V"] {
            let out = run(Path::new(SNP), &[flag], &[]);
            assert_eq!(out.status.code(), Some(0));
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
                "snp 2.0.0\n"
            );
            assert_eq!(out.stderr, b"");
        }
    }

    #[test]
    fn version_compat_brand() {
        let dir = tempfile::tempdir().unwrap();
        let alias = compat_alias(dir.path());
        let out = run(&alias, &["--version"], &[]);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
            "pre-commit 4.6.2\n"
        );
    }

    /// Whatever a command does, a Rust panic never surfaces as exit 101 (ADR-015).
    #[test]
    fn never_exit_101() {
        for args in [&["run"][..], &["__hazmat", "cd", "x", "true"][..]] {
            let out = run(Path::new(SNP), args, &[]);
            assert_ne!(out.status.code(), Some(101), "{args:?}");
            assert!(out.status.code().is_some(), "{args:?} died by a signal");
        }
    }

    /// ADR-066 / S14: the fault point fires inside the Printer's state lock; the panic hook records and cancels
    /// without touching the Printer, the envelope then reuses the (poison-recovered) Printer.
    #[cfg(feature = "testing")]
    #[test]
    fn panic_while_holding_the_printer_lock_does_not_hang() {
        let out = run(
            Path::new(SNP),
            &["--version"],
            &[("SNP_FAULT", "PrinterWrite")],
        );
        assert_eq!(out.status.code(), Some(3));
        // CRLF folded: the panic path prints through the text-mode Printer (ADR-029)
        let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        let (line1, rest) = stdout.split_once('\n').unwrap_or_default();
        assert_eq!(
            line1,
            "An unexpected error has occurred: PanicException: snp fault injection: PrinterWrite"
        );
        assert!(rest.starts_with("Check the log at "), "{stdout}");
        assert!(rest.ends_with("snp.log\n"), "{stdout}");
    }
}
