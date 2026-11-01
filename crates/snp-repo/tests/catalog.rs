//! Catalog rows owned by WP-31 that are decided by hook resolution and environment installs (T10-REP, T10-STO-03/04,
//! X13-GIT-01, X13-ENV-01…04, X13-SM-01/02), ported against the real store, the real git and local hook
//! repositories. Rows whose expectation is the *executed* hook (output, cwd, pty, partitioning) are checked here up
//! to the run plan the executor receives — the command is then spawned directly with the given files — and end to
//! end through the binary in `crates/snp/tests/it/repository.rs`.
//!
//! Owner: WP-31.

mod support;

use std::ffi::OsString;
use std::sync::Arc;

use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_core::stage::Stage;
use snp_lang::RunPlan;
use snp_lang::harness::Reply;
use snp_repo::{ExecKind, HookSrc, Resolve};
use snp_testkit::Fixture;
use support::{H, exc, git, head, initializing, installing, manifest_one};

/// Runs a plan's command with `files` appended; `(code, stdout)`.
fn run_plan(h: &H, plan: &RunPlan, files: &[&str]) -> (i32, String) {
    let RunPlan::Xargs { cmd, env, .. } = plan else {
        panic!("not an xargs plan: {plan:?}");
    };
    let cmd = snp_proc::resolve::normalize_cmd(cmd, env, &h.project).unwrap();
    let mut c = std::process::Command::new(&cmd[0]);
    c.args(&cmd[1..]).args(files).current_dir(&h.project).env_clear();
    env.apply_to(&mut c);
    let out = c.output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8(out.stdout).unwrap())
}

fn remote_cfg(repo: &str, rev: &str, hooks: &str) -> String {
    format!("repos:\n-   repo: {repo}\n    rev: {rev}\n    hooks:\n{hooks}")
}

#[test]
fn t10_sto_03_clone_logs_initializing_and_checks_out_the_rev() {
    let h = H::new();
    let repo = h.t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let rev = head(&h.env, &repo);
    let path = repo.to_string_lossy().into_owned();
    let cfg = h.config(&remote_cfg(&path, &rev, "    -   id: bash_hook\n"));
    let hooks = h.resolve(&cfg).unwrap();
    assert_eq!(h.out.take(), initializing(&path));
    let hook = &hooks[0];
    assert!(hook.prefix.starts_with(&h.store.location().path), "{:?}", hook.prefix);
    assert_eq!(head(&h.env, &hook.prefix), rev);
    assert!(hook.prefix.join(".pre-commit-hooks.yaml").is_file());
    // The entry is reused: no second line, no second clone.
    let again = h.resolve(&cfg).unwrap();
    assert_eq!(h.out.take(), "");
    assert_eq!(again[0].prefix, hook.prefix);
}

#[test]
fn t10_sto_04_deprecated_stages_warned_once_per_new_clone() {
    let h = H::new();
    let manifest = b"-   id: a\n    name: a\n    entry: a\n    language: system\n    stages: [commit, push]\n\
-   id: b\n    name: b\n    entry: b\n    language: system\n    stages: [merge-commit, commit]\n";
    let (old, old_rev) = h.hook_repo("old", &[(".pre-commit-hooks.yaml", manifest, false)]);
    let new_manifest = b"-   id: a\n    name: a\n    entry: a\n    language: system\n    stages: [pre-commit]\n";
    let (new, new_rev) = h.hook_repo("new", &[(".pre-commit-hooks.yaml", new_manifest, false)]);
    let (old, new) = (old.to_string_lossy().into_owned(), new.to_string_lossy().into_owned());
    let yaml = format!(
        "{}-   repo: {new}\n    rev: {new_rev}\n    hooks:\n    -   id: a\n",
        remote_cfg(&old, &old_rev, "    -   id: a\n")
    );
    let cfg = h.config(&yaml);
    h.resolve(&cfg).unwrap();
    let warning = format!(
        "[WARNING] repo `{old}` uses deprecated stage names (commit, push, merge-commit) which will be removed in a \
         future version.  Hint: often `pre-commit autoupdate --repo {old}` will fix this.  if it does not -- \
         consider reporting an issue to that repo.\n"
    );
    assert_eq!(
        h.out.take(),
        format!("{}{warning}{}", initializing(&old), initializing(&new))
    );
    h.resolve(&cfg).unwrap();
    assert_eq!(h.out.take(), "");
}

