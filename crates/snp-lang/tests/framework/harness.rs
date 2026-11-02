//! Shim harness self-test (WBS WP-16 acceptance: "recording and replay are deterministic; golden log format
//! documented"; 04b §5.12). Needs the `testing` feature (CI enables it through `snp/testing`).
#![cfg(feature = "testing")]

use std::ffi::OsString;
use std::path::Path;

use snp_core::env::Env;
use snp_lang::common;
use snp_lang::harness::{
    Reply, ShimRunner, Substitutions, TempToolDirs, parse_log, render_log, tool_name,
};
use snp_lang::{CmdRunner, LangError, ToolDirs};

fn argv(xs: &[&str]) -> Vec<OsString> {
    xs.iter().map(OsString::from).collect()
}

fn base() -> Env {
    Env::from_pairs([
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/u"),
        ("LANG", "C.UTF-8"),
    ])
}

/// (status, stdout, stderr) of each call.
type Outputs = Vec<(i64, Vec<u8>, Vec<u8>)>;

/// One scripted session; returns the runner and the outputs it produced.
fn session() -> (ShimRunner, Outputs) {
    let r = ShimRunner::new(base());
    r.reply(
        "conda",
        Reply::fail(2, b"solver failed".to_vec()).when(["env", "create"]),
    );
    r.reply(
        "conda",
        Reply::ok(b"installed".to_vec()).when(["install"]).sticky(),
    );
    r.reply("luarocks", Reply::ok(b"5.4\n".to_vec()));
    let activated = base().patched(
        &snp_core::env::EnvPatch::default()
            .set("CONDA_PREFIX", "/env")
            .unset("HOME")
            .prepend_path("PATH", ["/env/bin".into()]),
    );
    let mut outs = Vec::new();
    for (a, env) in [
        (vec!["conda", "env", "create", "-p", "/env"], base()),
        (vec!["conda", "env", "create", "-p", "/env"], base()),
        (
            vec!["/usr/bin/conda", "install", "-p", "/env", "six"],
            activated.clone(),
        ),
        (vec!["conda.exe", "install", "x"], activated),
        (vec!["luarocks", "config", "--lua-ver"], base()),
        (vec!["luarocks", "config", "--lua-ver"], base()),
    ] {
        let out = r.run(&argv(&a), &env, Path::new("/work/prefix")).unwrap();
        outs.push((out.status.0, out.stdout, out.stderr));
    }
    (r, outs)
}

#[test]
fn replay_matches_in_order_and_records_deltas() {
    let (r, outs) = session();
    assert_eq!(
        outs,
        [
            (2, Vec::new(), b"solver failed".to_vec()),
            // Consumed: the second identical call gets the default answer.
            (0, Vec::new(), Vec::new()),
            (0, b"installed".to_vec(), Vec::new()),
            // Sticky.
            (0, b"installed".to_vec(), Vec::new()),
            (0, b"5.4\n".to_vec(), Vec::new()),
            (0, Vec::new(), Vec::new()),
        ]
    );
    let calls = r.calls();
    assert_eq!(calls.len(), 6);
    assert_eq!(calls[0].env_delta.len(), 0);
    assert_eq!(
        calls[2].env_delta,
        [
            (OsString::from("CONDA_PREFIX"), Some(OsString::from("/env"))),
            (OsString::from("HOME"), None),
            (
                OsString::from("PATH"),
                Some(OsString::from(if cfg!(windows) {
                    "/env/bin;/usr/bin:/bin"
                } else {
                    "/env/bin:/usr/bin:/bin"
                }))
            ),
        ]
    );
    assert_eq!(calls[2].cwd, Path::new("/work/prefix"));
}

