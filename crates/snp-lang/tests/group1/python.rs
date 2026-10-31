//! `language: python` (10 §5.10 python row: default-version matrix, health checks; WP-24 acceptance: uv command lines,
//! the pip-config translation and every fallback trigger, `SNP_PYTHON_INSTALLER`, the health fast path, the pyenv
//! shim probe rule, `InstallOutcome.tools`).

use std::path::PathBuf;
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::harness::{GoldenCall, Reply, TestHost};
use snp_lang::langs::python::Python;
use snp_lang::{Health, InstallOutcome, InstallState, LangError, Language, ToolProbe};

use crate::support::{Eff, R, World, deps, env_ctx, exe, install_ctx, r, script};

/// The `version_info` the fake interpreters report.
const VI: &str = "3.12.3.final.0";

/// A world with a fake `python3` reference interpreter; `extra` goes into the (hermetic) environment.
fn setup(w: &World, extra: &[(&str, &str)]) -> TestHost {
    setup_with(w, extra, Vec::new())
}

/// [`setup`] with `first` scripted before everything else (replies match in insertion order).
fn setup_with(w: &World, extra: &[(&str, &str)], first: Vec<(&str, Reply)>) -> TestHost {
    let py3 = w.shim("python3");
    let t = w.host(w.isolated_env(extra));
    for (tool, reply) in first {
        t.runner.reply(tool, reply);
    }
    t.runner.reply(
        "python3",
        Reply::ok(format!("cpython\n{VI}\n{}\n", py3.display()))
            .when(["-S", "-c"])
            .sticky(),
    );
    script(&t, &creators("version_info = 3.12.3.final.0\n"));
    t
}

/// Every environment interpreter answers `version_info` (scripted after the test's own replies).
fn ok_version(t: &TestHost) {
    t.runner.reply(
        "python",
        Reply::ok(format!("{VI}\n")).when(["-S", "-c"]).sticky(),
    );
}

/// Fake environment creators (`python3 -m venv`, `uv venv`) writing `pyvenv`.
fn creators(pyvenv: &'static str) -> Vec<R> {
    vec![
        R {
            effects: vec![
                Eff::Exe("{arg3}/bin/python"),
                Eff::Write("{arg3}/pyvenv.cfg", pyvenv),
            ],
            ..r("python3", &["-m", "venv"], 0, "")
        },
        R {
            effects: vec![
                Eff::Exe("{arg2}/bin/python"),
                Eff::Write(
                    "{arg2}/pyvenv.cfg",
                    "version_info = 3.12.3\nuv = 0.8.8\nseed = true\n",
                ),
            ],
            ..r("uv", &["venv"], 0, "")
        },
    ]
}

/// The calls so far, with the world's placeholders.
fn calls(t: &TestHost, w: &World) -> Vec<GoldenCall> {
    t.runner.golden(&w.subst())
}

/// argv of the calls.
fn argvs(t: &TestHost, w: &World) -> Vec<Vec<String>> {
    calls(t, w).into_iter().map(|c| c.argv).collect()
}

/// Installs `language_version: version` with `deps` into `<prefix>/py_env-<version>`.
fn install(
    t: &TestHost,
    w: &World,
    version: &str,
    d: &[&str],
    local: bool,
) -> Result<InstallOutcome, LangError> {
    let host = t.host();
    let env_dir = w.prefix.join(format!("py_env-{version}"));
    let d = deps(d);
    Python.install(&install_ctx(&host, &w.prefix, version, &env_dir, &d, local))
}

/// The state snp-store would commit for `out`.
fn state(out: &InstallOutcome) -> InstallState {
    InstallState {
        v: InstallState::VERSION,
        language: Arc::from("python"),
        language_version: Arc::from("python3.12"),
        deps: Arc::from(Vec::<Arc<str>>::new()),
        installer: Arc::clone(&out.installer),
        fingerprint: out.fingerprint.clone(),
        snp_version: Arc::from("test"),
    }
}

/// `health` of `<prefix>/py_env-<version>`.
fn health(t: &TestHost, w: &World, version: &str, recorded: Option<&InstallState>) -> Health {
    let host = t.host();
    let env_dir = w.prefix.join(format!("py_env-{version}"));
    Python
        .health(&env_ctx(&host, &w.prefix, version, &env_dir), recorded)
        .unwrap()
}

#[test]
fn lang_python_default_version_from_the_reference_interpreter() {
    let w = World::new();
    w.shim("python3.12");
    let t = setup(&w, &[]);
    assert_eq!(&*Python.default_version(&t.host()).unwrap(), "python3.12");
    // Memoised: one probe per process.
    assert_eq!(&*Python.default_version(&t.host()).unwrap(), "python3.12");
    assert_eq!(argvs(&t, &w).len(), 1);
    assert_eq!(argvs(&t, &w)[0][..3], ["python3", "-S", "-c"]);
}

#[test]
fn lang_python_default_version_falls_back_to_the_major_and_to_default() {
    let w = World::new();
    w.shim("python3");
    let t = setup(&w, &[]);
    // `python3` exists (the reference itself): `python{X}`.
    assert_eq!(&*Python.default_version(&t.host()).unwrap(), "python3");
    let w = World::new();
    let t = w.host(w.isolated_env(&[]));
    assert_eq!(&*Python.default_version(&t.host()).unwrap(), "default");
    assert_eq!(t.runner.calls().len(), 0);
}

