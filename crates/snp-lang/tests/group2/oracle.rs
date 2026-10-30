//! The group-2 commands against pre-commit 4.6.2, through the shim harness (04b §5.12).
//!
//! One scenario per language: a repository with one `a.txt` and one local hook of that language (`require_serial`,
//! `files: \.txt$`, args and `additional_dependencies` where the language takes them), run with
//! `pre-commit run --all-files` and fake tools on `PATH` — the WP-16 logger (golden log format v1) behind a small
//! wrapper that performs the side effects the next step needs (the conda/cs/cpan environment directory, the dart
//! package a `pub cache add` downloads, the nupkg `dotnet pack` builds) and copies the temporary `script.R` and
//! `nuget.config` files the backends write. `testdata/group2/golden/<lang>.jsonl` is the log pre-commit produced,
//! `<lang>.captures.json` the captured files (`cargo test -p snp-lang --features testing --test group2 --
//! --ignored oracle_`, `SNP_UPDATE_GOLDENS=1` rewrites them; `SNP_ORACLE_PC462` names another pre-commit 4.6.2
//! venv than `target/oracle/pc462-py312`).
//!
//! The hermetic tests replay each scenario through the backend — install, the post-install health check, activation,
//! plan — with a `ShimRunner` answering like the fake tools, render every recorded command, the plan's pre-steps and
//! the hook command (with `a.txt` appended) in the same format, and require equality after replacing the run-specific
//! paths by placeholders (`{REPO}`, `{PREFIX}`, `{ENV}`, `{SHIMS}`, `{HOME}`, `{TAG}`, `{UID}:{GID}`, and `{TMP}/T<n>`
//! for temporary directories in order of appearance).
//!
//! Two pre-commit calls are folded on its side, both invisible: the repeated `luarocks config --lua-ver` (pre-commit
//! spawns it on every activation, snp once per process) and nothing else — docker's per-run build is rendered from
//! the plan's pre-step, whose `docker image inspect` skip probe pre-commit does not have (D-PERF-04).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::harness::{GoldenCall, Recorded, Reply, Substitutions, parse_log, render_log};
use snp_lang::{Health, RunPlan};

use super::util::{DOCKER_INFO, Fx};

/// One language scenario.
#[derive(Debug, Clone, Copy)]
struct Scenario {
    /// The config `language:`.
    lang: &'static str,
    /// Its id.
    id: LanguageId,
    /// The hook `entry`.
    entry: &'static str,
    /// The hook `args`.
    args: &'static [&'static str],
    /// `additional_dependencies`.
    deps: &'static [&'static str],
}

/// The eleven scenarios.
const SCENARIOS: [Scenario; 11] = [
    Scenario {
        lang: "conda",
        id: LanguageId::Conda,
        entry: "fakehook --flag",
        args: &["a b"],
        deps: &["pkgA", "pkgB=1.0"],
    },
    Scenario {
        lang: "coursier",
        id: LanguageId::Coursier,
        entry: "fakehook --flag",
        args: &[],
        deps: &["scalafmt:3.6.1", "other"],
    },
    Scenario {
        lang: "dart",
        id: LanguageId::Dart,
        entry: "fakehook",
        args: &["--x"],
        deps: &["pkg_x:1.2.3", "pkg_y"],
    },
    Scenario {
        lang: "docker",
        id: LanguageId::Docker,
        entry: "fakehook --flag",
        args: &["a b"],
        deps: &[],
    },
    Scenario {
        lang: "dotnet",
        id: LanguageId::Dotnet,
        entry: "fakehook",
        args: &[],
        deps: &[],
    },
    Scenario {
        lang: "haskell",
        id: LanguageId::Haskell,
        entry: "fakehook",
        args: &[],
        deps: &["hello", "other-1.0"],
    },
    Scenario {
        lang: "julia",
        id: LanguageId::Julia,
        entry: "hooks/main.jl --opt",
        args: &["x"],
        deps: &["Example@0.5"],
    },
    Scenario {
        lang: "lua",
        id: LanguageId::Lua,
        entry: "fakehook",
        args: &[],
        deps: &["penlight", "luacheck"],
    },
    Scenario {
        lang: "perl",
        id: LanguageId::Perl,
        entry: "fakehook",
        args: &[],
        deps: &["Foo::Bar"],
    },
    Scenario {
        lang: "r",
        id: LanguageId::R,
        entry: "Rscript hooks/check.R",
        args: &["--foo"],
        deps: &["rprojroot@1.0", "glue"],
    },
    Scenario {
        lang: "swift",
        id: LanguageId::Swift,
        entry: "fakehook --flag",
        args: &[],
        deps: &[],
    },
];

