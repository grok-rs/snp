//! Integration tests: the error envelope, crash log, exit codes and the command crash rows (T10-ERR-01…05,
//! X13-CFG-14, X13-CMD-01…12, X13-EH-01…05, X13-SM-08).
//!
//! Owner: WP-33.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use snp_testkit::TestEnv;

use crate::cli::{
    LOCAL_CONFIG, assert_code, compat, git, repo_with, slashed, snp, store, tenv, text,
};

/// Whether permission bits are enforced (not root).
#[cfg(unix)]
fn perms_enforced(dir: &Path) -> bool {
    let probe = dir.join(".perm-probe");
    std::fs::create_dir_all(&probe).unwrap();
    set_mode(&probe, 0o500);
    let enforced = std::fs::write(probe.join("x"), b"").is_err();
    set_mode(&probe, 0o700);
    drop(std::fs::remove_dir_all(&probe));
    enforced
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// The crash log of the compat brand in snp's store.
fn compat_log(env: &TestEnv) -> std::path::PathBuf {
    store(env).join("pre-commit.log")
}

/// A repository whose `.git/hooks/pre-commit` is a directory: `install` crashes with `IsADirectoryError` (exit 3).
fn hook_is_a_directory(env: &TestEnv) -> std::path::PathBuf {
    let r = repo_with(env, "r", LOCAL_CONFIG);
    std::fs::create_dir_all(r.join(".git/hooks/pre-commit")).unwrap();
    r
}

#[test]
fn t10_err_01_banners_and_exit_codes() {
    let env = tenv();
    // FatalError → exit 1
    let out = compat(&env, env.root(), &["install"]);
    assert_code(&out, 1);
    assert!(text(&out.stdout).starts_with("An error has occurred: FatalError: "));
    // any other exception → exit 3
    let r = hook_is_a_directory(&env);
    let out = compat(&env, &r, &["install"]);
    assert_code(&out, 3);
    assert!(text(&out.stdout).starts_with("An unexpected error has occurred: IsADirectoryError: "));
    // success → nothing
    let out = compat(&env, env.root(), &["sample-config"]);
    assert_code(&out, 0);
}

#[test]
fn t10_err_02_line_two_and_log_skeleton() {
    let env = tenv();
    let out = compat(&env, env.root(), &["install"]);
    let line1 = "An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?";
    assert_eq!(
        text(&out.stdout),
        format!("{line1}\nCheck the log at {}\n", compat_log(&env).display())
    );
    let log = std::fs::read_to_string(compat_log(&env)).unwrap();
    assert!(
        log.starts_with(
            "### version information\n\n```\npre-commit version: 4.6.2\ngit --version: "
        ),
        "{log}"
    );
    assert!(
        log.contains("\nsys.version:\n    snp 2.0.0 (rustc "),
        "{log}"
    );
    assert!(log.contains("\nsys.executable: "), "{log}");
    assert!(
        log.contains(&format!(
            "```\n\n### error information\n\n```\n{line1}\n```\n\n```\n"
        )),
        "{log}"
    );
    assert!(log.ends_with("```\n"), "{log}");
    // snp brand: snp.log, `snp version:`
    let out = snp(&env, env.root(), &["install"]);
    assert!(text(&out.stdout).ends_with(&format!(
        "Check the log at {}\n",
        store(&env).join("snp.log").display()
    )));
    let log = std::fs::read_to_string(store(&env).join("snp.log")).unwrap();
    assert!(log.contains("\nsnp version: 2.0.0\n"), "{log}");
    // overwritten each time, never an environment dump
    assert!(!log.contains("SNP_HOME") && !log.contains("PATH="));
}

#[cfg(unix)]
#[test]
fn t10_err_03_non_utf8_texts_exit_cleanly() {
    use std::os::unix::ffi::OsStrExt;
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    // A non-UTF-8 hooks path in the IsADirectoryError text.
    let weird = std::ffi::OsStr::from_bytes(b"cfg-\xff\xfe.yaml");
    std::fs::create_dir_all(r.join(".git/hooks/pre-push")).unwrap();
    let out = env
        .snp_as_pre_commit(&r)
        .arg("install")
        .arg("-c")
        .arg(weird)
        .arg("-t")
        .arg("pre-push")
        .output();
    assert_code(&out, 3);
    assert!(out.stdout.starts_with(b"An unexpected error has occurred: IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-push'"));
}

#[test]
fn t10_err_04_real_process_last_two_lines() {
    let env = tenv();
    let r = hook_is_a_directory(&env);
    let out = compat(&env, &r, &["install"]);
    assert_code(&out, 3);
    let o = text(&out.stdout);
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.len(), 2, "{o}");
    assert_eq!(lines[0], is_a_directory_line());
    assert_eq!(
        lines[1],
        format!("Check the log at {}", compat_log(&env).display())
    );
}

