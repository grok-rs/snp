//! Env-less plan goldens: `fail`/`pygrep` plans, `hook_cmd` (+ hazmat rewrite, C-hook errors), the
//! `unsupported_script` prefix join and the `docker_image` argv (rootless probe, tty, container path mapping).

use std::path::Path;
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
#[cfg(unix)]
use snp_lang::harness::Reply;
use snp_lang::harness::TestHost;
use snp_lang::{PlanCtx, RunPlan, backend};

/// `language`'s plan of `entry` + `args` with `prefix`.
fn plan(
    t: &TestHost,
    id: LanguageId,
    prefix: &Path,
    entry: &str,
    args: &[&str],
    color: bool,
) -> RunPlan {
    let args: Vec<Arc<str>> = args.iter().map(|a| Arc::from(*a)).collect();
    let host = t.host();
    backend(id)
        .plan(&PlanCtx {
            host: &host,
            prefix,
            version: "default",
            env_dir: None,
            env: &t.env,
            entry,
            args: &args,
            is_local: true,
            color,
            docker_tag: None,
        })
        .unwrap()
}

/// The command of an `Xargs` plan (lossy strings), after checking the plan's other fields.
fn cmd(t: &TestHost, p: &RunPlan) -> Vec<String> {
    let RunPlan::Xargs {
        cmd,
        env,
        shuffle,
        serial,
        pre,
    } = p
    else {
        panic!("not an xargs plan: {p:?}");
    };
    assert!(*shuffle && !*serial, "basic run_xargs semantics");
    assert!(pre.is_empty());
    assert_eq!(
        env.get("PATH"),
        t.env.get("PATH"),
        "the hook env is the base env (no activation)"
    );
    cmd.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// The C-hook error of a plan.
fn error(p: &RunPlan) -> (i32, String) {
    let RunPlan::Error { code, output } = p else {
        panic!("not an error plan: {p:?}");
    };
    (*code, String::from_utf8(output.clone()).unwrap())
}

fn host(root: &Path) -> TestHost {
    let empty = root.join("no-mountinfo");
    std::fs::write(&empty, b"").unwrap();
    let mut t = TestHost::new(
        root,
        Env::from_pairs([("PATH", "/usr/bin:/bin"), ("PRE_COMMIT", "1")]),
    );
    t.probe = snp_lang::ToolProbe::new(None).with_mountinfo(empty);
    t
}

#[test]
fn fail_plan() {
    let d = tempfile::tempdir().unwrap();
    let t = host(d.path());
    // fail_test.py:7-14: entry `watch out for`, files `bunnies` → (1, 'watch out for\n\nbunnies\n'): the executor
    // renders it from this plan, in-process.
    let p = plan(
        &t,
        LanguageId::Fail,
        d.path(),
        "watch out for",
        &["ignored"],
        true,
    );
    let RunPlan::Fail { entry } = p else {
        panic!("{p:?}")
    };
    assert_eq!(&*entry, "watch out for");
    let p = plan(
        &t,
        LanguageId::Fail,
        d.path(),
        "Files must not be txt\nsecond line",
        &[],
        false,
    );
    assert!(
        matches!(p, RunPlan::Fail { entry } if &*entry == "Files must not be txt\nsecond line")
    );
    assert!(t.runner.calls().is_empty());
}

#[test]
fn pygrep_plan() {
    let d = tempfile::tempdir().unwrap();
    let t = host(d.path());
    // The entry is the pattern: never shlex-split or hazmat-rewritten; args come first (`[*args, entry, *files]`).
    let p = plan(
        &t,
        LanguageId::Pygrep,
        d.path(),
        "pre-commit hazmat 'TODO",
        &["-i", "--multiline"],
        false,
    );
    let RunPlan::Pygrep { args, entry } = p else {
        panic!("{p:?}")
    };
    assert_eq!(&*entry, "pre-commit hazmat 'TODO");
    assert_eq!(
        args.iter().map(|a| &**a).collect::<Vec<_>>(),
        ["-i", "--multiline"]
    );
}

#[test]
fn unsupported_hook_cmd() {
    let d = tempfile::tempdir().unwrap();
    let t = host(d.path());
    let u = |entry: &str, args: &[&str]| {
        cmd(
            &t,
            &plan(&t, LanguageId::Unsupported, d.path(), entry, args, false),
        )
    };
    let exe = t.self_exe.to_string_lossy().into_owned();
    // lang_base_test.py:154-178.
    assert_eq!(u("echo hi", &["hello"]), ["echo", "hi", "hello"]);
    assert_eq!(u("echo hi", &[]), ["echo", "hi"]);
    // shlex.split: POSIX quoting, no comments, `\r` is whitespace, backslash kept inside double quotes before `$`.
    assert_eq!(u("a #b", &[]), ["a", "#b"]);
    assert_eq!(u("a \"x\\$y\"", &[]), ["a", "x\\$y"]);
    assert_eq!(u("a\rb ''", &[]), ["a", "b", ""]);
    assert_eq!(
        u("'tool' 'spaced arg'", &["x y"]),
        ["tool", "spaced arg", "x y"]
    );
    // The hazmat rewrite (03 §H.8.1): the first two tokens, any quoting, `pre-commit` or `snp`.
    assert_eq!(
        u("pre-commit hazmat cd a echo -- b", &[]),
        [exe.as_str(), "__hazmat", "cd", "a", "echo", "--", "b"]
    );
    assert_eq!(
        u("'pre-commit' \"hazmat\" n1 x --", &["y"]),
        [exe.as_str(), "__hazmat", "n1", "x", "--", "y"]
    );
    assert_eq!(
        u("snp hazmat ignore-exit-code false", &[]),
        [exe.as_str(), "__hazmat", "ignore-exit-code", "false"]
    );
    // Not rewritten: other subcommands, other spellings, args.
    assert_eq!(u("pre-commit run", &[]), ["pre-commit", "run"]);
    assert_eq!(
        u("pre-commit.exe hazmat cd", &[]),
        ["pre-commit.exe", "hazmat", "cd"]
    );
    assert_eq!(
        u("/usr/bin/pre-commit hazmat cd", &[]),
        ["/usr/bin/pre-commit", "hazmat", "cd"]
    );
    assert_eq!(u("pre-commit", &["hazmat"]), ["pre-commit", "hazmat"]);
    // An empty entry with args runs the args (verified in 13 X.4 EXE-01).
    assert_eq!(u("", &["echo", "hi"]), ["echo", "hi"]);
    assert_eq!(u("''", &[]), [""]);
}

#[test]
fn hook_cmd_errors_are_c_hook_plans() {
    let d = tempfile::tempdir().unwrap();
    let t = host(d.path());
    for id in [
        LanguageId::Unsupported,
        LanguageId::UnsupportedScript,
        LanguageId::DockerImage,
    ] {
        let e = |entry: &str| error(&plan(&t, id, d.path(), entry, &[], false));
        // X13-EXE-01: empty / whitespace-only entry without args.
        assert_eq!(e(""), (1, "Executable `` not found".to_owned()), "{id:?}");
        assert_eq!(e("   "), (1, "Executable `` not found".to_owned()));
        // X13-EXE-02: shlex errors, Python's last traceback line.
        assert_eq!(
            e("echo 'unterminated"),
            (1, "ValueError: No closing quotation".to_owned())
        );
        assert_eq!(
            e("a \"b"),
            (1, "ValueError: No closing quotation".to_owned())
        );
        assert_eq!(e("a\\"), (1, "ValueError: No escaped character".to_owned()));
    }
}

#[test]
fn unsupported_script_prefix_join() {
    let d = tempfile::tempdir().unwrap();
    let t = host(d.path());
    let prefix = Path::new("/store/repos/0123abcd/src");
    let s = |entry: &str, args: &[&str]| {
        cmd(
            &t,
            &plan(
                &t,
                LanguageId::UnsupportedScript,
                prefix,
                entry,
                args,
                false,
            ),
        )
    };
    if cfg!(windows) {
        assert_eq!(
            s("main.sh arg", &[]),
            ["\\store\\repos\\0123abcd\\src\\main.sh", "arg"]
        );
        return;
    }
    // unsupported_script.py:15-32: `prefix.path(cmd[0])` = normpath(join(prefix, cmd[0])).
    assert_eq!(
        s("main.sh arg", &["--x"]),
        ["/store/repos/0123abcd/src/main.sh", "arg", "--x"]
    );
    assert_eq!(
        s("./bin/../hook.sh", &[]),
        ["/store/repos/0123abcd/src/hook.sh"]
    );
    assert_eq!(
        s("hooks//nested/./x.py y", &[]),
        ["/store/repos/0123abcd/src/hooks/nested/x.py", "y"]
    );
    // An absolute first token discards the prefix.
    assert_eq!(s("/usr/bin/env python3", &[]), ["/usr/bin/env", "python3"]);
    // `..` is collapsed lexically (parity: the executor then reports "not found"/"is a directory").
    assert_eq!(s("../../outside.sh", &[]), ["/store/repos/outside.sh"]);
    // An empty first token is the prefix itself (→ "Executable `…` is a directory" at run time).
    assert_eq!(s("''", &[]), ["/store/repos/0123abcd/src"]);
    // The hazmat rewrite happens first; its absolute `self_exe` stays.
    let exe = t.self_exe.to_string_lossy().into_owned();
    assert_eq!(
        s("pre-commit hazmat cd sub cmd --", &[]),
        [exe.as_str(), "__hazmat", "cd", "sub", "cmd", "--"]
    );
}

#[cfg(unix)]
fn user() -> String {
    format!(
        "{}:{}",
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw()
    )
}

#[cfg(unix)]
#[test]
fn docker_image_argv() {
    let d = tempfile::tempdir().unwrap();
    let cwd = d.path().to_string_lossy().into_owned();
    let volume = format!("{cwd}:/src:rw,Z");
    let user = user();

    // Not rootless: `-u uid:gid`; the probe runs once per process (memoised).
    let t = host(d.path());
    t.runner.reply(
        "docker",
        Reply::ok(br#"{"SecurityOptions": ["name=seccomp,profile=builtin"]}"#.to_vec())
            .when(["system", "info"])
            .sticky(),
    );
    let c = cmd(
        &t,
        &plan(
            &t,
            LanguageId::DockerImage,
            d.path(),
            "--entrypoint sh alpine:3 -c 'echo hi'",
            &["--x"],
            false,
        ),
    );
    assert_eq!(
        c,
        [
            "docker",
            "run",
            "--rm",
            "-u",
            &user,
            "-v",
            &volume,
            "--workdir",
            "/src",
            "--entrypoint",
            "sh",
            "alpine:3",
            "-c",
            "echo hi",
            "--x"
        ]
    );
    // Colour adds `--tty` right after `--rm`.
    let c = cmd(
        &t,
        &plan(
            &t,
            LanguageId::DockerImage,
            d.path(),
            "ubuntu:22.04 cat",
            &[],
            true,
        ),
    );
    assert_eq!(c[..4], ["docker", "run", "--rm", "--tty"]);
    let calls = t.runner.calls();
    assert_eq!(calls.len(), 1, "one rootless probe per process");
    assert_eq!(
        calls[0].argv,
        ["docker", "system", "info", "--format", "{{ json . }}"]
    );
    assert_eq!(calls[0].cwd, d.path());

    // Rootless (docker and podman flavours): no `-u`.
    for info in [
        &br#"{"SecurityOptions": ["name=seccomp", "name=rootless"]}"#[..],
        br#"{"host": {"security": {"rootless": true}}}"#,
    ] {
        let t = host(d.path());
        t.runner
            .reply("docker", Reply::ok(info.to_vec()).when(["system", "info"]));
        let c = cmd(
            &t,
            &plan(&t, LanguageId::DockerImage, d.path(), "img", &[], false),
        );
        assert_eq!(
            c,
            [
                "docker",
                "run",
                "--rm",
                "-v",
                &volume,
                "--workdir",
                "/src",
                "img"
            ]
        );
    }

    // A failing probe (daemon down; pre-commit raises CalledProcessError, D-CRASH-21) means "not rootless".
    let t = host(d.path());
    t.runner.reply(
        "docker",
        Reply::fail(1, b"Cannot connect to the Docker daemon".to_vec()).when(["system", "info"]),
    );
    let c = cmd(
        &t,
        &plan(&t, LanguageId::DockerImage, d.path(), "img", &[], false),
    );
    assert_eq!(c[3..5], ["-u", user.as_str()]);
}

/// Running inside a container that talks to the host daemon: the cwd is mapped through the container's bind mounts
/// (`docker.py:45-66`; `docker_test.py` path cases).
#[cfg(unix)]
#[test]
fn docker_image_container_path_mapping() {
    const ID: &str = "c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7";
    let d = tempfile::tempdir().unwrap();
    let mountinfo = d.path().join("mountinfo");
    std::fs::write(
        &mountinfo,
        format!("767 759 8:5 /var/lib/docker/containers/{ID}/hostname /etc/hostname rw - ext4 /dev/sda5 rw\n"),
    )
    .unwrap();
    let inspect = br#"[{"Mounts": [{"Source": "/opt/my_code", "Destination": "/project"}, {"Source": "/x", "Destination": "/other"}]}]"#;
    let volume_for = |cwd: &str, reply: Reply| {
        let mut t = TestHost::new(Path::new(cwd), Env::from_pairs([("PATH", "/usr/bin")]));
        t.probe = snp_lang::ToolProbe::new(None).with_mountinfo(mountinfo.clone());
        t.runner
            .reply("docker", Reply::ok(b"{}".to_vec()).when(["system", "info"]));
        t.runner.reply("docker", reply.when(["inspect"]));
        let c = cmd(
            &t,
            &plan(
                &t,
                LanguageId::DockerImage,
                Path::new(cwd),
                "img",
                &[],
                false,
            ),
        );
        let calls = t.runner.calls();
        assert_eq!(calls.last().unwrap().argv, ["docker", "inspect", ID]);
        let v = c.iter().position(|a| a == "-v").unwrap();
        c[v + 1].clone()
    };
    assert_eq!(
        volume_for("/project", Reply::ok(inspect.to_vec())),
        "/opt/my_code:/src:rw,Z"
    );
    assert_eq!(
        volume_for("/project/test/something", Reply::ok(inspect.to_vec())),
        "/opt/my_code/test/something:/src:rw,Z"
    );
    // Component semantics: `/projectSuffix` is not below `/project`.
    assert_eq!(
        volume_for("/projectSuffix/x", Reply::ok(inspect.to_vec())),
        "/projectSuffix/x:/src:rw,Z"
    );
    // Docker-in-docker (inspect fails) and unexpected JSON keep the path.
    assert_eq!(
        volume_for("/project", Reply::fail(1, b"no such container".to_vec())),
        "/project:/src:rw,Z"
    );
    assert_eq!(
        volume_for("/project", Reply::ok(b"[]".to_vec())),
        "/project:/src:rw,Z"
    );
}