/// ADR-079: a `#!` interpreter (a pyenv shim) is probed in every process; a native one is persisted.
#[cfg(unix)]
#[test]
fn lang_python_pyenv_shim_is_probed_every_process() {
    for (head, persisted) in [
        (&b"#!/bin/sh\nexec python3.12 \"$@\"\n"[..], false),
        (&b"\x7fELF\x02"[..], true),
    ] {
        let w = World::new();
        let bin = w.root.join("bin");
        let py3 = bin.join("python3");
        exe(&py3);
        std::fs::write(&py3, head).unwrap();
        exe(&bin.join("python3.12"));
        let probes = w.root.join("probes");
        let env = Env::from_pairs([("PATH", bin.as_os_str()), ("HOME", w.home.as_os_str())]);
        let mut spawns = Vec::new();
        for _ in 0..2 {
            let mut t = w.host(env.clone());
            t.probe = ToolProbe::new(Some(probes.clone()));
            t.runner.reply(
                "python3",
                Reply::ok(format!("cpython\n{VI}\n{}\n", py3.display())).sticky(),
            );
            assert_eq!(&*Python.default_version(&t.host()).unwrap(), "python3.12");
            spawns.push(t.runner.calls().len());
        }
        assert_eq!(
            spawns,
            if persisted { vec![1, 0] } else { vec![1, 1] },
            "{head:?}"
        );
    }
}

#[test]
fn lang_python_pip_installer_builds_with_venv_and_pip() {
    let w = World::new();
    let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "pip")]);
    let out = install(&t, &w, "python3.12", &["six", "--user"], false).unwrap();
    assert_eq!(&*out.installer, "pip");
    assert_eq!(out.tools.len(), 0);
    let c = calls(&t, &w);
    assert_eq!(
        c[1].argv,
        ["python3", "-m", "venv", "{PREFIX}/py_env-python3.12"]
    );
    assert_eq!(c[1].cwd, "/");
    assert_eq!(
        c[2].argv,
        ["python", "-mpip", "install", ".", "six", "--user"]
    );
    assert_eq!(c[2].cwd, "{PREFIX}");
    assert_eq!(
        c[2].env.get("VIRTUAL_ENV"),
        Some(&Some("{PREFIX}/py_env-python3.12".to_owned()))
    );
    assert_eq!(
        c[2].env.get("PATH"),
        Some(&Some("{PREFIX}/py_env-python3.12/bin:{SHIMS}".to_owned()))
    );
    assert_eq!(c.len(), 3, "{c:?}");
    // The fingerprint is the reference interpreter's, with the recorded version.
    let fp = out.fingerprint.unwrap();
    assert_eq!(fp.exe, w.shims.join("python3"));
    assert_eq!(&*fp.detail, VI);
}

/// A uv world: `$SNP_UV` points at a fake uv.
fn uv_setup(w: &World, extra: &[(&str, &str)]) -> TestHost {
    let uv = w.shim("uv");
    let mut env: Vec<(&str, &str)> = extra.to_vec();
    let uv = uv.display().to_string();
    env.push(("SNP_UV", &uv));
    setup(w, &env)
}

#[test]
fn lang_python_uv_command_lines() {
    let w = World::new();
    let t = uv_setup(
        &w,
        &[
            ("UV_PYTHON", "3.9"),
            ("UV_CONSTRAINT", "/c.txt"),
            ("UV_INDEX", "https://keep/simple"),
            ("VIRTUAL_ENV", "/somewhere"),
            (
                "__PYVENV_LAUNCHER__",
                "/Library/Frameworks/Python.framework/python",
            ),
            ("PYTHONEXECUTABLE", "/usr/local/bin/python3"),
        ],
    );
    let out = install(&t, &w, "python3.12", &["six", "--pre"], false).unwrap();
    assert_eq!(&*out.installer, "uv");
    let c = calls(&t, &w);
    assert_eq!(
        c[1].argv,
        [
            "uv",
            "venv",
            "{PREFIX}/py_env-python3.12",
            "--python",
            "{SHIMS}/python3",
            "--seed",
            "--no-project",
            "--no-config",
            "--no-python-downloads"
        ]
    );
    assert_eq!(c[1].cwd, "/");
    let venv_env = &c[1].env;
    assert_eq!(venv_env.get("UV_PYTHON"), Some(&None));
    assert_eq!(venv_env.get("UV_CONSTRAINT"), Some(&None));
    assert_eq!(venv_env.get("VIRTUAL_ENV"), Some(&None));
    assert_eq!(venv_env.get("__PYVENV_LAUNCHER__"), Some(&None));
    assert_eq!(venv_env.get("PYTHONEXECUTABLE"), Some(&None));
    assert_eq!(venv_env.get("UV_INDEX"), None, "kept unchanged");
    assert_eq!(
        venv_env.get("UV_CACHE_DIR"),
        Some(&Some("{TOOLS}/cache/uv".to_owned()))
    );
    assert_eq!(
        c[2].argv,
        [
            "uv",
            "pip",
            "install",
            "--python",
            "{PREFIX}/py_env-python3.12/bin/python",
            "--no-config",
            ".",
            "six",
            "--pre"
        ]
    );
    assert_eq!(c[2].cwd, "{PREFIX}");
    assert_eq!(
        c[2].env.get("VIRTUAL_ENV"),
        Some(&Some("{PREFIX}/py_env-python3.12".to_owned()))
    );
    assert_eq!(
        c[2].env.get("PIP_DISABLE_PIP_VERSION_CHECK"),
        Some(&Some("1".to_owned()))
    );
    assert_eq!(c.len(), 3);
}

