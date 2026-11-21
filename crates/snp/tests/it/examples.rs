//! Integration tests: the example configurations of `examples/` stay valid (not a catalog row).
//!
//! Every `examples/<name>/` holds a `README.md` and exactly one config: `.pre-commit-config.yaml` (a plain pre-commit
//! config: valid in both brands, no snp extension) or `.snp.yaml` (snp only). All of them pass
//! `snp validate-config --strict` with no output, every `rev:` is a pinned tag, and the lefthook example still
//! converts without loss.
//!
//! Owner: WP-33.

use std::path::{Path, PathBuf};

use crate::cli::{assert_code, compat, snp, tenv, text};

/// The repository's `examples/` directory.
fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// `(example name, config path)` for every example directory, sorted.
fn example_configs() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(examples_dir()).expect("examples/ exists") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            dir.join("README.md").is_file(),
            "examples/{name}: README.md missing"
        );
        let configs: Vec<PathBuf> = [".pre-commit-config.yaml", ".snp.yaml"]
            .iter()
            .map(|f| dir.join(f))
            .filter(|p| p.is_file())
            .collect();
        assert_eq!(
            configs.len(),
            1,
            "examples/{name}: exactly one of .pre-commit-config.yaml and .snp.yaml"
        );
        out.push((name, configs.into_iter().next().unwrap()));
    }
    out.sort();
    out
}

#[test]
fn examples_validate() {
    let env = tenv();
    let configs = example_configs();
    assert!(configs.len() >= 6, "{configs:?}");
    for (name, cfg) in &configs {
        let path = cfg.to_str().unwrap();
        let strict = snp(&env, env.root(), &["validate-config", "--strict", path]);
        assert_code(&strict, 0);
        assert_eq!(text(&strict.stdout), "", "examples/{name}");
        assert_eq!(text(&strict.stderr), "", "examples/{name}");
        if cfg.ends_with(".pre-commit-config.yaml") {
            // a plain pre-commit config: no snp extension key, and valid for the compat brand too
            let portable = snp(&env, env.root(), &["validate-config", "--portable", path]);
            assert_code(&portable, 0);
            assert_eq!(text(&portable.stdout), "", "examples/{name}");
            let pc = compat(&env, env.root(), &["validate-config", path]);
            assert_code(&pc, 0);
            assert_eq!(text(&pc.stdout), "", "examples/{name}");
        }
    }
}

#[test]
fn examples_pin_tags() {
    for (name, cfg) in example_configs() {
        let config = std::fs::read_to_string(&cfg).unwrap();
        for line in config.lines() {
            let Some(rev) = line.trim_start().strip_prefix("rev:") else {
                continue;
            };
            let rev = rev.split('#').next().unwrap().trim();
            assert!(
                rev.starts_with(|c: char| c == 'v' || c.is_ascii_digit())
                    && rev.contains('.')
                    && !rev.contains(['*', '^', '~']),
                "examples/{name}: `rev: {rev}` is not a release tag"
            );
        }
    }
}

#[test]
fn examples_lefthook_converts_losslessly() {
    let env = tenv();
    let lefthook = examples_dir().join("extensions/lefthook.yml");
    let out = snp(
        &env,
        env.root(),
        &[
            "migrate",
            "lefthook",
            "--root",
            env.root().to_str().unwrap(),
            "--strict",
            "-c",
            lefthook.to_str().unwrap(),
            "-o",
            "-",
        ],
    );
    assert_code(&out, 0);
    assert!(
        text(&out.stderr).contains("(0 warnings, 0 lossy conversions)"),
        "{}",
        text(&out.stderr)
    );
}
