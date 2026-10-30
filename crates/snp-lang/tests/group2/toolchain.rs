//! Real-toolchain tests (`tc_<lang>_*`, `#[ignore = "toolchain"]`): the nightly job runs them where the tool exists
//! (10 §5.10 "toolchain" column); a missing tool skips the test body. They use the real `ProcRunner`, the network
//! where the tool needs it, and run the planned hook command the way the executor does (pre-steps, then the command
//! with the files appended, in the repository with the hook's environment).

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::{EnvCtx, Host, InstallCtx, PlanCtx, ProcRunner, RunPlan, backend};

use super::util::Fx;

/// A fixture whose base environment is the test process's (real tools on `PATH`), with its temporary directories.
fn real_fx() -> Fx {
    let mut fx = Fx::new(&[]);
    let env = Env::capture_process();
    let mut pairs: Vec<(OsString, OsString)> = env
        .iter()
        .map(|(k, v)| (k.to_os_string(), v.to_os_string()))
        .collect();
    pairs.push(("PRE_COMMIT".into(), "1".into()));
    fx.t.env = Env::from_pairs(pairs);
    fx
}

/// Whether `tool` is on the process `PATH`.
fn have(tool: &str) -> bool {
    snp_proc::resolve::find_executable(tool.as_ref(), &Env::capture_process(), Path::new("."))
        .is_some()
}

/// Installs `id` with `deps` through the real runner, plans `entry` and runs it on `files`; returns (status, output).
fn install_and_run(
    fx: &Fx,
    id: LanguageId,
    deps: &[&str],
    entry: &str,
    files: &[&str],
) -> (i32, Vec<u8>) {
    let runner = ProcRunner {
        cancel: CancelToken::new(),
    };
    let host = Host {
        runner: &runner,
        ..fx.t.host()
    };
    let env_dir = fx.env_dir(id, "default");
    let deps: Vec<Arc<str>> = deps.iter().map(|d| Arc::from(*d)).collect();
    let cx = EnvCtx {
        host: &host,
        prefix: &fx.prefix,
        version: "default",
        env_dir: &env_dir,
    };
    let lang = backend(id);
    lang.install(&InstallCtx {
        cx: EnvCtx {
            version: "default",
            ..cx
        },
        deps: &deps,
        is_local: false,
    })
    .unwrap();
    let env = fx.t.env.patched(&lang.activate(&cx).unwrap());
    let plan = lang
        .plan(&PlanCtx {
            host: &host,
            prefix: &fx.prefix,
            version: "default",
            env_dir: Some(&env_dir),
            env: &env,
            entry,
            args: &[],
            is_local: false,
            color: false,
            docker_tag: None,
        })
        .unwrap();
    let RunPlan::Xargs { cmd, env, pre, .. } = plan else {
        panic!("{plan:?}")
    };
    for step in pre {
        let skip = step.skip_if_ok.as_ref().is_some_and(|probe| {
            let mut c = std::process::Command::new(&probe[0]);
            c.args(&probe[1..]).current_dir(&step.cwd);
            step.env.apply_to(&mut c);
            c.output().is_ok_and(|o| o.status.success())
        });
        if !skip {
            let mut c = std::process::Command::new(&step.argv[0]);
            c.args(&step.argv[1..]).current_dir(&step.cwd);
            step.env.apply_to(&mut c);
            assert!(c.status().unwrap().success());
        }
    }
    let resolved = snp_proc::resolve::normalize_cmd(&cmd, &env, &fx.repo).unwrap();
    let mut c = std::process::Command::new(&resolved[0]);
    c.args(&resolved[1..]).args(files).current_dir(&fx.repo);
    env.apply_to(&mut c);
    let out = c.output().unwrap();
    let mut all = out.stdout;
    all.extend(out.stderr);
    (out.status.code().unwrap_or(-1), all)
}