/// The fake tools behind the wrapper.
const TOOLS: [&str; 11] = [
    "conda", "cs", "dart", "dotnet", "cabal", "julia", "luarocks", "cpan", "Rscript", "swift", "docker",
];

/// The nuspec of the fake `dotnet pack`.
const NUSPEC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><package \
                      xmlns=\"http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd\"><metadata><id>my.tool</id>\
                      <version>1.0.0</version></metadata></package>";

/// The side-effect wrapper of the pre-commit side (runs the effect, then execs the WP-16 logger of the same name).
const WRAPPER_PY: &str = r#"
import os
import sys
import zipfile

NUSPEC = '__NUSPEC__'


def capture(path):
    cap = os.environ['SNP_SHIM_CAPTURE']
    n = len(os.listdir(cap))
    with open(path, 'rb') as f:
        data = f.read()
    with open(os.path.join(cap, '%03d-%s' % (n, os.path.basename(path))), 'wb') as f:
        f.write(data)


def main():
    tool = os.path.basename(sys.argv[0])
    args = sys.argv[1:]
    for a in args:
        if (a.endswith('script.R') or a.endswith('nuget.config')) and os.path.isfile(a):
            capture(a)
    if tool == 'conda' and args[:2] == ['env', 'create']:
        os.makedirs(args[3], exist_ok=True)
    elif tool == 'cs' and args[:1] == ['install']:
        os.makedirs(args[2], exist_ok=True)
    elif tool == 'cpan':
        os.makedirs(os.environ['PERL_MB_OPT'].split(' ', 1)[1], exist_ok=True)
    elif tool == 'dart' and args[:3] == ['pub', 'cache', 'add']:
        dep = args[3]
        ver = args[5] if len(args) > 5 else '0.0.1'
        d = os.path.join(os.environ['PUB_CACHE'], 'hosted', 'pub.dev', f'{dep}-{ver}')
        os.makedirs(d)
        with open(os.path.join(d, 'pubspec.yaml'), 'w') as f:
            f.write(f'name: {dep}\nexecutables:\n  {dep}_tool:\n  aliased: other\n')
    elif tool == 'dotnet' and args[:1] == ['pack']:
        out = args[4].split('=', 1)[1]
        os.makedirs(out, exist_ok=True)
        with zipfile.ZipFile(os.path.join(out, 'my.tool.1.0.0.nupkg'), 'w') as z:
            z.writestr('my.tool.nuspec', NUSPEC)
    elif tool == 'dotnet' and args[:2] == ['tool', 'install']:
        os.makedirs(args[5], exist_ok=True)
    real = os.path.join(os.environ['SNP_SHIM_REAL'], tool)
    os.execv(real, [real, *args])


main()
"#;

/// The scripted answers (both sides).
const LUA_VERSION: &str = "5.4\n";
/// The R version the fake Rscript prints for the renv-activated scripts.
const R_VERSION: &str = "4.3.3";

/// The golden directory.
fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/group2/golden")
}

/// The committed golden log of `lang`.
fn golden_log(lang: &str) -> PathBuf {
    golden_dir().join(format!("{lang}.jsonl"))
}

/// The committed captures of `lang`.
fn golden_captures(lang: &str) -> PathBuf {
    golden_dir().join(format!("{lang}.captures.json"))
}