#[cfg(unix)]
#[test]
fn t10_err_05_read_only_store_prints_the_body() {
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let r = hook_is_a_directory(&env);
    let st = store(&env);
    set_mode(&st, 0o500);
    let out = compat(&env, &r, &["install"]);
    set_mode(&st, 0o700);
    assert_code(&out, 3);
    let o = text(&out.stdout);
    assert!(o.starts_with(&format!(
        "An unexpected error has occurred: IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit'\nFailed to write to log at {}\n### version information\n",
        compat_log(&env).display()
    )), "{o}");
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn x13_cfg_14_side_loads_fall_back() {
    let env = tenv();
    // A config pre-commit's loader crashes on (non-string root key): install falls back / warns, never exit 3.
    let r = repo_with(&env, "r", "repos: []\n1: x\n");
    let out = compat(&env, &r, &["install"]);
    assert_code(&out, 0);
    assert!(slashed(&text(&out.stdout)).contains("pre-commit installed at .git/hooks/pre-commit"));
    // gc treats a tracked config that fails to load as dead
    std::fs::write(r.join(".pre-commit-config.yaml"), "repos: [\n").unwrap();
    assert_eq!(
        text(&compat(&env, &r, &["gc"]).stdout),
        "0 repo(s) removed.\n"
    );
}

#[test]
fn x13_cmd_01_hook_path_is_a_directory() {
    let env = tenv();
    let r = hook_is_a_directory(&env);
    let out = compat(&env, &r, &["install"]);
    assert_code(&out, 3);
    assert!(text(&out.stdout).starts_with(&format!("{}\n", is_a_directory_line())));
}

/// pre-commit's crash line for a directory at the hook path: CPython's `open()` of a directory is `EISDIR` on POSIX
/// and `EACCES` on Windows (whose path pre-commit joins with `\`).
fn is_a_directory_line() -> &'static str {
    if cfg!(windows) {
        r"An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: '.git\\hooks\\pre-commit'"
    } else {
        "An unexpected error has occurred: IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit'"
    }
}

#[cfg(unix)]
#[test]
fn x13_cmd_02_hooks_dir_unwritable() {
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let hooks = r.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    set_mode(&hooks, 0o500);
    let out = compat(&env, &r, &["install"]);
    set_mode(&hooks, 0o700);
    assert_code(&out, 3);
    assert!(
        text(&out.stdout).starts_with(
            "An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: "
        ),
        "{}",
        text(&out.stdout)
    );
}

#[cfg(unix)]
#[test]
fn x13_cmd_03_overwrite_with_legacy_directory() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    std::fs::create_dir_all(r.join(".git/hooks/pre-commit.legacy")).unwrap();
    let out = compat(&env, &r, &["install", "-f"]);
    assert_code(&out, 3);
    let o = text(&out.stdout);
    #[cfg(target_os = "linux")]
    assert!(o.starts_with("An unexpected error has occurred: IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit.legacy'\n"), "{o}");
    #[cfg(not(target_os = "linux"))]
    assert!(o.starts_with("An unexpected error has occurred: "), "{o}");
}

#[cfg(unix)]
#[test]
fn x13_cmd_04_uninstall_remove_fails() {
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    assert_code(&compat(&env, &r, &["install"]), 0);
    let hooks = r.join(".git/hooks");
    set_mode(&hooks, 0o500);
    let out = compat(&env, &r, &["uninstall"]);
    set_mode(&hooks, 0o700);
    assert_code(&out, 3);
    assert!(text(&out.stdout).starts_with("An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: '.git/hooks/pre-commit'"));
}

#[test]
fn x13_cmd_05_could_not_find_rev_lines() {
    let env = tenv();
    let up = env
        .git_repo("up")
        .file(
            ".pre-commit-hooks.yaml",
            b"-   id: foo\n    name: foo\n    entry: echo\n    language: system\n",
        )
        .stage_all()
        .commit("1")
        .build();
    let old = git(&env, &up, &["rev-parse", "HEAD"]).1.trim().to_owned();
    env.git_repo("up")
        .file("x", b"x")
        .stage_all()
        .commit("2")
        .build();
    let cfg = format!(
        "repos:\n-   repo: {}\n    rev: {old}\n    hooks:\n    -   id: foo\n        rev: extra\n        args: [--rev]\n",
        up.display()
    );
    let r = repo_with(&env, "r", &cfg);
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 3);
    assert!(
        text(&out.stdout).contains(
            "An unexpected error has occurred: AssertionError: could not find rev lines\n"
        ),
        "{}",
        text(&out.stdout)
    );
    // Oracle (pc462): `_original_lines` writes `yaml_dump(yaml_load(original))` before raising.
    assert_eq!(
        std::fs::read_to_string(r.join(".pre-commit-config.yaml")).unwrap(),
        format!(
            "repos:\n-   repo: {}\n    rev: {old}\n    hooks:\n    -   id: foo\n        rev: extra\n        args:\n        - --rev\n",
            up.display()
        )
    );
}

