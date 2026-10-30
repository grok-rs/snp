//! `language: node` (10 §5.10 node row: default version; health after the system node disappears; WP-24: the
//! nodeenv `.node-version`/`[nodeenv]` quirk, shared toolchains, the 4.6.2 package rule, the health fast path).

use std::sync::Arc;

use snp_core::lang::LanguageId;
use snp_lang::harness::{Reply, TestHost};
use snp_lang::langs::node::Node;
use snp_lang::{Health, InstallOutcome, InstallState, LangError, Language};

use crate::support::{FakeFetch, World, deps, env_ctx, exe, install_ctx};

/// node's index.json: the newest release first.
const INDEX: &str = r#"[{"version": "v26.1.0", "lts": false, "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]},
{"version": "v24.11.0", "lts": "Krypton", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]}]"#;

/// Pre-installs the shared node toolchain `version`.
fn node_tool(w: &World, version: &str) {
    let dir = w.repo.join(".snp-tools/tools/node").join(version);
    if cfg!(windows) {
        exe(&dir.join("node.exe"));
    } else {
        exe(&dir.join("bin/node"));
        exe(&dir.join("bin/npm"));
        std::fs::create_dir_all(dir.join("include/node")).unwrap();
    }
}

/// A host whose fetcher serves `index.json`.
fn host(w: &World, extra: &[(&str, &str)]) -> TestHost {
    let mut t = w.host(w.isolated_env(extra));
    t.fetch = Some(Box::new(FakeFetch {
        texts: vec![(
            "https://nodejs.org/dist/index.json".to_owned(),
            INDEX.to_owned(),
        )],
        ..FakeFetch::default()
    }));
    t
}

/// Installs into `<prefix>/node_env-<version>`.
fn install(
    t: &TestHost,
    w: &World,
    version: &str,
    d: &[&str],
) -> Result<InstallOutcome, LangError> {
    let host = t.host();
    let env_dir = w.prefix.join(format!("node_env-{version}"));
    let d = deps(d);
    Node.install(&install_ctx(&host, &w.prefix, version, &env_dir, &d, false))
}

#[test]
fn lang_node_default_version() {
    // node and npm outside $HOME, not shims: system (never on Windows).
    let w = World::new();
    w.shim("node");
    w.shim("npm");
    let t = w.host(w.isolated_env(&[]));
    let want = if cfg!(windows) { "default" } else { "system" };
    assert_eq!(&*Node.default_version(&t.host()).unwrap(), want);
    // Only node: default.
    let w = World::new();
    w.shim("node");
    let t = w.host(w.isolated_env(&[]));
    assert_eq!(&*Node.default_version(&t.host()).unwrap(), "default");
    // Under $HOME (nvm): default, unless SNP_PREFER_SYSTEM_TOOLCHAINS=1.
    let w = World::new();
    let nvm = w.home.join(".nvm/bin");
    exe(&nvm.join("node"));
    exe(&nvm.join("npm"));
    let env = |extra: &[(&str, &str)]| {
        let mut pairs: Vec<(String, String)> = vec![
            ("PATH".into(), nvm.display().to_string()),
            ("HOME".into(), w.home.display().to_string()),
        ];
        pairs.extend(
            extra
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        );
        snp_core::env::Env::from_pairs(pairs)
    };
    let t = w.host(env(&[]));
    assert_eq!(&*Node.default_version(&t.host()).unwrap(), "default");
    let t = w.host(env(&[("SNP_PREFER_SYSTEM_TOOLCHAINS", "1")]));
    assert_eq!(&*Node.default_version(&t.host()).unwrap(), want);
    assert_eq!(t.runner.calls().len(), 0, "no spawn");
}