#[test]
fn x13_sm_01_initializing_lines_in_config_order_with_deps_variants() {
    let h = H::new();
    let (r, rev) = h.hook_repo(
        "perl-hooks",
        &[(".pre-commit-hooks.yaml", &manifest_one("p", "p", "perl", ""), false)],
    );
    let r = r.to_string_lossy().into_owned();
    let yaml = format!(
        "repos:\n-   repo: local\n    hooks:\n    -   id: l\n        name: l\n        entry: l\n        language: \
         perl\n        additional_dependencies: [six, attrs]\n-   repo: local\n    hooks:\n    -   id: s\n        \
         name: s\n        entry: s\n        language: system\n{}",
        remote_cfg(
            &r,
            &rev,
            "    -   id: p\n    -   id: p\n        additional_dependencies: [six, attrs==23.1.0]\n"
        )
        .trim_start_matches("repos:\n")
    );
    let cfg = h.config(&yaml);
    let hooks = h.resolve(&cfg).unwrap();
    assert_eq!(
        h.out.take(),
        format!(
            "{}{}{}",
            initializing("local:six,attrs"),
            initializing(&r),
            initializing(&format!("{r}:six,attrs==23.1.0"))
        )
    );
    // The system hook runs in the project root; the variant has its own prefix.
    assert_eq!(hooks[1].prefix, h.project);
    assert!(hooks[1].install_key.is_none());
    assert_ne!(hooks[2].prefix, hooks[3].prefix);
    assert_eq!(head(&h.env, &hooks[3].prefix), rev);
}

#[test]
fn x13_sm_02_installing_triplet_once_per_environment_in_config_order() {
    let h = H::new();
    h.runner.reply("cpan", Reply::ok("").sticky());
    let yaml = "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        \
                language: perl\n    -   id: b\n        name: b\n        entry: b\n        language: perl\n    -   id: \
                c\n        name: c\n        entry: c\n        language: perl\n        additional_dependencies: [x]\n";
    let cfg = h.config(yaml);
    h.resolve_and_install(&cfg).unwrap();
    assert_eq!(
        h.out.take(),
        format!(
            "{}{}{}{}",
            initializing("local"),
            initializing("local:x"),
            installing("local"),
            installing("local")
        )
    );
    // Warm: nothing.
    h.resolve_and_install(&cfg).unwrap();
    assert_eq!(h.out.take(), "");
}

#[test]
fn t10_rep_01_not_found_exe_reaches_the_executor_as_the_bare_name() {
    let h = H::new();
    let repo = h.t.fixture_repo(Fixture::Resource("not_found_exe"));
    let rev = head(&h.env, &repo);
    let cfg = h.config(&remote_cfg(&repo.to_string_lossy(), &rev, "    -   id: not-found-exe\n"));
    let hooks = h.resolve(&cfg).unwrap();
    let plan = h.with_ctx(&cfg, None, |cx| h.resolver.plan(&hooks[0], cx, false)).unwrap();
    let RunPlan::Xargs { cmd, env, .. } = &plan else {
        panic!("{plan:?}")
    };
    assert_eq!(&**cmd, [OsString::from("i-dont-exist-lol")]);
    let e = snp_proc::resolve::normalize_cmd(cmd, env, &h.project).unwrap_err();
    assert_eq!(e.message(), "Executable `i-dont-exist-lol` not found");
}

#[cfg(unix)]
#[test]
fn t10_rep_02_script_hook_argv_and_verbatim_args() {
    let h = H::new();
    let script = h.t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let per_line = h.t.fixture_repo(Fixture::Resource("arg_per_line_hooks_repo"));
    let yaml = format!(
        "{}-   repo: {}\n    rev: {}\n    hooks:\n    -   id: arg-per-line\n-   repo: {}\n    rev: {}\n    hooks:\n    \
         -   id: arg-per-line\n        args: ['hi {{1}}', \"I'm {{a}} problem\"]\n",
        remote_cfg(&script.to_string_lossy(), &head(&h.env, &script), "    -   id: bash_hook\n"),
        per_line.display(),
        head(&h.env, &per_line),
        per_line.display(),
        head(&h.env, &per_line),
    );
    let cfg = h.config(&yaml);
    let hooks = h.resolve(&cfg).unwrap();
    let plans: Vec<RunPlan> = hooks
        .iter()
        .map(|hk| h.with_ctx(&cfg, None, |cx| h.resolver.plan(hk, cx, false)).unwrap())
        .collect();
    assert_eq!(run_plan(&h, &plans[0], &["bar"]), (0, "bar\nHello World\n".to_owned()));
    assert_eq!(
        run_plan(&h, &plans[1], &["foo bar", "baz"]),
        (0, "arg: hello\narg: world\narg: foo bar\narg: baz\n".to_owned())
    );
    assert_eq!(
        run_plan(&h, &plans[2], &[]),
        (0, "arg: hi {1}\narg: I'm {a} problem\n".to_owned())
    );
}

