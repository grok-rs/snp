//! Integration tests: `autoupdate` (T10-AUP, X13-GIT-02 / D-CRASH-06).
//!
//! Every upstream is a local repository reached through its path (hermetic).
//!
//! Owner: WP-33.

use std::path::{Path, PathBuf};

use snp_testkit::TestEnv;

use crate::cli::{assert_code, compat, git, repo_with, tenv, text};

const MANIFEST: &str = "-   id: foo\n    name: foo\n    entry: echo\n    language: system\n";

/// An upstream with the manifest committed (`v1` state); returns its path and HEAD.
fn upstream(env: &TestEnv, name: &str) -> (PathBuf, String) {
    let p = env
        .git_repo(name)
        .file(".pre-commit-hooks.yaml", MANIFEST.as_bytes())
        .stage_all()
        .commit("manifest")
        .build();
    let head = rev(env, &p, "HEAD");
    (p, head)
}

fn rev(env: &TestEnv, repo: &Path, r: &str) -> String {
    git(env, repo, &["rev-parse", r]).1.trim().to_owned()
}

/// One more commit on the upstream.
fn advance(env: &TestEnv, up: &Path, file: &str) -> String {
    env.git_repo(up.file_name().unwrap().to_str().unwrap())
        .file(file, b"x\n")
        .stage_all()
        .commit(file)
        .build();
    rev(env, up, "HEAD")
}

fn config_for(repos: &[(&Path, &str)]) -> String {
    let mut s = String::from("repos:\n");
    for (p, r) in repos {
        s.push_str(&format!(
            "-   repo: {}\n    rev: {r}\n    hooks:\n    -   id: foo\n",
            p.display()
        ));
    }
    s
}

fn read_cfg(r: &Path) -> String {
    std::fs::read_to_string(r.join(".pre-commit-config.yaml")).unwrap()
}

#[test]
fn t10_aup_01_rev_info_from_config() {
    let env = tenv();
    let (up, head) = upstream(&env, "up");
    let r = repo_with(&env, "r", &config_for(&[(&up, &head)]));
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), format!("[{}] already up to date!\n", up.display()));
}

#[test]
fn t10_aup_02_rev_selection() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "a");
    // out of date, no tags → the sha of HEAD
    let r = repo_with(&env, "r", &config_for(&[(&up, &old)]));
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), format!("[{}] updating {old} -> {new}\n", up.display()));
    // a tag on the newest commit: the tag; a version-like tag preferred over `latest`
    env.git_repo("up").tag("latest").tag("v1.2.3").build();
    let r = repo_with(&env, "r2", &config_for(&[(&up, &old)]));
    let out = compat(&env, &r, &["autoupdate"]);
    assert_eq!(text(&out.stdout), format!("[{}] updating {old} -> v1.2.3\n", up.display()));
    // commits after the tag: still the tag (tags_only)
    let head = advance(&env, &up, "b");
    let out = compat(&env, &r, &["autoupdate"]);
    assert_eq!(text(&out.stdout), format!("[{}] already up to date!\n", up.display()));
    // --bleeding-edge: HEAD's sha
    let out = compat(&env, &r, &["autoupdate", "--bleeding-edge"]);
    assert_eq!(text(&out.stdout), format!("[{}] updating v1.2.3 -> {head}\n", up.display()));
    // freeze an already-sha rev: no frozen comment
    let out = compat(&env, &r, &["autoupdate", "--bleeding-edge", "--freeze"]);
    assert_eq!(text(&out.stdout), format!("[{}] already up to date!\n", up.display()));
}

#[test]
fn t10_aup_03_up_to_date_is_byte_identical() {
    let env = tenv();
    let (up, head) = upstream(&env, "up");
    let cfg = config_for(&[(&up, &head)]);
    let r = repo_with(&env, "r", &cfg);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(read_cfg(&r), cfg);
}

#[test]
fn t10_aup_04_old_rev_without_manifest() {
    let env = tenv();
    let p = env.git_repo("up").file("nope.yaml", MANIFEST.as_bytes()).stage_all().commit("1").build();
    let old = rev(&env, &p, "HEAD");
    env.git_repo("up").file(".pre-commit-hooks.yaml", MANIFEST.as_bytes()).stage_all().commit("2").build();
    let new = rev(&env, &p, "HEAD");
    let r = repo_with(&env, "r", &config_for(&[(&p, &old)]));
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert!(read_cfg(&r).contains(&format!("rev: {new}\n")));
}

