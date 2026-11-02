//! Integration tests: `snp migrate lefthook|builtin|upstream` through the binary (ARCHITECTURE §1.4.23, 05 §19.1,
//! 09 §7.1). The conversions themselves are golden-tested in `crates/snp-migrate/tests`; these tests pin what the
//! binary adds around them: where the result goes (`.snp.yaml`, `.pre-commit-config.yaml` with `--portable`, `-o FILE`,
//! `-o -`; ADR-087), the report on
//! stderr, refusing to overwrite without `--force`, `--strict` writing nothing, the `--local` overlay file, the in-place
//! builtin ⇄ upstream rewrite and the `--install` sequence.
//!
//! The `snp migrate` dispatch is WP-33's (`crates/snp/src/commands.rs`), running a converted config is WP-32's.
//!
//! Owner: WP-34.

use std::path::{Path, PathBuf};

use snp_testkit::{Bins, TestEnv};

/// `crates/snp-migrate/tests/<rel>`.
fn migrate_tests(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../snp-migrate/tests")
        .join(rel)
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(migrate_tests("goldens").join(name)).unwrap()
}

fn env() -> TestEnv {
    TestEnv::new(Bins::locate().expect("snp and snp-probe built (cargo build --workspace --bins)"))
}

/// The files of `dir` relative to it (`/`-separated), recursively.
fn tree(dir: &Path, rel: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for e in std::fs::read_dir(dir.join(rel)).unwrap() {
        let e = e.unwrap();
        let r = rel.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            tree(dir, &r, out);
        } else {
            let name = r.to_str().unwrap().replace('\\', "/");
            out.push((name, std::fs::read(e.path()).unwrap()));
        }
    }
}

/// A committed git repository holding the snp-migrate fixture `fixture`.
fn fixture_repo(env: &TestEnv, fixture: &str) -> PathBuf {
    let mut files = Vec::new();
    tree(
        &migrate_tests("fixtures").join(fixture),
        Path::new(""),
        &mut files,
    );
    let mut repo = env.git_repo(fixture);
    for (p, bytes) in &files {
        repo = if Path::new(p).extension().is_some_and(|e| e == "sh") {
            repo.exec_file(p, bytes)
        } else {
            repo.file(p, bytes)
        };
    }
    repo.stage_all().commit("lefthook").build()
}

/// Output bytes as text with CRLF folded (Windows text mode, like `cli::text`).
fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n")
}

#[test]
fn writes_the_worked_example_and_reports_on_stderr() {
    let env = env();
    let dir = fixture_repo(&env, "worked_example");
    let out = env.snp(&dir).args(["migrate", "lefthook"]).output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(out.stdout, b"");
    assert_eq!(text(&out.stderr), golden("worked_example.snp.report.txt"));
    // snp brand: `.snp.yaml` by default (ADR-087)
    let written = std::fs::read_to_string(dir.join(".snp.yaml")).unwrap();
    assert_eq!(written, golden("worked_example.snp.yaml"));
    assert!(!dir.join(".pre-commit-config.yaml").exists());
    // The source config is left alone.
    assert!(dir.join("lefthook.yml").is_file());
    // and `snp run` finds it without `-c`
    let out = env.snp(&dir).args(["config", "dump"]).output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
}

#[test]
fn portable_writes_pre_commit_config() {
    let env = env();
    let dir = fixture_repo(&env, "worked_example");
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "--portable"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(dir.join(".pre-commit-config.yaml")).unwrap(),
        golden("worked_example.portable.yaml")
    );
    assert!(!dir.join(".snp.yaml").exists());
}

#[test]
fn output_dash_prints_the_config_and_writes_nothing() {
    let env = env();
    let dir = fixture_repo(&env, "worked_example");
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "--portable", "-o", "-"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), golden("worked_example.portable.yaml"));
    assert_eq!(
        text(&out.stderr),
        golden("worked_example.portable.report.txt")
    );
    assert!(!dir.join(".pre-commit-config.yaml").exists());
}

