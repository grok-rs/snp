//! The ARCHITECTURE §10.4 signal matrix as POSIX process tests, plus the Windows console `CTRL_C` test (ADR-065):
//! a driver process (this test binary re-executed) installs snp's handler and runs an Inherit-group hook, an isolated hook group and a Critical
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
        "win_ctrlc" => win_ctrlc(),
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
    // Exits 0 by itself 2 s after the grace would end: a kill within the grace window would show as -9.
    let grace_ms: u64 = driver_param("SNP_KILL_GRACE")
        .and_then(|g| g.parse::<u64>().ok())
        .map_or(2_000, |s| s * 1_000);
    let critical_timeout = (grace_ms + 2_000).to_string();
    let critical: Vec<OsString> = [
        probe.as_str(),
        "trap",
        &file("critical"),
        "300",
        &critical_timeout,
    ]
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

/// The driver side of the Windows `CTRL_C` test: an Inherit and an Isolated (`CREATE_NEW_PROCESS_GROUP`) hook, each
/// a `snp-probe trap` lingering 5 s after an event and exiting 0 by itself after 60 s.
#[cfg(windows)]
fn win_ctrlc() {
    use std::ffi::OsString;

    use snp_core::cancel::CancelToken;
    use snp_proc::signals;
    use snp_proc::spawn::{self, Group, Role, Spec, Stdin};

    let dir = PathBuf::from(driver_param("SNP_PROC_TEST_DIR").unwrap());
    let cancel = CancelToken::new();
    let _guard = signals::install(cancel.clone(), false).unwrap();
    let probe = probe().into_os_string();
    let argv = |name: &str| -> Vec<OsString> {
        vec![
            probe.clone(),
            OsString::from("trap"),
            dir.join(name).into_os_string(),
            OsString::from("5000"),
            OsString::from("60000"),
        ]
    };
    let (inherit, isolated) = (argv("inherit"), argv("iso"));
    let env = crate::common::env();
    let budget = crate::common::budget();
    let spec = |argv, group| Spec {
        argv,
        env: &env,
        cwd: &dir,
        stdin: Stdin::EmptyPipe,
        group,
        role: Role::Hook,
        deadline: None,
        cancel: &cancel,
    };
    let t0 = std::time::Instant::now();
    std::thread::scope(|s| {
        let h_inherit = std::thread::Builder::new()
            .spawn_scoped(s, || {
                spawn::merged(&spec(&inherit, Group::Inherit), false, &budget).unwrap()
            })
            .unwrap();
        let h_iso = std::thread::Builder::new()
            .spawn_scoped(s, || {
                spawn::merged(&spec(&isolated, Group::Isolated), false, &budget).unwrap()
            })
            .unwrap();
        for n in ["inherit", "iso"] {
            assert!(
                wait_for(&dir.join(format!("{n}.ready")), 30),
                "{n} not ready"
            );
        }
        report("ready", 1);
        let i = h_inherit.join().unwrap();
        let g = h_iso.join().unwrap();
        report("inherit_status", i.status.0);
        report("inherit_interrupted", i.interrupted);
        report("iso_status", g.status.0);
        report("iso_interrupted", g.interrupted);
    });
    report("took_ms", t0.elapsed().as_millis());
    report("reason", format!("{:?}", cancel.reason()));
    report("done", 1);
}