#[test]
fn recording_is_deterministic_and_the_golden_log_round_trips() {
    let subst = Substitutions::new()
        .with("/work/prefix", "{PREFIX}")
        .with("/env", "{ENV}");
    let (a, _) = session();
    let (b, _) = session();
    let log = a.golden_log(&subst);
    assert_eq!(log, b.golden_log(&subst));
    let first = log.lines().next().unwrap();
    assert_eq!(
        first,
        r#"{"argv":["conda","env","create","-p","{ENV}"],"cwd":"{PREFIX}","env":{}}"#
    );
    let third = log.lines().nth(2).unwrap();
    assert!(third.starts_with(r#"{"argv":["conda","install","-p","{ENV}","six"],"cwd":"{PREFIX}","env":{"CONDA_PREFIX":"{ENV}","HOME":null,"PATH":"{ENV}/bin"#), "{third}");
    let parsed = parse_log(&log, &Substitutions::new()).unwrap();
    assert_eq!(parsed, a.golden(&subst));
    assert_eq!(render_log(&parsed), log);
    assert!(parse_log("{oops", &subst).is_err());
}

#[test]
fn tool_names() {
    assert_eq!(tool_name("/usr/bin/conda".as_ref()), "conda");
    assert_eq!(
        tool_name("C:\\x\\npm.CMD".as_ref()),
        if cfg!(windows) { "npm" } else { "C:\\x\\npm" }
    );
    assert_eq!(tool_name("dart.exe".as_ref()), "dart");
    assert_eq!(tool_name(".exe".as_ref()), ".exe");
}

#[test]
fn effects_run_before_the_answer_and_their_errors_fail_the_call() {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("created");
    let m2 = marker.clone();
    let r = ShimRunner::default();
    r.reply(
        "uv",
        Reply::ok(Vec::new())
            .when(["venv"])
            .with_effect(move |call| {
                assert_eq!(call.argv[1], "venv");
                std::fs::write(&m2, b"venv")
            }),
    );
    r.reply(
        "uv",
        Reply::ok(Vec::new())
            .when(["pip"])
            .with_effect(|_| Err(std::io::Error::other("boom"))),
    );
    r.run(
        &argv(&["uv", "venv", "/e"]),
        &Env::from_pairs::<&str, &str>([]),
        d.path(),
    )
    .unwrap();
    assert!(marker.exists());
    let e = r
        .run(
            &argv(&["uv", "pip", "install"]),
            &Env::from_pairs::<&str, &str>([]),
            d.path(),
        )
        .unwrap_err();
    assert!(
        matches!(
            e,
            LangError::Io {
                op: "shim effect",
                ..
            }
        ),
        "{e:?}"
    );
    // With the default base (empty), the delta is the whole environment.
    let calls = r.calls();
    assert_eq!(calls[0].env_delta.len(), 0);
}

#[test]
fn setup_cmd_through_the_shim_runner() {
    let r = ShimRunner::new(base());
    r.reply("cpan", Reply::fail(1, b"\x81\xfe".to_vec()));
    let e = common::setup_cmd(
        &r,
        &argv(&["cpan", "-T", "."]),
        &Env::from_pairs([("PATH", "")]),
        Path::new("/p"),
    )
    .unwrap_err();
    assert!(matches!(e, LangError::CalledProcess { rc: 1, .. }));
    assert_eq!(
        common::setup_cmd(&r, &argv(&["cpan", "x"]), &base(), Path::new("/p")).unwrap(),
        b""
    );
}

#[test]
fn temp_tool_dirs() {
    let d = tempfile::tempdir().unwrap();
    let t = TempToolDirs::new(d.path());
    assert_eq!(
        t.tool_dir("node", "22.11.0"),
        d.path().join("tools").join("node").join("22.11.0")
    );
    assert_eq!(t.cache_dir("uv"), d.path().join("cache").join("uv"));
    let c = snp_core::cancel::CancelToken::new();
    let lock = t.lock_tool("node", &c).unwrap();
    assert!(d.path().join("tools/node.lock").exists());
    drop(lock);
    let _again = t.lock_tool("node", &c).unwrap();
}

/// The Python logger used on the pre-commit side writes exactly the format the Rust side renders (needs a Python 3
/// interpreter; the oracle's when installed, else the host's).
#[cfg(unix)]
#[test]
fn the_python_logger_speaks_the_same_format() {
    use std::collections::BTreeMap;

    use snp_lang::harness::GoldenCall;
    let oracle =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oracle/pc462-py312/bin/python");
    // Not macOS's /usr/bin/python3: that xcrun shim adds CPATH, LIBRARY_PATH and MANPATH to the environment of the
    // Python it execs, which the logger would then record.
    let host = (!cfg!(target_os = "macos")).then_some(Path::new("/usr/bin/python3"));
    let python = std::iter::once(oracle.as_path())
        .chain(host)
        .find(|p| p.exists());
    let Some(python) = python else {
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let shims = d.path().join("shims");
    snp_lang::harness::install_shims(&shims, &["conda", "docker"], python).unwrap();
    let base_file = d.path().join("base.json");
    let base_env = Env::from_pairs([
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/u"),
        ("LANG", "C.UTF-8"),
    ]);
    snp_lang::harness::write_base_env(&base_file, &base_env).unwrap();
    let replies = d.path().join("replies.json");
    std::fs::write(
        &replies,
        br#"[{"tool": "docker", "when": ["system", "info"], "status": 3, "stdout": "{}", "stderr": "e"}]"#,
    )
    .unwrap();
    let log = d.path().join("log.jsonl");
    let child_env: BTreeMap<&str, String> = [
        ("PATH", "/usr/bin:/bin".to_owned()),
        ("LANG", "C.UTF-8".to_owned()),
        ("CONDA_PREFIX", "/env".to_owned()),
        ("SNP_SHIM_LOG", log.display().to_string()),
        ("SNP_SHIM_BASE", base_file.display().to_string()),
        ("SNP_SHIM_REPLIES", replies.display().to_string()),
    ]
    .into_iter()
    .collect();
    let out = std::process::Command::new(shims.join("conda"))
        .args(["install", "-p", "/env", "six"])
        .env_clear()
        .envs(&child_env)
        .current_dir(d.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let out = std::process::Command::new(shims.join("docker"))
        .args(["system", "info", "--format", "{{ json . }}"])
        .env_clear()
        .envs(&child_env)
        .current_dir(d.path())
        .output()
        .unwrap();
    assert_eq!(
        (out.status.code(), &out.stdout[..], &out.stderr[..]),
        (Some(3), &b"{}"[..], &b"e"[..])
    );

    // The same calls through the Rust runner.
    let r = ShimRunner::new(base_env.clone());
    let env = base_env.patched(
        &snp_core::env::EnvPatch::default()
            .set("CONDA_PREFIX", "/env")
            .unset("HOME"),
    );
    let real_cwd = d.path().canonicalize().unwrap();
    r.run(
        &argv(&["conda", "install", "-p", "/env", "six"]),
        &env,
        &real_cwd,
    )
    .unwrap();
    r.run(
        &argv(&["docker", "system", "info", "--format", "{{ json . }}"]),
        &env,
        &real_cwd,
    )
    .unwrap();
    let subst = Substitutions::new().with(real_cwd.as_os_str(), "{CWD}");
    let python_side: Vec<GoldenCall> =
        parse_log(&std::fs::read_to_string(&log).unwrap(), &subst).unwrap();
    assert_eq!(python_side, r.golden(&subst));
    assert_eq!(render_log(&python_side), r.golden_log(&subst));
}