#[test]
fn refuses_to_overwrite_without_force() {
    let env = env();
    let dir = fixture_repo(&env, "many_extends_levels");
    let target = dir.join("converted.yaml");
    std::fs::write(&target, "repos: []\n").unwrap();
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "-o", "converted.yaml"])
        .output();
    assert_eq!(out.code, 1, "{}", text(&out.stderr));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "repos: []\n");
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "-o", "converted.yaml", "--force"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        golden("many_extends_levels.snp.yaml")
    );
}

#[test]
fn strict_exits_2_and_writes_nothing() {
    let env = env();
    let dir = fixture_repo(&env, "features");
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "--strict"])
        .output();
    assert_eq!(out.code, 2, "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).ends_with(
            "snp migrate lefthook: --strict: the conversion is lossy; nothing written\n"
        ),
        "{}",
        text(&out.stderr)
    );
    assert!(!dir.join(".pre-commit-config.yaml").exists());
    assert!(!dir.join(".snp.yaml").exists());
    // A faithful conversion passes --strict.
    let dir = fixture_repo(&env, "many_extends_levels");
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "--strict"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
}

#[test]
fn local_writes_the_overlay() {
    let env = env();
    let dir = fixture_repo(&env, "features");
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "--local"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), golden("features.snp.local.report.txt"));
    assert_eq!(
        std::fs::read_to_string(dir.join(".snp.yaml")).unwrap(),
        golden("features.snp.local.yaml")
    );
    // the overlay name matches the config's (`.snp.yaml` ⇒ `.snp.local.yaml`, ADR-087)
    assert_eq!(
        std::fs::read_to_string(dir.join(".snp.local.yaml")).unwrap(),
        golden("features.snp.local.overlay.yaml")
    );
    assert!(!dir.join(".pre-commit-config.local.yaml").exists());
}

#[test]
fn local_overlay_follows_an_explicit_output_name() {
    let env = env();
    let dir = fixture_repo(&env, "features");
    let out = env
        .snp(&dir)
        .args([
            "migrate",
            "lefthook",
            "--local",
            "-o",
            ".pre-commit-config.yaml",
        ])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(dir.join(".pre-commit-config.local.yaml")).unwrap(),
        golden("features.snp.local.overlay.yaml")
    );
    assert!(!dir.join(".snp.local.yaml").exists());
    assert!(!dir.join(".snp.yaml").exists());
}

#[test]
fn load_errors_exit_1() {
    let env = env();
    let dir = env
        .git_repo("broken")
        .file("lefthook.yml", b"pre-commit: [\n")
        .stage_all()
        .commit("x")
        .build();
    let out = env.snp(&dir).args(["migrate", "lefthook"]).output();
    assert_eq!(out.code, 1);
    assert!(
        text(&out.stderr).starts_with("snp migrate lefthook: "),
        "{}",
        text(&out.stderr)
    );
    assert!(!dir.join(".pre-commit-config.yaml").exists());
    assert!(!dir.join(".snp.yaml").exists());
}

#[test]
fn builtin_and_upstream_rewrite_in_place() {
    let env = env();
    let upstream = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0  # pinned\n    hooks:\n    -   id: trailing-whitespace\n    -   id: check-yaml\n";
    let dir = env
        .git_repo("builtin")
        .file(".pre-commit-config.yaml", upstream.as_bytes())
        .stage_all()
        .commit("x")
        .build();
    let cfg = dir.join(".pre-commit-config.yaml");
    let out = env.snp(&dir).args(["migrate", "builtin"]).output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        "repos:\n# was: https://github.com/pre-commit/pre-commit-hooks@v6.0.0  # pinned\n-   repo: builtin\n    hooks:\n    -   id: trailing-whitespace\n    -   id: check-yaml\n"
    );
    let out = env
        .snp(&dir)
        .args(["migrate", "upstream", "-c", ".pre-commit-config.yaml"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), upstream);
}

