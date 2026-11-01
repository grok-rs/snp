//! `try-repo` (T10-TRY) through `snp_run::try_repo` with a real hook repository on disk; the fake resolver resolves
//! the remote repo from try-repo's throwaway store (ADR-059).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::harness::{H, args, rep};

const MANIFEST: &str = "-   id: bash_hook
    name: Bash hook
    entry: bin/hook.sh
    language: script
    files: grep-this
-   id: bash_hook2
    name: Bash hook
    entry: bin/hook2.sh
    language: script
-   id: bash_hook3
    name: Bash hook
    entry: bin/hook3.sh
    language: script
    files: grep-this
";

fn git(dir: &Path, h: &H, a: &[&str]) -> String {
    let out = Command::new("git")
        .args(a)
        .current_dir(dir)
        .env_clear()
        .envs(h.env.iter().map(|(k, v)| (k, v)))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {a:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `modified_file_returns_zero_repo`-like hook repository at `<base>/hooks`.
fn hooks_repo(h: &H) -> PathBuf {
    let dir = h.base.join("hooks");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    git(&dir, h, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join(".pre-commit-hooks.yaml"), MANIFEST).unwrap();
    for (name, body) in [
        ("hook.sh", "#!/bin/sh\necho hook\n"),
        ("hook2.sh", "#!/bin/sh\nfor f; do echo \"$f\"; done\n"),
        ("hook3.sh", "#!/bin/sh\nexit 0\n"),
    ] {
        let p = dir.join("bin").join(name);
        std::fs::write(&p, body).unwrap();
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(&p, perm).unwrap();
    }
    git(&dir, h, &["add", "."]);
    git(&dir, h, &["commit", "-q", "-m", "hooks"]);
    dir
}

fn try_args(repo: &Path, hooks: &[&str], verbose: bool) -> snp_cli::TryRepoArgs {
    let mut run = args();
    run.verbose = verbose;
    run.hooks = hooks.iter().map(OsString::from).collect();
    snp_cli::TryRepoArgs {
        repo: repo.as_os_str().to_os_string(),
        rev: None,
        run,
    }
}

/// stdout without the `[INFO]` lines and with `- duration: …` normalised.
fn norm(s: &str) -> String {
    let mut out = String::new();
    for l in s.lines().filter(|l| !l.starts_with("[INFO]")) {
        out.push_str(if l.starts_with("- duration: ") {
            "- duration: 0s"
        } else {
            l
        });
        out.push('\n');
    }
    out
}

fn consumer() -> H {
    let h = H::new();
    h.write("test-file", b"");
    h.stage("test-file");
    h
}

#[test]
fn t10_try_01_every_hook_sorted_by_id() {
    let h = consumer();
    let repo = hooks_repo(&h);
    let head = git(&repo, &h, &["rev-parse", "HEAD"]).trim().to_owned();
    let o = h.try_repo(try_args(&repo, &[], true));
    assert_eq!(o.code, 0, "{o:?}");
    let rule = rep("=", 79);
    assert!(
        o.stdout.contains("[INFO] Initializing environment for "),
        "{}",
        o.stdout
    );
    let want = format!(
        "{rule}\nUsing config:\n{rule}\nrepos:\n-   repo: ../hooks\n    rev: {head}\n    hooks:\n    -   id: bash_hook\n    \
         -   id: bash_hook2\n    -   id: bash_hook3\n{rule}\n\
         Bash hook............................................(no files to check)Skipped\n- hook id: bash_hook\n\
         Bash hook................................................................Passed\n- hook id: bash_hook2\n\
         - duration: 0s\n\ntest-file\n\n\
         Bash hook............................................(no files to check)Skipped\n- hook id: bash_hook3\n"
    );
    assert_eq!(norm(&o.stdout), want);
    // the throwaway store is gone; the main store gained no repos-v1 entry
    let repos = h.store.join("repos-v1");
    assert!(!repos.exists() || std::fs::read_dir(&repos).unwrap().next().is_none());
}

#[test]
fn t10_try_02_one_hook_without_the_manifest() {
    let h = consumer();
    let repo = hooks_repo(&h);
    let o = h.try_repo(try_args(&repo, &["bash_hook"], true));
    assert_eq!(o.code, 0, "{o:?}");
    let n = norm(&o.stdout);
    assert!(n.contains("    hooks:\n    -   id: bash_hook\n===="), "{n}");
    assert!(n.ends_with("Bash hook............................................(no files to check)Skipped\n- hook id: bash_hook\n"), "{n}");
    // the `Initializing` line follows the config block when a hook is named
    let rule_at = o.stdout.find("Using config:").unwrap();
    let init_at = o.stdout.find("[INFO] Initializing").unwrap();
    assert!(init_at > rule_at);
}

#[test]
fn t10_try_03_relative_repo() {
    let h = consumer();
    hooks_repo(&h);
    let o = h.try_repo(try_args(Path::new("../hooks"), &[], false));
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.contains("-   repo: ../hooks\n"), "{}", o.stdout);
}

#[test]
fn t10_try_04_explicit_ref() {
    let h = consumer();
    let repo = hooks_repo(&h);
    let first = git(&repo, &h, &["rev-parse", "HEAD"]).trim().to_owned();
    std::fs::write(repo.join("x"), b"x").unwrap();
    git(&repo, &h, &["add", "x"]);
    git(&repo, &h, &["commit", "-q", "-m", "two"]);
    let mut a = try_args(&repo, &[], false);
    a.rev = Some(OsString::from(&first));
    let o = h.try_repo(a);
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.contains(&format!("    rev: {first}\n")));
}

