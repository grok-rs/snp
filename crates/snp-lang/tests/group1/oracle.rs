//! Shim-harness goldens against pre-commit 4.7.0 (WBS WP-24 acceptance 1; 04b §5.12).
//!
//! Each scenario installs and runs one hook of the hook repository of [`HOOK_FILES`] — python, node, golang, rust and
//! ruby, with and without `additional_dependencies`, `language_version` default (resolved), `system` and explicit —
//! with every language tool faked. The pre-commit side is recorded by `testdata/group1/record.py` running
//! `pre-commit run --all-files` in-process with the tools' spawns faked (`#[ignore = "oracle"]` test below,
//! `SNP_UPDATE_GOLDENS=1` rewrites `testdata/group1/golden/*.jsonl`); the hermetic test runs snp's backend through the
//! same lifecycle (default version → install → health → activate → plan) with the same scripted replies and compares
//! the logs (argv with the tool basename, cwd, environment delta), after the documented, intentional differences are
//! set aside per scenario:
//!
//! * python: environment creation (`virtualenv` vs `<interp> -m venv`, D-PERF-01) and snp's reference-interpreter
//!   probe (pre-commit's reference is its own interpreter);
//! * node: nodeenv (snp reimplements it: no spawn);
//! * rust (non-system): the rustup bootstrap (persistent shared `RUSTUP_HOME`, 04a Q13), compared on argv, cwd and
//!   `RUSTUP_TOOLCHAIN` only;
//! * rust: `cargo install` order (pre-commit iterates a `set`; snp keeps config order) — compared as a multiset.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::lang::LanguageId;
use snp_lang::harness::{GoldenCall, Recorded, Substitutions, parse_log, render_log};
use snp_lang::{Health, RunPlan, backend, check_support};

use crate::support::{
    Eff, FakeFetch, HOOK_FILES, R, World, deps, env_ctx, install_ctx, plan_ctx, r, script,
};

/// One scenario.
struct Sc {
    /// Golden file stem.
    name: &'static str,
    /// Hook id.
    hook: &'static str,
    /// The language.
    lang: LanguageId,
    /// `language_version` in the config (`None`: omitted, resolved by the default version).
    version: Option<&'static str>,
    /// The resolved version (env directory suffix).
    resolved: &'static str,
    /// `additional_dependencies`.
    deps: &'static [&'static str],
    /// The hook's entry.
    entry: &'static str,
    /// Replies of both sides.
    replies: Vec<R>,
    /// Replies of snp only (tools pre-commit does not spawn).
    snp_replies: Vec<R>,
    /// Variables of snp's environment only.
    snp_env: &'static [(&'static str, &'static str)],
    /// Empty executables on `PATH` (default-version heuristics).
    shims: &'static [&'static str],
    /// pre-commit's golang `_install_go` stubbed.
    go_stub: bool,
    /// Shared toolchains pre-installed on the snp side: (tool, version, files).
    tools: &'static [(&'static str, &'static str, &'static [&'static str])],
    /// Texts the fake fetcher serves.
    fetch: &'static [(&'static str, &'static str)],
    /// Calls set aside before comparing (documented differences).
    skip: fn(&GoldenCall) -> bool,
    /// Variables compared for the calls `narrow` selects.
    narrow: fn(&GoldenCall) -> Option<&'static [&'static str]>,
    /// `PATH` holds only the shims directory (plus `git` for pre-commit), so the host's toolchains (a system ruby)
    /// do not steer the default version.
    bare_path: bool,
    /// The default version is resolved with `PATH` = the shims only (the rest of the lifecycle keeps the default
    /// `PATH`): the goldens were recorded where `/usr/bin` holds no `go`, but CI runners ship `/usr/bin/go`.
    bare_default: bool,
    /// Maps one of snp's calls to pre-commit's equivalent (documented differences of form).
    rewrite: fn(GoldenCall) -> GoldenCall,
}

/// The pyvenv.cfg the fake environment creators write.
const PYVENV: &str = "home = /usr/bin\nversion_info = 3.12.3.final.0\n";