#[test]
fn lang_python_uv_keeps_the_users_cache() {
    let w = World::new();
    let t = uv_setup(&w, &[("UV_CACHE_DIR", "/mine")]);
    install(&t, &w, "python3.12", &[], false).unwrap();
    for c in calls(&t, &w).iter().filter(|c| c.argv[0] == "uv") {
        assert_eq!(c.env.get("UV_CACHE_DIR"), None, "{c:?}");
    }
}

#[test]
fn lang_python_pip_configuration_is_translated() {
    let w = World::new();
    let conf = w.root.join("pip.conf");
    std::fs::write(
        &conf,
        "[global]\nindex-url = https://corp/simple\ntrusted-host = corp\n[install]\nextra-index-url =\n    https://extra/simple\n",
    )
    .unwrap();
    let conf = conf.display().to_string();
    let t = uv_setup(
        &w,
        &[
            ("PIP_CONFIG_FILE", &conf),
            ("PIP_PRE", "1"),
            ("PIP_CERT", "/ca.pem"),
            ("XDG_CONFIG_DIRS", "/nonexistent"),
        ],
    );
    install(&t, &w, "python3.12", &[], false).unwrap();
    let c = calls(&t, &w);
    let pip = c.iter().find(|c| c.argv[..2] == ["uv", "pip"]).unwrap();
    assert_eq!(
        pip.argv[6..],
        [
            "--default-index",
            "https://corp/simple",
            "--allow-insecure-host",
            "corp",
            "--index",
            "https://extra/simple",
            "--prerelease",
            "allow",
            "--index-strategy",
            "unsafe-best-match",
            "."
        ]
    );
    assert_eq!(
        pip.env.get("SSL_CERT_FILE"),
        Some(&Some("/ca.pem".to_owned()))
    );
}

/// Every fallback trigger selects the pip backend under `auto` (and fails under `uv`).
#[test]
fn lang_python_fallback_triggers_select_pip() {
    type Case = (
        &'static str,
        fn(&World) -> Vec<(String, String)>,
        &'static [&'static str],
        bool,
    );
    let conf_target = |w: &World| {
        let p = w.root.join("pip.conf");
        std::fs::write(&p, "[install]\ntarget = /elsewhere\n").unwrap();
        p.display().to_string()
    };
    let cases: [Case; 5] = [
        (
            "PIP_USE_FEATURE",
            |_| vec![("PIP_USE_FEATURE".into(), "truststore".into())],
            &[],
            true,
        ),
        (
            "pip.conf target",
            |_| vec![("PIP_CONFIG_FILE".into(), String::new())],
            &[],
            true,
        ),
        ("dash dependency", |_| Vec::new(), &["--user"], true),
        (
            "bad boolean",
            |_| vec![("PIP_NO_INDEX".into(), "maybe".into())],
            &[],
            true,
        ),
        ("no uv at all", |_| Vec::new(), &[], false),
    ];
    for (name, extra, d, with_uv) in cases {
        for mode in ["auto", "uv"] {
            let w = World::new();
            let mut env: Vec<(String, String)> = extra(&w);
            for (k, v) in &mut env {
                if k == "PIP_CONFIG_FILE" {
                    *v = conf_target(&w);
                }
            }
            env.push(("SNP_PYTHON_INSTALLER".into(), mode.into()));
            env.push(("XDG_CONFIG_DIRS".into(), "/nonexistent".into()));
            let pairs: Vec<(&str, &str)> =
                env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let t = if with_uv {
                uv_setup(&w, &pairs)
            } else {
                setup(&w, &pairs)
            };
            let got = install(&t, &w, "python3.12", d, false);
            let a = argvs(&t, &w);
            if mode == "auto" {
                let out = got.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(&*out.installer, "pip:fallback", "{name}");
                assert!(
                    a.iter().any(|c| c[..3] == ["python3", "-m", "venv"]),
                    "{name}: {a:?}"
                );
                assert!(
                    !a.iter()
                        .any(|c| c[0] == "uv" && c.get(1).is_some_and(|s| s != "--version")),
                    "{name}: {a:?}"
                );
            } else {
                let e = got.unwrap_err();
                assert!(matches!(e, LangError::Fatal(_)), "{name}: {e:?}");
                assert!(
                    e.to_string().starts_with("SNP_PYTHON_INSTALLER=uv: "),
                    "{e}"
                );
                assert!(!w.prefix.join("py_env-python3.12").exists(), "{name}");
            }
        }
    }
}

#[test]
fn lang_python_installer_value_is_validated() {
    let w = World::new();
    let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "conda")]);
    let e = install(&t, &w, "python3.12", &[], false).unwrap_err();
    assert_eq!(
        e.to_string(),
        "SNP_PYTHON_INSTALLER must be one of `auto`, `uv`, `pip` (got 'conda')"
    );
    assert_eq!(e.py_type(), "FatalError");
}

