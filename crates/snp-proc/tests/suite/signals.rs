//! The ARCHITECTURE §10.4 signal matrix as POSIX process tests (ADR-065): a driver process (this test binary
//! re-executed) installs snp's handler and runs an Inherit-group hook, an isolated hook group and a Critical
//! command, each a `snp-probe trap` that records the signals it receives; the test then signals the driver — with
//! kill(2) (origin Sent) or through a pseudo-terminal it controls (origin Kernel: ^C, hangup) — and checks who was
//! signalled, that kernel-generated signals are never re-sent, that isolated groups get the signal by `-pgid`, that
//! SIGKILL follows the grace, that the Critical child is never signalled (it outlives the grace window) and the
//! driver's own exit status (130-style return for SIGINT, death by SIGTERM/SIGHUP after the restore).

use std::path::PathBuf;
use std::time::Duration;

use crate::common::{
    driver_command, driver_param, probe, report, result, results, trapped, wait_for,
};

/// Runs a driver scenario of this module; `false` when `scenario` is not one of them.
pub(crate) fn drive(scenario: &str) -> bool {
    match scenario {
        "matrix" => matrix(),
        "shield" => shield(),
        _ => return false,
    }
    true
}

/// The driver side of the matrix.
#[cfg(unix)]
fn matrix() {
    use std::ffi::OsString;

    use snp_core::cancel::CancelToken;
    use snp_proc::signals;
    use snp_proc::spawn::{self, Group, Role, Spec, Stdin};

    let dir = PathBuf::from(driver_param("SNP_PROC_TEST_DIR").unwrap());
    let cancel = CancelToken::new();
    let guard = signals::install(cancel.clone(), false).unwrap();
    let probe = probe().into_os_string().into_string().unwrap();
    let file = |n: &str| dir.join(n).into_os_string().into_string().unwrap();
    // Linger 5 s after a signal: alive until the grace's SIGKILL.
    let inherit: Vec<OsString> = [probe.as_str(), "trap", &file("inherit"), "5000", "20000"]
        .iter()
        .map(OsString::from)
        .collect();
    let script = format!(
        "'{probe}' trap '{}' 5000 20000 & exec '{probe}' trap '{}' 5000 20000",
        file("iso_bg"),
        file("iso")
    );
    let isolated: Vec<OsString> = ["/bin/sh", "-c", &script]
        .iter()
        .map(OsString::from)
        .collect();
    // Exits 0 by itself after 3 s: well beyond the 1 s grace, so a kill would show as -9.
    let critical: Vec<OsString> = [probe.as_str(), "trap", &file("critical"), "300", "3000"]
        .iter()
        .map(OsString::from)
        .collect();
    let env = crate::common::env();
    let cwd = dir.clone();
    let budget = crate::common::budget();
    let spec = |argv, group, role| Spec {
        argv,
        env: &env,
        cwd: &cwd,
        stdin: Stdin::EmptyPipe,
        group,
        role,
        deadline: None,
        cancel: &cancel,
    };
    std::thread::scope(|s| {
        let h_inherit = std::thread::Builder::new()
            .spawn_scoped(s, || {
                spawn::merged(&spec(&inherit, Group::Inherit, Role::Hook), false, &budget).unwrap()
            })
            .unwrap();
        let h_iso = std::thread::Builder::new()
            .spawn_scoped(s, || {
                spawn::merged(
                    &spec(&isolated, Group::Isolated, Role::Hook),
                    false,
                    &budget,
                )
                .unwrap()
            })
            .unwrap();
        let h_crit = std::thread::Builder::new()
            .spawn_scoped(s, || {
                spawn::capture(&spec(&critical, Group::Isolated, Role::Critical)).unwrap()
            })
            .unwrap();
        for n in ["inherit", "iso", "iso_bg", "critical"] {
            assert!(
                wait_for(&dir.join(format!("{n}.ready")), 20),
                "{n} not ready"
            );
        }
        report("ready", 1);
        let i = h_inherit.join().unwrap();
        let g = h_iso.join().unwrap();
        let c = h_crit.join().unwrap();
        report("inherit_status", i.status.0);
        report("inherit_interrupted", i.interrupted);
        report("iso_status", g.status.0);
        report("critical_status", c.status.0);
    });
    report("reason", format!("{:?}", cancel.reason()));
    report("done", 1);
    if let Some(code) = guard.die_by_signal_if_terminated() {
        report("survived", code);
    }
}

#[cfg(not(unix))]
fn matrix() {}

/// The driver side of the restore-shield test: while shielded, signals print one notice.
#[cfg(unix)]
fn shield() {
    use snp_core::cancel::CancelToken;
    use snp_proc::signals;

    let cancel = CancelToken::new();
    let guard = signals::install(cancel.clone(), false).unwrap();
    let shield = guard.shield();
    report("ready", 1);
    assert!(cancel.wait_timeout(Duration::from_secs(20)));
    std::thread::sleep(Duration::from_millis(600));
    drop(shield);
    report("reason", format!("{:?}", cancel.reason()));
    report("done", 1);
}