/// `_version_info` and the reference probe.
fn python_replies() -> Vec<R> {
    vec![
        r("python", &["-S", "-c"], 0, "3.12.3.final.0\n"),
        R {
            effects: vec![
                Eff::Mkdir("{arg2}/bin"),
                Eff::Exe("{arg2}/bin/python"),
                Eff::Write("{arg2}/pyvenv.cfg", PYVENV),
            ],
            ..r("python", &["-mvirtualenv"], 0, "")
        },
        R {
            effects: vec![Eff::Exe("{env:VIRTUAL_ENV}/bin/py-hello")],
            ..r("python", &["-mpip", "install"], 0, "")
        },
    ]
}

/// snp's pip backend: the reference probe and `-m venv`.
fn python_snp_replies() -> Vec<R> {
    vec![
        r(
            "python3",
            &["-S", "-c"],
            0,
            "cpython\n3.12.3.final.0\n/usr/bin/python3\n",
        ),
        R {
            effects: vec![
                Eff::Mkdir("{arg3}/bin"),
                Eff::Exe("{arg3}/bin/python"),
                Eff::Write("{arg3}/pyvenv.cfg", PYVENV),
            ],
            ..r("python3", &["-m", "venv"], 0, "")
        },
    ]
}

/// The environment-creation calls (`virtualenv`, `-m venv`, `uv venv`) and snp's reference probe.
fn skip_python(c: &GoldenCall) -> bool {
    let a: Vec<&str> = c.argv.iter().map(String::as_str).collect();
    matches!(
        a.as_slice(),
        ["python", "-mvirtualenv", ..]
            | ["python3" | "python3.13", "-m", "venv", ..]
            | ["uv", "venv", ..]
    ) || (a.first() == Some(&"python3") && a.get(1) == Some(&"-S"))
}

/// snp's uv backend (D-PERF-01): `uv pip install --python <env python> --no-config [.] *deps` with the isolated uv
/// cache stands for pre-commit's `python -mpip install . *deps` in the same activated environment; the rest of the
/// argv and the environment delta must match exactly.
fn rewrite_uv(mut c: GoldenCall) -> GoldenCall {
    if c.argv.get(..3) == Some(&["uv".to_owned(), "pip".to_owned(), "install".to_owned()][..]) {
        assert_eq!(c.argv.get(3).map(String::as_str), Some("--python"), "{c:?}");
        assert_eq!(
            c.argv.get(5).map(String::as_str),
            Some("--no-config"),
            "{c:?}"
        );
        let rest: Vec<String> = c.argv.get(6..).unwrap_or_default().to_vec();
        c.argv = ["python", "-mpip", "install"]
            .into_iter()
            .map(str::to_owned)
            .chain(rest)
            .collect();
        assert_eq!(
            c.env.remove("UV_CACHE_DIR"),
            Some(Some("{TOOLS}/cache/uv".to_owned())),
            "the isolated uv cache"
        );
    }
    c
}

/// No rewriting.
fn rewrite_none(c: GoldenCall) -> GoldenCall {
    c
}

/// nodeenv.
fn skip_node(c: &GoldenCall) -> bool {
    c.argv.get(1).map(String::as_str) == Some("-mnodeenv")
}

/// rustup.
fn skip_rustup(c: &GoldenCall) -> bool {
    c.argv.first().map(String::as_str) == Some("rustup")
}

/// Nothing set aside.
fn skip_none(_: &GoldenCall) -> bool {
    false
}

/// Full comparison.
fn narrow_none(_: &GoldenCall) -> Option<&'static [&'static str]> {
    None
}

/// The managed-rust cargo calls: argv, cwd and `RUSTUP_TOOLCHAIN`.
fn narrow_cargo(c: &GoldenCall) -> Option<&'static [&'static str]> {
    let a: Vec<&str> = c.argv.iter().map(String::as_str).collect();
    matches!(a.as_slice(), ["cargo", "install" | "add", ..]).then_some(&["RUSTUP_TOOLCHAIN"][..])
}