/// D-PERF-02: a local hook without dependencies installs nothing; with dependencies uv installs only them.
#[test]
fn lang_python_local_placeholder_is_skipped() {
    for mode in ["pip", "uv"] {
        let w = World::new();
        let t = uv_setup(&w, &[("SNP_PYTHON_INSTALLER", mode)]);
        install(&t, &w, "python3.12", &[], true).unwrap();
        let a = argvs(&t, &w);
        assert!(
            !a.iter().any(|c| c.contains(&"install".to_owned())),
            "{mode}: {a:?}"
        );
    }
    let w = World::new();
    let t = uv_setup(&w, &[]);
    install(&t, &w, "python3.12", &["six"], true).unwrap();
    let a = argvs(&t, &w);
    assert_eq!(a.last().unwrap()[6..], ["six"]);
    // The pip backend keeps pre-commit's `.`.
    let w = World::new();
    let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "pip")]);
    install(&t, &w, "python3.12", &["six"], true).unwrap();
    assert_eq!(
        argvs(&t, &w).last().unwrap()[1..],
        ["-mpip", "install", ".", "six"]
    );
}

/// D-CRASH-19: an explicitly requested interpreter that is not installed is downloaded by uv into the shared tools,
/// unless `SNP_PYTHON_DOWNLOADS=never`.
#[test]
fn lang_python_missing_interpreter_is_downloaded() {
    let w = World::new();
    let t = uv_setup(&w, &[]);
    let managed = w.root.join("managed/bin/python3.11");
    t.runner.reply(
        "uv",
        Reply::fail(2, "error: No interpreter found").when([
            "python",
            "find",
            "--no-project",
            "--no-config",
            "--no-python-downloads",
            "3.11",
        ]),
    );
    t.runner.reply(
        "uv",
        Reply::ok(format!("{}\n", managed.display())).when([
            "python",
            "find",
            "--no-project",
            "--no-config",
            "--no-python-downloads",
            "--managed-python",
            "3.11",
        ]),
    );
    let out = install(&t, &w, "python3.11", &[], false).unwrap();
    let c = calls(&t, &w);
    let install_call = c
        .iter()
        .find(|c| c.argv[..3] == ["uv", "python", "install"])
        .unwrap();
    assert_eq!(
        install_call.argv,
        [
            "uv",
            "python",
            "install",
            "--no-config",
            "--no-bin",
            "--install-dir",
            "{TOOLS}/tools/python/uv",
            "3.11"
        ]
    );
    let venv = c.iter().find(|c| c.argv[..2] == ["uv", "venv"]).unwrap();
    assert_eq!(venv.argv[4], managed.display().to_string());
    assert!(out.tools.contains(&(Arc::from("python"), Arc::from("uv"))));

    let w = World::new();
    let t = uv_setup(&w, &[("SNP_PYTHON_DOWNLOADS", "never")]);
    t.runner.reply(
        "uv",
        Reply::fail(2, "error: No interpreter found").when(["python", "find"]),
    );
    let e = install(&t, &w, "python3.11", &[], false).unwrap_err();
    assert!(matches!(e, LangError::CalledProcess { rc: 2, .. }), "{e:?}");
    assert!(
        !argvs(&t, &w)
            .iter()
            .any(|c| c[..3] == ["uv", "python", "install"])
    );
}

/// A CPython snp downloaded earlier is found by `uv python find --managed-python` in the shared tools: no lock, no
/// `uv python install`.
#[test]
fn lang_python_downloaded_interpreter_is_reused() {
    let w = World::new();
    std::fs::create_dir_all(w.repo.join(".snp-tools/tools/python/uv")).unwrap();
    let t = uv_setup(&w, &[]);
    let managed = w.root.join("managed/bin/python3.11");
    t.runner.reply(
        "uv",
        Reply::fail(2, "error: No interpreter found").when([
            "python",
            "find",
            "--no-project",
            "--no-config",
            "--no-python-downloads",
            "3.11",
        ]),
    );
    t.runner.reply(
        "uv",
        Reply::ok(format!("{}\n", managed.display())).when([
            "python",
            "find",
            "--no-project",
            "--no-config",
            "--no-python-downloads",
            "--managed-python",
            "3.11",
        ]),
    );
    let out = install(&t, &w, "python3.11", &[], false).unwrap();
    let c = calls(&t, &w);
    let finds: Vec<&GoldenCall> = c
        .iter()
        .filter(|c| c.argv[..3] == ["uv", "python", "find"])
        .collect();
    assert_eq!(finds.len(), 2, "{c:?}");
    assert_eq!(
        finds[1].env.get("UV_PYTHON_INSTALL_DIR"),
        Some(&Some("{TOOLS}/tools/python/uv".to_owned()))
    );
    assert!(
        !c.iter().any(|c| c.argv[..3] == ["uv", "python", "install"]),
        "{c:?}"
    );
    let venv = c.iter().find(|c| c.argv[..2] == ["uv", "venv"]).unwrap();
    assert_eq!(venv.argv[4], managed.display().to_string());
    assert!(out.tools.contains(&(Arc::from("python"), Arc::from("uv"))));
}

/// `language_version: system` is not special for python: pre-commit hands `-p system` to virtualenv, which fails;
/// snp finds no interpreter named `system` and fails the same way (exit 3), creating nothing.
#[test]
fn lang_python_system_is_not_an_interpreter() {
    let w = World::new();
    let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "pip")]);
    let e = install(&t, &w, "system", &[], false).unwrap_err();
    assert_eq!(
        e.to_string(),
        "failed to find interpreter for python_spec='system'"
    );
    assert!(
        !argvs(&t, &w)
            .iter()
            .any(|c| c.get(1).is_some_and(|a| a == "-m"))
    );
    assert!(!w.prefix.join("py_env-system").exists());
}