#[test]
fn x13_cmd_06_autoupdate_git_failure_is_a_repo_line() {
    let env = tenv();
    let missing = env.root().join("gone");
    let r = repo_with(
        &env,
        "r",
        &format!(
            "repos:\n-   repo: {}\n    rev: v1.0.0\n    hooks:\n    -   id: foo\n",
            missing.display()
        ),
    );
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 1);
    let o = text(&out.stdout);
    assert!(
        o.starts_with(&format!("[{}] command: (", missing.display())),
        "{o}"
    );
    assert!(!o.contains("Check the log"), "{o}");
}

#[test]
fn x13_cmd_07_missing_repo_dir_is_tolerated() {
    let env = tenv();
    let st = store(&env);
    std::fs::create_dir_all(st.join("repos-v1/0123456789abcdef0123456789abcdef")).unwrap();
    for _ in 0..2 {
        let out = compat(&env, env.root(), &["gc"]);
        assert_code(&out, 0);
        assert!(text(&out.stdout).ends_with("repo(s) removed.\n"));
    }
}

#[cfg(unix)]
#[test]
fn x13_cmd_08_gc_removal_failure() {
    use std::sync::Arc;
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let s = snp_store::Store::new(snp_store::StoreLocation {
        path: store(&env),
        source: snp_store::StoreSource::SnpHome,
    });
    s.ensure_created(
        snp_core::brand::Brand::Snp,
        &snp_core::output::CollectWarnings::new(),
    )
    .unwrap();
    let id = snp_store::RepoId::Url(Arc::from("https://example.com/x"));
    let key = snp_store::Store::remote_key(&id, "v1.0.0", &[]);
    s.materialize(
        &key,
        snp_store::RepoMeta {
            kind: snp_store::RepoKind::Remote {
                id,
                rev: Arc::from("v1.0.0"),
            },
            deps: Arc::from(Vec::new()),
        },
        &mut |d: &Path| {
            std::fs::create_dir_all(d.join(".git")).unwrap();
            Ok(())
        },
        &mut || {},
        &snp_core::cancel::CancelToken::new(),
    )
    .unwrap();
    let repos = store(&env).join("repos-v1");
    set_mode(&repos, 0o500);
    let out = compat(&env, env.root(), &["gc"]);
    set_mode(&repos, 0o700);
    assert_code(&out, 3);
    assert!(
        text(&out.stdout).starts_with("An unexpected error has occurred: PermissionError: "),
        "{}",
        text(&out.stdout)
    );
}