/// The go.dev JSON of the default go version.
const GO_LATEST: &str = r#"[{"version": "go1.27.1", "stable": true, "files": []}]"#;
/// node's index.json with several 20.x releases (the newest first, like nodejs.org).
const NODE_INDEX_20: &str = r#"[{"version": "v21.0.0", "lts": false, "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl", "linux-arm64-musl"]},
{"version": "v20.11.1", "lts": "Iron", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl", "linux-arm64-musl"]},
{"version": "v20.9.0", "lts": "Iron", "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl", "linux-arm64-musl"]}]"#;
/// node's index.json.
const NODE_INDEX: &str = r#"[{"version": "v26.1.0", "lts": false, "files": ["linux-x64", "linux-arm64", "osx-arm64-tar", "osx-x64-tar", "win-x64-zip", "win-arm64-zip", "linux-x64-musl", "linux-arm64-musl"]}]"#;

/// nodeenv (faked), `npm install` and `node --version`.
fn node_replies() -> Vec<R> {
    vec![
        R {
            effects: vec![Eff::Mkdir("{arg4}/bin")],
            ..r("python", &["-mnodeenv"], 0, "")
        },
        R {
            effects: vec![Eff::Exe("{env:NODE_VIRTUAL_ENV}/bin/node-hello")],
            ..r("npm", &["install"], 0, "")
        },
        r("node", &["--version"], 0, "v20.11.1\n"),
    ]
}