/// A layering case: name, user file, site file, variables, expected `uv pip install` flags.
#[cfg(not(any(windows, target_os = "macos")))]
type LayerCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// pip's configuration layering pinned on the `uv pip install` argv (pip 26 `_get_ordered_configuration_items`):
/// (a) a site file's `[install]` beats a user file's `[global]`; (b) an existing `$PIP_CONFIG_FILE` skips the user
/// files; (c) empty values are ignored; (d) only upper-case `PIP_*` names count, `PIP_VERSION` is not an option;
/// (e) a leading `--` of a key is dropped. Indented options are options (configparser), not continuations.
#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn lang_python_pip_configuration_layering() {
    if std::path::Path::new("/etc/pip.conf").exists() {
        return;
    }
    let cases: [LayerCase; 6] = [
        (
            "a",
            "[global]\nindex-url = https://user-global/simple\n",
            "[install]\nindex-url = https://site-install/simple\n",
            &[],
            &["--default-index", "https://site-install/simple"],
        ),
        (
            "b",
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_CONFIG_FILE", "{XDG}/pip/pip.conf")],
            &[],
        ),
        (
            "c",
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_INDEX_URL", "")],
            &["--default-index", "https://user/simple"],
        ),
        (
            "d",
            "",
            "",
            &[
                ("pip_index_url", "https://lower/simple"),
                ("PIP_VERSION", "1"),
            ],
            &[],
        ),
        (
            "e",
            "[global]\n--index_url = https://dash/simple\n",
            "",
            &[],
            &["--default-index", "https://dash/simple"],
        ),
        (
            "indented",
            "[global]\n  index-url = https://corp/simple\n  trusted-host = corp\n",
            "",
            &[],
            &[
                "--default-index",
                "https://corp/simple",
                "--allow-insecure-host",
                "corp",
            ],
        ),
    ];
    for (name, user, site, extra, want) in cases {
        let w = World::new();
        let xdg = w.root.join("xdg");
        std::fs::create_dir_all(xdg.join("pip")).unwrap();
        std::fs::create_dir_all(w.home.join(".config/pip")).unwrap();
        std::fs::write(xdg.join("pip/pip.conf"), site).unwrap();
        std::fs::write(w.home.join(".config/pip/pip.conf"), user).unwrap();
        let xdg_s = xdg.display().to_string();
        let mut env: Vec<(String, String)> = vec![("XDG_CONFIG_DIRS".into(), xdg_s.clone())];
        for (k, v) in extra {
            env.push(((*k).to_owned(), v.replace("{XDG}", &xdg_s)));
        }
        env.push(("SNP_PYTHON_INSTALLER".into(), "uv".into()));
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let t = uv_setup(&w, &pairs);
        let out =
            install(&t, &w, "python3.12", &[], false).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&*out.installer, "uv", "{name}");
        let c = calls(&t, &w);
        let pip = c.iter().find(|c| c.argv[..2] == ["uv", "pip"]).unwrap();
        let mut expect: Vec<&str> = want.to_vec();
        expect.push(".");
        assert_eq!(pip.argv[6..], expect, "{name}");
    }
}

/// The pip backend has no discovery: an unknown interpreter is pre-commit's failure (exit 3).
#[test]
fn lang_python_pip_backend_without_interpreter() {
    let w = World::new();
    let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "pip")]);
    let e = install(&t, &w, "python3.99", &[], false).unwrap_err();
    assert_eq!(
        e.to_string(),
        "failed to find interpreter for python_spec='python3.99'"
    );
    assert_eq!(e.py_type(), "ValueError");
    // A path (with `~`) and a bare version are found.
    let w = World::new();
    let custom = w.home.join("pythons/py");
    exe(&custom);
    w.shim("python3.11");
    let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "pip")]);
    install(&t, &w, "~/pythons/py", &[], false).unwrap();
    install(&t, &w, "3.11", &[], false).unwrap();
    let a = argvs(&t, &w);
    let venvs: Vec<&Vec<String>> = a
        .iter()
        .filter(|c| c.get(2).is_some_and(|s| s == "venv"))
        .collect();
    assert_eq!(venvs[0][0], "py");
    assert_eq!(venvs[1][0], "python3.11");
}

/// uv on `PATH`: used when new enough and its `uv venv` knows snp's flags (probed once per binary).
#[test]
fn lang_python_path_uv_is_probed_once() {
    let w = World::new();
    w.shim("uv");
    let t = setup_with(
        &w,
        &[],
        vec![
            (
                "uv",
                Reply::ok("uv 0.8.8 (x 2025-08-01)\n")
                    .when(["--version"])
                    .sticky(),
            ),
            (
                "uv",
                Reply::ok("--seed --no-project --no-config --no-python-downloads\n")
                    .when(["venv", "--help"])
                    .sticky(),
            ),
        ],
    );
    install(&t, &w, "python3.12", &[], false).unwrap();
    install(&t, &w, "python3.12", &[], false).unwrap();
    let a = argvs(&t, &w);
    assert_eq!(a.iter().filter(|c| c[1] == "--version").count(), 1, "{a:?}");
    assert_eq!(
        a.iter()
            .filter(|c| c[..2] == ["uv", "venv"] && c.len() > 2 && c[2] != "--help")
            .count(),
        2
    );

    // Too old: not used; offline, no pinned uv: pip.
    let w = World::new();
    w.shim("uv");
    let t = setup_with(
        &w,
        &[],
        vec![("uv", Reply::ok("uv 0.7.22\n").when(["--version"]).sticky())],
    );
    let out = install(&t, &w, "python3.12", &[], false).unwrap();
    assert_eq!(&*out.installer, "pip:fallback");
}