#[test]
fn t10_aup_05_only_the_stale_rev_changes() {
    let env = tenv();
    let (a, a_head) = upstream(&env, "a");
    let (b, b_old) = upstream(&env, "b");
    let b_new = advance(&env, &b, "x");
    let r = repo_with(&env, "r", &config_for(&[(&a, &a_head), (&b, &b_old)]));
    git(&env, &r, &["config", "--global", "core.useBuiltinFSMonitor", "true"]);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(read_cfg(&r), config_for(&[(&a, &a_head), (&b, &b_new)]));
}

#[test]
fn t10_aup_06_repo_filter() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "x");
    let cfg = format!(
        "{}-   repo: local\n    hooks:\n    -   id: l\n        name: l\n        entry: l\n        language: system\n",
        config_for(&[(&up, &old)])
    );
    let r = repo_with(&env, "r", &cfg);
    let out = compat(&env, &r, &["autoupdate", "--repo", up.to_str().unwrap()]);
    assert_code(&out, 0);
    assert!(read_cfg(&r).contains(&format!("rev: {new}\n")));
    assert!(read_cfg(&r).contains("-   repo: local\n"));
    let before = read_cfg(&r);
    let out = compat(&env, &r, &["autoupdate", "--repo", "foo", "--repo", "dne"]);
    assert_code(&out, 1);
    assert_eq!(text(&out.stdout), "repos (dne, foo) were not found in .pre-commit-config.yaml\n");
    assert_eq!(read_cfg(&r), before);
}

#[test]
fn t10_aup_07_comments_and_flow_lists_preserved() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "x");
    let cfg = format!(
        "repos:\n-   repo: {}\n    rev: {old}  # definitely the version I want!\n    # a comment\n    hooks:\n    -   id: foo\n        args: [foo, bar, baz]\n",
        up.display()
    );
    let r = repo_with(&env, "r", &cfg);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(read_cfg(&r), cfg.replace(&old, &new));
}

#[test]
fn t10_aup_08_mixed_line_endings() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let cfg = format!(
        "repos:\r\n-   repo: {}\n    rev: {old}\r\n    hooks:\n    -   id: foo\r\n",
        up.display()
    );
    let r = repo_with(&env, "r", &cfg);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(std::fs::read(r.join(".pre-commit-config.yaml")).unwrap(), cfg.as_bytes());
    let new = advance(&env, &up, "x");
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(
        std::fs::read(r.join(".pre-commit-config.yaml")).unwrap(),
        cfg.replace(&old, &new).as_bytes()
    );
}

#[test]
fn t10_aup_09_flow_style_is_reformatted() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "x");
    let cfg = format!(
        "repos: [\n    {{\n        repo: {}, rev: {old},\n        hooks: [\n            # A comment!\n            {{id: foo}},\n        ],\n    }}\n]\n",
        up.display()
    );
    let r = repo_with(&env, "r", &cfg);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(
        read_cfg(&r),
        format!("repos:\n-   repo: {}\n    rev: {new}\n    hooks:\n    -   id: foo\n", up.display())
    );
}

#[test]
fn t10_aup_10_freeze_and_unfreeze() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    advance(&env, &up, "x");
    env.git_repo("up").tag("v1.2.3").build();
    let tagged = rev(&env, &up, "v1.2.3");
    let r = repo_with(&env, "r", &config_for(&[(&up, &old)]));
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert!(read_cfg(&r).contains("    rev: v1.2.3\n"));
    let out = compat(&env, &r, &["autoupdate", "--freeze"]);
    assert_eq!(text(&out.stdout), format!("[{}] updating v1.2.3 -> v1.2.3 (frozen)\n", up.display()));
    assert!(read_cfg(&r).contains(&format!("    rev: {tagged}  # frozen: v1.2.3\n")));
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert!(read_cfg(&r).contains("    rev: v1.2.3\n"));
}

#[test]
fn t10_aup_11_tags_only_default() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    advance(&env, &up, "x");
    env.git_repo("up").tag("v1.2.3").build();
    advance(&env, &up, "y");
    let r = repo_with(&env, "r", &config_for(&[(&up, &old)]));
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert!(read_cfg(&r).contains("    rev: v1.2.3\n"));
}

#[test]
fn t10_aup_12_upstream_without_manifest() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    git(&env, &up, &["rm", "-q", ".pre-commit-hooks.yaml"]);
    git(&env, &up, &["commit", "-qm", "rm"]);
    let cfg = config_for(&[(&up, &old)]);
    let r = repo_with(&env, "r", &cfg);
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 1);
    let o = text(&out.stdout);
    assert!(o.starts_with(&format!("[{}] \n=====> ", up.display())), "{o}");
    assert!(o.contains(".pre-commit-hooks.yaml is not a file"), "{o}");
    assert_eq!(read_cfg(&r), cfg);
}