#[test]
fn builtin_rewrites_the_discovered_snp_config() {
    let env = env();
    let upstream = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n    -   id: check-yaml\n";
    let dir = env
        .git_repo("builtin-snp")
        .file(".snp.yaml", upstream.as_bytes())
        .file("sub/x.txt", b"x\n")
        .stage_all()
        .commit("x")
        .build();
    // from a subdirectory: the root's `.snp.yaml` (ADR-087)
    let out = env
        .snp(&dir.join("sub"))
        .args(["migrate", "builtin"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert!(
        std::fs::read_to_string(dir.join(".snp.yaml"))
            .unwrap()
            .contains("-   repo: builtin\n"),
    );
    assert!(!dir.join(".pre-commit-config.yaml").exists());
    assert!(!dir.join("sub/.pre-commit-config.yaml").exists());
}

/// `--install` (ARCHITECTURE §1.4.23): lefthook's shim is removed and its `.old` restored, `.git/info/lefthook*` is
/// deleted and snp installs every emitted stage, chaining the restored hook as `.legacy`.
#[cfg(unix)]
#[test]
fn install_replaces_lefthook_hooks() {
    let env = env();
    let dir = fixture_repo(&env, "worked_example");
    let hooks = dir.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(
        hooks.join("pre-commit"),
        "#!/bin/sh\n# LEFTHOOK file\ncall_lefthook run \"pre-commit\" \"$@\"\n",
    )
    .unwrap();
    let old = "#!/bin/sh\necho previous hook\n";
    std::fs::write(hooks.join("pre-commit.old"), old).unwrap();
    std::fs::write(
        hooks.join("pre-push"),
        "#!/bin/sh\n# LEFTHOOK file\ncall_lefthook run \"pre-push\" \"$@\"\n",
    )
    .unwrap();
    // the test env's empty `GIT_TEMPLATE_DIR` leaves `.git/info` uncreated
    std::fs::create_dir_all(dir.join(".git/info")).unwrap();
    std::fs::write(dir.join(".git/info/lefthook.checksum"), "abc 1\n").unwrap();
    std::fs::create_dir_all(dir.join(".git/info/lefthook-remotes")).unwrap();
    let out = env
        .snp(&dir)
        .args(["migrate", "lefthook", "--install"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert!(dir.join(".snp.yaml").is_file());
    assert!(!dir.join(".git/info/lefthook.checksum").exists());
    assert!(!dir.join(".git/info/lefthook-remotes").exists());
    assert!(!hooks.join("pre-commit.old").exists());
    assert_eq!(
        std::fs::read_to_string(hooks.join("pre-commit.legacy")).unwrap(),
        old
    );
    for stage in ["pre-commit", "pre-push", "commit-msg"] {
        let hook = std::fs::read_to_string(hooks.join(stage)).unwrap();
        assert!(!hook.contains("LEFTHOOK"), "{stage}: {hook}");
        assert!(hook.contains("--config=.snp.yaml"), "{stage}: {hook}");
    }
    assert!(!hooks.join("pre-push.legacy").exists());
}

/// The snp-mode conversion of the `runnable` fixture runs with exit 0 under snp itself (the pre-commit 4.7.0 side is
/// `crates/snp-migrate/tests/oracle.rs`).
#[cfg(unix)]
#[test]
fn converted_config_runs_under_snp() {
    let env = env();
    let dir = fixture_repo(&env, "runnable");
    std::fs::write(
        dir.join(".pre-commit-config.yaml"),
        golden("runnable.snp.yaml"),
    )
    .unwrap();
    let out = env.snp(&dir).args(["run", "--all-files"]).output();
    assert_eq!(out.code, 0, "{}{}", text(&out.stdout), text(&out.stderr));
}