/// A uv next to the snp executable wins over `PATH`.
#[test]
fn lang_python_sibling_uv() {
    let w = World::new();
    let mut t = setup(&w, &[]);
    let snp = w.root.join("dist/snp");
    exe(&snp);
    exe(&w
        .root
        .join("dist")
        .join(if cfg!(windows) { "uv.exe" } else { "uv" }));
    t.self_exe = snp;
    let out = install(&t, &w, "python3.12", &[], false).unwrap();
    assert_eq!(&*out.installer, "uv");
    assert!(argvs(&t, &w).iter().any(|c| c[..2] == ["uv", "venv"]));
}

/// The pinned uv is downloaded (verified) into the shared tools and reported in `InstallOutcome.tools`.
#[test]
fn lang_python_pinned_uv_is_a_shared_tool() {
    let w = World::new();
    let t = setup(&w, &[]);
    let dir = w
        .repo
        .join(".snp-tools/tools/uv")
        .join(snp_net::tools::UV_PINNED);
    exe(&dir.join(if cfg!(windows) { "uv.exe" } else { "uv" }));
    let out = install(&t, &w, "python3.12", &[], false).unwrap();
    assert_eq!(&*out.installer, "uv");
    assert_eq!(
        out.tools,
        vec![(Arc::from("uv"), Arc::from(snp_net::tools::UV_PINNED))]
    );
}

/// Health checks (`python_test.py:160-282`).
mod health {
    use super::*;

    fn built(extra: &[(&str, &str)]) -> (World, TestHost, InstallOutcome) {
        let w = World::new();
        let t = setup(&w, extra);
        let out = install(&t, &w, "python3.12", &[], false).unwrap();
        (w, t, out)
    }

    fn env_dir(w: &World) -> PathBuf {
        w.prefix.join("py_env-python3.12")
    }