#[cfg(unix)]
#[test]
fn x13_cmd_09_crashing_tracked_config_is_dead() {
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    std::fs::create_dir_all(store(&env).join("configs-v1")).unwrap();
    assert_code(&compat(&env, &r, &["install"]), 0); // tracks the config
    let cfg = r.join(".pre-commit-config.yaml");
    set_mode(&cfg, 0o000);
    let out = compat(&env, &r, &["gc"]);
    set_mode(&cfg, 0o644);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "0 repo(s) removed.\n");
    assert!(
        std::fs::read_dir(store(&env).join("configs-v1"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn x13_cmd_10_clean_removal_failure() {
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let deep = store(&env).join("repos-v1/0123456789abcdef0123456789abcdef/src");
    std::fs::create_dir_all(deep.join("d")).unwrap();
    set_mode(&deep, 0o500);
    let out = compat(&env, env.root(), &["clean"]);
    if deep.exists() {
        set_mode(&deep, 0o700);
    }
    // pre-commit's rmtree chmods and retries; a removal that still fails is the OSError (exit 3) — or, when the
    // chmod retry succeeds, the usual line.
    if out.code == 0 {
        assert!(text(&out.stdout).starts_with("Cleaned "));
    } else {
        assert_code(&out, 3);
        assert!(text(&out.stdout).starts_with("An unexpected error has occurred: "));
    }
}

#[cfg(unix)]
#[test]
fn x13_cmd_11_validate_unreadable_file() {
    let env = tenv();
    if !perms_enforced(env.root()) {
        return;
    }
    let f = env.root().join("c.yaml");
    std::fs::write(&f, LOCAL_CONFIG).unwrap();
    set_mode(&f, 0o000);
    let out = compat(&env, env.root(), &["validate-config", "c.yaml"]);
    let out2 = compat(&env, env.root(), &["validate-manifest", "c.yaml"]);
    set_mode(&f, 0o644);
    for o in [out, out2] {
        assert_code(&o, 3);
        assert!(text(&o.stdout).starts_with("An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: 'c.yaml'\n"), "{}", text(&o.stdout));
    }
}

#[test]
fn x13_cmd_12_try_repo_git_failure() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let out = compat(
        &env,
        &r,
        &["try-repo", env.root().join("nope").to_str().unwrap()],
    );
    assert_code(&out, 3);
    assert!(
        text(&out.stdout).starts_with("An unexpected error has occurred: CalledProcessError: ")
    );
}

/// The store path is a file: the handler never double-faults (`Failed to write to log at …`).
#[test]
fn x13_eh_01_store_unusable_inside_the_handler() {
    let env = tenv();
    let file = env.root().join("store-file");
    std::fs::write(&file, b"x").unwrap();
    let env = env.with_env("SNP_HOME", file.as_os_str());
    let out = compat(&env, env.root(), &["install"]);
    assert_code(&out, 1);
    let o = text(&out.stdout);
    assert!(
        o.starts_with("An error has occurred: FatalError: git failed."),
        "{o}"
    );
    assert!(o.contains("\nFailed to write to log at "), "{o}");
    assert!(o.contains("### version information"), "{o}");
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
}

#[test]
fn x13_eh_02_log_is_a_directory() {
    let env = tenv();
    std::fs::create_dir_all(compat_log(&env)).unwrap();
    let out = compat(&env, env.root(), &["install"]);
    assert_code(&out, 1);
    let o = text(&out.stdout);
    assert!(
        o.contains(&format!(
            "\nFailed to write to log at {}\n### version information\n",
            compat_log(&env).display()
        )),
        "{o}"
    );
    assert_eq!(text(&out.stderr), "");
}

/// stdout closed early: exit 120, nothing on stderr, no crash log.
#[test]
fn x13_eh_03_stdout_closed() {
    let env = tenv();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_snp"));
    env.env().apply_to(&mut cmd);
    let out = cmd
        .arg("sample-config")
        .current_dir(env.root())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(120));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert!(!store(&env).join("snp.log").exists());
}

/// `> /dev/full` (ENOSPC): the output is lost, so the command fails closed with 120 (pre-commit 4.6.2: CPython's
/// shutdown flush fails, exit 120 with `Exception ignored ... OSError: [Errno 28]`); snp prints nothing on stderr
/// (D-CRASH-24).
#[cfg(target_os = "linux")]
#[test]
fn x13_eh_03_stdout_full() {
    let env = tenv();
    for args in [
        &["--version"][..],
        &["--help"][..],
        &["sample-config"][..],
        &["clean"][..],
    ] {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_snp"));
        env.env().apply_to(&mut cmd);
        let out = cmd
            .args(args)
            .current_dir(env.root())
            .stdout(Stdio::from(full))
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(120), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
    }
}

/// stdout closed before the start (`>&-`): the Rust runtime reopens fd 1 on `/dev/null` before `main` (so no file
/// snp opens can become fd 1), and the command then behaves as under `> /dev/null`. pre-commit 4.6.2 exits 1 with an
/// import-time traceback instead (`sys.stdout` is `None`); recorded in D-CRASH-24.
#[cfg(unix)]
#[test]
fn x13_eh_03_stdout_closed_at_start_is_dev_null() {
    let env = tenv();
    let mut cmd = Command::new("/bin/sh");
    env.env().apply_to(&mut cmd);
    let out = cmd
        .arg("-c")
        .arg("exec \"$0\" sample-config >&-")
        .arg(env!("CARGO_BIN_EXE_snp"))
        .current_dir(env.root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn x13_eh_04_keyboard_interrupt() {
    let env = tenv();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_snp"));
    env.env().apply_to(&mut cmd);
    let child = cmd
        .args(["hazmat", "ignore-exit-code", "sleep", "5"])
        .current_dir(env.root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let pid = child.id().to_string();
    Command::new("kill").args(["-INT", &pid]).status().unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130));
    assert!(
        text(&out.stdout).starts_with("Interrupted (^C): KeyboardInterrupt: \nCheck the log at "),
        "{}",
        text(&out.stdout)
    );
}

/// SIGTERM: death by the re-raised signal (no banner).
#[cfg(unix)]
#[test]
fn sigterm_dies_by_the_signal() {
    use std::os::unix::process::ExitStatusExt;
    let env = tenv();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_snp"));
    env.env().apply_to(&mut cmd);
    let child = cmd
        .args(["hazmat", "ignore-exit-code", "sleep", "5"])
        .current_dir(env.root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.signal(), Some(15));
    assert_eq!(text(&out.stdout), "");
}

/// A panic injected through the test-only feature: exit 3, the `PanicException` banner, the crash log with the
/// location and the bug-report hint.
#[cfg(feature = "testing")]
#[test]
fn x13_eh_05_rust_panic() {
    let env = tenv();
    let out = env
        .snp(env.root())
        .arg("sample-config")
        .env("SNP_FAULT", "PrinterWrite")
        .output();
    assert_code(&out, 3);
    let o = text(&out.stdout);
    assert!(o.starts_with("An unexpected error has occurred: PanicException: snp fault injection: PrinterWrite\nCheck the log at "), "{o}");
    let log = std::fs::read_to_string(store(&env).join("snp.log")).unwrap();
    assert!(log.contains("panicked at "), "{log}");
    assert!(
        log.contains("https://github.com/grok-rs/snp/issues"),
        "{log}"
    );
}

#[cfg(not(feature = "testing"))]
#[test]
#[ignore = "needs --features testing"]
fn x13_eh_05_rust_panic() {}

#[cfg(feature = "testing")]
#[test]
fn panic_during_run_restores_the_stash_before_the_banner() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    std::fs::write(r.join("f"), b"1\n").unwrap();
    git(&env, &r, &["add", "f"]);
    std::fs::write(r.join("f"), b"2\n").unwrap();
    let out = env
        .snp(&r)
        .arg("run")
        .env("SNP_FAULT", "WorkerPartition")
        .output();
    assert_code(&out, 3);
    let o = text(&out.stdout);
    let restored = o.find("Restored changes from").unwrap();
    let banner = o
        .find("An unexpected error has occurred: PanicException")
        .unwrap();
    assert!(restored < banner, "{o}");
    assert_eq!(std::fs::read(r.join("f")).unwrap(), b"2\n");
}

/// SM-08: EPIPE prints none of Python's traceback noise.
#[test]
fn x13_sm_08_no_epipe_noise() {
    let env = tenv();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_snp"));
    env.env().apply_to(&mut cmd);
    let out = cmd
        .args(["--help"])
        .current_dir(env.root())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(120));
    assert!(!text(&out.stderr).contains("BrokenPipe"));
    assert_eq!(text(&out.stderr), "");
}