/// The scenarios.
fn scenarios() -> Vec<Sc> {
    let ruby_gem = || R {
        effects: vec![Eff::Write("{cwd}/hooks-0.0.0.gem", "gem")],
        ..r("gem", &["build"], 0, "")
    };
    let base = |name, hook, lang| Sc {
        name,
        hook,
        lang,
        version: None,
        resolved: "",
        deps: &[],
        entry: "",
        replies: Vec::new(),
        snp_replies: Vec::new(),
        snp_env: &[],
        shims: &[],
        go_stub: false,
        tools: &[],
        fetch: &[],
        skip: skip_none,
        narrow: narrow_none,
        rewrite: rewrite_none,
        bare_path: false,
        bare_default: false,
    };
    vec![
        Sc {
            version: Some("python3.12"),
            resolved: "python3.12",
            deps: &["six"],
            entry: "py-hello --flag",
            replies: python_replies(),
            snp_replies: python_snp_replies(),
            snp_env: &[("SNP_PYTHON_INSTALLER", "pip")],
            skip: skip_python,
            ..base("python_explicit_deps", "py-hook", LanguageId::Python)
        },
        Sc {
            resolved: "python3.12",
            entry: "py-hello --flag",
            replies: python_replies(),
            snp_replies: python_snp_replies(),
            snp_env: &[("SNP_PYTHON_INSTALLER", "pip")],
            // The reference interpreter (`python3`, CPython 3.12) and its `python3.12` are shims, not the host's.
            shims: &["python3", "python3.12"],
            skip: skip_python,
            ..base("python_default", "py-hook", LanguageId::Python)
        },
        // An explicit interpreter other than the reference one: pre-commit passes `-p python3.13` to virtualenv,
        // snp creates the environment with that interpreter.
        Sc {
            version: Some("python3.13"),
            resolved: "python3.13",
            entry: "py-hello --flag",
            replies: python_replies(),
            snp_replies: {
                let mut v = python_snp_replies();
                v.push(R {
                    effects: vec![
                        Eff::Mkdir("{arg3}/bin"),
                        Eff::Exe("{arg3}/bin/python"),
                        Eff::Write("{arg3}/pyvenv.cfg", PYVENV),
                    ],
                    ..r("python3.13", &["-m", "venv"], 0, "")
                });
                v
            },
            snp_env: &[("SNP_PYTHON_INSTALLER", "pip")],
            shims: &["python3.13"],
            skip: skip_python,
            ..base("python_explicit_other", "py-hook", LanguageId::Python)
        },
        // The default installer (uv): the install's activated environment and the hook run equal pre-commit's pip
        // install and run.
        Sc {
            version: Some("python3.12"),
            resolved: "python3.12",
            deps: &["six"],
            entry: "py-hello --flag",
            replies: python_replies(),
            snp_replies: {
                let mut v = python_snp_replies();
                v.push(R {
                    effects: vec![
                        Eff::Mkdir("{arg2}/bin"),
                        Eff::Exe("{arg2}/bin/python"),
                        Eff::Write("{arg2}/pyvenv.cfg", PYVENV),
                    ],
                    ..r("uv", &["venv"], 0, "")
                });
                v.push(R {
                    effects: vec![Eff::Exe("{env:VIRTUAL_ENV}/bin/py-hello")],
                    ..r("uv", &["pip", "install"], 0, "")
                });
                v
            },
            snp_env: &[("SNP_PYTHON_INSTALLER", "uv"), ("SNP_UV", "uv")],
            skip: skip_python,
            rewrite: rewrite_uv,
            ..base("python_uv_deps", "py-hook", LanguageId::Python)
        },
        Sc {
            version: Some("system"),
            resolved: "system",
            deps: &["lodash"],
            entry: "node-hello",
            replies: vec![
                R {
                    effects: vec![Eff::Mkdir("{arg4}/bin")],
                    ..r("python", &["-mnodeenv"], 0, "")
                },
                R {
                    effects: vec![Eff::Exe("{env:NODE_VIRTUAL_ENV}/bin/node-hello")],
                    ..r("npm", &["install"], 0, "")
                },
                r("node", &["--version"], 0, "v26.1.0\n"),
            ],
            skip: skip_node,
            ..base("node_system_deps", "node-hook", LanguageId::Node)
        },
        Sc {
            resolved: "default",
            entry: "node-hello",
            replies: vec![
                R {
                    effects: vec![Eff::Mkdir("{arg4}/bin")],
                    ..r("python", &["-mnodeenv"], 0, "")
                },
                R {
                    effects: vec![Eff::Exe("{env:NODE_VIRTUAL_ENV}/bin/node-hello")],
                    ..r("npm", &["install"], 0, "")
                },
                r("node", &["--version"], 0, "v26.1.0\n"),
            ],
            tools: &[("node", "26.1.0", &["bin/node", "bin/npm"])],
            fetch: &[("https://nodejs.org/dist/index.json", NODE_INDEX)],
            skip: skip_node,
            ..base("node_default", "node-hook", LanguageId::Node)
        },
        Sc {
            version: Some("20.11.1"),
            resolved: "20.11.1",
            entry: "node-hello",
            replies: node_replies(),
            tools: &[("node", "20.11.1", &["bin/node", "bin/npm"])],
            skip: skip_node,
            ..base("node_explicit", "node-hook", LanguageId::Node)
        },
        // nodeenv 1.11.0 resolves a range against index.json: `20` is the newest 20.x release.
        Sc {
            version: Some("20"),
            resolved: "20",
            deps: &["prettier@3"],
            entry: "node-hello",
            replies: node_replies(),
            tools: &[("node", "20.11.1", &["bin/node", "bin/npm"])],
            fetch: &[("https://nodejs.org/dist/index.json", NODE_INDEX_20)],
            skip: skip_node,
            ..base("node_range_deps", "node-hook", LanguageId::Node)
        },
        Sc {
            version: Some("system"),
            resolved: "system",
            deps: &["golang.org/x/tools/cmd/goimports@latest"],
            entry: "go-hello",
            replies: vec![R {
                effects: vec![
                    Eff::Exe("{env:GOPATH}/bin/go-hello"),
                    Eff::Mkdir("{env:GOPATH}/pkg/mod"),
                ],
                ..r("go", &["install"], 0, "")
            }],
            ..base("golang_system_deps", "go-hook", LanguageId::Golang)
        },
        Sc {
            version: Some("1.22.1"),
            resolved: "1.22.1",
            entry: "go-hello",
            replies: vec![R {
                effects: vec![Eff::Exe("{env:GOPATH}/bin/go-hello")],
                ..r("go", &["install"], 0, "")
            }],
            go_stub: true,
            tools: &[("go", "1.22.1", &["bin/go"])],
            ..base("golang_explicit", "go-hook", LanguageId::Golang)
        },
        Sc {
            resolved: "default",
            entry: "go-hello",
            replies: vec![R {
                effects: vec![Eff::Exe("{env:GOPATH}/bin/go-hello")],
                ..r("go", &["install"], 0, "")
            }],
            go_stub: true,
            tools: &[("go", "1.27.1", &["bin/go"])],
            fetch: &[("https://go.dev/dl/?mode=json", GO_LATEST)],
            bare_default: true,
            ..base("golang_default", "go-hook", LanguageId::Golang)
        },
        Sc {
            version: Some("system"),
            resolved: "system",
            deps: &["cli:shellharden:4.2.0", "serde:1.0"],
            entry: "rust-hello",
            replies: vec![
                R {
                    effects: vec![Eff::Exe("{arg4}/bin/rust-hello")],
                    ..r("cargo", &["install"], 0, "")
                },
                r("cargo", &["add"], 0, ""),
            ],
            ..base("rust_system_deps", "rust-hook", LanguageId::Rust)
        },
        Sc {
            resolved: "default",
            entry: "rust-hello",
            replies: vec![
                r("cargo", &["--version"], 1, ""),
                R {
                    effects: vec![Eff::Exe("{arg4}/bin/rust-hello")],
                    ..r("cargo", &["install"], 0, "")
                },
            ],
            shims: &["rustup"],
            skip: skip_rustup,
            narrow: narrow_cargo,
            ..base("rust_default", "rust-hook", LanguageId::Rust)
        },
        Sc {
            version: Some("1.79.0"),
            resolved: "1.79.0",
            deps: &["cli:shellharden:4.2.0", "serde:1.0"],
            entry: "rust-hello",
            replies: vec![
                R {
                    effects: vec![Eff::Exe("{arg4}/bin/rust-hello")],
                    ..r("cargo", &["install"], 0, "")
                },
                r("cargo", &["add"], 0, ""),
            ],
            shims: &["rustup"],
            skip: skip_rustup,
            narrow: narrow_cargo,
            ..base("rust_explicit_deps", "rust-hook", LanguageId::Rust)
        },
        Sc {
            resolved: "default",
            entry: "ruby-hello",
            replies: vec![
                ruby_gem(),
                R {
                    effects: vec![Eff::Exe("{env:GEM_HOME}/bin/ruby-hello")],
                    ..r("gem", &["install"], 0, "")
                },
            ],
            bare_path: true,
            ..base("ruby_default", "ruby-hook", LanguageId::Ruby)
        },
        Sc {
            version: Some("system"),
            resolved: "system",
            deps: &["rake"],
            entry: "ruby-hello",
            replies: vec![
                ruby_gem(),
                R {
                    effects: vec![Eff::Exe("{env:GEM_HOME}/bin/ruby-hello")],
                    ..r("gem", &["install"], 0, "")
                },
            ],
            ..base("ruby_system_deps", "ruby-hook", LanguageId::Ruby)
        },
        Sc {
            version: Some("3.2.2"),
            resolved: "3.2.2",
            entry: "ruby-hello",
            replies: vec![
                ruby_gem(),
                R {
                    effects: vec![Eff::Exe("{env:GEM_HOME}/bin/ruby-hello")],
                    ..r("gem", &["install"], 0, "")
                },
            ],
            ..base("ruby_explicit", "ruby-hook", LanguageId::Ruby)
        },
    ]
}