    #[test]
    fn lang_python_healthy_default_creator() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        ok_version(&t);
        assert_eq!(health(&t, &w, "python3.12", None), Health::Healthy);
        // A `types.py` in the repository changes nothing (pre-commit runs `-S`).
        std::fs::write(w.repo.join("types.py"), b"").unwrap();
        assert_eq!(health(&t, &w, "python3.12", None), Health::Healthy);
    }

    #[test]
    fn lang_python_healthy_uv_creator() {
        let w = World::new();
        let t = uv_setup(&w, &[]);
        install(&t, &w, "python3.12", &[], false).unwrap();
        ok_version(&t);
        // uv writes `version_info = 3.12.3` (no release level): accepted (G5).
        assert_eq!(health(&t, &w, "python3.12", None), Health::Healthy);
    }

    #[test]
    fn lang_python_healthy_venv_creator() {
        let w = World::new();
        let t = setup(&w, &[("SNP_PYTHON_INSTALLER", "pip")]);
        let pv = env_dir(&w);
        std::fs::create_dir_all(pv.join("bin")).unwrap();
        exe(&pv.join("bin/python"));
        std::fs::write(
            pv.join("pyvenv.cfg"),
            "home = /usr/bin\nversion = 3.12.3\nexecutable = /usr/bin/python3.12\n",
        )
        .unwrap();
        ok_version(&t);
        assert_eq!(health(&t, &w, "python3.12", None), Health::Healthy);
    }

    #[test]
    fn lang_python_unhealthy_python_goes_missing() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        let py = env_dir(&w).join("bin/python");
        t.runner.reply("python", Reply::fail(1, "").when(["-S"]));
        let got = health(&t, &w, "python3.12", None);
        assert_eq!(
            got,
            Health::Unhealthy(format!(
                "virtualenv python version did not match created version:\n\
                 - actual version: <<error retrieving version from {}>>\n\
                 - expected version: {VI}\n",
                py.display()
            ))
        );
    }

    #[test]
    fn lang_python_unhealthy_with_version_change() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        let cfg = env_dir(&w).join("pyvenv.cfg");
        let mut body = std::fs::read_to_string(&cfg).unwrap();
        body.push_str("version_info = 1.2.3\n");
        std::fs::write(&cfg, body).unwrap();
        ok_version(&t);
        assert_eq!(
            health(&t, &w, "python3.12", None),
            Health::Unhealthy(format!(
                "virtualenv python version did not match created version:\n\
                 - actual version: {VI}\n\
                 - expected version: 1.2.3\n"
            ))
        );
    }

    #[test]
    fn lang_python_unhealthy_system_version_changes() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        let cfg = env_dir(&w).join("pyvenv.cfg");
        let mut body = std::fs::read_to_string(&cfg).unwrap();
        body.push_str("base-executable = /does/not/exist\n");
        std::fs::write(&cfg, body).unwrap();
        ok_version(&t);
        t.runner.reply("exist", Reply::fail(1, ""));
        assert_eq!(
            health(&t, &w, "python3.12", None),
            Health::Unhealthy(format!(
                "base executable python version does not match created version:\n\
                 - base-executable version: <<error retrieving version from /does/not/exist>>\n\
                 - expected version: {VI}\n"
            ))
        );
    }

    #[test]
    fn lang_python_unhealthy_old_virtualenv() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        std::fs::remove_file(env_dir(&w).join("pyvenv.cfg")).unwrap();
        assert_eq!(
            health(&t, &w, "python3.12", None),
            Health::Unhealthy("pyvenv.cfg does not exist (old virtualenv?)".to_owned())
        );
    }

    #[test]
    fn lang_python_unhealthy_unexpected_pyvenv() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        std::fs::write(env_dir(&w).join("pyvenv.cfg"), b"").unwrap();
        assert_eq!(
            health(&t, &w, "python3.12", None),
            Health::Unhealthy(
                "created virtualenv's pyvenv.cfg is missing `version_info`".to_owned()
            )
        );
    }

    #[test]
    fn lang_python_unhealthy_then_replaced() {
        let (w, t, _) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        t.runner.reply("python", Reply::ok("1.2.3\n").when(["-S"]));
        ok_version(&t);
        assert_eq!(
            health(&t, &w, "python3.12", None),
            Health::Unhealthy(format!(
                "virtualenv python version did not match created version:\n\
                 - actual version: 1.2.3\n\
                 - expected version: {VI}\n"
            ))
        );
        assert_eq!(health(&t, &w, "python3.12", None), Health::Healthy);
    }

    /// The fast path: zero spawns while the recorded interpreter is unchanged; a change re-probes.
    #[test]
    fn lang_python_health_fast_path() {
        let (w, t, out) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        ok_version(&t);
        let st = state(&out);
        let before = t.runner.calls().len();
        assert_eq!(health(&t, &w, "python3.12", Some(&st)), Health::Healthy);
        assert_eq!(t.runner.calls().len(), before, "zero spawns");
        // The base interpreter is replaced (another length): pre-commit's check runs.
        std::fs::write(w.shims.join("python3"), b"#!/bin/sh\n# upgraded\n").unwrap();
        assert_eq!(health(&t, &w, "python3.12", Some(&st)), Health::Healthy);
        let after = t.runner.calls();
        assert_eq!(after.len(), before + 1);
        assert_eq!(
            after.last().unwrap().argv[1..3],
            [
                std::ffi::OsString::from("-S"),
                std::ffi::OsString::from("-c")
            ]
        );
        // Without a fingerprint: always the slow path.
        let mut no_fp = state(&out);
        no_fp.fingerprint = None;
        assert_eq!(health(&t, &w, "python3.12", Some(&no_fp)), Health::Healthy);
        assert_eq!(t.runner.calls().len(), before + 2);
    }

    /// ADR-073: a different `SNP_PYTHON_INSTALLER` choice reinstalls.
    #[test]
    fn lang_python_installer_switch_reinstalls() {
        let (w, _, out) = built(&[("SNP_PYTHON_INSTALLER", "pip")]);
        let st = state(&out);
        assert_eq!(&*st.installer, "pip");
        for (mode, healthy) in [("pip", true), ("auto", false), ("uv", false)] {
            let t = w.host(w.isolated_env(&[("SNP_PYTHON_INSTALLER", mode)]));
            let got = health(&t, &w, "python3.12", Some(&st));
            assert_eq!(got == Health::Healthy, healthy, "{mode}: {got:?}");
            if !healthy {
                assert_eq!(t.runner.calls().len(), 0);
            }
        }
        let mut uv_state = state(&out);
        uv_state.installer = Arc::from("uv");
        for (mode, healthy) in [("uv", true), ("auto", true), ("pip", false)] {
            let t = w.host(w.isolated_env(&[("SNP_PYTHON_INSTALLER", mode)]));
            assert_eq!(
                health(&t, &w, "python3.12", Some(&uv_state)) == Health::Healthy,
                healthy,
                "{mode}"
            );
        }
    }
}

#[test]
fn lang_python_activation_and_plan() {
    let w = World::new();
    let t = w.host(w.isolated_env(&[("PYTHONHOME", "/x")]));
    let host = t.host();
    let env_dir = w.prefix.join("py_env-python3.12");
    let patch = Python
        .activate(&env_ctx(&host, &w.prefix, "python3.12", &env_dir))
        .unwrap();
    let env = t.env.patched(&patch);
    assert_eq!(env.get("PYTHONHOME"), None);
    assert_eq!(env.get("PIP_DISABLE_PIP_VERSION_CHECK").unwrap(), "1");
    assert_eq!(env.get("VIRTUAL_ENV").unwrap(), env_dir.as_os_str());
    let mut path = env_dir.join("bin").into_os_string();
    path.push(":");
    path.push(&w.shims);
    if cfg!(unix) {
        assert_eq!(env.get("PATH").unwrap(), path);
    }
    let plan = Python
        .plan(&crate::support::plan_ctx(
            &host,
            &w.prefix,
            "python3.12",
            &env_dir,
            &env,
            "pre-commit hazmat n1 x",
        ))
        .unwrap();
    let snp_lang::RunPlan::Xargs {
        cmd,
        shuffle,
        serial,
        ..
    } = plan
    else {
        panic!()
    };
    assert_eq!(
        cmd[..2],
        [t.self_exe.clone().into_os_string(), "__hazmat".into()]
    );
    assert!(shuffle && !serial);
    let plan = Python
        .plan(&crate::support::plan_ctx(
            &host,
            &w.prefix,
            "python3.12",
            &env_dir,
            &env,
            "'unclosed",
        ))
        .unwrap();
    let snp_lang::RunPlan::Error { code, output } = plan else {
        panic!()
    };
    assert_eq!(
        (code, output),
        (1, b"ValueError: No closing quotation".to_vec())
    );
    assert_eq!(Python.id(), LanguageId::Python);
    let s = Python.support();
    assert!(s.deps);
}

