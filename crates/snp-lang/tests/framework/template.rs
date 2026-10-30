//! `LOCAL_TEMPLATE`: pre-commit 4.6.2's `store._LOCAL_RESOURCES` with the `empty_template_*` bytes (03 §7.7).

use std::path::Path;

use snp_lang::{LOCAL_TEMPLATE, LOCAL_TEMPLATE_VERSION};

#[test]
fn names_order_and_key_contents() {
    let names: Vec<&str> = LOCAL_TEMPLATE.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "Cargo.toml",
            "main.go",
            "go.mod",
            "main.rs",
            "pre-commit-package-dev-1.rockspec",
            "pre_commit_placeholder_package.gemspec",
            "setup.py",
            "environment.yml",
            "Makefile.PL",
            "pubspec.yaml",
            "renv.lock",
            "renv/activate.R",
            "renv/LICENSE.renv",
        ]
    );
    let get = |n: &str| {
        LOCAL_TEMPLATE
            .iter()
            .find(|(name, _)| *name == n)
            .unwrap()
            .1
    };
    assert_eq!(
        get("setup.py"),
        b"from setuptools import setup\n\n\nsetup(name='pre-commit-placeholder-package', version='0.0.0', py_modules=[])\n"
    );
    assert_eq!(
        get("Cargo.toml"),
        b"[package]\nname = \"__fake_crate\"\nversion = \"0.0.0\"\n\n[[bin]]\nname = \"__fake_cmd\"\npath = \"main.rs\"\n"
    );
    assert_eq!(get("main.rs"), b"fn main() {}\n");
    assert_eq!(
        get("go.mod"),
        b"module pre-commit-placeholder-empty-module\n"
    );
    assert_eq!(get("main.go"), b"package main\n\nfunc main() {}\n");
    assert_eq!(get("pre-commit-package-dev-1.rockspec").len(), 212);
    assert_eq!(get("renv/activate.R").len(), 12037);
    assert_eq!(get("renv/LICENSE.renv").len(), 1052);
    assert!(
        String::from_utf8(get("pubspec.yaml").to_vec())
            .unwrap()
            .contains("sdk: '>=2.12.0'")
    );
    // 4.6.1 dropped package.json and .npmignore; LOCAL_REPO_VERSION stayed 1.
    assert!(!names.contains(&"package.json"));
    assert_eq!(LOCAL_TEMPLATE_VERSION, 1);
}

/// Byte equality with the oracle's resources (`cargo xtask oracle install`).
#[test]
#[ignore = "oracle"]
fn oracle_resources_are_byte_identical() {
    let res = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/oracle/pc462-py312/lib/python3.12/site-packages/pre_commit/resources");
    for (name, bytes) in LOCAL_TEMPLATE {
        let base = name.rsplit('/').next().unwrap();
        let oracle = std::fs::read(res.join(format!("empty_template_{base}"))).unwrap();
        assert_eq!(&oracle[..], *bytes, "{name}");
    }
}
