//! Nightly real-toolchain tests (`tc_<lang>_*`, `#[ignore = "toolchain"]`; WBS WP-24): each builds an environment
//! with the real tools of the host through the real [`ProcRunner`], then runs the installed hook executable with the
//! activated environment. A test whose toolchain is missing returns early (the nightly job provides them).

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::harness::TestHost;
use snp_lang::{Health, Host, ProcRunner, RunPlan, backend, check_support};

use crate::support::{World, deps, env_ctx, install_ctx, plan_ctx};

/// The real process environment with `extra` on top.
fn real_env(extra: &[(&str, &str)]) -> Env {
    let mut env = Env::capture_process();
    for (k, v) in extra {
        env = env.patched(&snp_core::env::EnvPatch::default().set((*k).to_owned(), *v));
    }
    env
}

/// Whether every tool exists on `env`'s `PATH`.
fn have(env: &Env, tools: &[&str]) -> bool {
    tools
        .iter()
        .all(|t| snp_proc::resolve::find_executable(OsStr::new(t), env, Path::new(".")).is_some())
}

/// Installs `lang` at `version` with `deps` into `prefix`, checks health, runs `entry` with the activated environment
/// and returns its stdout.
fn build_and_run(
    world: &World,
    env: &Env,
    lang: LanguageId,
    version: &str,
    dependencies: &[&str],
    is_local: bool,
    entry: &str,
) -> String {
    let t = TestHost::new(&world.repo, env.clone());
    let runner = ProcRunner {
        cancel: t.cancel.clone(),
    };
    let fetch = snp_net::HttpFetch::from_env(env);
    let host = Host {
        base_env: &t.env,
        cwd: &t.cwd,
        self_exe: &t.self_exe,
        probe: &t.probe,
        fetch: Some(&fetch),
        tools: &t.tools,
        runner: &runner,
        printer: &t.printer,
        cancel: &t.cancel,
        brand: t.brand,
    };
    let be = backend(lang);
    let version: Arc<str> = if version == "default" {
        be.default_version(&host).unwrap()
    } else {
        Arc::from(version)
    };
    let dependencies = deps(dependencies);
    check_support(lang, &version, &dependencies, host.brand).unwrap();
    let env_dir = world
        .prefix
        .join(format!("{}-{version}", lang.env_dir_base().unwrap()));
    let out = be
        .install(&install_ctx(
            &host,
            &world.prefix,
            &version,
            &env_dir,
            &dependencies,
            is_local,
        ))
        .unwrap_or_else(|e| panic!("{lang:?} install: {e}"));
    let ecx = env_ctx(&host, &world.prefix, &version, &env_dir);
    assert_eq!(be.health(&ecx, None).unwrap(), Health::Healthy);
    assert!(!out.installer.is_empty());
    let run_env = t.env.patched(&be.activate(&ecx).unwrap());
    let plan = be
        .plan(&plan_ctx(
            &host,
            &world.prefix,
            &version,
            &env_dir,
            &run_env,
            entry,
        ))
        .unwrap();
    let RunPlan::Xargs {
        cmd, env: hook_env, ..
    } = plan
    else {
        panic!("{plan:?}")
    };
    let argv = snp_proc::resolve::normalize_cmd(&cmd, &hook_env, &world.repo).unwrap();
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir(&world.repo);
    hook_env.apply_to(&mut command);
    let o = command.output().unwrap();
    assert!(o.status.success(), "{o:?}");
    String::from_utf8_lossy_owned(o.stdout)
}

/// A hook repository with one console script per language.
fn python_repo(w: &World) {
    std::fs::write(
        w.prefix.join("setup.py"),
        "from setuptools import setup\nsetup(name='hooks', version='0.0.0', py_modules=['hello'],\n      \
         entry_points={'console_scripts': ['py-hello=hello:main']})\n",
    )
    .unwrap();
    std::fs::write(
        w.prefix.join("hello.py"),
        "def main():\n    print('hello from python')\n",
    )
    .unwrap();
}

#[test]
#[ignore = "toolchain"]
fn tc_python_uv_and_pip() {
    for installer in ["uv", "pip"] {
        let env = real_env(&[("SNP_PYTHON_INSTALLER", installer)]);
        if !have(&env, &["python3", "uv"]) {
            return;
        }
        let w = World::new();
        python_repo(&w);
        let out = build_and_run(
            &w,
            &env,
            LanguageId::Python,
            "default",
            &[],
            false,
            "py-hello",
        );
        assert_eq!(out, "hello from python\n", "{installer}");
    }
}

#[test]
#[ignore = "toolchain"]
fn tc_node_system() {
    let env = real_env(&[("SNP_PREFER_SYSTEM_TOOLCHAINS", "1")]);
    if !have(&env, &["node", "npm", "git"]) {
        return;
    }
    let w = World::new();
    std::fs::remove_dir_all(w.prefix.join(".git")).unwrap();
    std::fs::write(
        w.prefix.join("hello.js"),
        "#!/usr/bin/env node\nconsole.log('hello from node');\n",
    )
    .unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", "."],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "x",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&w.prefix)
                .status()
                .unwrap()
                .success()
        );
    }
    let out = build_and_run(
        &w,
        &env,
        LanguageId::Node,
        "system",
        &[],
        false,
        "node-hello",
    );
    assert_eq!(out, "hello from node\n");
}

#[test]
#[ignore = "toolchain"]
fn tc_golang_system() {
    let env = real_env(&[]);
    if !have(&env, &["go"]) {
        return;
    }
    let w = World::new();
    // A real clone has a real `.git`; the placeholder directory would break go's VCS stamping.
    std::fs::remove_dir_all(w.prefix.join(".git")).unwrap();
    std::fs::write(
        w.prefix.join("main.go"),
        "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(\"hello from go\") }\n",
    )
    .unwrap();
    let out = build_and_run(&w, &env, LanguageId::Golang, "system", &[], false, "hooks");
    assert_eq!(out, "hello from go\n");
}

#[test]
#[ignore = "toolchain"]
fn tc_rust_system() {
    let env = real_env(&[]);
    if !have(&env, &["cargo"]) {
        return;
    }
    let w = World::new();
    std::fs::write(
        w.prefix.join("main.rs"),
        "fn main() { println!(\"hello from rust\"); }\n",
    )
    .unwrap();
    let out = build_and_run(
        &w,
        &env,
        LanguageId::Rust,
        "system",
        &[],
        false,
        "rust-hello",
    );
    assert_eq!(out, "hello from rust\n");
}

#[test]
#[ignore = "toolchain"]
fn tc_ruby_system() {
    let env = real_env(&[("SNP_PREFER_SYSTEM_TOOLCHAINS", "1")]);
    if !have(&env, &["ruby", "gem"]) {
        return;
    }
    let w = World::new();
    std::fs::create_dir_all(w.prefix.join("bin")).unwrap();
    std::fs::write(
        w.prefix.join("bin/ruby-hello"),
        "#!/usr/bin/env ruby\nputs 'hello from ruby'\n",
    )
    .unwrap();
    std::fs::write(
        w.prefix.join("hooks.gemspec"),
        "Gem::Specification.new do |s|\n  s.name = 'hooks'\n  s.version = '0.0.0'\n  s.summary = 'x'\n  \
         s.authors = ['x']\n  s.files = ['bin/ruby-hello']\n  s.executables = ['ruby-hello']\n  s.license = 'MIT'\nend\n",
    )
    .unwrap();
    let out = build_and_run(
        &w,
        &env,
        LanguageId::Ruby,
        "system",
        &[],
        false,
        "ruby-hello",
    );
    assert_eq!(out, "hello from ruby\n");
}
