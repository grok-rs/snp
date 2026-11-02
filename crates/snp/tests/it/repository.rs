//! Integration tests: hook resolution, clones, installs and the fast path (T10-REP, X13-ENV) through the `snp`
//! binary. The resolution and install halves of these rows are ported against the library in
//! `crates/snp-repo/tests/{catalog,acceptance}.rs`; the rows here need the executed hook (output channel, pty, cwd,
//! partitioning) or the whole cold/warm transcript, which `snp run` (WP-32) produces.
//!
//! Owner: WP-31.

use std::path::{Path, PathBuf};

use snp_testkit::{Bins, Fixture, TestEnv};

/// The upstream pre-commit-hooks URL: a Tier A repository at `v6.0.0` is never contacted (D-PERF-03).
const PCH: &str = "https://github.com/pre-commit/pre-commit-hooks";

/// pre-commit 4.7.0 on CPython 3.12.3 (`target/oracle/pc470-py312`), cold `run --all-files` of the three hooks of
/// [`pch_config`] over `a.json` = `{}\n` (the clone redirected to a local mirror of the upstream repository).
const PC462_COLD: &str =
    include_str!("../../../snp-repo/tests/goldens/pc470-py312-pch-v6-cold.txt");
/// The same run repeated with the warm store.
const PC462_WARM: &str =
    include_str!("../../../snp-repo/tests/goldens/pc470-py312-pch-v6-warm.txt");

fn bins() -> Bins {
    Bins::locate().expect("`cargo build --workspace --bins` first (CI does)")
}

/// A project repository holding `files`, staged and committed, plus `.pre-commit-config.yaml` = `config`.
fn project(t: &TestEnv, config: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let mut b = t
        .git_repo("project")
        .file(".pre-commit-config.yaml", config.as_bytes());
    for (name, bytes) in files {
        b = b.file(name, bytes);
    }
    b.stage_all().commit("init").build()
}

fn remote(repo: &Path, rev: &str, hooks: &str) -> String {
    format!(
        "repos:\n-   repo: {}\n    rev: {rev}\n    hooks:\n{hooks}",
        repo.display()
    )
}

fn head(t: &TestEnv, repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .env_clear()
        .envs(t.env().iter())
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn pch_config() -> String {
    format!(
        "repos:\n-   repo: {PCH}\n    rev: v6.0.0\n    hooks:\n    -   id: trailing-whitespace\n    -   id: \
         end-of-file-fixer\n    -   id: check-json\n"
    )
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn tier_a_pre_commit_hooks_cold_and_warm_transcripts_match_pc470_py312() {
    let t = TestEnv::new(bins());
    let proj = project(&t, &pch_config(), &[("a.json", b"{}\n")]);
    let cold = t
        .snp_as_pre_commit(&proj)
        .args(["run", "--all-files"])
        .output();
    assert_eq!((cold.code, text(&cold.stdout)), (0, PC462_COLD.to_owned()));
    let warm = t
        .snp_as_pre_commit(&proj)
        .args(["run", "--all-files"])
        .output();
    assert_eq!((warm.code, text(&warm.stdout)), (0, PC462_WARM.to_owned()));
    // D-PERF-03: Tier A is never cloned. A clone is a directory; the `<key>.lock` file is the repository lock the
    // `Initializing` line is printed under (ADR-032), not a clone.
    let repos = t.root().join("snp-store/repos-v1");
    let clones = std::fs::read_dir(&repos).map_or(0, |d| {
        d.filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|f| f.is_dir()))
            .count()
    });
    assert_eq!(clones, 0, "{}", repos.display());
}