/// A pip differential case: user file, site file, variables.
#[cfg(all(unix, not(target_os = "macos")))]
type PipCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// Differential against pip itself (the pip 26 wheel the oracle's virtualenv seeds, run by the oracle interpreter):
/// `InstallCommand.parse_args([])` under the same files and variables gives the index options that snp's
/// `uv pip install` flags must express.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
#[ignore = "oracle"]
fn oracle_pip_configuration_matches_pip() {
    if std::path::Path::new("/etc/pip.conf").exists() {
        return;
    }
    let root = crate::oracle::oracle_root();
    let python = root.join("bin/python");
    let embed = root.join("lib/python3.12/site-packages/virtualenv/seed/wheels/embed");
    let wheel = std::fs::read_dir(&embed)
        .unwrap_or_else(|e| panic!("{}: {e} (set SNP_ORACLE_PY312)", embed.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("pip-"))
        .max()
        .expect("a pip wheel");
    let script = "import json, sys\n\
        sys.path.insert(0, sys.argv[1])\n\
        from pip._internal.commands import create_command\n\
        o, _ = create_command('install').parse_args([])\n\
        print(json.dumps([o.index_url, o.extra_index_urls, o.trusted_hosts, o.find_links, bool(o.pre)]))\n";
    let cases: [PipCase; 12] = [
        (
            "[global]\nindex-url = https://user-global/simple\n",
            "[install]\nindex-url = https://site-install/simple\n",
            &[],
        ),
        (
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_CONFIG_FILE", "{XDG}/pip/pip.conf")],
        ),
        (
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_CONFIG_FILE", "{XDG}/missing.conf")],
        ),
        (
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_INDEX_URL", "")],
        ),
        (
            "",
            "",
            &[
                ("pip_index_url", "https://lower/simple"),
                ("PIP_VERSION", "1"),
            ],
        ),
        ("[global]\n--index_url = https://dash/simple\n", "", &[]),
        (
            "[global]\n  index-url = https://corp/simple\n  trusted-host = corp\n",
            "",
            &[],
        ),
        (
            "[global]\nextra-index-url =\n    https://a/simple\n\n    https://b/simple\n",
            "",
            &[],
        ),
        (
            "[global]\nindex-url = https://g/simple\n[install] ; corp\nindex-url = https://i/simple\n",
            "",
            &[],
        ),
        (
            "[DEFAULT]\ntrusted-host = d\n[install]\npre = yes\n",
            "",
            &[],
        ),
        (
            "[global]\nfind-links = /w1 /w2\n",
            "[global]\nfind-links = /site\n",
            &[("PIP_PRE", "1")],
        ),
        (
            "[install]\nextra-index-url = https://u/simple\n",
            "[global]\nextra-index-url = https://s/simple\n",
            &[("PIP_TRUSTED_HOST", "x y")],
        ),
    ];
    for (i, (user, site, extra)) in cases.into_iter().enumerate() {
        let w = World::new();
        let xdg = w.root.join("xdg");
        std::fs::create_dir_all(xdg.join("pip")).unwrap();
        std::fs::create_dir_all(w.home.join(".config/pip")).unwrap();
        std::fs::write(xdg.join("pip/pip.conf"), site).unwrap();
        std::fs::write(w.home.join(".config/pip/pip.conf"), user).unwrap();
        let xdg_s = xdg.display().to_string();
        let mut env: Vec<(String, String)> = vec![("XDG_CONFIG_DIRS".into(), xdg_s.clone())];
        for (k, v) in extra {
            env.push(((*k).to_owned(), v.replace("{XDG}", &xdg_s)));
        }
        // pip.
        let out = std::process::Command::new(&python)
            .arg("-I")
            .arg("-c")
            .arg(script)
            .arg(&wheel)
            .env_clear()
            .env("HOME", &w.home)
            .env("PATH", "/usr/bin:/bin")
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let pip: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        // snp.
        env.push(("SNP_PYTHON_INSTALLER".into(), "uv".into()));
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let t = uv_setup(&w, &pairs);
        install(&t, &w, "python3.12", &[], false).unwrap_or_else(|e| panic!("case {i}: {e}"));
        let c = calls(&t, &w);
        let argv = &c
            .iter()
            .find(|c| c.argv[..2] == ["uv", "pip"])
            .unwrap()
            .argv;
        let mut index_url = "https://pypi.org/simple".to_owned();
        let (mut extra_urls, mut hosts, mut links) = (Vec::new(), Vec::new(), Vec::new());
        let mut pre = false;
        let mut it = argv[6..].iter();
        while let Some(a) = it.next() {
            let mut val = || it.next().cloned().unwrap_or_default();
            match a.as_str() {
                "--default-index" => index_url = val(),
                "--index" => extra_urls.push(val()),
                "--allow-insecure-host" => hosts.push(val()),
                "--find-links" => links.push(val()),
                "--prerelease" => pre = val() == "allow",
                "--index-strategy" => assert_eq!(val(), "unsafe-best-match"),
                "." => {}
                other => panic!("case {i}: unexpected {other}"),
            }
        }
        let ours = serde_json::json!([index_url, extra_urls, hosts, links, pre]);
        assert_eq!(
            ours, pip,
            "case {i}: user {user:?} site {site:?} env {extra:?}"
        );
    }
}