#[test]
fn t10_rep_07_unexpected_keys_on_local() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: too-much\n        name: too much\n        hello: world\n        \
         foo: bar\n        language: system\n        entry: 'true'\n",
    );
    h.resolve(&cfg).unwrap();
    assert_eq!(
        h.out.take(),
        "[WARNING] Unexpected key(s) present on local => too-much: foo, hello\n"
    );
}

#[test]
fn t10_rep_08_config_files_overrides_manifest() {
    let h = H::new();
    let (r, rev) = h.hook_repo(
        "files",
        &[(".pre-commit-hooks.yaml", &manifest_one("x", "x", "system", "    files: '\\.py$'\n"), false)],
    );
    let r = r.to_string_lossy().into_owned();
    let cfg = h.config(&remote_cfg(&r, &rev, "    -   id: x\n        files: 'bar\\.txt'\n"));
    let hooks = h.resolve(&cfg).unwrap();
    assert_eq!(&*hooks[0].def.files, "bar\\.txt");
    let cfg = h.config(&remote_cfg(&r, &rev, "    -   id: x\n"));
    assert_eq!(&*h.resolve(&cfg).unwrap()[0].def.files, "\\.py$");
}

#[test]
fn t10_rep_09_rev_is_a_tag_for_two_repos() {
    let h = H::new();
    let manifest = manifest_one("t", "t", "system", "");
    let mut paths = Vec::new();
    for name in ["tag-a", "tag-b"] {
        let dir = h
            .t
            .git_repo(name)
            .file(".pre-commit-hooks.yaml", &manifest)
            .stage_all()
            .commit("init")
            .tag("v1.1")
            .build();
        // Move the branch past the tag: the clone must check out the tag, not HEAD.
        h.t.git_repo(name).file("later", b"x").stage_all().commit("later").build();
        paths.push(dir);
    }
    let yaml = format!(
        "repos:\n-   repo: {}\n    rev: v1.1\n    hooks:\n    -   id: t\n-   repo: {}\n    rev: v1.1\n    hooks:\n    \
         -   id: t\n",
        paths[0].display(),
        paths[1].display()
    );
    let cfg = h.config(&yaml);
    let hooks = h.resolve(&cfg).unwrap();
    assert_eq!(
        h.out.take(),
        format!(
            "{}{}",
            initializing(&paths[0].to_string_lossy()),
            initializing(&paths[1].to_string_lossy())
        )
    );
    for (hk, p) in hooks.iter().zip(&paths) {
        let tag = git(&h.env, p, &["rev-parse", "v1.1^{commit}"]);
        assert_eq!(head(&h.env, &hk.prefix), tag.trim());
        assert!(!hk.prefix.join("later").exists());
    }
}

