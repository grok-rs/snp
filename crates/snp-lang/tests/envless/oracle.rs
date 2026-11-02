//! The env-less argv against pre-commit 4.7.0, through the shim harness (04b §5.12).
//!
//! Scenario: a repository with one `a.txt` and four local hooks — `docker_image`, `system` (alias of
//! `unsupported`), `script` (alias of `unsupported_script`, a committed script) and `fail`-free — run with
//! `require_serial: true` (no shuffle) and fake `docker`/`fakehook` tools on `PATH` that log every call in the golden
//! log format v1. `golden/oracle_envless.jsonl` is the log pre-commit 4.7.0 produced (`cargo test -p snp-lang
//! --features testing --test envless -- --ignored oracle_`, with `SNP_UPDATE_GOLDENS=1` to rewrite it); the hermetic
//! test renders snp's side — the rootless probe recorded by the `ShimRunner`, then each plan's argv with the file
//! appended — and requires equality.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::harness::{GoldenCall, Reply, Substitutions, TestHost, parse_log, render_log};
use snp_lang::{PlanCtx, RunPlan, backend};

/// The committed golden.
fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/envless/golden/oracle_envless.jsonl")
}

/// What the fake docker answers to `docker system info --format '{{ json . }}'`.
const INFO: &str = r#"{"SecurityOptions": ["name=seccomp,profile=builtin"]}"#;

/// The hooks: (id, language, entry, args).
const HOOKS: [(&str, &str, &str, &[&str]); 3] = [
    (
        "img",
        "docker_image",
        "--entrypoint echo alpine:3 hi",
        &["--flag"],
    ),
    ("sys", "system", "fakehook --flag", &["arg one"]),
    ("script", "script", "hooks/check.sh x", &[]),
];

/// The base environment of both sides (snp-run and `pre-commit run` add `PRE_COMMIT=1`).
fn base_env(home: &Path, path: &str) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), path.into()),
        ("HOME".into(), home.display().to_string()),
        ("LANG".into(), "C.UTF-8".into()),
    ]
}

/// Placeholders for the paths and ids that differ between runs and machines.
fn substitutions(repo: &Path) -> Substitutions {
    let s = Substitutions::new().with(repo.as_os_str(), "{REPO}");
    #[cfg(unix)]
    let s = s.with(
        format!(
            "{}:{}",
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw()
        ),
        "{UID}:{GID}",
    );
    s
}

/// snp's side of the scenario: the run's base env (with `PRE_COMMIT=1`, as snp-run builds it) is the delta base, like
/// `$SNP_SHIM_BASE` on the pre-commit side.
fn snp_side(repo: &Path) -> Vec<GoldenCall> {
    let home = repo.join("home");
    let mut base: Vec<(String, String)> = base_env(&home, "/usr/bin:/bin");
    base.push(("PRE_COMMIT".into(), "1".into()));
    let mut t = TestHost::new(repo, Env::from_pairs(base));
    let empty = repo.join(".no-mountinfo");
    std::fs::write(&empty, b"").unwrap();
    t.probe = snp_lang::ToolProbe::new(None).with_mountinfo(empty);
    t.runner.reply(
        "docker",
        Reply::ok(INFO.as_bytes().to_vec())
            .when(["system", "info"])
            .sticky(),
    );
    let subst = substitutions(repo);
    let host = t.host();
    let mut calls = Vec::new();
    let mut seen = 0_usize;
    for (_, lang, entry, args) in HOOKS {
        let id = LanguageId::from_config(lang).unwrap();
        let args: Vec<Arc<str>> = args.iter().map(|a| Arc::from(*a)).collect();
        let plan = backend(id)
            .plan(&PlanCtx {
                host: &host,
                prefix: repo,
                version: "default",
                env_dir: None,
                env: &t.env,
                entry,
                args: &args,
                is_local: true,
                color: false,
                docker_tag: None,
            })
            .unwrap();
        // The setup/probe calls of this plan (recorded by the shim runner), then the hook run itself.
        let shim = t.runner.golden(&subst);
        calls.extend(shim.iter().skip(seen).cloned());
        seen = shim.len();
        let RunPlan::Xargs { cmd, env, .. } = plan else {
            panic!("{plan:?}")
        };
        let mut argv = cmd.to_vec();
        argv.push("a.txt".into());
        let run = snp_lang::harness::Recorded {
            argv,
            cwd: repo.to_path_buf(),
            env_delta: env
                .iter()
                .filter(|(k, v)| t.env.get(&k.to_string_lossy()) != Some(*v))
                .map(|(k, v)| (k.to_os_string(), Some(v.to_os_string())))
                .collect(),
        };
        calls.push(GoldenCall::from_recorded(&run, &subst));
    }
    calls
}