#[cfg(not(windows))]
fn win_ctrlc() {}

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
        start_with_grace(scenario, "1")
    }

    /// Starts a driver with `SNP_KILL_GRACE=grace` (whole seconds).
    pub(super) fn start_with_grace(scenario: &str, grace: &str) -> Driver {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("results");
        let mut cmd = driver_command("catalog::driver_entry", scenario);
        cmd.env("SNP_KILL_GRACE", grace)
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
/// the envelope's). Runs with the default 2 s grace: the Critical child is not signalled across the whole window.
#[cfg(unix)]
#[test]
fn signal_matrix_sent_sigint() {
    use std::os::unix::process::ExitStatusExt;
    let mut d = posix::start_with_grace("matrix", "2");
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

/// Windows (ARCH §10.4, WBS WP-15): a console `CTRL_C` reaches the Inherit hook (the console delivers it; snp only
/// cancels), never the Isolated (`CREATE_NEW_PROCESS_GROUP`) one, and both hooks' Job Objects are terminated after
/// the grace (`SNP_KILL_GRACE=1`), long before the probes' own 60 s timeout. The driver runs on a console of its own
/// (`CREATE_NEW_CONSOLE`); a PowerShell helper attaches to it and calls `GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)`
/// through `Add-Type` (no unsafe Rust anywhere).
#[cfg(windows)]
#[test]
fn windows_ctrl_c_reaches_inherit_children_only() {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    use std::time::Instant;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("results");
    // The driver runs under a PowerShell launcher on the new console that first clears the inherited "ignore
    // CTRL_C" flag: a process created with CREATE_NEW_PROCESS_GROUP (CI runners and test runners start steps that
    // way) implies `SetConsoleCtrlHandler(NULL, TRUE)`, which every descendant inherits, so the console's CTRL_C
    // would never reach the driver (a terminal user's snp starts with the flag clear).
    let driver = driver_command("catalog::driver_entry", "win_ctrlc");
    let args: Vec<String> = driver
        .get_args()
        .map(|a| format!("'{}'", a.to_string_lossy().replace('\'', "''")))
        .collect();
    let launcher = format!(
        r#"Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SnpCtrl {{
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);
}}
'@
[void][SnpCtrl]::SetConsoleCtrlHandler([IntPtr]::Zero, $false)
& '{exe}' {args}
exit $LASTEXITCODE
"#,
        exe = driver.get_program().to_string_lossy().replace('\'', "''"),
        args = args.join(" ")
    );
    let mut child = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(encoded_command(&launcher))
        .env(crate::common::DRIVER_VAR, "win_ctrlc")
        .env("SNP_KILL_GRACE", "1")
        .env("SNP_PROC_TEST_DIR", dir.path())
        .env("SNP_PROC_TEST_OUT", &out)
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    while result(&results(&out), "ready").is_none() {
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "driver never became ready: {:?}",
            results(&out)
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "driver exited early: {:?}",
            results(&out)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SnpConsole {{
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GenerateConsoleCtrlEvent(uint ev, uint group);
}}
'@
[void][SnpConsole]::FreeConsole()
if (-not [SnpConsole]::AttachConsole({pid})) {{ exit 2 }}
[void][SnpConsole]::SetConsoleCtrlHandler([IntPtr]::Zero, $true)
if (-not [SnpConsole]::GenerateConsoleCtrlEvent(0, 0)) {{ exit 3 }}
Start-Sleep -Milliseconds 300
[void][SnpConsole]::FreeConsole()
exit 0
"#,
        pid = child.id()
    );
    let st = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(encoded_command(&script))
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "the CTRL_C helper failed: {st:?}");
    let t1 = Instant::now();
    let st = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(
            t1.elapsed() < Duration::from_secs(60),
            "driver hung: {:?}",
            results(&out)
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let r = results(&out);
    assert!(st.success(), "{st:?} {r:?}");
    assert_eq!(
        trapped(&dir.path().join("inherit")),
        ["INT"],
        "the console delivers CTRL_C to Inherit children: {r:?}"
    );
    assert!(
        trapped(&dir.path().join("iso")).is_empty(),
        "CREATE_NEW_PROCESS_GROUP children never see CTRL_C: {r:?}"
    );
    assert_eq!(result(&r, "reason"), Some("Some(Interrupt)"), "{r:?}");
    // Both were terminated with their Job Object after the 1 s grace: neither lingered 5 s nor reached its timeout.
    assert_eq!(result(&r, "inherit_interrupted"), Some("true"), "{r:?}");
    assert_eq!(result(&r, "iso_interrupted"), Some("true"), "{r:?}");
    assert_ne!(result(&r, "iso_status"), Some("0"), "{r:?}");
    assert_ne!(result(&r, "inherit_status"), Some("130"), "{r:?}");
    assert!(t1.elapsed() < Duration::from_secs(15), "{r:?}");
}

/// PowerShell's `-EncodedCommand`: base64 of the UTF-16LE script.
fn encoded_command(script: &str) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::new();
    let (full, rest) = bytes.as_chunks::<3>();
    let mut last = [0_u8; 3];
    for (dst, src) in last.iter_mut().zip(rest) {
        *dst = *src;
    }
    let groups = full
        .iter()
        .map(|c| (*c, 3))
        .chain((!rest.is_empty()).then_some((last, rest.len())));
    for (b, len) in groups {
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= len {
                out.push(char::from(
                    B64[usize::try_from((n >> (18 - 6 * i)) & 63).unwrap()],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The base64 encoder of the Windows test (checked on every OS).
#[test]
fn encoded_command_base64() {
    // "ab" in UTF-16LE = 61 00 62 00; "abc" = 61 00 62 00 63 00.
    assert_eq!(encoded_command("ab"), "YQBiAA==");
    assert_eq!(encoded_command("a"), "YQA=");
    assert_eq!(encoded_command("abc"), "YQBiAGMA");
}
