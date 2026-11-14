//! Tests of the `file-identify` binary (requires `--features cli`).
#![cfg(feature = "cli")]

use std::fs;
use std::process::{Command, Output};

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_file-identify"))
        .args(args)
        .output()
        .expect("run file-identify")
}

fn stdout(out: &Output) -> &str {
    std::str::from_utf8(&out.stdout).expect("utf-8")
}

#[test]
fn identifies_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("setup.py");
    fs::write(&path, "print('hello')\n").expect("write");
    let out = run(&[path.as_os_str()]);
    assert!(out.status.success());
    // Same output as Python's `identify-cli`: json.dumps(sorted(tags)).
    // identify reports every file as executable on Windows.
    let expected = if cfg!(windows) {
        "[\"executable\", \"file\", \"python\", \"text\"]\n"
    } else {
        "[\"file\", \"non-executable\", \"python\", \"text\"]\n"
    };
    assert_eq!(stdout(&out), expected);
}

#[test]
fn filename_only() {
    let out = run(&["--filename-only".as_ref(), "setup.py".as_ref()]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "[\"python\", \"text\"]\n");
    let tags: Vec<String> = serde_json::from_str(stdout(&out)).expect("json");
    assert_eq!(tags, ["python", "text"]);
}

#[test]
fn filename_only_unidentified() {
    let out = run(&["--filename-only".as_ref(), "x.unknown".as_ref()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
}

#[test]
fn file_not_found() {
    let out = run(&["x.unknown".as_ref()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "x.unknown does not exist.\n"
    );
}

#[test]
fn directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(&[dir.path().as_os_str()]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "[\"directory\"]\n");
}

#[cfg(unix)]
#[test]
fn executable_script() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("script");
    fs::write(&path, "#!/bin/bash\necho hello").expect("write");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    let out = run(&[path.as_os_str()]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&out),
        "[\"bash\", \"executable\", \"file\", \"shell\", \"text\"]\n"
    );
}

#[test]
fn help_and_version() {
    let out = run(&["--help".as_ref()]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("--filename-only"));
    let out = run(&["--version".as_ref()]);
    assert!(out.status.success());
    assert!(stdout(&out).starts_with("file-identify "));
}
