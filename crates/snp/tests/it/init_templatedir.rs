//! Integration tests: `init-templatedir` (T10-ITD).
//!
//! Owner: WP-33.

use crate::cli::{LOCAL_CONFIG, assert_code, compat, git, repo_with, snp, tenv, text};

#[test]
fn t10_itd_01_template_dir_not_configured() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let tmpl = env.root().join("tmpl");
    let out = compat(&env, &r, &["init-templatedir", tmpl.to_str().unwrap(), "-t", "pre-commit"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!(
            "pre-commit installed at {t}/hooks/pre-commit\n[WARNING] `init.templateDir` not set to the target directory\n[WARNING] maybe `git config --global init.templateDir {t}`?\n",
            t = tmpl.display()
        )
    );
    let script = std::fs::read_to_string(tmpl.join("hooks/pre-commit")).unwrap();
    assert!(script.contains("--skip-on-missing-config"));
    // A repository created from the template gets the script.
    let fresh = env.root().join("fresh");
    let (code, _, err) = git(&env, env.root(), &["init", "-q", "--template", tmpl.to_str().unwrap(), fresh.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(fresh.join(".git/hooks/pre-commit").is_file());
}

#[test]
fn t10_itd_02_template_dir_configured() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let tmpl = env.root().join("home/templatedir");
    git(&env, &r, &["config", "--global", "init.templateDir", "~/templatedir"]);
    let out = compat(&env, &r, &["init-templatedir", tmpl.to_str().unwrap()]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!("pre-commit installed at {}/hooks/pre-commit\n", tmpl.display())
    );
}

#[test]
fn t10_itd_03_target_is_the_cwd() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let out = compat(&env, &r, &["init-templatedir", "."]);
    assert_code(&out, 0);
    let o = text(&out.stdout);
    assert_eq!(o.lines().count(), 3, "{o}");
    assert!(o.starts_with("pre-commit installed at ./hooks/pre-commit\n"), "{o}");
    assert!(r.join("hooks/pre-commit").is_file());
}

#[test]
fn t10_itd_04_core_hooks_path_does_not_refuse() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    git(&env, &r, &["config", "--local", "core.hooksPath", "hooks"]);
    let tmpl = env.root().join("tmpl");
    let out = compat(&env, &r, &["init-templatedir", tmpl.to_str().unwrap()]);
    assert_code(&out, 0);
    assert!(tmpl.join("hooks/pre-commit").is_file());
}

#[test]
fn t10_itd_05_skip_on_missing_config_default() {
    let env = tenv();
    let tmpl = env.root().join("tmpl");
    // No git, no repository needed (COMMANDS_NO_GIT).
    let out = snp(&env, env.root(), &["init-templatedir", tmpl.to_str().unwrap()]);
    assert_code(&out, 0);
    assert!(std::fs::read_to_string(tmpl.join("hooks/pre-commit")).unwrap().contains("--skip-on-missing-config"));
    let out = snp(&env, env.root(), &["init-templatedir", tmpl.to_str().unwrap(), "--no-allow-missing-config"]);
    assert_code(&out, 0);
    assert!(!std::fs::read_to_string(tmpl.join("hooks/pre-commit")).unwrap().contains("--skip-on-missing-config"));
}