#[cfg(unix)]
#[test]
fn t10_rep_03_interleaved_stdout_stderr_in_order_and_pty_with_color() {
    let t = TestEnv::new(bins());
    let repo = t.fixture_repo(Fixture::Resource("stdout_stderr_repo"));
    let rev = head(&t, &repo);
    let cfg = remote(
        &repo,
        &rev,
        "    -   id: stdout-stderr\n        verbose: true\n    -   id: tty-check\n        verbose: true\n",
    );
    let proj = project(&t, &cfg, &[("f", b"x\n")]);
    let out = t
        .snp(&proj)
        .args(["run", "--all-files", "--color", "never"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stdout));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("\n0\n1\n2\n3\n4\n5\n"),
        "one pipe keeps the order:\n{stdout}"
    );
    assert!(
        stdout.contains("\nstdin: False\nstdout: False\nstderr: False\n"),
        "{stdout}"
    );
    let out = t
        .snp(&proj)
        .args(["run", "--all-files", "--color", "always"])
        .output();
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("\nstdin: False\nstdout: True\nstderr: True\n"),
        "color runs on a pty:\n{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn t10_rep_04_hook_cwd_is_the_repository_root() {
    let t = TestEnv::new(bins());
    let repo = t.fixture_repo(Fixture::Resource("prints_cwd_repo"));
    let rev = head(&t, &repo);
    let cfg = remote(
        &repo,
        &rev,
        // No file names: BSD `pwd` (macOS) refuses operands.
        "    -   id: prints_cwd\n        args: [-L]\n        pass_filenames: false\n        verbose: true\n",
    );
    let proj = project(&t, &cfg, &[("a.sh", b"\n")]);
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    // Run from a subdirectory: pre-commit chdirs to the root before running hooks.
    let out = t.snp(&sub).args(["run", "--all-files"]).output();
    assert_eq!(out.code, 0, "{}", text(&out.stdout));
    // `pwd -L` prints the logical cwd: `PWD` set by snp to the root, as `cd <root> && pwd` would (the test root is
    // already canonical).
    assert!(
        text(&out.stdout).contains(&format!("\n{}\n", proj.display())),
        "{}",
        text(&out.stdout)
    );
}

#[cfg(unix)]
#[test]
fn t10_rep_05_fifteen_thousand_files() {
    let t = TestEnv::new(bins());
    let repo = t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let rev = head(&t, &repo);
    let cfg = remote(&repo, &rev, "    -   id: bash_hook\n");
    // `test_lots_of_files` passes `[os.devnull] * 15000`: one existing file 15 000 times (`--files` keeps
    // duplicates, and a nonexistent name would be dropped by `lexists`, leaving nothing to check)
    let proj = project(&t, &cfg, &[("f", b"x\n")]);
    let mut cmd = t.snp(&proj).args(["run", "--files"]);
    for _ in 0..15_000 {
        cmd = cmd.arg("f");
    }
    let out = cmd.output();
    let stdout = text(&out.stdout);
    assert_eq!(out.code, 0, "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.starts_with(&format!(
            "[INFO] Initializing environment for {}.\n",
            repo.display()
        )),
        "{stdout}"
    );
    assert!(stdout.ends_with("Passed\n"), "{stdout}");
    assert!(
        stdout.lines().last().unwrap().starts_with("Bash hook."),
        "{stdout}"
    );
}

#[test]
fn t10_rep_12_unknown_id_exits_1_through_the_binary() {
    let t = TestEnv::new(bins());
    let repo = t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let rev = head(&t, &repo);
    let url = format!("file://{}", repo.display());
    let cfg =
        format!("repos:\n-   repo: {url}\n    rev: {rev}\n    hooks:\n    -   id: i-dont-exist\n");
    let proj = project(&t, &cfg, &[("f", b"x\n")]);
    let out = t
        .snp_as_pre_commit(&proj)
        .args(["run", "--all-files"])
        .output();
    assert_eq!(out.code, 1);
    assert_eq!(
        text(&out.stdout),
        format!(
            "[INFO] Initializing environment for {url}.\n[ERROR] `i-dont-exist` is not present in repository \
             {url}.  Typo? Perhaps it is introduced in a newer version?  Often `pre-commit autoupdate` fixes this.\n"
        )
    );
}
