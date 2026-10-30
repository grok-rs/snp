//! `ProcRunner`: `cmd_output_b(check=False)` semantics on real processes (POSIX).

use std::ffi::OsString;
use std::path::Path;

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::env::Env;
use snp_lang::{CmdRunner, LangError, ProcRunner};

fn argv(xs: &[&str]) -> Vec<OsString> {
    xs.iter().map(OsString::from).collect()
}

fn runner() -> ProcRunner {
    ProcRunner {
        cancel: CancelToken::new(),
    }
}

#[cfg(unix)]
fn sys_env() -> Env {
    Env::from_pairs([("PATH", "/usr/bin:/bin")])
}

#[cfg(unix)]
#[test]
fn captures_status_and_both_streams_in_the_given_cwd_and_env() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([("PATH", "/usr/bin:/bin"), ("SNP_T", "v")]);
    let out = runner()
        .run(
            &argv(&["sh", "-c", "pwd; printf \"$SNP_T\" >&2; exit 3"]),
            &env,
            d.path(),
        )
        .unwrap();
    assert_eq!(out.status.0, 3);
    let pwd = String::from_utf8(out.stdout).unwrap();
    assert_eq!(Path::new(pwd.trim_end()), d.path().canonicalize().unwrap());
    assert_eq!(out.stderr, b"v");
}

#[cfg(unix)]
#[test]
fn stdin_is_empty() {
    let out = runner()
        .run(&argv(&["cat"]), &sys_env(), Path::new("/"))
        .unwrap();
    assert_eq!(out.status.0, 0);
    assert_eq!(out.stdout, b"");
}

#[test]
fn an_unknown_executable_is_a_status_1_result() {
    let env = Env::from_pairs([("PATH", "")]);
    let out = runner()
        .run(
            &argv(&["definitely-not-a-cmd-xyz", "--version"]),
            &env,
            Path::new("."),
        )
        .unwrap();
    assert_eq!(out.status.0, 1);
    assert_eq!(
        out.stdout,
        b"Executable `definitely-not-a-cmd-xyz` not found"
    );
    assert_eq!(out.stderr, b"");
}

#[cfg(unix)]
#[test]
fn shebang_resolution_uses_the_child_path_and_spawn_errors_are_results() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    // A script whose interpreter exists only on the child's PATH.
    let tools = d.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("myinterp"), b"#!/bin/sh\necho interp \"$@\"\n").unwrap();
    std::fs::set_permissions(
        tools.join("myinterp"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(d.path().join("script"), b"#!/usr/bin/env myinterp\n").unwrap();
    std::fs::set_permissions(
        d.path().join("script"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let env = Env::from_pairs([("PATH", format!("{}:/usr/bin:/bin", tools.display()))]);
    let out = runner()
        .run(&argv(&["./script", "a"]), &env, d.path())
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "interp ./script a\n"
    );

    // An executable file that is not a valid program: OSError text + "\n", status 1 (util.py:104-105).
    std::fs::write(d.path().join("junk"), b"\x00\x01\x02 not a program").unwrap();
    std::fs::set_permissions(
        d.path().join("junk"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let out = runner().run(&argv(&["./junk"]), &env, d.path()).unwrap();
    assert_eq!(out.status.0, 1);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("[Errno 8] Exec format error: "), "{text}");
    assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{text:?}");
}

#[test]
fn a_cancelled_token_is_interrupted() {
    let cancel = CancelToken::new();
    cancel.cancel(CancelReason::Interrupt);
    let r = ProcRunner { cancel }.run(
        &argv(&["true"]),
        &Env::from_pairs([("PATH", "")]),
        Path::new("."),
    );
    assert!(matches!(r, Err(LangError::Interrupted)));
}

#[cfg(unix)]
#[test]
fn cancelling_a_running_setup_command_interrupts_it() {
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let t = std::thread::Builder::new()
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            c2.cancel(CancelReason::Interrupt);
        })
        .unwrap();
    let start = std::time::Instant::now();
    let r = ProcRunner { cancel }.run(&argv(&["sleep", "30"]), &sys_env(), Path::new("/"));
    t.join().unwrap();
    assert!(matches!(r, Err(LangError::Interrupted)), "{r:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(20));
}
