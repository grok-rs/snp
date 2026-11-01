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
        // An exit-3 crash through the envelope (never a panic, never 101).
        assert_code(&out, 3);
        assert!(
            text(&out.stdout).starts_with("An unexpected error has occurred: "),
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