/// The committed golden of a scenario.
fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/group1/golden/{name}.jsonl"))
}

/// The environment directory name.
fn env_dir_name(lang: LanguageId, resolved: &str) -> String {
    format!("{}-{resolved}", lang.env_dir_base().unwrap())
}

/// snp's side of a scenario.
fn snp_side(sc: &Sc) -> Vec<GoldenCall> {
    let w = World::new();
    for s in sc.shims {
        w.shim(s);
    }
    let env = if sc.bare_path {
        w.env(sc.snp_env)
            .patched(&snp_core::env::EnvPatch::default().set("PATH", w.shims.as_os_str()))
    } else {
        w.env(sc.snp_env)
    };
    let mut t = w.host(env);
    script(&t, &sc.replies);
    script(&t, &sc.snp_replies);
    for (tool, version, files) in sc.tools {
        let dir = w.repo.join(".snp-tools/tools").join(tool).join(version);
        for f in *files {
            crate::support::exe(&dir.join(f));
        }
    }
    if !sc.fetch.is_empty() {
        t.fetch = Some(Box::new(FakeFetch {
            texts: sc
                .fetch
                .iter()
                .map(|(u, b)| ((*u).to_owned(), (*b).to_owned()))
                .collect(),
            ..FakeFetch::default()
        }));
    }
    let host = t.host();
    let be = backend(sc.lang);
    let bare_env = host
        .base_env
        .patched(&snp_core::env::EnvPatch::default().set("PATH", w.shims.as_os_str()));
    let default_host = snp_lang::Host {
        base_env: &bare_env,
        cwd: host.cwd,
        self_exe: host.self_exe,
        probe: host.probe,
        fetch: host.fetch,
        tools: host.tools,
        runner: host.runner,
        printer: host.printer,
        cancel: host.cancel,
        brand: host.brand,
    };
    let version: Arc<str> = match sc.version {
        Some(v) => Arc::from(v),
        None if sc.bare_default => be.default_version(&default_host).unwrap(),
        None => be.default_version(&host).unwrap(),
    };
    assert_eq!(&*version, sc.resolved, "{}", sc.name);
    let deps = deps(sc.deps);
    check_support(sc.lang, &version, &deps, host.brand).unwrap();
    let env_dir = w.prefix.join(env_dir_name(sc.lang, &version));
    be.install(&install_ctx(
        &host, &w.prefix, &version, &env_dir, &deps, false,
    ))
    .unwrap();
    let ecx = env_ctx(&host, &w.prefix, &version, &env_dir);
    assert_eq!(
        be.health(&ecx, None).unwrap(),
        Health::Healthy,
        "{}",
        sc.name
    );
    let patch = be.activate(&ecx).unwrap();
    let env = t.env.patched(&patch);
    let plan = be
        .plan(&plan_ctx(
            &host, &w.prefix, &version, &env_dir, &env, sc.entry,
        ))
        .unwrap();
    let RunPlan::Xargs {
        cmd, env: run_env, ..
    } = plan
    else {
        panic!("{plan:?}")
    };
    let subst = w.subst();
    let mut calls = t.runner.golden(&subst);
    let mut argv = cmd.to_vec();
    argv.push("a.txt".into());
    let run = Recorded {
        argv,
        cwd: w.repo.clone(),
        env_delta: run_env
            .iter()
            .filter(|(k, v)| t.env.get(&k.to_string_lossy()) != Some(*v))
            .map(|(k, v)| (k.to_os_string(), Some(v.to_os_string())))
            .chain(
                t.env
                    .iter()
                    .filter(|(k, _)| run_env.get(&k.to_string_lossy()).is_none())
                    .map(|(k, _)| (k.to_os_string(), None)),
            )
            .collect(),
    };
    calls.push(GoldenCall::from_recorded(&run, &subst));
    calls
}