/// `snp migrate builtin|upstream` reads the config through the loader's capped read (ARCHITECTURE §11.4, D-SEC-11):
/// `/dev/zero` behind the config name, a FIFO and a file over the 16 MiB limit fail at once with exit 1 instead of
/// reading without bound (a `/dev/zero` config once drove `fs::read` into the OOM killer).
#[cfg(unix)]
#[test]
fn migrate_config_read_is_capped() {
    let env = tenv();
    let r = env.root().join("r");
    std::fs::create_dir_all(&r).unwrap();
    git(&env, &r, &["init", "-q"]);
    let cfg = r.join(".pre-commit-config.yaml");
    std::os::unix::fs::symlink("/dev/zero", &cfg).unwrap();
    for sub in ["builtin", "upstream"] {
        let out = snp(&env, &r, &["migrate", sub]);
        assert_code(&out, 1);
        assert!(
            text(&out.stdout).starts_with(
                "An error has occurred: InvalidConfigError: \n=====> .pre-commit-config.yaml is not a file\nCheck the log at "
            ),
            "{sub}: {}",
            text(&out.stdout)
        );
    }
    std::fs::remove_file(&cfg).unwrap();
    let st = Command::new("mkfifo").arg(&cfg).status().unwrap();
    assert!(st.success());
    let out = snp(&env, &r, &["migrate", "builtin"]);
    assert_code(&out, 1);
    assert!(
        text(&out.stdout).contains("is not a file"),
        "{}",
        text(&out.stdout)
    );
    std::fs::remove_file(&cfg).unwrap();
    // Sparse: no 16 MiB is ever written.
    let f = std::fs::File::create(&cfg).unwrap();
    f.set_len((16 << 20) + 1).unwrap();
    drop(f);
    let out = snp(&env, &r, &["migrate", "builtin"]);
    assert_code(&out, 1);
    assert!(
        text(&out.stdout).contains("bytes long, more than the limit of 16777216 bytes"),
        "{}",
        text(&out.stdout)
    );
}
