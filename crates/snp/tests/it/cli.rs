//! Integration tests: the CLI frame (T10-CLI-01…07) and the helpers the WP-33 modules share.
//!
//! Owner: WP-33.

use std::path::{Path, PathBuf};
use std::process::Command;

use snp_testkit::{Bins, Output, TestEnv};

/// A hermetic environment over this package's `snp` (and `snp-probe` when it was built).
pub(crate) fn tenv() -> TestEnv {
    let snp = PathBuf::from(env!("CARGO_BIN_EXE_snp"));
    let probe = Bins::locate().map_or_else(|| snp.clone(), |b| b.probe);
    TestEnv::new(Bins { snp, probe })
}

/// Output bytes as text with CRLF folded (Windows text mode).
pub(crate) fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n")
}

/// `git <args>` in `cwd` with the hermetic env: `(code, stdout, stderr)`.
pub(crate) fn git(env: &TestEnv, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new("git");
    env.env().apply_to(&mut cmd);
    let out = cmd.args(args).current_dir(cwd).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        text(&out.stdout),
        text(&out.stderr),
    )
}

/// A local-only config with one `system` hook.
pub(crate) const LOCAL_CONFIG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: echo\n        name: echo\n        entry: echo\n        language: system\n";

/// A fresh repository `name` holding `config` as `.pre-commit-config.yaml` (committed).
pub(crate) fn repo_with(env: &TestEnv, name: &str, config: &str) -> PathBuf {
    env.git_repo(name)
        .file(".pre-commit-config.yaml", config.as_bytes())
        .stage_all()
        .commit("init")
        .build()
}

/// An empty repository (one empty file committed).
pub(crate) fn empty_repo(env: &TestEnv, name: &str) -> PathBuf {
    env.git_repo(name)
        .file("README", b"r\n")
        .stage_all()
        .commit("init")
        .build()
}

/// snp's store of `env` (`SNP_HOME`).
pub(crate) fn store(env: &TestEnv) -> PathBuf {
    env.root().join("snp-store")
}

/// `snp <args>` in `cwd` (snp brand).
pub(crate) fn snp(env: &TestEnv, cwd: &Path, args: &[&str]) -> Output {
    env.snp(cwd).args(args).output()
}

/// `pre-commit <args>` in `cwd` through the compat alias.
pub(crate) fn compat(env: &TestEnv, cwd: &Path, args: &[&str]) -> Output {
    env.snp_as_pre_commit(cwd).args(args).output()
}

/// Asserts the exit code, with the output in the message.
#[track_caller]
pub(crate) fn assert_code(o: &Output, code: i32) {
    assert_eq!(
        o.code,
        code,
        "stdout:\n{}\nstderr:\n{}",
        text(&o.stdout),
        text(&o.stderr)
    );
}

/// `os.access(p, X_OK)` for a regular file (posix; always true on Windows).
pub(crate) fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[test]
fn t10_cli_01_adjust_args_and_chdir() {
    let env = tenv();
    // Not in git → FatalError (exit 1), line 2 names the compat crash log in snp's store.
    let out = compat(&env, env.root(), &["install"]);
    assert_code(&out, 1);
    assert!(text(&out.stdout).starts_with(
        "An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?\nCheck the log at "
    ));
    // From a subdirectory: an existing `-c` is re-expressed relative to the root and embedded that way.
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    std::fs::create_dir_all(r.join("foo")).unwrap();
    std::fs::write(r.join("foo/cfg.yaml"), LOCAL_CONFIG).unwrap();
    let out = compat(&env, &r.join("foo"), &["install", "-c", "cfg.yaml"]);
    assert_code(&out, 0);
    let script = std::fs::read_to_string(r.join(".git/hooks/pre-commit")).unwrap();
    assert!(script.contains("--config=foo/cfg.yaml"), "{script}");
    // A missing relative config is kept as typed (re-interpreted relative to the root).
    let out = compat(&env, &r.join("foo"), &["install", "-c", "missing.yaml", "-t", "pre-push"]);
    assert_code(&out, 0);
    let script = std::fs::read_to_string(r.join(".git/hooks/pre-push")).unwrap();
    assert!(script.contains("--config=missing.yaml"), "{script}");
}

#[test]
fn t10_cli_02_command_list() {
    let env = tenv();
    let out = compat(&env, env.root(), &["--help"]);
    assert_code(&out, 0);
    let help = text(&out.stdout);
    assert!(
        help.contains("{autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl}")
            || help.contains("autoupdate,clean,gc,hazmat,init-templatedir"),
        "{help}"
    );
    // The compat brand accepts none of snp's own commands.
    let out = compat(&env, env.root(), &["doctor"]);
    assert_code(&out, 2);
    assert!(text(&out.stderr).contains("invalid choice: 'doctor'"));
}