#[test]
fn t10_rep_11_default_stages_apply_unless_the_hook_sets_stages() {
    let h = H::new();
    let cfg = h.config(
        "default_stages: [pre-push]\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        \
         entry: a\n        language: system\n    -   id: b\n        name: b\n        entry: b\n        language: \
         system\n        stages: [pre-commit]\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    assert!(hooks[0].def.stages.contains(Stage::parse("pre-push").unwrap()));
    assert!(!hooks[0].def.stages.contains(Stage::parse("pre-commit").unwrap()));
    assert!(hooks[1].def.stages.contains(Stage::parse("pre-commit").unwrap()));
    assert!(!hooks[1].def.stages.contains(Stage::parse("pre-push").unwrap()));
}

#[test]
fn t10_rep_12_unknown_id_is_an_error_after_the_clone() {
    let h = H::new();
    let repo = h.t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let rev = head(&h.env, &repo);
    let url = format!("file://{}", repo.display());
    let cfg = h.config(&remote_cfg(&url, &rev, "    -   id: i-dont-exist\n"));
    let err = h.resolve(&cfg).unwrap_err();
    assert!(matches!(err, Failure::Exit(1)), "{err:?}");
    assert_eq!(
        h.out.take(),
        format!(
            "{}[ERROR] `i-dont-exist` is not present in repository {url}.  Typo? Perhaps it is introduced in a newer \
             version?  Often `pre-commit autoupdate` fixes this.\n",
            initializing(&url)
        )
    );
}

#[test]
fn t10_rep_13_resolved_manifest_hook_defaults() {
    let h = H::new();
    let repo = h.t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let cfg = h.config(&remote_cfg(&repo.to_string_lossy(), &head(&h.env, &repo), "    -   id: bash_hook\n"));
    let d = &h.resolve(&cfg).unwrap()[0].def;
    assert_eq!(&*d.alias, "");
    assert!(!d.always_run);
    assert!(d.args.is_empty());
    assert_eq!(&*d.description, "");
    assert_eq!(&*d.exclude, "^$");
    assert!(d.exclude_types.is_empty());
    assert_eq!(&*d.files, "");
    assert_eq!(d.language, LanguageId::UnsupportedScript);
    assert_eq!(&*d.language_version, "default");
    assert_eq!(&*d.log_file, "");
    assert_eq!(&*d.minimum_pre_commit_version, "0");
    assert!(d.pass_filenames);
    assert!(!d.require_serial);
    assert_eq!(d.stages, cfg.default_stages);
    assert!(!d.verbose);
    assert!(!d.fail_fast);
    assert!(d.types_or.is_empty());
    assert_eq!(format!("{:?}", d.types), file_types_debug());
}

/// `types: [file]` (Debug), through the config loader of a local hook without `types`.
fn file_types_debug() -> String {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n",
    );
    format!("{:?}", h.resolve(&cfg).unwrap()[0].def.types)
}

#[test]
fn t10_rep_14_envless_language_with_version_or_deps_is_an_error() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: system-hook\n        name: s\n        entry: s\n        \
         language: unsupported\n        language_version: python3.10\n",
    );
    assert!(matches!(h.resolve(&cfg).unwrap_err(), Failure::Exit(1)));
    assert_eq!(
        h.out.take(),
        "[ERROR] The hook `system-hook` specifies `language_version` but is using language `unsupported` which does \
         not install an environment.  Perhaps you meant to use a specific language?\n"
    );
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: system-hook\n        name: s\n        entry: s\n        \
         language: unsupported\n        additional_dependencies: [x]\n",
    );
    assert!(matches!(h.resolve(&cfg).unwrap_err(), Failure::Exit(1)));
    assert_eq!(
        h.out.take(),
        "[ERROR] The hook `system-hook` specifies `additional_dependencies` but is using language `unsupported` \
         which does not install an environment.  Perhaps you meant to use a specific language?\n"
    );
}

#[cfg(unix)]
#[test]
fn t10_rep_15_args_reach_the_process_unchanged() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: p\n        name: p\n        entry: printf '[%s]\\n'\n        \
         language: system\n        args: ['i have spaces', 'and\"''quotes', '$and !this']\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    let plan = h.with_ctx(&cfg, None, |cx| h.resolver.plan(&hooks[0], cx, false)).unwrap();
    assert_eq!(
        run_plan(&h, &plan, &[]),
        (0, "[i have spaces]\n[and\"'quotes]\n[$and !this]\n".to_owned())
    );
}

#[test]
fn t10_rep_16_hazmat_entry_is_rewritten_to_the_running_binary() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: hz\n        name: hz\n        entry: pre-commit hazmat \
         ignore-exit-code python3 -c 'raise SystemExit(sys.argv[1:])'\n        language: system\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    let plan = h.with_ctx(&cfg, None, |cx| h.resolver.plan(&hooks[0], cx, false)).unwrap();
    let RunPlan::Xargs { cmd, .. } = plan else {
        panic!()
    };
    let want: Vec<OsString> = [
        h.self_exe.to_str().unwrap(),
        "__hazmat",
        "ignore-exit-code",
        "python3",
        "-c",
        "raise SystemExit(sys.argv[1:])",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    assert_eq!(&*cmd, &want[..]);
}

#[test]
#[ignore = "needs a python toolchain on PATH (non-hermetic row)"]
fn t10_rep_10_local_python_language_version_is_resolved() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: py\n        name: py\n        entry: py\n        language: \
         python\n        additional_dependencies: [six]\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    assert_ne!(&*hooks[0].language_version, "default");
    let cfg = h.config(
        "default_language_version:\n    python: python3\nrepos:\n-   repo: local\n    hooks:\n    -   id: py\n        \
         name: py\n        entry: py\n        language: python\n",
    );
    assert_eq!(&*h.resolve(&cfg).unwrap()[0].language_version, "python3");
}