#[test]
fn t10_try_05_uncommitted_changes_use_a_shadow_repo() {
    let h = consumer();
    let repo = hooks_repo(&h);
    let m = MANIFEST.replacen(
        "    name: Bash hook\n    entry: bin/hook2.sh",
        "    name: modified name!\n    entry: bin/hook2.sh",
        1,
    );
    std::fs::write(repo.join(".pre-commit-hooks.yaml"), m).unwrap();
    let o = h.try_repo(try_args(&repo, &["bash_hook2"], false));
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout
            .starts_with("[WARNING] Creating temporary repo with uncommitted changes...\n"),
        "{}",
        o.stdout
    );
    let n = norm(&o.stdout);
    assert!(n.contains("shadow-repo\n"), "{n}");
    assert!(
        n.ends_with(
            "modified name!...........................................................Passed\n"
        ),
        "{n}"
    );
}

#[test]
fn t10_try_06_staged_files_reach_the_shadow_repo() {
    let h = consumer();
    let repo = hooks_repo(&h);
    std::fs::write(
        repo.join("bin/hook2.sh"),
        "#!/bin/sh\necho staged-version\n",
    )
    .unwrap();
    git(&repo, &h, &["add", "bin/hook2.sh"]);
    let o = h.try_repo(try_args(&repo, &["bash_hook2"], true));
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.contains("\n\nstaged-version\n\n"), "{}", o.stdout);
}

#[test]
fn t10_try_07_repo_relative_to_the_root_from_a_subdirectory() {
    let mut h = consumer();
    let repo = hooks_repo(&h);
    std::fs::rename(&repo, h.path("foo")).unwrap();
    std::fs::create_dir_all(h.path("sub")).unwrap();
    h.cwd = Some(h.path("sub"));
    let o = h.try_repo(try_args(Path::new("../foo"), &["bash_hook2"], false));
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.contains("-   repo: foo\n"), "{}", o.stdout);
}

#[test]
fn x13_git_03_nonexistent_repo() {
    let h = consumer();
    let o = h.try_repo(try_args(Path::new("/nonexistent/repo"), &[], false));
    assert_eq!(o.code, 3, "{o:?}");
    match o.failure {
        Some(snp_core::failure::Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "CalledProcessError");
            assert!(
                String::from_utf8_lossy(&exc.text)
                    .contains("'ls-remote', '--exit-code', '/nonexistent/repo', 'HEAD')")
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn try_repo_twice_initializes_both_times() {
    let h = consumer();
    let repo = hooks_repo(&h);
    for _ in 0..2 {
        let o = h.try_repo(try_args(&repo, &[], false));
        assert_eq!(
            o.stdout
                .matches("[INFO] Initializing environment for ")
                .count(),
            1,
            "{}",
            o.stdout
        );
    }
}

#[test]
fn try_repo_stashes_into_the_throwaway_store() {
    // ADR-059: `patches/` lives in the temp store, as pre-commit's `Store(tempdir)`
    let h = consumer();
    let repo = hooks_repo(&h);
    h.write("test-file", b"unstaged\n");
    let o = h.try_repo(try_args(&repo, &[], false));
    assert_eq!(o.code, 0, "{o:?}");
    let line = o
        .stdout
        .lines()
        .find(|l| l.starts_with("[INFO] Stashing unstaged files to "))
        .unwrap_or_else(|| panic!("{}", o.stdout));
    let path = line
        .trim_start_matches("[INFO] Stashing unstaged files to ")
        .trim_end_matches('.');
    assert!(
        !Path::new(path).starts_with(&h.store),
        "patch in the main store: {path}"
    );
    assert!(!Path::new(path).exists(), "the throwaway store is gone");
    assert_eq!(std::fs::read(h.path("test-file")).unwrap(), b"unstaged\n");
}
