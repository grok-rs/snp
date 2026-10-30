//! `language: docker` (04a §7; 10 §5.10 docker row; ADR-035; D-PERF-04; D-CRASH-21).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use snp_core::brand::Brand;
use snp_core::lang::LanguageId;
use snp_lang::harness::Reply;
use snp_lang::langs::docker::{
    ALWAYS_BUILD_VAR, build_argv, container_id_from_mountinfo, mounts_from_inspect,
    rootless_from_info, tag_for_prefix,
};
use snp_lang::{RunPlan, ToolProbe, common};

use super::util::{DOCKER_INFO, Fx, call_argvs, xargs_argv};

/// `docker_test.py`'s container id.
const CONTAINER_ID: &str = "c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7";

/// A fixture of `testdata/group2/mountinfo`.
fn mountinfo(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/group2/mountinfo")
            .join(name),
    )
    .unwrap()
}

#[test]
fn lang_docker_tags_differ_per_repository_key() {
    let a = Path::new("/store/repos-v1/0123456789abcdef0123456789abcdef/src");
    let b = Path::new("/store/repos-v1/fedcba9876543210fedcba9876543210/src");
    assert_ne!(tag_for_prefix(a), tag_for_prefix(b));
    assert_eq!(
        tag_for_prefix(a),
        common::docker_tag("0123456789abcdef0123456789abcdef")
    );
    // Never the `src` basename (ADR-035).
    assert_ne!(tag_for_prefix(a), common::docker_tag("src"));
    // Other layouts keep pre-commit's basename rule: md5("repoegjh37aw") (04a §7.1, observed).
    assert_eq!(
        tag_for_prefix(Path::new("/x/repoegjh37aw")),
        "pre-commit-f9bbd8a6ea92c181768b56d24cd49a45"
    );
}

#[test]
fn lang_docker_install_builds_with_pull_and_creates_the_env_dir() {
    let fx = Fx::new(&[]);
    let out = fx.install(LanguageId::Docker, &[]).unwrap();
    assert_eq!(&*out.installer, "docker");
    let tag = tag_for_prefix(&fx.prefix);
    assert_eq!(
        call_argvs(&fx.t),
        vec![vec![
            "docker".to_owned(),
            "build".into(),
            "--tag".into(),
            tag,
            "--label".into(),
            "PRE_COMMIT".into(),
            "--pull".into(),
            ".".into()
        ]]
    );
    assert_eq!(fx.t.runner.calls()[0].cwd, fx.prefix);
    assert!(fx.env_dir(LanguageId::Docker, "default").is_dir());
}

#[test]
fn lang_docker_install_failure_is_called_process_error() {
    let fx = Fx::new(&[]);
    fx.t.runner
        .reply("docker", Reply::fail(1, "no Dockerfile\n"));
    let e = fx.install(LanguageId::Docker, &[]).unwrap_err();
    assert_eq!(e.py_type(), "CalledProcessError");
    assert!(e.to_string().contains("return code: 1"), "{e}");
    assert!(!fx.env_dir(LanguageId::Docker, "default").exists());
}

#[test]
fn lang_docker_rejects_version_and_deps_in_both_brands() {
    let mut fx = Fx::new(&[]);
    for brand in [Brand::PreCommit, Brand::Snp] {
        fx.t.brand = brand;
        let e = fx.install_v(LanguageId::Docker, "1.0", &[]).unwrap_err();
        assert_eq!(
            e.to_string(),
            format!(
                "for now, {} requires system-installed docker -- you selected `language_version: 1.0`",
                brand.prog()
            )
        );
        let e = fx.install(LanguageId::Docker, &["x"]).unwrap_err();
        assert_eq!(
            e.to_string(),
            format!(
                "for now, {} does not support additional_dependencies for docker -- you selected \
                 `additional_dependencies: ['x']`",
                brand.prog()
            )
        );
    }
    assert!(fx.t.runner.calls().is_empty());
}

/// The plan of `entry` with `SNP_DOCKER_ALWAYS_BUILD` set to `always` (or unset).
fn plan(always: Option<&str>, docker_tag: Option<&str>, color: bool) -> (Fx, RunPlan) {
    let extra: Vec<(&str, &str)> = always.map(|v| (ALWAYS_BUILD_VAR, v)).into_iter().collect();
    let fx = Fx::new(&extra);
    fx.t.runner.reply(
        "docker",
        Reply::ok(DOCKER_INFO).when(["system", "info"]).sticky(),
    );
    let env = fx.t.env.clone();
    let p = fx
        .plan_full(
            LanguageId::Docker,
            "my-entry --flag",
            &["a b"],
            false,
            color,
            docker_tag,
            &env,
        )
        .unwrap();
    (fx, p)
}