/// Sets the documented differences aside and makes the comparison order-insensitive where pre-commit is.
fn normalize(sc: &Sc, calls: Vec<GoldenCall>) -> Vec<GoldenCall> {
    let mut out: Vec<GoldenCall> = calls
        .into_iter()
        .filter(|c| !(sc.skip)(c))
        .map(sc.rewrite)
        .map(|c| match (sc.narrow)(&c) {
            Some(keep) => c.only_env(keep),
            None => c,
        })
        .collect();
    sort_cargo_installs(&mut out);
    out
}

/// pre-commit iterates `cargo install` packages from a set, whose order follows the string hashes: the run of
/// `cargo install` calls is sorted. The recorder runs under `python -I` (which implies `-E`, so `PYTHONHASHSEED`
/// is ignored and the order differs between recordings); goldens are written sorted.
fn sort_cargo_installs(out: &mut [GoldenCall]) {
    let is_install =
        |c: &GoldenCall| c.argv.get(..2) == Some(&["cargo".to_owned(), "install".to_owned()][..]);
    if let Some(s) = out.iter().position(is_install) {
        let end = out
            .get(s..)
            .and_then(|rest| rest.iter().position(|c| !is_install(c)))
            .map_or(out.len(), |n| s + n);
        if let Some(run) = out.get_mut(s..end) {
            run.sort_by(|a, b| a.argv.cmp(&b.argv));
        }
    }
}