#[test]
fn lang_node_system_layout_and_npm_install() {
    let w = World::new();
    let t = host(&w, &[]);
    let out = install(&t, &w, "system", &["lodash"]).unwrap();
    assert_eq!(&*out.installer, "npm");
    assert_eq!(out.tools.len(), 0);
    let env_dir = w.prefix.join("node_env-system");
    let (bin, modules) = if cfg!(windows) {
        (
            env_dir.join("Scripts"),
            env_dir.join("Scripts/node_modules"),
        )
    } else {
        (env_dir.join("bin"), env_dir.join("lib/node_modules"))
    };
    assert!(bin.is_dir() && modules.is_dir());
    let c = t.runner.golden(&w.subst());
    assert_eq!(c.len(), 1);
    assert_eq!(
        c[0].argv,
        [
            "npm",
            "install",
            "--allow-git=root",
            "--install-links",
            "-g",
            "git+file://{PREFIX}",
            "lodash"
        ]
    );
    assert_eq!(c[0].cwd, "{PREFIX}");
}

/// 4.6.1+: a repository without `package.json` (every local repository) installs only its dependencies; none is
/// pre-commit's `AssertionError`.
#[test]
fn lang_node_requires_a_package_or_dependencies() {
    let w = World::new();
    std::fs::remove_file(w.prefix.join("package.json")).unwrap();
    let t = host(&w, &[]);
    let e = install(&t, &w, "system", &[]).unwrap_err();
    assert_eq!(e.py_type(), "AssertionError");
    assert_eq!(
        e.to_string(),
        "`language: node` must have package.json or additional_dependencies"
    );
    assert!(!w.prefix.join("node_env-system").exists());
    install(&t, &w, "system", &["prettier@3"]).unwrap();
    let c = t.runner.golden(&w.subst());
    assert_eq!(c[0].argv[5..], ["prettier@3"]);
}

/// The shared toolchain is linked into the environment and reported in `InstallOutcome.tools`; `default` is the
/// newest release, `lts` the newest LTS.
#[test]
fn lang_node_shared_toolchain() {
    for (version, want) in [
        ("default", "26.1.0"),
        ("lts", "24.11.0"),
        ("20.11.1", "20.11.1"),
    ] {
        let w = World::new();
        node_tool(&w, want);
        let t = host(&w, &[]);
        let out = install(&t, &w, version, &[]).unwrap();
        assert_eq!(
            out.tools,
            vec![(Arc::from("node"), Arc::from(want))],
            "{version}"
        );
        let env_dir = w.prefix.join(format!("node_env-{version}"));
        if cfg!(unix) {
            let link = std::fs::read_link(env_dir.join("bin/node")).unwrap();
            assert_eq!(
                link,
                w.repo
                    .join(".snp-tools/tools/node")
                    .join(want)
                    .join("bin/node")
            );
            assert!(std::fs::read_link(env_dir.join("include")).is_ok());
        }
    }
}

/// nodeenv's config files steer `default` (04a Q9): `[nodeenv] node=` of `~/.nodeenvrc` < `setup.cfg` < `tox.ini`,
/// then `.node-version`, all relative to the user's repository.
#[test]
fn lang_node_nodeenv_files_quirk() {
    let w = World::new();
    for v in ["18.0.0", "19.0.0", "20.0.0", "21.0.0"] {
        node_tool(&w, v);
    }
    let t = host(&w, &[]);
    let pick = |t: &TestHost| {
        install(t, &w, "default", &[]).unwrap().tools[0]
            .1
            .to_string()
    };
    std::fs::write(w.home.join(".nodeenvrc"), "[nodeenv]\nnode = 18.0.0\n").unwrap();
    assert_eq!(pick(&t), "18.0.0");
    std::fs::remove_dir_all(w.prefix.join("node_env-default")).unwrap();
    std::fs::write(
        w.repo.join("setup.cfg"),
        "[metadata]\nname = x\n[nodeenv]\nnode = 19.0.0\n",
    )
    .unwrap();
    assert_eq!(pick(&t), "19.0.0");
    std::fs::remove_dir_all(w.prefix.join("node_env-default")).unwrap();
    std::fs::write(w.repo.join("tox.ini"), "[nodeenv]\nnode: 20.0.0\n").unwrap();
    assert_eq!(pick(&t), "20.0.0");
    std::fs::remove_dir_all(w.prefix.join("node_env-default")).unwrap();
    std::fs::write(w.repo.join(".node-version"), "21.0.0\nignored\n").unwrap();
    assert_eq!(pick(&t), "21.0.0");
    // An explicit version is never steered.
    let out = install(&t, &w, "20.0.0", &[]).unwrap();
    assert_eq!(&*out.tools[0].1, "20.0.0");
}

