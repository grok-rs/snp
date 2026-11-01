//! Integration tests: `validate-config`, `validate-manifest` (T10-VAL-01…04).
//!
//! Owner: WP-33.

use crate::cli::{assert_code, compat, snp, tenv, text};

const GOOD: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n";

#[test]
fn t10_val_01_valid_config() {
    let env = tenv();
    std::fs::write(env.root().join("c.yaml"), GOOD).unwrap();
    let out = compat(&env, env.root(), &["validate-config", "c.yaml"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "");
    // no file at all: exit 0, nothing printed
    let out = compat(&env, env.root(), &["validate-config"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn t10_val_02_invalid_configs() {
    let env = tenv();
    std::fs::write(env.root().join("bad.yaml"), "{\n").unwrap();
    std::fs::write(env.root().join("empty.yaml"), "{}\n").unwrap();
    for f in ["does-not-exist", "bad.yaml", "empty.yaml"] {
        let out = compat(&env, env.root(), &["validate-config", f]);
        assert_code(&out, 1);
    }
    let out = compat(
        &env,
        env.root(),
        &["validate-config", "empty.yaml", "does-not-exist"],
    );
    assert_eq!(
        text(&out.stdout),
        "\n==> File empty.yaml\n==> At Config()\n=====> Missing required key: repos\n\n=====> does-not-exist is not a file\n"
    );
}

#[test]
fn t10_val_03_invalid_manifests() {
    let env = tenv();
    std::fs::write(env.root().join("bad.yaml"), "{\n").unwrap();
    std::fs::write(env.root().join("empty.yaml"), "{}\n").unwrap();
    for f in ["does-not-exist", "bad.yaml", "empty.yaml"] {
        let out = compat(&env, env.root(), &["validate-manifest", f]);
        assert_code(&out, 1);
    }
    std::fs::write(
        env.root().join("m.yaml"),
        "-   id: x\n    name: x\n    entry: x\n    language: system\n",
    )
    .unwrap();
    assert_code(
        &compat(&env, env.root(), &["validate-manifest", "m.yaml"]),
        0,
    );
}

#[test]
fn t10_val_04_sample_config_exact() {
    let env = tenv();
    let out = compat(&env, env.root(), &["sample-config"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        "# See https://pre-commit.com for more information\n# See https://pre-commit.com/hooks.html for more hooks\nrepos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v3.2.0\n    hooks:\n    -   id: trailing-whitespace\n    -   id: end-of-file-fixer\n    -   id: check-yaml\n    -   id: check-added-large-files\n"
    );
}

/// snp brand: `--strict` reports hook keys at validation time; `--portable` validates as pre-commit would.
#[test]
fn strict_and_portable() {
    let env = tenv();
    let cfg = "repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n        bogus: 1\n        priority: 1\n";
    std::fs::write(env.root().join("c.yaml"), cfg).unwrap();
    let plain = snp(&env, env.root(), &["validate-config", "c.yaml"]);
    assert_code(&plain, 0);
    assert!(!text(&plain.stdout).contains("bogus"));
    let strict = snp(&env, env.root(), &["validate-config", "--strict", "c.yaml"]);
    assert_code(&strict, 0);
    assert!(
        text(&strict.stdout).contains("[WARNING] Unexpected key(s) present on local => x: bogus"),
        "{}",
        text(&strict.stdout)
    );
    let portable = snp(
        &env,
        env.root(),
        &["validate-config", "--portable", "c.yaml"],
    );
    assert_code(&portable, 0);
    let p = text(&portable.stdout);
    assert!(p.contains("bogus") && p.contains("priority"), "{p}");
    // the compat brand rejects both flags
    assert_code(
        &compat(&env, env.root(), &["validate-config", "--strict", "c.yaml"]),
        2,
    );
}