#[test]
fn snp_matches_the_pre_commit_golden() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let ours = snp_side(&repo);
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    let mut theirs = parse_log(&golden, &Substitutions::new()).unwrap();
    // The golden is POSIX pre-commit's; on Windows `docker.get_docker_user()` finds no `os.getuid` and adds no `-u`.
    if cfg!(windows) {
        for call in &mut theirs {
            if let Some(i) = call.argv.iter().position(|a| a == "-u") {
                let user = [i, i.saturating_add(1)];
                call.argv = std::mem::take(&mut call.argv)
                    .into_iter()
                    .enumerate()
                    .filter(|(j, _)| !user.contains(j))
                    .map(|(_, a)| a)
                    .collect();
            }
        }
    }
    assert_eq!(render_log(&ours), render_log(&theirs));
}

/// Records the golden from pre-commit 4.7.0 (`target/oracle/pc470-py312`).
#[cfg(unix)]
#[test]
#[ignore = "oracle"]
fn oracle_envless_golden_is_current() {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    use std::os::unix::fs::PermissionsExt;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oracle/pc470-py312");
    let python = root.join("bin/python");
    let pre_commit = root.join("bin/pre-commit");
    assert!(pre_commit.exists(), "run `cargo xtask oracle install`");
    let d = tempfile::tempdir().unwrap();
    let base_dir = d.path().canonicalize().unwrap();
    let repo = base_dir.join("repo");
    let shims = base_dir.join("shims");
    let home = repo.join("home");
    std::fs::create_dir_all(repo.join("hooks")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    snp_lang::harness::install_shims(&shims, &["docker", "fakehook"], &python).unwrap();
    snp_lang::harness::install_shims(&repo.join("hooks"), &["check.sh"], &python).unwrap();
    std::fs::set_permissions(
        repo.join("hooks/check.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(repo.join("a.txt"), b"a\n").unwrap();
    let mut config = String::from("repos:\n-   repo: local\n    hooks:\n");
    for (id, lang, entry, args) in HOOKS {
        let args: Vec<String> = args.iter().map(|a| format!("'{a}'")).collect();
        let _ignored = write!(
            config,
            "    -   id: {id}\n        name: {id}\n        entry: {entry}\n        language: {lang}\n        \
             args: [{}]\n        files: \\.txt$\n        require_serial: true\n",
            args.join(", ")
        );
    }
    std::fs::write(repo.join(".pre-commit-config.yaml"), config).unwrap();
    std::fs::write(repo.join(".gitignore"), b"home/\n").unwrap();

    let path = format!("{}:/usr/bin:/bin", shims.display());
    let base = base_env(&home, &path);
    let mut marker = base.clone();
    marker.push(("PRE_COMMIT".into(), "1".into()));
    let base_file = base_dir.join("base.json");
    snp_lang::harness::write_base_env(&base_file, &Env::from_pairs(marker)).unwrap();
    let replies = base_dir.join("replies.json");
    std::fs::write(
        &replies,
        serde_json::json!([{"tool": "docker", "when": ["system", "info"], "status": 0, "stdout": INFO}]).to_string(),
    )
    .unwrap();
    let log = base_dir.join("log.jsonl");
    let mut env: BTreeMap<String, String> = base.into_iter().collect();
    for (k, v) in [
        (
            "PRE_COMMIT_HOME",
            base_dir.join("store").display().to_string(),
        ),
        ("SNP_SHIM_LOG", log.display().to_string()),
        ("SNP_SHIM_BASE", base_file.display().to_string()),
        ("SNP_SHIM_REPLIES", replies.display().to_string()),
        ("GIT_AUTHOR_NAME", "t".into()),
        ("GIT_AUTHOR_EMAIL", "t@t".into()),
        ("GIT_COMMITTER_NAME", "t".into()),
        ("GIT_COMMITTER_EMAIL", "t@t".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
    ] {
        env.insert(k.into(), v);
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
    // The log of git's own commit hooks is not part of the scenario.
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
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let theirs = parse_log(
        &std::fs::read_to_string(&log).unwrap(),
        &substitutions(&repo),
    )
    .unwrap();
    // The PATH of pre-commit's children starts with the shims directory; snp's scenario has none.
    let theirs: Vec<GoldenCall> = theirs
        .into_iter()
        .map(|c| c.only_env(&["PRE_COMMIT"]))
        .collect();
    let rendered = render_log(&theirs);
    #[expect(
        clippy::disallowed_methods,
        reason = "test switch to rewrite the golden"
    )]
    let update = std::env::var_os("SNP_UPDATE_GOLDENS").is_some();
    if update {
        std::fs::create_dir_all(golden_path().parent().unwrap()).unwrap();
        std::fs::write(golden_path(), &rendered).unwrap();
    }
    assert_eq!(std::fs::read_to_string(golden_path()).unwrap(), rendered);
}