/// No network and no toolchain: snp-net's offline error, nothing left behind by the backend itself.
#[test]
fn lang_node_offline_download_fails() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    let e = install(&t, &w, "default", &[]).unwrap_err();
    assert_eq!(e.py_type(), "NetError");
}

#[test]
fn lang_node_health() {
    let w = World::new();
    let t = host(&w, &[]);
    let out = install(&t, &w, "system", &[]).unwrap();
    let env_dir = w.prefix.join("node_env-system");
    let host_ = t.host();
    let cx = env_ctx(&host_, &w.prefix, "system", &env_dir);
    // `node_test.py:80-91`: the system node behind the environment disappears.
    t.runner.reply("node", Reply::fail(127, ""));
    assert_eq!(
        Node.health(&cx, None).unwrap(),
        Health::Unhealthy("`node --version` returned 127".to_owned())
    );
    assert_eq!(Node.health(&cx, None).unwrap(), Health::Healthy);
    let c = t.runner.golden(&w.subst());
    let last = c.last().unwrap();
    assert_eq!(last.argv, ["node", "--version"]);
    assert_eq!(
        last.env.get("NODE_VIRTUAL_ENV"),
        Some(&Some("{PREFIX}/node_env-system".to_owned()))
    );
    // No `node` at all: no fingerprint; with one, the fast path spawns nothing.
    assert!(out.fingerprint.is_none());
    let node = w.shim("node");
    let fp = t.probe.fingerprint(&node).unwrap();
    let st = InstallState {
        v: InstallState::VERSION,
        language: Arc::from("node"),
        language_version: Arc::from("system"),
        deps: Arc::from(Vec::<Arc<str>>::new()),
        installer: Arc::from("npm"),
        fingerprint: Some(fp),
        snp_version: Arc::from("test"),
    };
    let before = t.runner.calls().len();
    assert_eq!(Node.health(&cx, Some(&st)).unwrap(), Health::Healthy);
    assert_eq!(t.runner.calls().len(), before);
    std::fs::write(&node, b"changed").unwrap();
    assert_eq!(Node.health(&cx, Some(&st)).unwrap(), Health::Healthy);
    assert_eq!(t.runner.calls().len(), before + 1);
}

#[test]
fn lang_node_activation() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[
        ("NPM_CONFIG_USERCONFIG", "/x"),
        ("npm_config_userconfig", "/y"),
    ]));
    let host_ = t.host();
    let env_dir = w.prefix.join("node_env-system");
    let env = t.env.patched(
        &Node
            .activate(&env_ctx(&host_, &w.prefix, "system", &env_dir))
            .unwrap(),
    );
    assert_eq!(env.get("NODE_VIRTUAL_ENV").unwrap(), env_dir.as_os_str());
    assert_eq!(env.get("NPM_CONFIG_USERCONFIG"), None);
    assert_eq!(env.get("npm_config_userconfig"), None);
    if cfg!(unix) {
        assert_eq!(env.get("NPM_CONFIG_PREFIX").unwrap(), env_dir.as_os_str());
        assert_eq!(env.get("npm_config_prefix").unwrap(), env_dir.as_os_str());
        assert_eq!(
            env.get("NODE_PATH").unwrap(),
            env_dir.join("lib/node_modules").as_os_str()
        );
    }
    assert_eq!(Node.id(), LanguageId::Node);
}