#[test]
#[ignore = "needs a python toolchain and the network (non-hermetic row)"]
fn t10_rep_06_python_env_deps_roll_forward_isolation() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: \
         python\n        additional_dependencies: [mccabe]\n    -   id: b\n        name: b\n        entry: b\n        \
         language: python\n",
    );
    let hooks = h.resolve_and_install(&cfg).unwrap();
    assert_ne!(hooks[0].prefix, hooks[1].prefix);
}

#[test]
fn x13_git_01_missing_repository_is_the_fallback_called_process_error() {
    let h = H::new();
    let missing = h.t.root().join("nonexistent/repo");
    let missing = missing.to_string_lossy().into_owned();
    let cfg = h.config(&remote_cfg(&missing, "v1", "    -   id: x\n"));
    let err = h.resolve(&cfg).unwrap_err();
    let (ty, text) = exc(&err);
    assert_eq!(ty, "CalledProcessError");
    assert!(text.contains("'fetch', 'origin', '--tags')"), "{text}");
    assert!(text.contains("return code: 128"), "{text}");
    assert!(text.contains(&format!("'{missing}' does not appear to be a git repository")), "{text}");
    assert_eq!(h.out.take(), initializing(&missing));
    // Nothing is left behind: the scratch directory of the failed clone is gone.
    let scratch = h.store.scratch_dir();
    assert_eq!(std::fs::read_dir(&scratch).map_or(0, Iterator::count), 0);
    // The next run prints the line again (no store row was created).
    let _again = h.resolve(&cfg).unwrap_err();
    assert_eq!(h.out.take(), initializing(&missing));
}

#[test]
fn x13_git_01_unknown_rev_is_the_checkout_error() {
    let h = H::new();
    let repo = h.t.fixture_repo(Fixture::Resource("script_hooks_repo"));
    let cfg = h.config(&remote_cfg(&repo.to_string_lossy(), "v9.9.9", "    -   id: bash_hook\n"));
    let (ty, text) = exc(&h.resolve(&cfg).unwrap_err());
    assert_eq!(ty, "CalledProcessError");
    assert!(text.contains("'checkout', 'v9.9.9')"), "{text}");
}

/// A local perl hook (its install is one scripted `cpan` call).
fn perl_cfg(h: &H) -> snp_config::Config {
    h.runner.reply("cpan", Reply::ok("").sticky());
    h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: pl\n        name: pl\n        entry: pl\n        language: \
         perl\n",
    )
}

#[test]
fn x13_env_01_malformed_install_state_reinstalls() {
    let h = H::new();
    let cfg = perl_cfg(&h);
    let hooks = h.resolve_and_install(&cfg).unwrap();
    let env_dir = h.store.env_dir(hooks[0].repo.as_ref().unwrap(), LanguageId::Perl, "default");
    let state = env_dir.join(".snp-install-state.json");
    assert!(state.is_file());
    for garbage in [&b"{bad"[..], b"", b"\xff"] {
        h.out.take();
        std::fs::write(&state, garbage).unwrap();
        h.resolve_and_install(&cfg).unwrap();
        assert_eq!(h.out.take(), installing("local"));
        assert!(std::fs::read(&state).unwrap().starts_with(b"{"));
    }
    // pre-commit's own state files are never read nor written.
    std::fs::write(env_dir.join(".install_state_v1"), b"{bad").unwrap();
    h.resolve_and_install(&cfg).unwrap();
    assert_eq!(h.out.take(), "");
    assert!(!env_dir.join(".install_state_v2").exists());
}

#[cfg(unix)]
#[test]
fn x13_env_02_unreadable_install_state_is_a_permission_error() {
    use std::os::unix::fs::PermissionsExt;
    if rustix_is_root() {
        return;
    }
    let h = H::new();
    let cfg = perl_cfg(&h);
    let hooks = h.resolve_and_install(&cfg).unwrap();
    let env_dir = h.store.env_dir(hooks[0].repo.as_ref().unwrap(), LanguageId::Perl, "default");
    let state = env_dir.join(".snp-install-state.json");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o000)).unwrap();
    let err = h.resolve_and_install(&cfg).unwrap_err();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        exc(&err),
        (
            "PermissionError".to_owned(),
            format!("[Errno 13] Permission denied: '{}'", state.display())
        )
    );
}

/// Whether the tests run as root (permission tests are meaningless then).
#[cfg(unix)]
fn rustix_is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| o.stdout.starts_with(b"0\n"))
}

