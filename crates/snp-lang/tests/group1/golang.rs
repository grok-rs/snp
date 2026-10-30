//! `language: golang` (10 §5.10 golang row: default `system` iff `exe_exists('go')`, `_infer_go_version` passthrough;
//! WP-24: verified download into the shared tools, the 404 `ValueError`, the install environment, `pkg` removal).

use std::sync::Arc;

use snp_core::lang::LanguageId;
use snp_lang::harness::{Reply, TestHost};
use snp_lang::langs::golang::Golang;
use snp_lang::{Health, InstallOutcome, LangError, Language};

use crate::support::{FakeFetch, World, deps, env_ctx, exe, install_ctx, sha256_hex, tar_gz};

/// The go archive of `version` for this host and the go.dev JSON listing it.
fn go_release(version: &str) -> (String, Vec<u8>, String) {
    let target = snp_net::tools::host_target();
    let os = if target.contains("darwin") {
        "darwin"
    } else if target.contains("windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    let file = format!("go{version}.{os}-{arch}.{ext}");
    let archive = tar_gz(&[
        ("go/bin/go", b"#!/bin/sh\n", 0o755),
        ("go/VERSION", b"go\n", 0o644),
    ]);
    let json = format!(
        r#"[{{"version": "go{version}", "stable": true, "files": [{{"filename": "{file}", "kind": "archive", "sha256": "{}"}}]}}]"#,
        sha256_hex(&archive)
    );
    (format!("https://dl.google.com/go/{file}"), archive, json)
}

/// Installs into `<prefix>/golangenv-<version>`.
fn install(
    t: &TestHost,
    w: &World,
    version: &str,
    d: &[&str],
) -> Result<InstallOutcome, LangError> {
    let host = t.host();
    let env_dir = w.prefix.join(format!("golangenv-{version}"));
    let d = deps(d);
    Golang.install(&install_ctx(&host, &w.prefix, version, &env_dir, &d, false))
}

#[test]
fn lang_golang_default_version() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    assert_eq!(&*Golang.default_version(&t.host()).unwrap(), "default");
    let w = World::new();
    w.shim(if cfg!(windows) { "go.exe" } else { "go" });
    let t = w.host(w.isolated_env(&[("PATHEXT", if cfg!(windows) { ".EXE" } else { "" })]));
    if cfg!(unix) {
        assert_eq!(&*Golang.default_version(&t.host()).unwrap(), "system");
    }
}

/// A managed toolchain: downloaded once (verified by the go.dev JSON), shared, linked as `<env>/.go`.
#[cfg(unix)]
#[test]
fn lang_golang_download_is_verified_and_shared() {
    let w = World::new();
    let (url, archive, json) = go_release("1.22.1");
    let mut t = w.host(w.isolated_env(&[("GOBIN", "/user/gobin"), ("GIT_DIR", "/x/.git")]));
    t.fetch = Some(Box::new(FakeFetch {
        texts: vec![("https://go.dev/dl/?mode=json&include=all".to_owned(), json)],
        files: vec![(url.clone(), archive)],
        ..FakeFetch::default()
    }));
    t.runner.reply(
        "go",
        Reply::ok(Vec::new()).sticky().with_effect(|rec| {
            let gopath = rec
                .env_delta
                .iter()
                .find(|(k, _)| k == "GOPATH")
                .unwrap()
                .1
                .clone()
                .unwrap();
            let ro = std::path::Path::new(&gopath).join("pkg/mod/x@v1/f.go");
            if ro.exists() {
                return Ok(());
            }
            std::fs::create_dir_all(ro.parent().unwrap())?;
            std::fs::write(&ro, b"package x")?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444))?;
            std::fs::set_permissions(ro.parent().unwrap(), std::fs::Permissions::from_mode(0o555))?;
            Ok(())
        }),
    );
    let out = install(
        &t,
        &w,
        "1.22.1",
        &["golang.org/x/tools/cmd/goimports@latest"],
    )
    .unwrap();
    assert_eq!(out.tools, vec![(Arc::from("go"), Arc::from("1.22.1"))]);
    let env_dir = w.prefix.join("golangenv-1.22.1");
    let shared = w.repo.join(".snp-tools/tools/go/1.22.1");
    assert!(shared.join("bin/go").is_file());
    assert_eq!(std::fs::read_link(env_dir.join(".go")).unwrap(), shared);
    // The read-only module cache is gone.
    assert!(!env_dir.join("pkg").exists());
    let c = t.runner.golden(&w.subst());
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].argv, ["go", "install", "./..."]);
    assert_eq!(
        c[1].argv,
        ["go", "install", "golang.org/x/tools/cmd/goimports@latest"]
    );
    for call in &c {
        assert_eq!(call.cwd, "{PREFIX}");
        assert_eq!(call.env.get("GOBIN"), Some(&None));
        assert_eq!(call.env.get("GIT_DIR"), Some(&None), "no_git_env");
        assert_eq!(call.env.get("GOTOOLCHAIN"), Some(&Some("local".to_owned())));
        assert_eq!(
            call.env.get("GOPATH"),
            Some(&Some("{PREFIX}/golangenv-1.22.1".to_owned()))
        );
        assert_eq!(
            call.env.get("GOROOT"),
            Some(&Some("{PREFIX}/golangenv-1.22.1/.go".to_owned()))
        );
        assert_eq!(
            call.env.get("PATH"),
            Some(&Some(
                "{PREFIX}/golangenv-1.22.1/.go/bin:{SHIMS}".to_owned()
            ))
        );
    }
    // A second environment reuses the shared toolchain without the network.
    let t2 = w.host(w.isolated_env(&[]));
    let host = t2.host();
    let other = w.root.join("other/golangenv-1.22.1");
    std::fs::create_dir_all(w.root.join("other")).unwrap();
    let d = deps(&[]);
    let out = Golang
        .install(&install_ctx(&host, &w.prefix, "1.22.1", &other, &d, false))
        .unwrap();
    assert_eq!(out.tools.len(), 1);
}

