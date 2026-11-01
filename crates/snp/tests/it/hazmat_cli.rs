//! Integration tests: the public `hazmat` command, path C (X13-HZ-01…05), and the reserved `__hazmat` entry
//! (D-MISC-02).
//!
//! Owner: WP-33.

use crate::cli::{assert_code, compat, empty_repo, snp, tenv, text};

#[test]
fn x13_hz_01_no_tool_is_a_usage_error() {
    let env = tenv();
    let out = compat(&env, env.root(), &["hazmat"]);
    assert_code(&out, 2);
    let err = text(&out.stderr);
    assert!(
        err.starts_with("usage: pre-commit hazmat [-h] [--color {auto,always,never}]\n"),
        "{err}"
    );
    assert!(
        err.ends_with("pre-commit hazmat: error: the following arguments are required: tool\n"),
        "{err}"
    );
}

#[test]
fn x13_hz_02_missing_executable() {
    let env = tenv();
    for args in [
        &["hazmat", "n1", "nonexistent-exe", "--"][..],
        &["hazmat", "ignore-exit-code", "nonexistent-exe"][..],
    ] {
        let out = compat(&env, env.root(), args);
        assert_code(&out, 3);
        assert!(
            text(&out.stdout).starts_with("An unexpected error has occurred: ExecutableNotFoundError: Executable `nonexistent-exe` not found\nCheck the log at "),
            "{}",
            text(&out.stdout)
        );
    }
}

#[test]
fn x13_hz_03_empty_command() {
    let env = tenv();
    for args in [
        &["hazmat", "ignore-exit-code"][..],
        &["hazmat", "n1", "--"][..],
    ] {
        let out = compat(&env, env.root(), args);
        // An exit-3 crash through the envelope (never a panic, never 101), byte parity with the oracle's
        // `normalize_cmd(())` IndexError (docs/research/README.md errata: the 13 X.4 snp column is not used).
        assert_code(&out, 3);
        assert!(
            text(&out.stdout).starts_with(
                "An unexpected error has occurred: IndexError: tuple index out of range\nCheck the log at "
            ),
            "{}",
            text(&out.stdout)
        );
    }
}

#[test]
fn x13_hz_04_missing_cd_directory() {
    let env = tenv();
    let r = empty_repo(&env, "r");
    let out = compat(&env, &r, &["hazmat", "cd", "nodir", "true", "--"]);
    assert_code(&out, 3);
    #[cfg(unix)]
    assert!(
        text(&out.stdout).starts_with("An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: 'nodir'\n"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn x13_hz_05_no_double_dash() {
    let env = tenv();
    let out = compat(&env, env.root(), &["hazmat", "n1", "echo"]);
    assert_code(&out, 1);
    assert_eq!(text(&out.stderr), "hazmat entry must end with `--`\n");
    assert_eq!(text(&out.stdout), "");
}

/// D-MISC-02: `argv[1] == "__hazmat"` is the in-hook entry in both brands (prog `hazmat.py`).
#[test]
fn reserved_in_hook_entry() {
    let env = tenv();
    for out in [
        compat(&env, env.root(), &["__hazmat", "n1", "echo"]),
        snp(&env, env.root(), &["__hazmat", "n1", "echo"]),
    ] {
        assert_code(&out, 1);
        assert_eq!(text(&out.stderr), "hazmat entry must end with `--`\n");
    }
    let out = compat(&env, env.root(), &["__hazmat", "bogus"]);
    assert_code(&out, 2);
    assert!(
        text(&out.stderr).contains("hazmat.py: error: argument tool: invalid choice: 'bogus'"),
        "{}",
        text(&out.stderr)
    );
}

/// A signal inherited as `SIG_IGN` (`nohup`) stays ignored: snp neither cancels nor dies, and the child (which
/// inherits `SIG_IGN`) finishes, as under pre-commit (CPython keeps an inherited `SIG_IGN`).
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn inherited_sig_ign_is_kept() {
    use std::io::Read as _;
    let env = tenv();
    for (sig, name) in [("HUP", "SIGHUP"), ("INT", "SIGINT"), ("TERM", "SIGTERM")] {
        let mut cmd = std::process::Command::new("/bin/sh");
        env.env().apply_to(&mut cmd);
        let script = format!(
            "trap '' {sig}; exec \"$0\" hazmat ignore-exit-code sh -c 'sleep 1; echo child-finished'"
        );
        // Its own process group, signalled as a whole (a hangup reaches the whole foreground group).
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd
            .arg("-c")
            .arg(script)
            .arg(env!("CARGO_BIN_EXE_snp"))
            .current_dir(env.root())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let st = std::process::Command::new("kill")
            .arg(format!("-{sig}"))
            .arg("--")
            .arg(format!("-{}", child.id()))
            .status()
            .unwrap();
        assert!(st.success());
        let mut out = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        let mut err = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(0), "{name}: {out}{err}");
        assert_eq!(out, "child-finished\n", "{name}: {err}");
    }
}