/// Replaces `{TMP}/<name>` path components by `{TMP}/T<n>`, numbered in order of first appearance (argv, cwd, env
/// values in key order, call by call).
fn normalize_tmp(calls: &mut [GoldenCall]) {
    let mut names: Vec<String> = Vec::new();
    let mut fix = |s: &mut String| {
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(pos) = rest.find("{TMP}/") {
            out.push_str(&rest[..pos + 6]);
            let tail = &rest[pos + 6..];
            let end = tail.find(['/', ':', ';', ' ']).unwrap_or(tail.len());
            let name = &tail[..end];
            let idx = match names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    names.push(name.to_owned());
                    names.len() - 1
                }
            };
            out.push_str(&format!("T{}", idx + 1));
            rest = &tail[end..];
        }
        out.push_str(rest);
        *s = out;
    };
    for c in calls {
        for a in &mut c.argv {
            fix(a);
        }
        fix(&mut c.cwd);
        for v in c.env.values_mut().flatten() {
            fix(v);
        }
    }
}

/// The `-u` value of docker runs.
fn uid_gid() -> Option<String> {
    #[cfg(unix)]
    {
        Some(format!(
            "{}:{}",
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw()
        ))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Path placeholders.
fn substitutions(
    repo: &Path,
    prefix: &Path,
    env_dir: &Path,
    shims: &Path,
    tmp: &Path,
    home: &Path,
    tag: &str,
) -> Substitutions {
    let s = Substitutions::new()
        .with(repo.as_os_str(), "{REPO}")
        .with(prefix.as_os_str(), "{PREFIX}")
        .with(env_dir.as_os_str(), "{ENV}")
        .with(shims.as_os_str(), "{SHIMS}")
        .with(tmp.as_os_str(), "{TMP}")
        .with(home.as_os_str(), "{HOME}")
        .with(tag, "{TAG}");
    match uid_gid() {
        Some(u) => s.with(u, "{UID}:{GID}"),
        None => s,
    }
}

/// The variables of `env` that differ from `base` (`None`: removed), as a recorded call.
fn as_recorded(argv: Vec<OsString>, cwd: &Path, env: &Env, base: &Env) -> Recorded {
    let mut delta: Vec<(OsString, Option<OsString>)> = Vec::new();
    for (k, v) in env.iter() {
        if base.get(&k.to_string_lossy()) != Some(v) {
            delta.push((k.to_os_string(), Some(v.to_os_string())));
        }
    }
    for (k, _) in base.iter() {
        if env.get(&k.to_string_lossy()).is_none() {
            delta.push((k.to_os_string(), None));
        }
    }
    delta.sort();
    Recorded {
        argv,
        cwd: cwd.to_path_buf(),
        env_delta: delta,
    }
}

/// Files the shim side captured: (basename, bytes as text).
type Captures = Vec<(String, String)>;

/// Captures every `script.R` / `nuget.config` argument of a recorded call.
fn capture_args(r: &Recorded, into: &Mutex<Captures>) {
    for a in &r.argv {
        let p = Path::new(a);
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        if let Some(name) = name
            && (name == "script.R" || name == "nuget.config")
            && p.is_file()
        {
            let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned();
            into.lock().unwrap().push((name, text));
        }
    }
}

/// The value of `name` in a recorded call's environment delta.
fn delta_var(r: &Recorded, name: &str) -> Option<OsString> {
    r.env_delta
        .iter()
        .find(|(k, _)| k == name)
        .and_then(|(_, v)| v.clone())
}

/// snp's side of scenario `s`: (golden calls, captures).
fn snp_side(s: &Scenario) -> (Vec<GoldenCall>, Captures) {
    let fx = Fx::new(&[]);
    let captures: Arc<Mutex<Captures>> = Arc::new(Mutex::new(Vec::new()));
    let runner = &fx.t.runner;
    fx.tool("cs");
    runner.reply(
        "luarocks",
        Reply::ok(LUA_VERSION).when(["config", "--lua-ver"]).sticky(),
    );
    runner.reply(
        "docker",
        Reply::ok(DOCKER_INFO).when(["system", "info"]).sticky(),
    );
    let cap = Arc::clone(&captures);
    runner.reply(
        "Rscript",
        Reply::ok(R_VERSION)
            .when(["--no-save"])
            .sticky()
            .with_effect(move |r| {
                capture_args(r, &cap);
                Ok(())
            }),
    );
    let cap = Arc::clone(&captures);
    runner.reply(
        "Rscript",
        Reply::ok("").sticky().with_effect(move |r| {
            capture_args(r, &cap);
            Ok(())
        }),
    );
    runner.reply(
        "dotnet",
        Reply::ok("").when(["pack"]).sticky().with_effect(|r| {
            let arg = r.argv[5].to_string_lossy().into_owned();
            let out = PathBuf::from(arg.split_once('=').unwrap().1);
            std::fs::create_dir_all(&out)?;
            let f = std::fs::File::create(out.join("my.tool.1.0.0.nupkg"))?;
            let mut z = zip::ZipWriter::new(f);
            z.start_file("my.tool.nuspec", zip::write::SimpleFileOptions::default())
                .map_err(std::io::Error::other)?;
            std::io::Write::write_all(&mut z, NUSPEC.as_bytes())?;
            z.finish().map_err(std::io::Error::other)?;
            Ok(())
        }),
    );
    let cap = Arc::clone(&captures);
    runner.reply(
        "dotnet",
        Reply::ok("").sticky().with_effect(move |r| {
            capture_args(r, &cap);
            Ok(())
        }),
    );
    runner.reply(
        "dart",
        Reply::ok("")
            .when(["pub", "cache", "add"])
            .sticky()
            .with_effect(|r| {
                let dep = r.argv[4].to_string_lossy().into_owned();
                let ver = r
                    .argv
                    .get(6)
                    .map_or_else(|| "0.0.1".to_owned(), |v| v.to_string_lossy().into_owned());
                let cache = PathBuf::from(delta_var(r, "PUB_CACHE").unwrap());
                let d = cache
                    .join("hosted")
                    .join("pub.dev")
                    .join(format!("{dep}-{ver}"));
                std::fs::create_dir_all(&d)?;
                std::fs::write(
                    d.join("pubspec.yaml"),
                    format!("name: {dep}\nexecutables:\n  {dep}_tool:\n  aliased: other\n"),
                )
            }),
    );

    let id = s.id;
    let env_dir = fx.env_dir(id, "default");
    fx.install(id, s.deps).unwrap();
    if id == LanguageId::R {
        // pre-commit's post-install health check (no recorded state: the slow path).
        assert_eq!(fx.health(id, None).unwrap(), Health::Healthy);
    }
    let env = fx.activated(id).unwrap();
    let before = runner.calls().len();
    let plan = fx.plan(id, s.entry, s.args, true, &env).unwrap();
    let RunPlan::Xargs {
        cmd, env: run_env, pre, ..
    } = plan
    else {
        panic!("{plan:?}")
    };
    let all = runner.calls();
    let mut recorded: Vec<Recorded> = all[..before].to_vec();
    for p in &pre {
        recorded.push(as_recorded(p.argv.clone(), &p.cwd, &p.env, &fx.t.env));
    }
    recorded.extend(all[before..].iter().cloned());
    let mut argv = cmd.to_vec();
    argv.push("a.txt".into());
    recorded.push(as_recorded(argv, &fx.repo, &run_env, &fx.t.env));

    let tag = snp_lang::langs::docker::tag_for_prefix(&fx.prefix);
    let subst = substitutions(
        &fx.repo, &fx.prefix, &env_dir, &fx.shims, &fx.tmp, &fx.home, &tag,
    );
    let mut calls: Vec<GoldenCall> = recorded
        .iter()
        .map(|r| GoldenCall::from_recorded(r, &subst))
        .collect();
    normalize_tmp(&mut calls);
    let caps = captures
        .lock()
        .unwrap()
        .iter()
        .map(|(n, t)| (n.clone(), subst.apply(t)))
        .collect();
    (calls, caps)
}

/// The committed golden of `lang`.
fn theirs(lang: &str) -> (Vec<GoldenCall>, Captures) {
    let log = std::fs::read_to_string(golden_log(lang)).unwrap();
    let calls = parse_log(&log, &Substitutions::new()).unwrap();
    let caps: Captures = std::fs::read(golden_captures(lang))
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap_or_default();
    (calls, caps)
}

/// Compares one scenario.
fn check(lang: &str) {
    let s = SCENARIOS.iter().find(|s| s.lang == lang).unwrap();
    let (ours, our_caps) = snp_side(s);
    let (golden, golden_caps) = theirs(lang);
    assert_eq!(render_log(&ours), render_log(&golden), "{lang}: commands");
    assert_eq!(our_caps, golden_caps, "{lang}: captured files");
}

#[cfg(unix)]
#[test]
fn oracle_golden_conda() {
    check("conda");
}

#[cfg(unix)]
#[test]
fn oracle_golden_coursier() {
    check("coursier");
}

#[cfg(unix)]
#[test]
fn oracle_golden_dart() {
    check("dart");
}

#[cfg(unix)]
#[test]
fn oracle_golden_docker() {
    check("docker");
}

#[cfg(unix)]
#[test]
fn oracle_golden_dotnet() {
    check("dotnet");
}

#[cfg(unix)]
#[test]
fn oracle_golden_haskell() {
    check("haskell");
}

#[cfg(unix)]
#[test]
fn oracle_golden_julia() {
    check("julia");
}

#[cfg(unix)]
#[test]
fn oracle_golden_lua() {
    check("lua");
}

#[cfg(unix)]
#[test]
fn oracle_golden_perl() {
    check("perl");
}

#[cfg(unix)]
#[test]
fn oracle_golden_r() {
    check("r");
}

#[cfg(unix)]
#[test]
fn oracle_golden_swift() {
    check("swift");
}

/// The pre-commit 4.6.2 venv.
#[cfg(unix)]
fn oracle_venv() -> PathBuf {
    #[expect(clippy::disallowed_methods, reason = "test switch naming the oracle")]
    let over = std::env::var_os("SNP_ORACLE_PC462");
    over.map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oracle/pc462-py312"),
        PathBuf::from,
    )
}

/// Records scenario `s` with pre-commit 4.6.2.
#[cfg(unix)]
fn record(s: &Scenario) -> (Vec<GoldenCall>, Captures) {
    use std::fmt::Write as _;
    use std::os::unix::fs::PermissionsExt;
    let venv = oracle_venv();
    let python = venv.join("bin/python");
    let pre_commit = venv.join("bin/pre-commit");
    assert!(
        pre_commit.exists(),
        "run `cargo xtask oracle install` or set SNP_ORACLE_PC462"
    );
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let repo = root.join("repo");
    let shims = root.join("shims");
    let real = root.join("shims-real");
    let tmp = root.join("tmp");
    let home = root.join("home");
    let capture = root.join("capture");
    let store = root.join("store");
    for p in [&repo, &shims, &real, &tmp, &home, &capture] {
        std::fs::create_dir_all(p).unwrap();
    }
    snp_lang::harness::install_shims(&real, &TOOLS, &python).unwrap();
    snp_lang::harness::install_shims(&shims, &["fakehook"], &python).unwrap();
    let wrapper = format!(
        "#!{} -I\n{}",
        python.display(),
        WRAPPER_PY.replace("__NUSPEC__", NUSPEC)
    );
    for tool in TOOLS {
        let p = shims.join(tool);
        std::fs::write(&p, &wrapper).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(repo.join("a.txt"), b"a\n").unwrap();
    let quote = |xs: &[&str]| -> String {
        xs.iter()
            .map(|a| format!("'{a}'"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut config = String::from("repos:\n-   repo: local\n    hooks:\n");
    let _ignored = write!(
        config,
        "    -   id: h\n        name: h\n        entry: {}\n        language: {}\n        args: [{}]\n        \
         additional_dependencies: [{}]\n        files: \\.txt$\n        require_serial: true\n",
        s.entry,
        s.lang,
        quote(s.args),
        quote(s.deps)
    );
    std::fs::write(repo.join(".pre-commit-config.yaml"), config).unwrap();
    std::fs::write(repo.join(".gitignore"), b"").unwrap();

    let path = format!("{}:/usr/bin:/bin", shims.display());
    let mut env: BTreeMap<String, String> = [
        ("PATH", path.as_str()),
        ("HOME", &home.display().to_string()),
        ("LANG", "C.UTF-8"),
        ("TMPDIR", &tmp.display().to_string()),
        ("PRE_COMMIT_HOME", &store.display().to_string()),
        ("GIT_AUTHOR_NAME", "t"),
        ("GIT_AUTHOR_EMAIL", "t@t"),
        ("GIT_COMMITTER_NAME", "t"),
        ("GIT_COMMITTER_EMAIL", "t@t"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    let mut base: Vec<(String, String)> = env.clone().into_iter().collect();
    base.push(("PRE_COMMIT".into(), "1".into()));
    let base_file = root.join("base.json");
    snp_lang::harness::write_base_env(&base_file, &Env::from_pairs(base)).unwrap();
    let replies = root.join("replies.json");
    std::fs::write(
        &replies,
        serde_json::json!([
            {"tool": "luarocks", "when": ["config", "--lua-ver"], "stdout": LUA_VERSION},
            {"tool": "docker", "when": ["system", "info"], "stdout": DOCKER_INFO},
            {"tool": "Rscript", "when": ["--no-save"], "stdout": R_VERSION},
        ])
        .to_string(),
    )
    .unwrap();
    let log = root.join("log.jsonl");
    for (k, v) in [
        ("SNP_SHIM_LOG", &log),
        ("SNP_SHIM_BASE", &base_file),
        ("SNP_SHIM_REPLIES", &replies),
        ("SNP_SHIM_REAL", &real),
        ("SNP_SHIM_CAPTURE", &capture),
    ] {
        env.insert(k.into(), v.display().to_string());
    }
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .env_clear()
            .envs(&env)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    let _ignored = std::fs::remove_file(&log);
    let out = std::process::Command::new(&pre_commit)
        .args(["run", "--all-files"])
        .env_clear()
        .envs(&env)
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The hook repository pre-commit created for `local`.
    let q = std::process::Command::new(&python)
        .args([
            "-I",
            "-c",
            "import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute(\
             \"select path from repos where repo like 'local%'\").fetchone()[0])",
        ])
        .arg(store.join("db.db"))
        .output()
        .unwrap();
    assert!(q.status.success(), "{q:?}");
    let prefix = PathBuf::from(String::from_utf8(q.stdout).unwrap().trim());
    let env_dir = prefix.join(format!("{}-default", s.id.env_dir_base().unwrap()));
    let tag = snp_lang::common::docker_tag(&prefix.file_name().unwrap().to_string_lossy());
    let subst = substitutions(&repo, &prefix, &env_dir, &shims, &tmp, &home, &tag);
    let mut calls = parse_log(&std::fs::read_to_string(&log).unwrap(), &subst).unwrap();
    // pre-commit probes the Lua version on every activation; snp once per process.
    let mut seen_lua = false;
    calls.retain(|c| {
        let is_probe = c.argv == ["luarocks", "config", "--lua-ver"];
        let keep = !(is_probe && seen_lua);
        seen_lua |= is_probe;
        keep
    });
    normalize_tmp(&mut calls);
    let mut names: Vec<PathBuf> = std::fs::read_dir(&capture)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    let caps: Captures = names
        .iter()
        .map(|p| {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            let base = n.split_once('-').unwrap().1.to_owned();
            let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned();
            (base, subst.apply(&text))
        })
        .collect();
    (calls, caps)
}

/// Records every scenario with pre-commit 4.6.2 and compares with (or, with `SNP_UPDATE_GOLDENS=1`, rewrites) the
/// committed goldens.
#[cfg(unix)]
#[test]
#[ignore = "oracle"]
fn oracle_group2_goldens_are_current() {
    #[expect(
        clippy::disallowed_methods,
        reason = "test switch to rewrite the golden"
    )]
    let update = std::env::var_os("SNP_UPDATE_GOLDENS").is_some();
    std::fs::create_dir_all(golden_dir()).unwrap();
    for s in &SCENARIOS {
        let (calls, caps) = record(s);
        let rendered = render_log(&calls);
        let caps_json = if caps.is_empty() {
            None
        } else {
            Some(serde_json::to_string_pretty(&caps).unwrap() + "\n")
        };
        if update {
            std::fs::write(golden_log(s.lang), &rendered).unwrap();
            match &caps_json {
                Some(j) => std::fs::write(golden_captures(s.lang), j).unwrap(),
                None => {
                    let _ignored = std::fs::remove_file(golden_captures(s.lang));
                }
            }
        }
        assert_eq!(
            std::fs::read_to_string(golden_log(s.lang)).unwrap(),
            rendered,
            "{}",
            s.lang
        );
        assert_eq!(
            std::fs::read_to_string(golden_captures(s.lang)).ok(),
            caps_json,
            "{}",
            s.lang
        );
    }
}