#[test]
fn lang_docker_run_build_is_skipped_when_the_image_exists() {
    let (fx, p) = plan(None, None, false);
    let tag = tag_for_prefix(&fx.prefix);
    let RunPlan::Xargs { pre, .. } = &p else {
        panic!()
    };
    let [step] = pre.as_slice() else { panic!() };
    assert_eq!(step.argv, build_argv(&tag, false));
    assert_eq!(step.cwd, fx.prefix);
    assert_eq!(
        step.skip_if_ok,
        Some(
            ["docker", "image", "inspect", tag.as_str()]
                .map(OsString::from)
                .to_vec()
        )
    );
    #[cfg(unix)]
    {
        let uid = format!(
            "{}:{}",
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw()
        );
        assert_eq!(
            xargs_argv(&p),
            vec![
                "docker".to_owned(),
                "run".into(),
                "--rm".into(),
                "-u".into(),
                uid,
                "-v".into(),
                format!("{}:/src:rw,Z", fx.repo.display()),
                "--workdir".into(),
                "/src".into(),
                "--entrypoint".into(),
                "my-entry".into(),
                tag,
                "--flag".into(),
                "a b".into(),
            ]
        );
    }
}

#[test]
fn lang_docker_always_build_forces_the_rebuild() {
    for v in ["1", "yes"] {
        let (_fx, p) = plan(Some(v), None, false);
        let RunPlan::Xargs { pre, .. } = &p else {
            panic!()
        };
        assert_eq!(pre[0].skip_if_ok, None, "{v}");
    }
    // `0`, `false` and empty are not set (`Env::is_truthy_flag`).
    for v in ["0", "false", ""] {
        let (_fx, p) = plan(Some(v), None, false);
        let RunPlan::Xargs { pre, .. } = &p else {
            panic!()
        };
        assert!(pre[0].skip_if_ok.is_some(), "{v:?}");
    }
}

#[test]
fn lang_docker_plan_uses_the_resolved_tag_and_tty() {
    let (_fx, p) = plan(None, Some("pre-commit-abc"), true);
    let argv = xargs_argv(&p);
    assert_eq!(argv[..4], ["docker", "run", "--rm", "--tty"]);
    let at = argv.iter().position(|a| a == "--entrypoint").unwrap();
    assert_eq!(argv[at + 1..at + 3], ["my-entry", "pre-commit-abc"]);
    let RunPlan::Xargs { pre, .. } = &p else {
        panic!()
    };
    assert_eq!(pre[0].argv, build_argv("pre-commit-abc", false));
}

#[test]
fn lang_docker_bad_entry_fails_the_hook_only() {
    let fx = Fx::new(&[]);
    let env = fx.t.env.clone();
    for (entry, text) in [
        ("", "Executable `` not found"),
        ("echo 'x", "ValueError: No closing quotation"),
    ] {
        let p = fx
            .plan(LanguageId::Docker, entry, &[], false, &env)
            .unwrap();
        let RunPlan::Error { code, output } = p else {
            panic!("{p:?}")
        };
        assert_eq!((code, output), (1, text.as_bytes().to_vec()));
    }
}