#[cfg(not(unix))]
fn shield() {}

#[cfg(unix)]
mod posix {
    use std::process::{Child, Stdio};
    use std::time::Instant;

    use rustix::process::{Pid, Signal};

    use super::*;

    /// A driver started for `scenario` with its result file and work directory.
    pub(super) struct Driver {
        pub(super) child: Child,
        pub(super) out: PathBuf,
        pub(super) dir: tempfile::TempDir,
    }

    /// Starts the `matrix`/`shield` driver (1 s kill grace) with stdout and stderr piped.
    pub(super) fn start(scenario: &str) -> Driver {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("results");
        let mut cmd = driver_command("catalog::driver_entry", scenario);
        cmd.env("SNP_KILL_GRACE", "1")
            .env("SNP_PROC_TEST_DIR", dir.path())
            .env("SNP_PROC_TEST_OUT", &out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();
        Driver { child, out, dir }
    }

    /// Waits for the driver's `ready` line.
    pub(super) fn wait_ready(d: &Driver) {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(30) {
            if result(&results(&d.out), "ready").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("driver never became ready: {:?}", results(&d.out));
    }

    /// kill(2) to the driver: origin Sent.
    pub(super) fn send(d: &Driver, sig: Signal) {
        let pid = Pid::from_raw(i32::try_from(d.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, sig).unwrap();
    }

    /// Waits for the driver to end (bounded) and returns its wait status.
    pub(super) fn finish(d: &mut Driver) -> std::process::ExitStatus {
        let t0 = Instant::now();
        loop {
            if let Some(st) = d.child.try_wait().unwrap() {
                return st;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(30),
                "driver hung: {:?}",
                results(&d.out)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The trap file of a child.
    pub(super) fn trap_of(d: &Driver, name: &str) -> Vec<String> {
        trapped(&d.dir.path().join(name))
    }
}

/// Common assertions for every matrix row: isolated groups got `sig` by `-pgid` (leader and background member),
/// SIGKILL followed the grace, the Critical child was never signalled and ended by itself.
#[cfg(unix)]
fn assert_isolated_and_critical(d: &posix::Driver, sig: &str) {
    let r = results(&d.out);
    assert_eq!(posix::trap_of(d, "iso"), [sig], "{r:?}");
    assert_eq!(
        posix::trap_of(d, "iso_bg"),
        [sig],
        "isolated groups are signalled by -pgid: {r:?}"
    );
    assert_eq!(
        result(&r, "iso_status"),
        Some("-9"),
        "SIGKILL after the grace: {r:?}"
    );
    assert!(
        posix::trap_of(d, "critical").is_empty(),
        "Critical children are never signalled: {r:?}"
    );
    assert_eq!(
        result(&r, "critical_status"),
        Some("0"),
        "Critical children outlive the grace: {r:?}"
    );
}

/// Sent SIGINT: forwarded to Inherit children by pid and to isolated groups by `-pgid`; snp returns (exit 130 is
/// the envelope's).
#[cfg(unix)]
#[test]
fn signal_matrix_sent_sigint() {
    use std::os::unix::process::ExitStatusExt;
    let mut d = posix::start("matrix");
    posix::wait_ready(&d);
    posix::send(&d, rustix::process::Signal::INT);
    let st = posix::finish(&mut d);
    let r = results(&d.out);
    assert_eq!(posix::trap_of(&d, "inherit"), ["INT"], "{r:?}");
    assert_eq!(result(&r, "inherit_status"), Some("-9"), "{r:?}");
    assert_eq!(result(&r, "inherit_interrupted"), Some("true"), "{r:?}");
    assert_isolated_and_critical(&d, "INT");
    assert_eq!(result(&r, "reason"), Some("Some(Interrupt)"));
    assert!(st.success(), "SIGINT is not re-raised: {st:?} {r:?}");
    assert_eq!(st.signal(), None);
}

/// Sent SIGTERM: forwarded as SIGTERM; after the restore the driver dies by SIGTERM (status 143).
#[cfg(unix)]
#[test]
fn signal_matrix_sent_sigterm() {
    use std::os::unix::process::ExitStatusExt;
    let mut d = posix::start("matrix");
    posix::wait_ready(&d);
    posix::send(&d, rustix::process::Signal::TERM);
    let st = posix::finish(&mut d);
    let r = results(&d.out);
    assert_eq!(posix::trap_of(&d, "inherit"), ["TERM"], "{r:?}");
    assert_isolated_and_critical(&d, "TERM");
    assert_eq!(result(&r, "reason"), Some("Some(Terminate)"));
    assert_eq!(result(&r, "survived"), None, "{r:?}");
    assert_eq!(st.signal(), Some(15), "{st:?}");
}

/// Sent SIGHUP: forwarded as SIGHUP; the driver dies by SIGHUP (status 129).
#[cfg(unix)]
#[test]
fn signal_matrix_sent_sighup() {
    use std::os::unix::process::ExitStatusExt;
    let mut d = posix::start("matrix");
    posix::wait_ready(&d);
    posix::send(&d, rustix::process::Signal::HUP);
    let st = posix::finish(&mut d);
    let r = results(&d.out);
    assert_eq!(posix::trap_of(&d, "inherit"), ["HUP"], "{r:?}");
    assert_isolated_and_critical(&d, "HUP");
    assert_eq!(result(&r, "reason"), Some("Some(Hangup)"));
    assert_eq!(st.signal(), Some(1), "{st:?}");
}

/// A pty whose slave becomes the driver's controlling terminal (`setsid -c`, Linux util-linux).
#[cfg(target_os = "linux")]
mod tty {
    use std::os::fd::OwnedFd;
    use std::process::Stdio;

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::OpenptFlags;

    use super::*;

    /// Starts the matrix driver as a session leader whose controlling terminal is a fresh pty; returns the
    /// driver and the master (writing `\x03` sends ^C, closing it hangs the terminal up). `None` without `setsid`.
    pub(super) fn start_on_pty() -> Option<(posix::Driver, OwnedFd)> {
        if !std::path::Path::new("/usr/bin/setsid").exists() {
            return None;
        }
        let master =
            rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
                .unwrap();
        rustix::pty::grantpt(&master).unwrap();
        rustix::pty::unlockpt(&master).unwrap();
        let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
        let open_slave = || -> OwnedFd {
            rustix::fs::open(
                name.as_c_str(),
                OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap()
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("results");
        let base = driver_command("catalog::driver_entry", "matrix");
        let mut cmd = std::process::Command::new("/usr/bin/setsid");
        cmd.arg("-c")
            .arg(base.get_program())
            .args(base.get_args())
            .env(crate::common::DRIVER_VAR, "matrix")
            .env("SNP_KILL_GRACE", "1")
            .env("SNP_PROC_TEST_DIR", dir.path())
            .env("SNP_PROC_TEST_OUT", &out)
            // The terminal is only the controlling tty (stdin, for TIOCSCTTY); output goes nowhere, so no clone of
            // the master is needed and dropping it hangs the terminal up.
            .stdin(Stdio::from(open_slave()))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().unwrap();
        drop(cmd);
        Some((posix::Driver { child, out, dir }, master))
    }
}

/// Kernel SIGINT (terminal ^C): the kernel delivered it to the Inherit child already — it is not re-sent (exactly
/// one INT recorded); the isolated group, which does not see the terminal, gets it by `-pgid`.
#[cfg(target_os = "linux")]
#[test]
fn signal_matrix_kernel_sigint() {
    let Some((mut d, master)) = tty::start_on_pty() else {
        return;
    };
    posix::wait_ready(&d);
    rustix::io::write(&master, b"\x03").unwrap();
    let st = posix::finish(&mut d);
    let r = results(&d.out);
    assert_eq!(
        posix::trap_of(&d, "inherit"),
        ["INT"],
        "kernel-origin INT is never re-sent: {r:?}"
    );
    assert_eq!(result(&r, "inherit_status"), Some("-9"), "{r:?}");
    assert_isolated_and_critical(&d, "INT");
    assert_eq!(result(&r, "reason"), Some("Some(Interrupt)"));
    assert!(st.success(), "{st:?} {r:?}");
    drop(master);
}

/// Kernel SIGHUP (terminal hangup): the controlling process gets it from the kernel; it is not re-sent to Inherit
/// children (which are killed after the grace), the isolated group gets HUP by `-pgid`, Critical nothing.
#[cfg(target_os = "linux")]
#[test]
fn signal_matrix_kernel_sighup() {
    use std::os::unix::process::ExitStatusExt;
    let Some((mut d, master)) = tty::start_on_pty() else {
        return;
    };
    posix::wait_ready(&d);
    drop(master);
    let st = posix::finish(&mut d);
    let r = results(&d.out);
    assert!(
        posix::trap_of(&d, "inherit").is_empty(),
        "kernel-origin HUP is never re-sent: {r:?}"
    );
    assert_eq!(result(&r, "inherit_status"), Some("-9"), "{r:?}");
    assert_isolated_and_critical(&d, "HUP");
    assert_eq!(result(&r, "reason"), Some("Some(Hangup)"));
    assert_eq!(st.signal(), Some(1), "{st:?}");
}

/// Further signals while a `RestoreShield` is alive print one stderr notice; the first one still cancels.
#[cfg(unix)]
#[test]
fn restore_shield_prints_one_notice() {
    use std::io::Read;
    let mut d = posix::start("shield");
    posix::wait_ready(&d);
    for _ in 0..3 {
        posix::send(&d, rustix::process::Signal::INT);
        std::thread::sleep(Duration::from_millis(100));
    }
    let st = posix::finish(&mut d);
    let mut err = String::new();
    d.child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    let notices = err
        .matches("snp: restoring stashed changes, please wait\n")
        .count();
    assert_eq!(notices, 1, "{err}");
    let r = results(&d.out);
    assert_eq!(result(&r, "reason"), Some("Some(Interrupt)"), "{r:?}");
    assert_eq!(result(&r, "done"), Some("1"));
    assert!(st.success());
}