#[test]
fn snp_matches_the_pre_commit_goldens() {
    for sc in scenarios() {
        let golden = std::fs::read_to_string(golden_path(sc.name))
            .unwrap_or_else(|e| panic!("{}: {e}", sc.name));
        let theirs = normalize(&sc, parse_log(&golden, &Substitutions::new()).unwrap());
        let ours = normalize(&sc, snp_side(&sc));
        assert_eq!(
            render_log(&ours),
            render_log(&theirs),
            "scenario {}",
            sc.name
        );
    }
}

/// The oracle venv: `target/oracle/pc470-py312`, or `$SNP_ORACLE_PY312`.
pub(crate) fn oracle_root() -> PathBuf {
    #[expect(
        clippy::disallowed_methods,
        reason = "test switch selecting the oracle venv"
    )]
    let explicit = std::env::var_os("SNP_ORACLE_PY312");
    explicit.map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oracle/pc470-py312"),
        PathBuf::from,
    )
}

/// Records one scenario with pre-commit 4.7.0.
#[cfg(unix)]
fn record(sc: &Sc, python: &Path) -> String {
    let w = World::new();
    for s in sc.shims {
        w.shim(s);
    }
    let hooks = w.root.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for (name, body) in HOOK_FILES {
        std::fs::write(hooks.join(name), body).unwrap();
    }
    let git_env: Vec<(&str, String)> = vec![
        ("GIT_AUTHOR_NAME", "t".into()),
        ("GIT_AUTHOR_EMAIL", "t@t".into()),
        ("GIT_COMMITTER_NAME", "t".into()),
        ("GIT_COMMITTER_EMAIL", "t@t".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("HOME", w.home.display().to_string()),
        ("PATH", "/usr/bin:/bin".into()),
    ];
    let git = |dir: &Path, args: &[&str]| -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .env_clear()
            .envs(git_env.iter().map(|(k, v)| (*k, v.as_str())))
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&hooks, &["init", "-q"]);
    git(&hooks, &["add", "."]);
    git(&hooks, &["commit", "-q", "-m", "hooks"]);
    let rev = git(&hooks, &["rev-parse", "HEAD"]);
    std::fs::write(w.repo.join("a.txt"), b"a\n").unwrap();
    let mut hook = format!("        -   id: {}\n", sc.hook);
    if let Some(v) = sc.version {
        writeln!(hook, "            language_version: '{v}'").unwrap();
    }
    if !sc.deps.is_empty() {
        let d: Vec<String> = sc.deps.iter().map(|d| format!("'{d}'")).collect();
        writeln!(
            hook,
            "            additional_dependencies: [{}]",
            d.join(", ")
        )
        .unwrap();
    }
    let config = format!(
        "repos:\n-   repo: {}\n    rev: {rev}\n    hooks:\n{hook}",
        hooks.display()
    );
    std::fs::write(w.repo.join(".pre-commit-config.yaml"), config).unwrap();
    git(&w.repo, &["init", "-q"]);
    git(&w.repo, &["add", "."]);
    git(&w.repo, &["commit", "-q", "-m", "init"]);

    let store = w.root.join("store");
    let tmp = w.root.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in w.env(&[]).iter() {
        if k != "PRE_COMMIT" {
            env.insert(
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            );
        }
    }
    if sc.bare_path {
        use std::os::unix::fs::PermissionsExt as _;
        let git_exe = ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.is_file())
            .expect("git");
        // git's own helpers (`git submodule` is a shell script) get the usual PATH back.
        let wrapper = w.shims.join("git");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nPATH=/usr/bin:/bin exec {} \"$@\"\n",
                git_exe.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        env.insert("PATH".into(), w.shims.display().to_string());
    }
    env.insert("PRE_COMMIT_HOME".into(), store.display().to_string());
    env.insert("TMPDIR".into(), tmp.display().to_string());
    env.insert("PYTHONHASHSEED".into(), "0".into());
    let mut base = env.clone();
    base.insert("PRE_COMMIT".into(), "1".into());
    let log = w.root.join("log.jsonl");
    let scenario = serde_json::json!({
        "argv": ["run", "--all-files"],
        "base_env": base,
        "fake": ["python", "npm", "node", "go", "cargo", "rustup", "gem", "rbenv",
                 "py-hello", "node-hello", "go-hello", "rust-hello", "ruby-hello"],
        "replies": sc.replies.iter().map(R::json).collect::<Vec<_>>(),
        "log": log,
        "go_stub": sc.go_stub,
    });
    let sc_file = w.root.join("scenario.json");
    std::fs::write(&sc_file, scenario.to_string()).unwrap();
    let recorder = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/group1/record.py");
    let out = std::process::Command::new(python)
        .arg("-I")
        .arg(&recorder)
        .arg(&sc_file)
        .env_clear()
        .envs(&env)
        .current_dir(&w.repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}: {}{}",
        sc.name,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The prefix is the store's clone holding the environment (with dependencies pre-commit also clones the
    // dependency-less variant to read the manifest).
    let env_dir = env_dir_name(sc.lang, sc.resolved);
    let clone = std::fs::read_dir(&store)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name().unwrap().to_string_lossy().starts_with("repo")
                && std::fs::symlink_metadata(p.join(&env_dir)).is_ok()
        })
        .unwrap_or_else(|| panic!("{}: no clone holds {env_dir}", sc.name));
    let subst = w.subst().with(clone.as_os_str(), "{PREFIX}");
    let calls = parse_log(&std::fs::read_to_string(&log).unwrap(), &subst).unwrap();
    // pre-commit's temporary `RUSTUP_HOME`s.
    let mut calls: Vec<GoldenCall> = calls
        .into_iter()
        .map(|mut c| {
            for v in c.env.values_mut().flatten() {
                if v.starts_with(&tmp.display().to_string()) {
                    *v = "{TMP}".to_owned();
                }
            }
            c
        })
        .collect();
    sort_cargo_installs(&mut calls);
    render_log(&calls)
}

/// Re-records every golden with pre-commit 4.7.0 and requires the committed files to be current.
#[cfg(unix)]
#[test]
#[ignore = "oracle"]
fn oracle_group1_goldens_are_current() {
    let root = oracle_root();
    let python = root.join("bin/python");
    assert!(
        python.exists(),
        "run `cargo xtask oracle install` (or set SNP_ORACLE_PY312)"
    );
    #[expect(
        clippy::disallowed_methods,
        reason = "test switch to rewrite the goldens"
    )]
    let update = std::env::var_os("SNP_UPDATE_GOLDENS").is_some();
    for sc in scenarios() {
        let rendered = record(&sc, &python);
        if update {
            std::fs::create_dir_all(golden_path(sc.name).parent().unwrap()).unwrap();
            std::fs::write(golden_path(sc.name), &rendered).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(golden_path(sc.name)).unwrap(),
            rendered,
            "scenario {}",
            sc.name
        );
    }
}
