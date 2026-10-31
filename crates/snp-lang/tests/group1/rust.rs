//! `language: rust` (10 §5.10 rust row: default `system` when `cargo --version` returns 0, `default` on 127, probed
//! with cwd `/` so a bogus `rust-toolchain.toml` cannot break it; WP-24: the persistent rustup, `cli:` and library
//! dependencies, deps-variant isolation).

use std::sync::Arc;

use snp_core::lang::LanguageId;
use snp_lang::harness::{Reply, TestHost};
use snp_lang::langs::rust::Rust;
use snp_lang::{Health, InstallOutcome, LangError, Language};

use crate::support::{FakeFetch, World, deps, env_ctx, install_ctx, sha256_hex};

/// Installs into `<prefix>/rustenv-<version>`.
fn install(
    t: &TestHost,
    prefix: &std::path::Path,
    version: &str,
    d: &[&str],
) -> Result<InstallOutcome, LangError> {
    let host = t.host();
    let env_dir = prefix.join(format!("rustenv-{version}"));
    let d = deps(d);
    Rust.install(&install_ctx(&host, prefix, version, &env_dir, &d, false))
}

#[test]
fn lang_rust_default_version() {
    for (status, want) in [(0, "system"), (127, "default"), (1, "default")] {
        let w = World::new();
        // A bogus toolchain file in the repository: the probe runs in `/`.
        std::fs::write(
            w.repo.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"bogus\"\n",
        )
        .unwrap();
        let t = w.host(w.isolated_env(&[]));
        t.runner
            .reply("cargo", Reply::fail(status, "").when(["--version"]));
        assert_eq!(&*Rust.default_version(&t.host()).unwrap(), want);
        let c = t.runner.golden(&w.subst());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].argv, ["cargo", "--version"]);
        assert_eq!(c[0].cwd, if cfg!(windows) { "\\" } else { "/" });
    }
}

#[test]
fn lang_rust_system_dependencies_in_config_order() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    let out = install(
        &t,
        &w.prefix,
        "system",
        &["cli:shellharden:4.2.0", "serde:1.0", "cli:ripgrep", "rand"],
    )
    .unwrap();
    assert_eq!(out.tools.len(), 0);
    let c = t.runner.golden(&w.subst());
    let argv: Vec<Vec<String>> = c.iter().map(|c| c.argv.clone()).collect();
    assert_eq!(
        argv,
        [
            vec!["cargo", "add", "serde@1.0", "rand@*"],
            vec![
                "cargo",
                "install",
                "--bins",
                "--root",
                "{PREFIX}/rustenv-system",
                "--path",
                "."
            ],
            vec![
                "cargo",
                "install",
                "--bins",
                "--root",
                "{PREFIX}/rustenv-system",
                "shellharden",
                "--version",
                "4.2.0"
            ],
            vec![
                "cargo",
                "install",
                "--bins",
                "--root",
                "{PREFIX}/rustenv-system",
                "ripgrep"
            ],
        ]
        .map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>())
    );
    for call in &c {
        assert_eq!(call.cwd, "{PREFIX}");
        assert_eq!(call.env.get("RUSTUP_TOOLCHAIN"), None);
    }
}

/// ADR-025, the backend's half: `cargo add` runs in (and edits) only the prefix it is handed — never the user's
/// repository (the run's cwd) or another directory — so the per-`(repo, rev, deps)` clones that snp-store creates
/// (WP-22, `git clone --local` per variant; covered by the store's tests) stay independent. A variant without
/// library dependencies runs no `cargo add`.
#[test]
fn lang_rust_deps_variants_are_isolated() {
    let w = World::new();
    let variant_a = w.root.join("variant-a");
    let variant_b = w.root.join("variant-b");
    for p in [&variant_a, &variant_b] {
        std::fs::create_dir_all(p).unwrap();
        std::fs::write(p.join("Cargo.toml"), "[package]\nname = \"hooks\"\n").unwrap();
    }
    let t = w.host(w.isolated_env(&[]));
    t.runner.reply(
        "cargo",
        Reply::ok(Vec::new())
            .when(["add"])
            .sticky()
            .with_effect(|rec| {
                let manifest = rec.cwd.join("Cargo.toml");
                let mut body = std::fs::read_to_string(&manifest)?;
                body.push_str("[dependencies]\nserde = \"1.0\"\n");
                std::fs::write(manifest, body)
            }),
    );
    install(&t, &variant_a, "system", &["serde:1.0"]).unwrap();
    install(&t, &variant_b, "system", &[]).unwrap();
    assert!(
        std::fs::read_to_string(variant_a.join("Cargo.toml"))
            .unwrap()
            .contains("serde")
    );
    assert_eq!(
        std::fs::read_to_string(variant_b.join("Cargo.toml")).unwrap(),
        "[package]\nname = \"hooks\"\n"
    );
    let c = t.runner.calls();
    let adds: Vec<_> = c.iter().filter(|c| c.argv[1] == "add").collect();
    assert_eq!(adds.len(), 1);
    assert_eq!(adds[0].cwd, variant_a);
    assert!(
        !w.repo.join("Cargo.toml").exists(),
        "the user's repository is untouched"
    );
}