#[test]
fn x13_env_03_install_failure_is_the_called_process_error_and_leaves_nothing() {
    let h = H::new();
    h.runner.reply("cpan", Reply::fail(2, "cpan: boom\n"));
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: pl\n        name: pl\n        entry: pl\n        language: \
         perl\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    h.out.take();
    let err = h
        .with_ctx(&cfg, None, |cx| h.resolver.install_envs(&[&hooks[0]], cx))
        .unwrap_err();
    let (ty, text) = exc(&err);
    assert_eq!(ty, "CalledProcessError");
    assert!(text.contains("cpan: boom"), "{text}");
    assert_eq!(h.out.take(), installing("local"));
    let env_dir = h.store.env_dir(hooks[0].repo.as_ref().unwrap(), LanguageId::Perl, "default");
    assert!(!env_dir.exists());
}

#[test]
fn x13_env_03_version_assertion_runs_before_any_filesystem_mutation() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: local\n    hooks:\n    -   id: pl\n        name: pl\n        entry: pl\n        language: \
         perl\n        language_version: ../../../escape\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    h.out.take();
    let repo = hooks[0].repo.as_ref().unwrap();
    let before: Vec<_> = std::fs::read_dir(&repo.envs).unwrap().collect();
    let err = h
        .with_ctx(&cfg, None, |cx| h.resolver.install_envs(&[&hooks[0]], cx))
        .unwrap_err();
    assert_eq!(exc(&err).0, "AssertionError");
    assert_eq!(h.out.take(), installing("local"));
    assert_eq!(std::fs::read_dir(&repo.envs).unwrap().count(), before.len());
    assert!(h.runner.calls().is_empty());
}

#[test]
fn meta_and_builtin_hooks_resolve_without_the_store() {
    let h = H::new();
    let cfg = h.config(
        "repos:\n-   repo: meta\n    hooks:\n    -   id: identity\n-   repo: builtin\n    hooks:\n    -   id: \
         trailing-whitespace\n    -   id: forbid-submodules\n",
    );
    let hooks = h.resolve(&cfg).unwrap();
    assert_eq!(h.out.take(), "");
    assert!(!h.store.location().path.join("repos-v1").exists());
    assert!(matches!(hooks[0].exec, ExecKind::Meta(snp_repo::MetaHook::Identity)));
    assert!(matches!(hooks[0].src, HookSrc::Meta));
    assert!(matches!(
        &hooks[1].exec,
        ExecKind::Native { source: snp_hooks_data::NativeSource::Builtin, id, .. } if &**id == "trailing-whitespace"
    ));
    assert!(matches!(hooks[2].exec, ExecKind::Language));
    assert_eq!(hooks[2].def.language, LanguageId::Fail);
    let (i0, i1, i2) = (hooks[0].index, hooks[1].index, hooks[2].index);
    assert_eq!((i0, i1, i2), (0, 1, 2));
    let unknown = h.config("repos:\n-   repo: builtin\n    hooks:\n    -   id: nope\n");
    assert!(matches!(h.resolve(&unknown).unwrap_err(), Failure::Exit(1)));
    assert_eq!(
        h.out.take(),
        "[ERROR] `nope` is not present in repository builtin.  Typo? Perhaps it is introduced in a newer version?  \
         Often `pre-commit autoupdate` fixes this.\n"
    );
}

#[test]
fn hook_env_is_cached_per_install_key() {
    let h = H::new();
    let cfg = perl_cfg(&h);
    let hooks = h.resolve_and_install(&cfg).unwrap();
    let (a, b) = h.with_ctx(&cfg, None, |cx| {
        (h.resolver.hook_env(&hooks[0], cx).unwrap(), h.resolver.hook_env(&hooks[0], cx).unwrap())
    });
    let env_dir = h.store.env_dir(hooks[0].repo.as_ref().unwrap(), LanguageId::Perl, "default");
    let perl5lib = a.get("PERL5LIB").unwrap().to_string_lossy().into_owned();
    assert!(perl5lib.contains(&*env_dir.to_string_lossy()), "{perl5lib}");
    assert_eq!(a.get("PERL5LIB"), b.get("PERL5LIB"));
    // Native and meta hooks have no language plan.
    let meta = h.config("repos:\n-   repo: meta\n    hooks:\n    -   id: identity\n");
    let mh = h.resolve(&meta).unwrap();
    let e = h.with_ctx(&meta, None, |cx| h.resolver.plan(&mh[0], cx, false)).unwrap_err();
    assert_eq!(exc(&e).0, "AssertionError");
    assert_eq!(Arc::strong_count(&hooks[0].def), 1);
}
