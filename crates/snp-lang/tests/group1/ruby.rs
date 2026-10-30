//! `language: ruby` (10 §5.10 ruby row: default `system` iff ruby and gem are found, the vendored archives; WP-24:
//! embedded rbenv/ruby-build, `rbenv download` → `rbenv install` fallback, sorted gemspecs).

use snp_core::lang::LanguageId;
use snp_lang::harness::{Reply, TestHost};
use snp_lang::langs::ruby::Ruby;
use snp_lang::{Health, InstallOutcome, LangError, Language};

use crate::support::{World, deps, env_ctx, install_ctx};

/// Installs into `<prefix>/rbenv-<version>`.
fn install(
    t: &TestHost,
    w: &World,
    version: &str,
    d: &[&str],
) -> Result<InstallOutcome, LangError> {
    let host = t.host();
    let env_dir = w.prefix.join(format!("rbenv-{version}"));
    let d = deps(d);
    Ruby.install(&install_ctx(&host, &w.prefix, version, &env_dir, &d, false))
}

#[test]
fn lang_ruby_default_version() {
    let w = World::new();
    w.shim("ruby");
    let t = w.host(w.isolated_env(&[]));
    assert_eq!(&*Ruby.default_version(&t.host()).unwrap(), "default");
    let w = World::new();
    w.shim("ruby");
    w.shim("gem");
    let t = w.host(w.isolated_env(&[]));
    if cfg!(unix) {
        assert_eq!(&*Ruby.default_version(&t.host()).unwrap(), "system");
    }
    // rbenv shims are not system.
    let w = World::new();
    let shims = w.root.join("rbenv/shims");
    crate::support::exe(&shims.join("ruby"));
    crate::support::exe(&shims.join("gem"));
    let t = w.host(snp_core::env::Env::from_pairs([
        ("PATH", shims.as_os_str()),
        ("HOME", w.home.as_os_str()),
    ]));
    assert_eq!(&*Ruby.default_version(&t.host()).unwrap(), "default");
}

#[test]
fn lang_ruby_system_gems_sorted() {
    let w = World::new();
    std::fs::write(w.prefix.join("a.gemspec"), "x").unwrap();
    let t = w.host(w.isolated_env(&[("GEM_PATH", "/user/gems")]));
    t.runner.reply(
        "gem",
        Reply::ok(Vec::new())
            .when(["build"])
            .sticky()
            .with_effect(|rec| {
                std::fs::write(rec.cwd.join("zz-1.gem"), b"")?;
                std::fs::write(rec.cwd.join("aa-1.gem"), b"")
            }),
    );
    let out = install(&t, &w, "system", &["rake"]).unwrap();
    assert_eq!(&*out.installer, "gem");
    let c = t.runner.golden(&w.subst());
    assert_eq!(c[0].argv, ["gem", "build", "a.gemspec", "hooks.gemspec"]);
    assert_eq!(
        c[1].argv,
        [
            "gem",
            "install",
            "--no-document",
            "--no-format-executable",
            "--no-user-install",
            "--install-dir",
            "{PREFIX}/rbenv-system/gems",
            "--bindir",
            "{PREFIX}/rbenv-system/gems/bin",
            "aa-1.gem",
            "zz-1.gem",
            "rake"
        ]
    );
    assert_eq!(c[1].env.get("GEM_PATH"), Some(&None));
    assert_eq!(
        c[1].env.get("BUNDLE_IGNORE_CONFIG"),
        Some(&Some("1".to_owned()))
    );
}

/// The embedded rbenv is extracted into the environment (`default`: no plugins); a version adds ruby-download and
/// ruby-build, and a failing `rbenv download` falls back to `rbenv install`.
#[cfg(unix)]
#[test]
fn lang_ruby_rbenv_and_plugins() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    install(&t, &w, "default", &[]).unwrap();
    let env_dir = w.prefix.join("rbenv-default");
    assert!(env_dir.join("bin/rbenv").exists() || env_dir.join("libexec/rbenv").exists());
    assert!(!env_dir.join("plugins/ruby-build").exists());
    let a: Vec<Vec<String>> = t
        .runner
        .golden(&w.subst())
        .into_iter()
        .map(|c| c.argv)
        .collect();
    assert_eq!(a[0], ["rbenv", "init", "-"]);
    assert_eq!(a[1], ["rbenv", "rehash"]);

    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    t.runner
        .reply("rbenv", Reply::fail(1, "no binary").when(["download"]));
    install(&t, &w, "3.2.2", &[]).unwrap();
    let env_dir = w.prefix.join("rbenv-3.2.2");
    assert!(env_dir.join("plugins/ruby-build/bin/ruby-build").exists());
    assert!(env_dir.join("plugins/ruby-download").is_dir());
    let a: Vec<Vec<String>> = t
        .runner
        .golden(&w.subst())
        .into_iter()
        .map(|c| c.argv)
        .collect();
    assert_eq!(
        a[..4],
        [
            vec!["rbenv".to_owned(), "init".to_owned(), "-".to_owned()],
            vec![
                "rbenv".to_owned(),
                "download".to_owned(),
                "3.2.2".to_owned()
            ],
            vec!["rbenv".to_owned(), "install".to_owned(), "3.2.2".to_owned()],
            vec!["rbenv".to_owned(), "rehash".to_owned()],
        ]
    );
}

#[test]
fn lang_ruby_activation() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    let host = t.host();
    for (version, rbenv_root, rbenv_version) in [
        ("system", false, None),
        ("default", true, None),
        ("3.2.2", true, Some("3.2.2")),
    ] {
        let env_dir = w.prefix.join(format!("rbenv-{version}"));
        let cx = env_ctx(&host, &w.prefix, version, &env_dir);
        assert_eq!(Ruby.health(&cx, None).unwrap(), Health::Healthy);
        let env = t.env.patched(&Ruby.activate(&cx).unwrap());
        assert_eq!(
            env.get("GEM_HOME").unwrap(),
            env_dir.join("gems").as_os_str()
        );
        assert_eq!(env.get("BUNDLE_IGNORE_CONFIG").unwrap(), "1");
        assert_eq!(env.get("RBENV_ROOT").is_some(), rbenv_root, "{version}");
        assert_eq!(
            env.get("RBENV_VERSION")
                .map(|v| v.to_string_lossy().into_owned())
                .as_deref(),
            rbenv_version
        );
    }
    assert_eq!(Ruby.id(), LanguageId::Ruby);
}