/// `golang.py:96-103`: a version the origin does not list is pre-commit's `ValueError`.
#[test]
fn lang_golang_unknown_version_is_a_value_error() {
    let w = World::new();
    let (_, _, json) = go_release("1.22.1");
    let mut t = w.host(w.isolated_env(&[]));
    t.fetch = Some(Box::new(FakeFetch {
        texts: vec![("https://go.dev/dl/?mode=json&include=all".to_owned(), json)],
        ..FakeFetch::default()
    }));
    let e = install(&t, &w, "1.22", &[]).unwrap_err();
    assert_eq!(e.py_type(), "ValueError");
    let target = snp_net::tools::host_target();
    let os = if target.contains("darwin") {
        "darwin"
    } else if target.contains("windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    assert_eq!(
        e.to_string(),
        format!(
            "Could not find a version matching your system requirements (os={os}; arch={arch})"
        )
    );
    assert_eq!(t.runner.calls().len(), 0);
}

#[test]
fn lang_golang_system_install_and_activation() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[("GOPATH", "/user/go")]));
    install(&t, &w, "system", &[]).unwrap();
    let c = t.runner.golden(&w.subst());
    assert_eq!(c.len(), 1);
    assert_eq!(
        c[0].env.get("GOPATH"),
        Some(&Some("{PREFIX}/golangenv-system".to_owned()))
    );
    assert_eq!(c[0].env.get("GOTOOLCHAIN"), None);
    let host = t.host();
    let env_dir = w.prefix.join("golangenv-system");
    let cx = env_ctx(&host, &w.prefix, "system", &env_dir);
    assert_eq!(Golang.health(&cx, None).unwrap(), Health::Healthy);
    let env = t.env.patched(&Golang.activate(&cx).unwrap());
    // GOPATH is the user's at run time (parity).
    assert_eq!(env.get("GOPATH").unwrap(), "/user/go");
    assert_eq!(env.get("GOROOT"), None);
    let managed = w.prefix.join("golangenv-1.22.1");
    let env = t.env.patched(
        &Golang
            .activate(&env_ctx(&host, &w.prefix, "1.22.1", &managed))
            .unwrap(),
    );
    assert_eq!(env.get("GOROOT").unwrap(), managed.join(".go").as_os_str());
    assert_eq!(env.get("GOTOOLCHAIN").unwrap(), "local");
    assert_eq!(Golang.id(), LanguageId::Golang);
    let _unused = exe;
}