#[test]
fn t10_aup_13_hook_disappeared() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    env.git_repo("up")
        .file(".pre-commit-hooks.yaml", MANIFEST.replace("foo", "bar").as_bytes())
        .stage_all()
        .commit("rename")
        .build();
    let cfg = config_for(&[(&up, &old)]);
    let r = repo_with(&env, "r", &cfg);
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 1);
    assert_eq!(
        text(&out.stdout),
        format!("[{}] Cannot update because the update target is missing these hooks: foo\n", up.display())
    );
    assert_eq!(read_cfg(&r), cfg);
}

#[test]
fn t10_aup_14_local_repos_are_untouched() {
    let env = tenv();
    let local = "-   repo: local\n    hooks:\n    -   id: l\n        name: l\n        entry: l\n        language: system\n";
    let r = repo_with(&env, "r", &format!("repos:\n{local}"));
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 0);
    assert!(out.stdout.is_empty());
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "x");
    let cfg = format!("repos:\n{local}{}", config_for(&[(&up, &old)]).trim_start_matches("repos:\n"));
    let r = repo_with(&env, "r2", &cfg);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(read_cfg(&r), cfg.replace(&old, &new));
}

#[test]
fn t10_aup_15_meta_only() {
    let env = tenv();
    let cfg = "repos:\n-   repo: meta\n    hooks:\n    -   id: identity\n";
    let r = repo_with(&env, "r", cfg);
    assert_code(&compat(&env, &r, &["autoupdate"]), 0);
    assert_eq!(read_cfg(&r), cfg);
}

#[test]
fn t10_aup_16_list_format_is_migrated_first() {
    let env = tenv();
    let (up, head) = upstream(&env, "up");
    let cfg = format!("-   repo: {}\n    sha: {head}\n    hooks:\n    -   id: foo\n", up.display());
    let r = repo_with(&env, "r", &cfg);
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!("[{}] already up to date!\nConfiguration has been migrated.\n", up.display())
    );
    assert!(read_cfg(&r).starts_with("repos:\n-   repo: "));
    assert!(read_cfg(&r).contains(&format!("    rev: {head}\n")));
}

#[test]
fn t10_aup_17_quote_style_preserved() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "x");
    for q in ['"', '\''] {
        let cfg = format!(
            "repos:\n-   repo: {}\n    rev: {q}{old}{q}\n    hooks:\n    -   id: foo\n",
            up.display()
        );
        let r = repo_with(&env, &format!("r{}", u32::from(q)), &cfg);
        assert_code(&compat(&env, &r, &["autoupdate"]), 0);
        assert_eq!(read_cfg(&r), cfg.replace(&old, &new));
    }
}

/// D-CRASH-06: one unreachable repository is its own `[repo] <CalledProcessError>` line; the others are updated
/// and written; exit 1 (pre-commit: exit 3, nothing written).
#[test]
fn x13_git_02_one_fetch_fails() {
    let env = tenv();
    let (up, old) = upstream(&env, "up");
    let new = advance(&env, &up, "x");
    let missing = env.root().join("nonexistent");
    let r = repo_with(&env, "r", &config_for(&[(&up, &old), (&missing, "v1")]));
    let out = compat(&env, &r, &["autoupdate"]);
    assert_code(&out, 1);
    let o = text(&out.stdout);
    assert!(o.contains(&format!("[{}] updating {old} -> {new}\n", up.display())), "{o}");
    assert!(o.contains(&format!("[{}] command: (", missing.display())), "{o}");
    assert!(o.contains("'fetch', 'origin', 'HEAD', '--quiet', '--filter=blob:none', '--tags')\nreturn code: 128\n"), "{o}");
    assert!(!o.contains("An unexpected error"), "{o}");
    assert!(read_cfg(&r).contains(&format!("rev: {new}\n")));
}

/// `-j` runs repositories in parallel; with several workers every repository still gets exactly one line.
#[test]
fn parallel_jobs() {
    let env = tenv();
    let mut ups = Vec::new();
    for i in 0..4 {
        let (p, old) = upstream(&env, &format!("up{i}"));
        let new = advance(&env, &p, "x");
        ups.push((p, old, new));
    }
    let pairs: Vec<(&Path, &str)> = ups.iter().map(|(p, o, _)| (p.as_path(), o.as_str())).collect();
    let r = repo_with(&env, "r", &config_for(&pairs));
    let out = compat(&env, &r, &["autoupdate", "-j", "0"]);
    assert_code(&out, 0);
    let o = text(&out.stdout);
    assert_eq!(o.lines().count(), 4, "{o}");
    let cfg = read_cfg(&r);
    for (_, _, new) in &ups {
        assert!(cfg.contains(new.as_str()));
    }
}