#[test]
fn lang_docker_rootless_probe_once_per_process() {
    let fx = Fx::new(&[]);
    fx.t.runner.reply(
        "docker",
        Reply::ok(r#"{"SecurityOptions": ["name=rootless","name=cgroupns"]}"#)
            .when(["system", "info"])
            .sticky(),
    );
    let env = fx.t.env.clone();
    for _ in 0..3 {
        let p = fx.plan(LanguageId::Docker, "e", &[], false, &env).unwrap();
        assert!(!xargs_argv(&p).contains(&"-u".to_owned()));
    }
    let probes = call_argvs(&fx.t)
        .into_iter()
        .filter(|c| c[1] == "system")
        .count();
    assert_eq!(probes, 1);
}

#[test]
fn lang_docker_rootless_probe_failure_is_not_rootless() {
    // D-CRASH-21: pre-commit raises CalledProcessError (exit 3); snp runs with `-u uid:gid`.
    let fx = Fx::new(&[]);
    fx.t.runner.reply(
        "docker",
        Reply::fail(1, "Cannot connect to the Docker daemon\n")
            .when(["system", "info"])
            .sticky(),
    );
    let env = fx.t.env.clone();
    let p = fx.plan(LanguageId::Docker, "e", &[], false, &env).unwrap();
    assert_eq!(xargs_argv(&p).contains(&"-u".to_owned()), cfg!(unix));
}

#[test]
fn lang_docker_rootless_json_rows() {
    // docker_test.py:test_docker_user_rootless / _non_rootless.
    assert!(rootless_from_info(
        br#"{"SecurityOptions": ["name=rootless","name=cgroupns"]}"#
    ));
    assert!(rootless_from_info(
        br#"{"host": {"security": {"rootless": true}}}"#
    ));
    for out in [
        &br#"{"SecurityOptions": ["name=cgroupns"]}"#[..],
        br#"{"host": {"security": {"rootless": false}}}"#,
        br#"{"response_from_some_other_container_engine": true}"#,
        br#"{"SecurityOptions": null}"#,
        b"",
        b"\xff\xfe",
        b"[]",
    ] {
        assert!(!rootless_from_info(out), "{}", String::from_utf8_lossy(out));
    }
}

#[test]
fn lang_docker_container_id_fixtures() {
    for f in [
        "docker_cgroups_v1.mountinfo",
        "docker_cgroups_v2.mountinfo",
        "podman_cgroups_v1.mountinfo",
        "podman_cgroups_v2.mountinfo",
    ] {
        assert_eq!(
            container_id_from_mountinfo(&mountinfo(f)).as_deref(),
            Some(CONTAINER_ID),
            "{f}"
        );
    }
    assert_eq!(
        container_id_from_mountinfo(&mountinfo("non_docker.mountinfo")),
        None
    );
    assert_eq!(container_id_from_mountinfo(b""), None);
}

#[test]
fn lang_docker_inspect_mounts() {
    let many = br#"[{"Mounts": [
        {"Source": "/something_random", "Destination": "/not-related"},
        {"Source": "/opt/my_code", "Destination": "/project"},
        {"Source": "/something-random-2", "Destination": "/not-related-2"}]}]"#;
    assert_eq!(mounts_from_inspect(many).unwrap().len(), 3);
    assert_eq!(mounts_from_inspect(br#"[{"Mounts": []}]"#), Some(vec![]));
    assert_eq!(mounts_from_inspect(b"[1, 2]"), None);
    assert_eq!(mounts_from_inspect(b"not json"), None);
}

/// A fixture running "inside" the docker cgroup v1 container: `cwd` is `cwd`, `docker inspect` answers `inspect`.
fn in_container(cwd: &str, inspect: Option<&str>) -> Vec<String> {
    let mut fx = Fx::new(&[]);
    let dir = tempfile::tempdir().unwrap();
    let mi: PathBuf = dir.path().join("mountinfo");
    std::fs::write(&mi, mountinfo("docker_cgroups_v1.mountinfo")).unwrap();
    fx.t.probe = ToolProbe::new(None).with_mountinfo(mi);
    fx.t.cwd = PathBuf::from(cwd);
    fx.t.runner.reply(
        "docker",
        Reply::ok(DOCKER_INFO).when(["system", "info"]).sticky(),
    );
    match inspect {
        Some(out) => fx.t.runner.reply(
            "docker",
            Reply::ok(out.as_bytes().to_vec())
                .when(["inspect", CONTAINER_ID])
                .sticky(),
        ),
        None => {
            fx.t.runner
                .reply("docker", Reply::fail(1, "").when(["inspect"]).sticky());
        }
    }
    let env = fx.t.env.clone();
    let p = fx.plan(LanguageId::Docker, "e", &[], false, &env).unwrap();
    let argv = xargs_argv(&p);
    let v = argv.iter().position(|a| a == "-v").unwrap();
    let volume = argv[v + 1].clone();
    vec![volume]
}

#[cfg(unix)]
#[test]
fn lang_docker_bind_path_translation() {
    let binds = r#"[{"Mounts": [{"Source": "/opt/my_code", "Destination": "/project"}]}]"#;
    assert_eq!(
        in_container("/project", Some(binds)),
        ["/opt/my_code:/src:rw,Z"]
    );
    assert_eq!(
        in_container("/project/test/something", Some(binds)),
        ["/opt/my_code/test/something:/src:rw,Z"]
    );
    // Path-component semantics: `/projectSuffix` is not inside `/project`.
    assert_eq!(
        in_container("/projectSuffix/test/something", Some(binds)),
        ["/projectSuffix/test/something:/src:rw,Z"]
    );
    // No binds; docker-in-docker (inspect fails): unchanged.
    assert_eq!(
        in_container("/project", Some(r#"[{"Mounts": []}]"#)),
        ["/project:/src:rw,Z"]
    );
    assert_eq!(in_container("/project", None), ["/project:/src:rw,Z"]);
}
