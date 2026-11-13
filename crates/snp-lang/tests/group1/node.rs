//! `language: node` (10 §5.10 node row: default version; health after the system node disappears; WP-24: the
//! nodeenv `.node-version`/`[nodeenv]` quirk, shared toolchains, the 4.6.1+ package rule, the health fast path).

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

/// 4.7.0 (pre-commit#3774, `test_node_hook_install_does_not_write_callers_index`): `npm install` runs with
/// `no_git_env()`, so the `GIT_DIR`/`GIT_INDEX_FILE` git exports to hooks (a commit with `-a`, a submodule) never
/// reach the `git+file://` install; the allow-listed `GIT_*` variables stay.
#[test]
fn lang_node_npm_install_gets_no_git_env() {
    let w = World::new();
    let t = host(
        &w,
        &[
            ("GIT_DIR", "/user/.git"),
            ("GIT_INDEX_FILE", "/user/.git/index"),
            ("GIT_WORK_TREE", "/user"),
            ("GIT_SSH_COMMAND", "ssh -i k"),
        ],
    );
    install(&t, &w, "system", &[]).unwrap();
    let c = t.runner.golden(&w.subst());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].argv[0], "npm");
    for k in ["GIT_DIR", "GIT_INDEX_FILE", "GIT_WORK_TREE"] {
        assert_eq!(c[0].env.get(k), Some(&None), "{k} reaches npm install");
    }
    assert_eq!(c[0].env.get("GIT_SSH_COMMAND"), None, "kept unchanged");
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

/// An index with several releases per major line (the newest first).
const RANGE_INDEX: &str = r#"[{"version": "v21.0.0", "lts": false, "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]},
{"version": "v20.11.1", "lts": "Iron", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]},
{"version": "v20.9.0", "lts": "Iron", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]},
{"version": "v18.19.1", "lts": "Hydrogen", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]},
{"version": "v18.17.0", "lts": "Hydrogen", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl"]}]"#;

/// A host serving [`RANGE_INDEX`] (official and unofficial builds) with every release's toolchain pre-installed.
fn range_world() -> (World, TestHost) {
    let w = World::new();
    for v in ["21.0.0", "20.11.1", "20.9.0", "18.19.1", "18.17.0"] {
        node_tool(&w, v);
    }
    let mut t = w.host(w.isolated_env(&[]));
    t.fetch = Some(Box::new(FakeFetch {
        texts: vec![
            (
                "https://nodejs.org/dist/index.json".to_owned(),
                RANGE_INDEX.to_owned(),
            ),
            (
                "https://unofficial-builds.nodejs.org/download/release/index.json".to_owned(),
                RANGE_INDEX.to_owned(),
            ),
        ],
        ..FakeFetch::default()
    }));
    (w, t)
}

/// nodeenv 1.12.0 resolves partial versions and npm-style ranges to the highest matching release (the values
/// nodeenv's `resolve_node_version` prints for this index); an exact version is used as written.
#[test]
fn lang_node_version_ranges_resolve_like_nodeenv() {
    for (spec, want) in [
        ("20", "20.11.1"),
        ("18.19", "18.19.1"),
        ("^20.1", "20.11.1"),
        (">=18 <20", "18.19.1"),
        ("20.x", "20.11.1"),
        ("v18", "18.19.1"),
        ("~20.9", "20.9.0"),
        ("16 || 18.17", "18.17.0"),
        ("20.9.0", "20.9.0"),
    ] {
        let (w, t) = range_world();
        let out = install(&t, &w, spec, &[]).unwrap_or_else(|e| panic!("{spec}: {e}"));
        assert_eq!(
            out.tools,
            vec![(Arc::from("node"), Arc::from(want))],
            "{spec}"
        );
    }
    // No release matches: nodeenv's message, exit 1.
    let (w, t) = range_world();
    let e = install(&t, &w, "19", &[]).unwrap_err();
    assert_eq!(e.py_type(), "CalledProcessError");
    let LangError::CalledProcess { rc, stderr, .. } = &e else {
        panic!("{e:?}")
    };
    assert_eq!(*rc, 1);
    assert_eq!(
        String::from_utf8_lossy(stderr),
        "No available node.js version matches '19'\n"
    );
}

/// `.node-version` is read like nodeenv: the first line stripped and without leading `v`s, and a range resolves;
/// an empty first line resets the choice to `latest` even when `setup.cfg` names a version.
#[test]
fn lang_node_node_version_file_values() {
    let (w, t) = range_world();
    std::fs::write(w.repo.join(".node-version"), "v20\n").unwrap();
    let out = install(&t, &w, "default", &[]).unwrap();
    assert_eq!(&*out.tools[0].1, "20.11.1");
    let (w, t) = range_world();
    std::fs::write(w.repo.join("setup.cfg"), "[nodeenv]\nnode = 18.17.0\n").unwrap();
    std::fs::write(w.repo.join(".node-version"), "\n20\n").unwrap();
    let out = install(&t, &w, "default", &[]).unwrap();
    assert_eq!(&*out.tools[0].1, "21.0.0", "latest");
    // `[DEFAULT]` options apply to `[nodeenv]` (configparser).
    let (w, t) = range_world();
    std::fs::write(
        w.repo.join("tox.ini"),
        "[DEFAULT]\nnode = 18.19\n[nodeenv]\njobs = 4\n",
    )
    .unwrap();
    let out = install(&t, &w, "default", &[]).unwrap();
    assert_eq!(&*out.tools[0].1, "18.19.1");
}

/// nodeenv 1.12.0 reads a `[nodeenv]` value with `ConfigParser.get` and drops one pair of matching outer quotes
/// (nodeenv#321): `"18.17.0"` and `'20'` work, `%(name)s` interpolates; mismatched quotes stay, and a reference
/// nodeenv cannot resolve (it crashes) fails the install.
#[test]
fn lang_node_nodeenv_config_values() {
    let pick = |cfg: &str| {
        let (w, t) = range_world();
        std::fs::write(w.repo.join("setup.cfg"), cfg).unwrap();
        install(&t, &w, "default", &[]).map(|o| o.tools[0].1.to_string())
    };
    assert_eq!(pick("[nodeenv]\nnode = \"18.17.0\"\n").unwrap(), "18.17.0");
    assert_eq!(pick("[nodeenv]\nnode = '20'\n").unwrap(), "20.11.1");
    assert_eq!(
        pick("[DEFAULT]\nmajor = 18\n[nodeenv]\nnode = \"%(major)s.19\"\n").unwrap(),
        "18.19.1"
    );
    assert!(pick("[nodeenv]\nnode = \"18.17.0'\n").is_err());
    assert!(pick("[nodeenv]\nnode = %(nope)s\n").is_err());
}

/// A `language_version` or `.node-version` that is not a plain version never selects a directory outside the
/// shared tools, even when that directory exists.
#[test]
fn lang_node_version_is_not_a_path() {
    let (w, t) = range_world();
    let outside = w.root.join("usr");
    exe(&outside.join("bin/node"));
    let rel = "../../../../usr";
    assert!(w.repo.join(".snp-tools/tools/node").join(rel).is_dir());
    std::fs::write(w.repo.join(".node-version"), format!("{rel}\n")).unwrap();
    let e = install(&t, &w, "default", &[]).unwrap_err();
    assert!(e.to_string().contains("invalid"), "{e}");
    assert!(!w.prefix.join("node_env-default/bin/node").exists());
}