#[test]
#[ignore = "toolchain"]
fn tc_docker_build_and_run() {
    if !have("docker") {
        return;
    }
    let fx = real_fx();
    fx.put("Dockerfile", b"FROM alpine:3\n");
    std::fs::write(fx.repo.join("README.md"), b"").unwrap();
    let (code, out) = install_and_run(&fx, LanguageId::Docker, &[], "echo hello hello world", &[]);
    assert_eq!((code, out), (0, b"hello hello world\n".to_vec()));
    // Mount permissions (docker_test.py:test_docker_hook_mount_permissions): the hook can write the cwd. The image
    // exists now, so the per-run build is skipped (D-PERF-04).
    std::fs::remove_dir_all(fx.env_dir(LanguageId::Docker, "default")).unwrap();
    let (code, _) = install_and_run(&fx, LanguageId::Docker, &[], "touch", &["README.md"]);
    assert_eq!(code, 0);
}

#[test]
#[ignore = "toolchain"]
fn tc_perl_local_template() {
    if !have("cpan") || !have("perl") {
        return;
    }
    let fx = real_fx();
    let (code, out) = install_and_run(&fx, LanguageId::Perl, &[], "perl -e print(42)", &[]);
    assert_eq!((code, out), (0, b"42".to_vec()));
}

#[test]
#[ignore = "toolchain"]
fn tc_dart_local_template() {
    if !have("dart") {
        return;
    }
    let fx = real_fx();
    let (code, _) = install_and_run(&fx, LanguageId::Dart, &[], "dart --version", &[]);
    assert_eq!(code, 0);
}

#[test]
#[ignore = "toolchain"]
fn tc_r_local_template() {
    if !have("Rscript") {
        return;
    }
    let fx = real_fx();
    let (code, out) = install_and_run(&fx, LanguageId::R, &[], "Rscript -e cat(1+1)", &[]);
    assert_eq!((code, out), (0, b"2".to_vec()));
}

/// The remaining toolchains of the group: each test body runs only where its tool is installed.
macro_rules! tc_smoke {
    ($name:ident, $tool:literal, $id:expr, $deps:expr, $entry:literal) => {
        #[test]
        #[ignore = "toolchain"]
        fn $name() {
            if !have($tool) {
                return;
            }
            let fx = real_fx();
            let (code, out) = install_and_run(&fx, $id, $deps, $entry, &[]);
            assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
        }
    };
}

tc_smoke!(
    tc_conda_local_template,
    "conda",
    LanguageId::Conda,
    &[],
    "openssl version"
);
tc_smoke!(
    tc_coursier_deps,
    "cs",
    LanguageId::Coursier,
    &["scalafmt:3.6.1"],
    "scalafmt --version"
);
tc_smoke!(
    tc_haskell_deps,
    "cabal",
    LanguageId::Haskell,
    &["hello"],
    "hello"
);
tc_smoke!(
    tc_lua_deps,
    "luarocks",
    LanguageId::Lua,
    &["luacheck"],
    "luacheck --version"
);

#[test]
#[ignore = "toolchain"]
fn tc_julia_script_in_repository() {
    if !have("julia") {
        return;
    }
    let fx = real_fx();
    fx.put("hook.jl", b"println(\"hi\")\n");
    let (code, out) = install_and_run(&fx, LanguageId::Julia, &[], "hook.jl", &[]);
    assert_eq!((code, out), (0, b"hi\n".to_vec()));
}

/// Without a project file the build fails with pre-commit's `CalledProcessError` (04b §1.9: local swift and dotnet
/// hooks cannot work).
fn install_fails(tool: &str, id: LanguageId) {
    if !have(tool) {
        return;
    }
    let fx = real_fx();
    let runner = ProcRunner {
        cancel: CancelToken::new(),
    };
    let host = Host {
        runner: &runner,
        ..fx.t.host()
    };
    let env_dir = fx.env_dir(id, "default");
    let e = backend(id)
        .install(&InstallCtx {
            cx: EnvCtx {
                host: &host,
                prefix: &fx.prefix,
                version: "default",
                env_dir: &env_dir,
            },
            deps: &[],
            is_local: true,
        })
        .unwrap_err();
    assert_eq!(e.py_type(), "CalledProcessError");
}

#[test]
#[ignore = "toolchain"]
fn tc_swift_without_package_fails() {
    install_fails("swift", LanguageId::Swift);
}

#[test]
#[ignore = "toolchain"]
fn tc_dotnet_without_project_fails() {
    install_fails("dotnet", LanguageId::Dotnet);
}