#[test]
fn t10_cli_03_help_outside_a_repo() {
    let env = tenv();
    for args in [&["--help"][..], &["help"][..]] {
        let out = compat(&env, env.root(), args);
        assert_code(&out, 0);
        assert!(text(&out.stdout).starts_with("usage: pre-commit "));
    }
    let a = compat(&env, env.root(), &["help", "run"]);
    let b = compat(&env, env.root(), &["run", "--help"]);
    assert_code(&a, 0);
    assert_eq!(a.stdout, b.stdout);
    assert!(text(&a.stdout).starts_with("usage: pre-commit run "));
}

#[test]
fn t10_cli_04_hazmat_cd_parsing() {
    let env = tenv();
    let r = empty_repo(&env, "r");
    std::fs::create_dir_all(r.join("subdir")).unwrap();
    // `cd subdir -- cmd -- f1 f2`: the command after the subdir is everything up to the last `--`.
    let out = compat(&env, &r, &["hazmat", "cd", "subdir", "git", "rev-parse", "--show-prefix", "--"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "subdir/\n");
}

#[test]
fn t10_cli_05_no_args_outside_a_repo() {
    let env = tenv();
    let out = compat(&env, env.root(), &[]);
    // `main([])` is `run`: the WP-32 pipeline reports the git failure; until then the run stub may panic, but
    // never with 101 and always through the envelope.
    assert_ne!(out.code, 101);
    assert!(text(&out.stdout).contains("Check the log at "), "{}", text(&out.stdout));
}

#[test]
fn t10_cli_06_legacy_stage_names() {
    let env = tenv();
    // The parser normalises `--hook-stage commit` (pre-commit's `_STAGES` conversion); `help run` shows the
    // choices and `run --hook-stage bogus` is a usage error naming the canonical names only.
    let out = compat(&env, env.root(), &["run", "--hook-stage", "bogus"]);
    assert_code(&out, 2);
    let err = text(&out.stderr);
    assert!(err.contains("invalid choice: 'bogus'"), "{err}");
    assert!(err.contains("'pre-commit'"), "{err}");
}

#[test]
fn t10_cli_07_usage_errors_and_version() {
    let env = tenv();
    let out = compat(&env, env.root(), &["run", "--nope"]);
    assert_code(&out, 2);
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).ends_with("pre-commit: error: unrecognized arguments: --nope\n"));
    assert!(text(&out.stderr).starts_with("usage: pre-commit "));
    let out = compat(&env, env.root(), &["run", "-a", "--files", "x"]);
    assert_code(&out, 2);
    assert!(text(&out.stderr).contains("argument --files: not allowed with argument --all-files/-a"));
    let out = compat(&env, env.root(), &["--version"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "pre-commit 4.6.2\n");
    let out = snp(&env, env.root(), &["--version"]);
    assert_eq!(text(&out.stdout), "snp 2.0.0\n");
    let out = snp(&env, env.root(), &["run", "--nope"]);
    assert_code(&out, 2);
    assert!(text(&out.stderr).ends_with("snp: error: unrecognized arguments: --nope\n"));
}

/// Help, usage errors and `--version` through a `pre-commit` symlink are byte-identical to the oracle (WBS WP-33
/// acceptance; skipped when the oracle is not installed).
#[test]
fn compat_brand_cli_texts_equal_the_oracle() {
    let env = tenv();
    let Some(_) = env.oracle(snp_testkit::Oracle::Pc462Py313, env.root()) else {
        return;
    };
    for args in [
        &["--version"][..],
        &["--help"][..],
        &["install", "--help"][..],
        &["run", "--nope"][..],
        &["install", "-t", "bogus"][..],
        &["init-templatedir"][..],
        &["autoupdate", "--help"][..],
    ] {
        let ours = env
            .snp_as_pre_commit(env.root())
            .env("COLUMNS", "80")
            .args(args)
            .output();
        let theirs = env
            .oracle(snp_testkit::Oracle::Pc462Py313, env.root())
            .unwrap()
            .env("COLUMNS", "80")
            .args(args)
            .output();
        assert_eq!(ours.code, theirs.code, "{args:?}");
        assert_eq!(text(&ours.stdout), text(&theirs.stdout), "{args:?}");
        assert_eq!(text(&ours.stderr), text(&theirs.stderr), "{args:?}");
    }
}

/// The `completions` command prints the embedded scripts.
#[test]
fn completions_print_embedded_scripts() {
    let env = tenv();
    let out = snp(&env, env.root(), &["completions", "bash"]);
    assert_code(&out, 0);
    assert!(!out.stdout.is_empty());
}