/// A managed toolchain: rustup from `PATH` with the persistent shared `RUSTUP_HOME`, `--profile minimal`.
#[test]
fn lang_rust_managed_toolchain_uses_the_shared_rustup() {
    let w = World::new();
    w.shim("rustup");
    let t = w.host(w.isolated_env(&[]));
    let out = install(&t, &w.prefix, "1.79.0", &[]).unwrap();
    assert_eq!(out.tools, vec![(Arc::from("rustup"), Arc::from("v1"))]);
    let c = t.runner.golden(&w.subst());
    assert_eq!(
        c[0].argv,
        [
            "rustup",
            "toolchain",
            "install",
            "--no-self-update",
            "--profile",
            "minimal",
            "1.79.0"
        ]
    );
    assert_eq!(
        c[0].env.get("RUSTUP_HOME"),
        Some(&Some("{TOOLS}/tools/rustup/v1/rustup".to_owned()))
    );
    assert_eq!(
        c[0].env.get("CARGO_HOME"),
        Some(&Some("{TOOLS}/tools/rustup/v1/cargo".to_owned()))
    );
    let install_call = &c[1];
    assert_eq!(install_call.argv[..2], ["cargo", "install"]);
    assert_eq!(
        install_call.env.get("RUSTUP_TOOLCHAIN"),
        Some(&Some("1.79.0".to_owned()))
    );
    assert_eq!(
        install_call.env.get("RUSTUP_HOME"),
        Some(&Some("{TOOLS}/tools/rustup/v1/rustup".to_owned()))
    );
    if cfg!(unix) {
        assert_eq!(
            install_call.env.get("PATH"),
            Some(&Some(
                "{PREFIX}/rustenv-1.79.0/bin:{TOOLS}/tools/rustup/v1/cargo/bin:{SHIMS}".to_owned()
            ))
        );
    }
    // `default` is `stable`.
    let w = World::new();
    w.shim("rustup");
    let t = w.host(w.isolated_env(&[]));
    install(&t, &w.prefix, "default", &[]).unwrap();
    assert_eq!(
        t.runner.golden(&w.subst())[0].argv.last().unwrap(),
        "stable"
    );
}

/// No rustup anywhere: the rustup-init binary is downloaded (verified by its `.sha256`) and bootstraps the shared
/// home.
#[test]
fn lang_rust_rustup_bootstrap() {
    let w = World::new();
    let target = snp_net::tools::host_target();
    let exe = if cfg!(windows) {
        "rustup-init.exe"
    } else {
        "rustup-init"
    };
    let url = format!("https://static.rust-lang.org/rustup/archive/1.28.2/{target}/{exe}");
    let body = b"\x7fELF fake rustup-init".to_vec();
    let mut t = w.host(w.isolated_env(&[]));
    t.fetch = Some(Box::new(FakeFetch {
        texts: vec![
            (
                "https://static.rust-lang.org/rustup/release-stable.toml".to_owned(),
                "schema-version = '1'\nversion = '1.28.2'\n".to_owned(),
            ),
            (
                format!("{url}.sha256"),
                format!("{}  {exe}\n", sha256_hex(&body)),
            ),
        ],
        files: vec![(url, body)],
        ..FakeFetch::default()
    }));
    install(&t, &w.prefix, "stable", &[]).unwrap();
    let c = t.runner.golden(&w.subst());
    assert_eq!(
        c[0].argv,
        [
            "rustup-init",
            "-y",
            "--quiet",
            "--no-modify-path",
            "--default-toolchain",
            "none",
            "--profile",
            "minimal"
        ]
    );
    assert_eq!(c[1].argv[..3], ["rustup", "toolchain", "install"]);
    let init = w.repo.join(".snp-tools/tools/rustup-init/1.28.2").join(exe);
    assert!(init.is_file());
}

#[test]
fn lang_rust_activation_and_health() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    let host = t.host();
    let env_dir = w.prefix.join("rustenv-system");
    let cx = env_ctx(&host, &w.prefix, "system", &env_dir);
    assert_eq!(Rust.health(&cx, None).unwrap(), Health::Healthy);
    let env = t.env.patched(&Rust.activate(&cx).unwrap());
    assert_eq!(env.get("RUSTUP_TOOLCHAIN"), None);
    let managed = w.prefix.join("rustenv-default");
    let env = t.env.patched(
        &Rust
            .activate(&env_ctx(&host, &w.prefix, "default", &managed))
            .unwrap(),
    );
    assert_eq!(env.get("RUSTUP_TOOLCHAIN").unwrap(), "stable");
    assert_eq!(t.runner.calls().len(), 0);
    assert_eq!(Rust.id(), LanguageId::Rust);
}
