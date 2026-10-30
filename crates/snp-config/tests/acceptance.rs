//! WP-21 acceptance (WBS "### WP-21"): extension keys (D-EXT-01, `ExtMode::Strict`, `x-*`), `repo: builtin`
//! (D-EXT-06), the local overlay (D-EXT-02/03), the JSON schema, `SAMPLE_CONFIG`, `config dump`, `validate_file`.
//!
//! Owner: WP-21.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod acceptance {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use sha2::Digest as _;
    use snp_config::ext::{Condition, FailOnChanges, FastPath, OutputMode, Priority, StageFixed};
    use snp_config::overlay;
    use snp_config::validate::{FileKind, validate_file};
    use snp_config::{Config, ExtMode, HookDef, RepoConfig, finalize_local, merge_hook};
    use snp_core::brand::Brand;
    use snp_core::env::Env;
    use snp_core::output::CollectWarnings;
    use snp_core::stage::Stage;

    use crate::support::{
        BRANDS, err, file_in_tempdir, hook_ctx, load_config_text, load_manifest_text, ok, with_opts,
    };

    const E: ExtMode = ExtMode::Enabled;
    /// No warnings.
    const NONE: [String; 0] = [];
    /// No unknown keys.
    const NO_KEYS: [Arc<str>; 0] = [];
    const S: ExtMode = ExtMode::Strict;
    const PC: Brand = Brand::PreCommit;

    const EXT_CONFIG: &str = "\
minimum_snp_version: '2.0'
priorities: {early: 1, late: 50}
fail_on_changes: ci
skip: [merge, rebase]
only: {ref: 'release/*', ci: true}
allow_disable: false
fast_path: exact
fast_path_exclude: [check-yaml]
fast_path_coalesce: true
output: summary
repos:
-   repo: local
    hooks:
    -   id: fmt
        name: fmt
        entry: cargo fmt
        language: system
        pass_filenames: false
        priority: early
        env: {RUSTFLAGS: -Dwarnings, A: b}
        groups: [rust, fast]
        stage_fixed: true
        skip: [{env: NO_FMT}, cherry-pick]
        only: bisect
        timeout: 1m30s
        fail_text: run cargo fmt
        root: ./crates//core/
        use_stdin: true
        fail_on_changes: non-ci
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
    -   id: trailing-whitespace
        priority: 3
        stage_fixed: trust
        fail_on_changes: false
";

    fn local_hooks(cfg: &Config) -> &[HookDef] {
        cfg.repos
            .iter()
            .find_map(|r| match r {
                RepoConfig::Local { hooks } => Some(hooks.as_slice()),
                _ => None,
            })
            .expect("local repo")
    }

    #[test]
    fn extension_keys_are_recognised_without_warnings() {
        for brand in BRANDS {
            let (cfg, warnings) = ok(load_config_text(EXT_CONFIG, brand, E));
            assert_eq!(warnings, NONE);
            let r = &cfg.root_ext;
            assert_eq!(r.minimum_snp_version.as_deref(), Some("2.0"));
            assert_eq!(r.priorities.get("late"), Some(&50));
            assert_eq!(r.fail_on_changes, FailOnChanges::Ci);
            assert!(matches!(
                r.skip.as_slice(),
                [Condition::Merge, Condition::Rebase]
            ));
            assert!(
                matches!(r.only.as_slice(), [Condition::All(all)] if matches!(all.as_slice(), [Condition::Ref(g), Condition::Ci(true)] if &**g == "release/*"))
            );
            assert!(!r.allow_disable);
            assert_eq!(r.fast_path, FastPath::Exact);
            assert_eq!(r.fast_path_exclude, [Arc::<str>::from("check-yaml")]);
            assert!(r.fast_path_coalesce);
            assert_eq!(r.output, OutputMode::Summary);
            let h = &local_hooks(&cfg)[0];
            assert_eq!(h.extra_keys, NO_KEYS);
            let x = &h.ext;
            assert!(matches!(&x.priority, Some(Priority::Alias(a)) if &**a == "early"));
            assert_eq!(
                x.env,
                [
                    (Arc::from("RUSTFLAGS"), Arc::from("-Dwarnings")),
                    (Arc::from("A"), Arc::from("b"))
                ]
            );
            assert_eq!(x.groups, [Arc::<str>::from("rust"), Arc::from("fast")]);
            assert_eq!(x.stage_fixed, Some(StageFixed::Verify));
            assert!(
                matches!(x.skip.as_slice(), [Condition::Env(n), Condition::CherryPick] if &**n == "NO_FMT")
            );
            assert!(matches!(x.only.as_slice(), [Condition::Bisect]));
            assert_eq!(x.timeout, Some(Duration::from_secs(90)));
            assert_eq!(x.fail_text.as_deref(), Some("run cargo fmt"));
            assert_eq!(
                x.root.as_ref().map(|p| p.lossy().into_owned()),
                Some("crates/core".to_owned())
            );
            assert!(x.use_stdin);
            assert_eq!(x.fail_on_changes, Some(FailOnChanges::NonCi));
            let RepoConfig::Remote { hooks, .. } = &cfg.repos[1] else {
                panic!("remote")
            };
            assert!(matches!(hooks[0].ext.priority, Some(Priority::Value(3))));
            assert_eq!(hooks[0].ext.stage_fixed, Some(StageFixed::Trust));
            assert_eq!(hooks[0].ext.fail_on_changes, Some(FailOnChanges::Never));
            assert_eq!(
                hooks[0].raw.len(),
                1,
                "extension keys are not hook keys of the merge"
            );
            // No W11 at hook build either.
            let sink = CollectWarnings::new();
            finalize_local(h, &cfg, "local", None, &sink).expect("merge");
            assert_eq!(sink.take(), NONE);
        }
    }

    #[test]
    fn strict_mode_reproduces_pre_commit_warnings() {
        let (cfg, w) = ok(load_config_text(EXT_CONFIG, PC, S));
        assert_eq!(
            w,
            [
                "Unexpected key(s) present at root: allow_disable, fail_on_changes, fast_path, fast_path_coalesce, fast_path_exclude, minimum_snp_version, only, output, priorities, skip"
            ]
        );
        let h = &local_hooks(&cfg)[0];
        let sink = CollectWarnings::new();
        finalize_local(h, &cfg, "local", None, &sink).expect("merge");
        assert_eq!(
            sink.take(),
            [
                "Unexpected key(s) present on local => fmt: env, fail_on_changes, fail_text, groups, only, priority, root, skip, stage_fixed, timeout, use_stdin"
            ]
        );
        assert!(h.ext.priority.is_none() && h.ext.env.is_empty());
        assert!(cfg.root_ext.priorities.is_empty());
        // A remote hook's extension keys stay in the raw dict and warn at merge time.
        let RepoConfig::Remote { hooks, repo, .. } = &cfg.repos[1] else {
            panic!("remote")
        };
        let man = ok(load_manifest_text(
            "- {id: trailing-whitespace, name: t, entry: t, language: python}\n",
            PC,
        ))
        .0;
        merge_hook(&man.hooks[0], &hooks[0], &cfg, repo, None, &sink).expect("merge");
        assert_eq!(
            sink.take(),
            [
                "Unexpected key(s) present on https://github.com/pre-commit/pre-commit-hooks => trailing-whitespace: fail_on_changes, priority, stage_fixed"
            ]
        );
    }

    #[test]
    fn x_keys_warn_exactly_like_pre_commit() {
        let text = "x-anchors: &a {language: system}\nrepos:\n-   repo: local\n    x-repo: 1\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        <<: *a\n        x-hook: 1\n";
        for ext in [E, S] {
            let (cfg, w) = ok(load_config_text(text, PC, ext));
            assert_eq!(
                w,
                [
                    "Unexpected key(s) present on local: x-repo",
                    "Unexpected key(s) present at root: x-anchors"
                ]
            );
            let sink = CollectWarnings::new();
            finalize_local(&local_hooks(&cfg)[0], &cfg, "local", None, &sink).expect("merge");
            assert_eq!(
                sink.take(),
                ["Unexpected key(s) present on local => a: x-hook"]
            );
        }
    }

    #[test]
    fn manifest_extension_keys_are_unknown_keys() {
        let (man, w) = ok(load_manifest_text(
            "- {id: a, name: a, entry: a, language: system, priority: 1}\n",
            PC,
        ));
        assert_eq!(w, NONE);
        assert_eq!(man.hooks[0].extra_keys, [Arc::<str>::from("priority")]);
    }

    fn ext_error(extra: &str) -> String {
        let text = format!(
            "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        {extra}\n"
        );
        err(&load_config_text(&text, PC, E))
    }

    #[test]
    fn extension_values_are_validated_with_pre_commit_contexts() {
        let ctx = hook_ctx("'local'", "'a'");
        let cases = [
            (
                "priority: -1",
                "==> At key: priority\n=====> priority -1 is out of range (expected 0..=4294967295)",
            ),
            (
                "priority: 4294967296",
                "==> At key: priority\n=====> priority 4294967296 is out of range (expected 0..=4294967295)",
            ),
            (
                "priority: true",
                "==> At key: priority\n=====> Expected a non-negative integer or a priority name got bool",
            ),
            (
                "priority: nope",
                "==> At key: priority\n=====> priority name 'nope' is not defined in the top-level `priorities`",
            ),
            ("env: [A]", "==> At key: env\n=====> Expected dict got list"),
            (
                "env: {'A=B': x}",
                "==> At key: env\n==> For key: A=B\n=====> invalid environment variable name 'A=B': names must be non-empty without '=' or NUL",
            ),
            (
                "env: {A: 1}",
                "==> At key: env\n==> At key: A\n=====> Expected string got int",
            ),
            (
                "groups: ['@x']",
                "==> At key: groups\n==> At index 0\n=====> invalid group name '@x': names must be non-empty, without whitespace and must not start with '@' (reserved for selectors such as @ungrouped)",
            ),
            (
                "groups: ['a b']",
                "==> At key: groups\n==> At index 0\n=====> invalid group name 'a b': names must be non-empty, without whitespace and must not start with '@' (reserved for selectors such as @ungrouped)",
            ),
            (
                "groups: ['']",
                "==> At key: groups\n==> At index 0\n=====> invalid group name '': names must be non-empty, without whitespace and must not start with '@' (reserved for selectors such as @ungrouped)",
            ),
            (
                "groups: lint",
                "==> At key: groups\n=====> Expected array but got 'str'",
            ),
            (
                "stage_fixed: always",
                "==> At key: stage_fixed\n=====> Expected one of False, True, trust, verify but got: 'always'",
            ),
            (
                "skip: sometimes",
                "==> At key: skip\n=====> Expected one of bisect, cherry-pick, merge, merge-commit, rebase, revert but got: 'sometimes'",
            ),
            (
                "skip: [{branch: main}]",
                "==> At key: skip\n==> At index 0\n=====> Additional keys found: branch.  Only these keys are allowed: ci, env, ref, run",
            ),
            (
                "only: {ref: '[a'}",
                "==> At key: only\n==> At key: ref\n=====> invalid glob '[a': unclosed character class; missing ']'",
            ),
            (
                "only: {ci: yes-please}",
                "==> At key: only\n==> At key: ci\n=====> Expected bool got str",
            ),
            (
                "skip: {}",
                "==> At key: skip\n=====> Expected at least one condition (merge, rebase, merge-commit, cherry-pick, revert, bisect, ref, env, ci, run)",
            ),
            (
                "skip: {run: ''}",
                "==> At key: skip\n==> At key: run\n=====> Expected a non-empty string for run",
            ),
            (
                "timeout: 30",
                "==> At key: timeout\n=====> Expected string got int",
            ),
            (
                "timeout: 30x",
                "==> At key: timeout\n=====> time: unknown unit \"x\" in duration \"30x\"",
            ),
            (
                "timeout: '0s'",
                "==> At key: timeout\n=====> timeout '0s' must be greater than 0s and at most 24h",
            ),
            (
                "timeout: 25h",
                "==> At key: timeout\n=====> timeout '25h' must be greater than 0s and at most 24h",
            ),
            (
                "fail_text: 1",
                "==> At key: fail_text\n=====> Expected string got int",
            ),
            (
                "root: ../x",
                "==> At key: root\n=====> root '../x' must be a relative directory inside the repository (no '..', not absolute)",
            ),
            (
                "root: /abs",
                "==> At key: root\n=====> root '/abs' must be a relative directory inside the repository (no '..', not absolute)",
            ),
            (
                "use_stdin: 1",
                "==> At key: use_stdin\n=====> Expected bool got int",
            ),
            (
                "fail_on_changes: sometimes",
                "==> At key: fail_on_changes\n=====> Expected one of always, ci, never, non-ci but got: 'sometimes'",
            ),
            (
                "interactive: true",
                "==> At key: interactive\n=====> 'interactive' is not available in this version",
            ),
            (
                "follow: false",
                "==> At key: follow\n=====> 'follow' is not available in this version",
            ),
        ];
        for (extra, leaf) in cases {
            assert_eq!(ext_error(extra), format!("{ctx}{leaf}"), "{extra}");
        }
        let root = |t: &str| err(&load_config_text(&format!("{t}\nrepos: []\n"), PC, E));
        assert_eq!(
            root("minimum_snp_version: '99'"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: minimum_snp_version\n=====> snp 99 is required but snp 2.0.0 is installed."
        );
        assert_eq!(
            root("minimum_snp_version: v2"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: minimum_snp_version\n=====> invalid version 'v2': expected MAJOR.MINOR.PATCH"
        );
        assert_eq!(
            root("priorities: {a: x}"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: priorities\n==> At key: a\n=====> Expected a non-negative integer or a priority name got str"
        );
        assert_eq!(
            root("fast_path: always"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: fast_path\n=====> Expected one of auto, exact, off but got: 'always'"
        );
        assert_eq!(
            root("output: loud"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: output\n=====> Expected one of default, quiet, summary but got: 'loud'"
        );
        assert_eq!(
            root("allow_disable: 0"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: allow_disable\n=====> Expected bool got int"
        );
        // Strict mode: the same values are just unknown keys.
        assert!(
            load_config_text("repos: []\noutput: loud\n", PC, S)
                .result
                .is_ok()
        );
    }

    #[test]
    fn extension_errors_come_after_pre_commit_errors() {
        // A pre-commit error in the same hook wins over an extension error (items are appended).
        let e = ext_error("timeout: nope\n        stages: [bogus]");
        assert!(e.ends_with("==> At key: stages\n==> At index 0\n=====> Expected one of commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg but got: 'bogus'"), "{e}");
    }

    #[test]
    fn priority_alias_resolves_against_root_priorities() {
        let text = "priorities: {early: 1}\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        priority: early\n";
        let (cfg, _) = ok(load_config_text(text, PC, E));
        assert!(
            matches!(&local_hooks(&cfg)[0].ext.priority, Some(Priority::Alias(a)) if &**a == "early")
        );
        let e = err(&load_config_text(
            &text.replace("{early: 1}", "{late: 1}"),
            PC,
            E,
        ));
        assert_eq!(
            e,
            format!(
                "{}==> At key: priority\n=====> priority name 'early' is not defined in the top-level `priorities`",
                hook_ctx("'local'", "'a'")
            )
        );
    }

    // --- repo: builtin ------------------------------------------------------------------------------------------

    fn validate_text(text: &str, ext: ExtMode) -> Option<String> {
        let (_dir, path) = file_in_tempdir("cfg.yaml", Some(text.as_bytes()));
        let l = with_opts(PC, ext, |o| {
            Ok::<_, snp_config::ConfigError>(validate_file(
                FileKind::Config,
                path.as_os_str(),
                o,
                false,
            ))
        });
        l.result
            .expect("infallible")
            .map(|t| t.replace(&path.to_string_lossy().into_owned(), "cfg.yaml"))
    }

    #[test]
    fn builtin_repo_is_validated_by_the_config_schema() {
        let ok_text = "repos:\n-   repo: builtin\n    hooks:\n    -   id: trailing-whitespace\n        args: [--markdown-linebreak-ext=md]\n    -   id: check-yaml\n        language: system\n        priority: 2\n";
        let (cfg, w) = ok(load_config_text(ok_text, PC, E));
        assert_eq!(w, NONE);
        let RepoConfig::Builtin { hooks } = &cfg.repos[0] else {
            panic!("builtin")
        };
        assert_eq!(hooks.len(), 2);
        assert_eq!(
            hooks[1].raw[&cfgv::Value::from("language")],
            cfgv::Value::from("unsupported")
        );
        assert!(matches!(hooks[1].ext.priority, Some(Priority::Value(2))));
        let ctx = |id: &str| {
            format!(
                "\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='builtin')\n==> At key: hooks\n==> At Hook(id='{id}')\n"
            )
        };
        let cases = [
            ("repos:\n-   repo: builtin\n    rev: v6.0.0\n    hooks: [{id: check-yaml}]\n".to_owned(),
             "\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='builtin')\n=====> Expected rev to be absent when repo is 'builtin', found rev: 'v6.0.0'".to_owned()),
            ("repos:\n-   repo: builtin\n    hooks: [{id: check-yaml, entry: x}]\n".to_owned(), format!("{}=====> 'entry' cannot be overridden", ctx("check-yaml"))),
            ("repos:\n-   repo: builtin\n    hooks: [{id: check-yaml, additional_dependencies: [x]}]\n".to_owned(), format!("{}=====> 'additional_dependencies' cannot be overridden", ctx("check-yaml"))),
            ("repos:\n-   repo: builtin\n    hooks: [{id: check-yaml, language: python}]\n".to_owned(), format!("{}==> At key: language\n=====> Expected one of unsupported but got: 'python'", ctx("check-yaml"))),
            ("repos:\n-   repo: builtin\n    hooks: [{id: check-yaml, types: [nope]}]\n".to_owned(), format!("{}==> At key: types\n==> At index 0\n=====> Type tag 'nope' is not recognized.  Try upgrading identify and pre-commit?", ctx("check-yaml"))),
        ];
        for (text, want) in &cases {
            assert_eq!(&err(&load_config_text(text, PC, E)), want);
            assert_eq!(
                validate_text(text, E).as_deref(),
                Some(want.as_str()),
                "validate_file agrees"
            );
        }
        // pre-commit (and snp in Strict mode) sees an ordinary remote repo without a rev.
        let e = err(&load_config_text(
            "repos:\n-   repo: builtin\n    hooks: [{id: check-yaml}]\n",
            PC,
            S,
        ));
        assert_eq!(
            e,
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='builtin')\n=====> Missing required key: rev"
        );
        // The texts of the other repo kinds are unchanged by the builtin items.
        let e = err(&load_config_text(
            "repos:\n-   repo: local\n    rev: v1\n    hooks: []\n",
            PC,
            E,
        ));
        assert!(e.ends_with("=====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1'"));
    }

    // --- local overlay ------------------------------------------------------------------------------------------

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    #[test]
    fn overlay_discovery_rules() {
        let dir = tempfile::tempdir().expect("tmp");
        let cfg = dir.path().join(".pre-commit-config.yaml");
        std::fs::write(&cfg, "repos: []\n").expect("write");
        let never = |_: &Path| false;
        let always = |_: &Path| true;
        assert_eq!(overlay::locate(&cfg, &env(&[]), &never), Ok(None));
        let yml = dir.path().join(".pre-commit-config.local.yml");
        std::fs::write(&yml, "hooks: {}\n").expect("write");
        assert_eq!(overlay::locate(&cfg, &env(&[]), &never), Ok(Some(yml)));
        let yaml = dir.path().join(".pre-commit-config.local.yaml");
        std::fs::write(&yaml, "hooks: {}\n").expect("write");
        assert_eq!(
            overlay::locate(&cfg, &env(&[]), &never),
            Ok(Some(yaml.clone())),
            ".yaml first"
        );
        // Tracked by git ⇒ ignored with a note.
        let note = overlay::locate(&cfg, &env(&[]), &always).expect_err("tracked");
        assert!(
            note.starts_with("snp: ignoring local config ") && note.contains("tracked by git"),
            "{note}"
        );
        // CI ⇒ ignored, unless named by SNP_LOCAL_CONFIG.
        let note = overlay::locate(&cfg, &env(&[("CI", "true")]), &never).expect_err("ci");
        assert!(note.contains("CI is set"), "{note}");
        assert_eq!(
            overlay::locate(&cfg, &env(&[("CI", "0")]), &never),
            Ok(Some(yaml.clone())),
            "CI=0 is not truthy"
        );
        assert_eq!(
            overlay::locate(
                &cfg,
                &env(&[
                    ("CI", "1"),
                    ("SNP_LOCAL_CONFIG", ".pre-commit-config.local.yml")
                ]),
                &always
            ),
            Ok(Some(dir.path().join(".pre-commit-config.local.yml")))
        );
        let abs = yaml.to_string_lossy().into_owned();
        assert_eq!(
            overlay::locate(&cfg, &env(&[("SNP_LOCAL_CONFIG", &abs)]), &always),
            Ok(Some(yaml))
        );
        // Empty / 0 disable; SNP_COMPAT_STRICT disables.
        assert_eq!(
            overlay::locate(&cfg, &env(&[("SNP_LOCAL_CONFIG", "")]), &never),
            Ok(None)
        );
        assert_eq!(
            overlay::locate(&cfg, &env(&[("SNP_LOCAL_CONFIG", "0")]), &never),
            Ok(None)
        );
        assert_eq!(
            overlay::locate(&cfg, &env(&[("SNP_COMPAT_STRICT", "1")]), &never),
            Ok(None)
        );
        let note = overlay::locate(&cfg, &env(&[("SNP_LOCAL_CONFIG", "nope.yaml")]), &never)
            .expect_err("missing");
        assert!(note.contains("does not exist"), "{note}");
    }

    const BASE: &str = "\
repos:
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
    -   id: check-yaml
        groups: [slow]
    -   id: end-of-file-fixer
-   repo: local
    hooks:
    -   id: pytest
        alias: tests
        name: pytest
        entry: pytest -q
        language: system
        pass_filenames: false
        env: {A: '1', B: '2'}
";

    const OVERLAY: &str = "\
hooks:
  tests:
    skip: true
    args: ['-x']
    env: {B: overlay, C: '3'}
  check-yaml:
    entry: docker compose run --rm app {entry}
    language: docker_image
  end-of-file-fixer:
    language: python
    additional_dependencies: [pre-commit-hooks==6.0.0]
    priority: 10
    stage_fixed: true
  nonexistent:
    verbose: true
skip_groups: [slow]
fail_on_changes: never
repos:
-   repo: local
    hooks:
    -   id: extra
        name: extra
        entry: echo extra
        language: system
";

    fn loaded_overlay(
        text: &str,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        Result<overlay::Overlay, snp_config::ConfigError>,
        Vec<String>,
    ) {
        let (dir, path) = file_in_tempdir(".pre-commit-config.local.yaml", Some(text.as_bytes()));
        let l = with_opts(PC, E, |o| overlay::load(&path, o));
        (dir, path, l.result, l.warnings)
    }

    #[test]
    fn overlay_patches_merge_after_manifest_and_config() {
        let (_d, path, ov, w) = loaded_overlay(OVERLAY);
        let ov = ov.expect("overlay");
        assert_eq!(w, NONE);
        let (mut cfg, _) = ok(load_config_text(BASE, PC, E));
        let diag = CollectWarnings::new();
        overlay::apply_root(&mut cfg, &ov, &diag);
        assert_eq!(
            diag.take(),
            [format!(
                "snp: using local overrides from {}",
                path.display()
            )]
        );
        assert_eq!(cfg.root_ext.fail_on_changes, FailOnChanges::Never);
        assert_eq!(cfg.repos.len(), 3, "overlay repos appended");
        let man = ok(load_manifest_text(
            "- {id: check-yaml, name: check yaml, entry: check-yaml, language: python, types: [yaml]}\n- {id: end-of-file-fixer, name: fix eof, entry: end-of-file-fixer, language: python}\n",
            PC,
        ))
        .0;
        let sink = CollectWarnings::new();
        let RepoConfig::Remote { hooks, repo, .. } = &cfg.repos[0] else {
            panic!("remote")
        };
        // {entry} expands against the merged manifest entry; the patch's language changes the install identity.
        let cy = &hooks[0];
        assert!(
            matches!(cy.ext.skip.as_slice(), [Condition::All(a)] if a.is_empty()),
            "skip_groups applied"
        );
        let patch = ov.hook_patch(&cy.id, "").expect("patch");
        assert_eq!(patch.keys().collect::<Vec<_>>(), ["entry", "language"]);
        let merged = merge_hook(
            man.get("check-yaml").expect("m"),
            cy,
            &cfg,
            repo,
            Some(patch),
            &sink,
        )
        .expect("merge");
        assert_eq!(&*merged.entry, "docker compose run --rm app check-yaml");
        assert_eq!(merged.language.as_str(), "docker_image");
        assert_eq!(merged.types.names().collect::<Vec<_>>(), ["yaml"]);
        let eof = &hooks[1];
        let patch = ov.hook_patch(&eof.id, "").expect("patch");
        let merged = merge_hook(
            man.get("end-of-file-fixer").expect("m"),
            eof,
            &cfg,
            repo,
            Some(patch),
            &sink,
        )
        .expect("merge");
        assert_eq!(
            &*merged.additional_dependencies,
            [Arc::<str>::from("pre-commit-hooks==6.0.0")]
        );
        assert!(matches!(merged.ext.priority, Some(Priority::Value(10))));
        assert_eq!(merged.ext.stage_fixed, Some(StageFixed::Verify));
        // Local hook keyed by its alias: args replace, env merges with the overlay winning, skip set.
        let RepoConfig::Local { hooks } = &cfg.repos[1] else {
            panic!("local")
        };
        let pytest = &hooks[0];
        let patch = ov
            .hook_patch(&pytest.id, &pytest.alias)
            .expect("alias match");
        let merged = finalize_local(pytest, &cfg, "local", Some(patch), &sink).expect("merge");
        assert_eq!(&*merged.args, [Arc::<str>::from("-x")]);
        assert_eq!(
            merged.ext.env,
            [
                (Arc::from("A"), Arc::from("1")),
                (Arc::from("B"), Arc::from("overlay")),
                (Arc::from("C"), Arc::from("3"))
            ]
        );
        assert!(matches!(merged.ext.skip.as_slice(), [Condition::All(a)] if a.is_empty()));
        assert_eq!(sink.take(), NONE);
        assert!(
            ov.hook_patch("pytest", "").is_none(),
            "keyed by alias, not id"
        );
        // Unknown ids are reported.
        let matched = [
            "check-yaml",
            "end-of-file-fixer",
            "pytest",
            "tests",
            "extra",
        ];
        assert_eq!(ov.unmatched(&matched), ["nonexistent"]);
        assert_eq!(
            overlay::unknown_hook_note("nonexistent"),
            "snp: local config overrides unknown hook 'nonexistent'"
        );
        // `config dump` marks the overlay.
        let dump = snp_config::effective_dump(&cfg, false);
        assert!(
            dump.starts_with(&format!("# snp: local overrides from {}\n", path.display())),
            "{dump}"
        );
        assert!(dump.contains("#   overridden: hooks.tests.args\n"));
        assert!(dump.contains("#   overridden: repos (+1)\n"));
        assert!(dump.contains("    -   id: extra\n"));
        let json: serde_json::Value =
            serde_json::from_str(&snp_config::effective_dump(&cfg, true)).expect("json");
        assert_eq!(
            json["_snp_local_overrides"]["path"],
            path.to_string_lossy().as_ref()
        );
        assert_eq!(json["repos"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn overlay_patch_language_changes_the_install_key_and_guards_apply() {
        let (_d, _p, ov, _) = loaded_overlay("hooks:\n  h:\n    language: system\n");
        let ov = ov.expect("overlay");
        let (cfg, _) = ok(load_config_text(
            "repos:\n-   repo: https://x/y\n    rev: v1.0\n    hooks: [{id: h}]\n",
            PC,
            E,
        ));
        let man = ok(load_manifest_text(
            "- {id: h, name: h, entry: h, language: python, additional_dependencies: [x]}\n",
            PC,
        ))
        .0;
        let RepoConfig::Remote { hooks, repo, .. } = &cfg.repos[0] else {
            panic!("remote")
        };
        let sink = CollectWarnings::new();
        let e = merge_hook(
            &man.hooks[0],
            &hooks[0],
            &cfg,
            repo,
            ov.hook_patch("h", ""),
            &sink,
        )
        .expect_err("env-less");
        assert_eq!(
            e.message(PC),
            "The hook `h` specifies `additional_dependencies` but is using language `unsupported` which does not install an environment.  Perhaps you meant to use a specific language?"
        );
        let plain = merge_hook(&man.hooks[0], &hooks[0], &cfg, repo, None, &sink).expect("merge");
        assert_eq!(plain.language.as_str(), "python");
    }

    #[test]
    fn overlay_validation_errors_and_warnings() {
        let (_d, _p, ov, _) = loaded_overlay("hooks:\n  a:\n    id: b\n");
        let e = ov.expect_err("id");
        assert!(e.rendered.ends_with("==> At LocalConfig()\n==> At key: hooks\n==> At Hooks()\n==> At key: a\n==> At Hook()\n=====> 'id' cannot be overridden"), "{}", e.rendered);
        let (_d, _p, ov, _) = loaded_overlay("hooks:\n  a:\n    timeout: soon\n");
        assert!(
            ov.expect_err("timeout")
                .rendered
                .ends_with("==> At key: timeout\n=====> time: invalid duration \"soon\"")
        );
        let (_d, _p, ov, _) = loaded_overlay("skip_groups: ['@x']\n");
        assert!(
            ov.expect_err("group")
                .rendered
                .contains("invalid group name '@x'")
        );
        let (_d, _p, ov, w) = loaded_overlay("hooks: {}\nbogus: 1\n");
        assert!(ov.is_ok());
        assert_eq!(w, ["Unexpected key(s) present at root: bogus"]);
        let (_d, _p, ov, _) =
            loaded_overlay("repos:\n-   repo: local\n    rev: x\n    hooks: []\n");
        assert!(ov.expect_err("repo").rendered.ends_with("found rev: 'x'"));
    }

    // --- JSON schema, sample config, dump, validate_file ------------------------------------------------------------

    /// A small JSON Schema (draft 2020-12) evaluator for the keywords `config.schema.json` uses.
    fn check(schema: &serde_json::Value, v: &serde_json::Value, root: &serde_json::Value) -> bool {
        use serde_json::Value as J;
        let Some(s) = schema.as_object() else {
            return schema.as_bool().unwrap_or(true);
        };
        if let Some(r) = s.get("$ref").and_then(J::as_str) {
            let name = r.trim_start_matches("#/$defs/");
            if !check(&root["$defs"][name], v, root) {
                return false;
            }
        }
        if let Some(t) = s.get("type").and_then(J::as_str) {
            let ok = match t {
                "object" => v.is_object(),
                "array" => v.is_array(),
                "string" => v.is_string(),
                "boolean" => v.is_boolean(),
                "integer" => v.is_i64() || v.is_u64(),
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        if let Some(e) = s.get("enum").and_then(J::as_array)
            && !e.contains(v)
        {
            return false;
        }
        if let Some(c) = s.get("const")
            && c != v
        {
            return false;
        }
        if let Some(min) = s.get("minimum").and_then(J::as_i64)
            && v.as_i64().is_some_and(|x| x < min)
        {
            return false;
        }
        if let Some(max) = s.get("maximum").and_then(J::as_u64)
            && v.as_u64().is_some_and(|x| x > max)
        {
            return false;
        }
        if let Some(min) = s.get("minLength").and_then(J::as_u64)
            && v.as_str()
                .is_some_and(|x| u64::try_from(x.chars().count()).expect("len") < min)
        {
            return false;
        }
        if let Some(p) = s.get("pattern").and_then(J::as_str)
            && let Some(x) = v.as_str()
        {
            let rx =
                snp_pyre::PyRegex::compile(p, snp_pyre::Options::CONFIG).expect("schema pattern");
            if !rx.search(x).expect("search") {
                return false;
            }
        }
        if let Some(items) = s.get("items")
            && let Some(arr) = v.as_array()
            && !arr.iter().all(|x| check(items, x, root))
        {
            return false;
        }
        if let Some(obj) = v.as_object() {
            if let Some(req) = s.get("required").and_then(J::as_array)
                && !req
                    .iter()
                    .all(|k| k.as_str().is_some_and(|k| obj.contains_key(k)))
            {
                return false;
            }
            if let Some(min) = s.get("minProperties").and_then(J::as_u64)
                && u64::try_from(obj.len()).expect("len") < min
            {
                return false;
            }
            let props = s.get("properties").and_then(J::as_object);
            for (k, x) in obj {
                match props.and_then(|p| p.get(k)) {
                    Some(ps) => {
                        if !check(ps, x, root) {
                            return false;
                        }
                    }
                    None => {
                        if let Some(ap) = s.get("additionalProperties")
                            && !check(ap, x, root)
                        {
                            return false;
                        }
                    }
                }
                if let Some(pn) = s.get("propertyNames")
                    && !check(pn, &J::from(k.as_str()), root)
                {
                    return false;
                }
            }
        }
        if let Some(all) = s.get("allOf").and_then(J::as_array)
            && !all.iter().all(|x| check(x, v, root))
        {
            return false;
        }
        if let Some(any) = s.get("anyOf").and_then(J::as_array)
            && !any.iter().any(|x| check(x, v, root))
        {
            return false;
        }
        if let Some(one) = s.get("oneOf").and_then(J::as_array)
            && one.iter().filter(|x| check(x, v, root)).count() != 1
        {
            return false;
        }
        if let Some(not) = s.get("not")
            && check(not, v, root)
        {
            return false;
        }
        true
    }

    fn yaml_json(text: &str) -> serde_json::Value {
        let v = snp_yaml::load(text, &snp_yaml::Limits::default()).expect("yaml");
        let cfg = Config {
            source: snp_config::ConfigSource {
                path: PathBuf::new(),
                display: "x".into(),
                project_root: PathBuf::new(),
            },
            repos: Vec::new(),
            default_install_hook_types: Vec::new(),
            default_language_version: indexmap::IndexMap::new(),
            default_stages: snp_core::stage::StageSet::ALL,
            files: Arc::from(""),
            exclude: Arc::from("^$"),
            fail_fast: false,
            minimum_pre_commit_version: Arc::from("0"),
            root_ext: snp_config::ext::RootExt::default(),
            raw: v,
        };
        serde_json::from_str(&snp_config::effective_dump(&cfg, true)).expect("json")
    }

    #[test]
    fn json_schema_validates_sample_and_extension_examples() {
        let schema: serde_json::Value =
            serde_json::from_str(snp_config::json_schema()).expect("schema json");
        let good = [
            snp_config::SAMPLE_CONFIG,
            EXT_CONFIG,
            BASE,
            "repos:\n-   repo: builtin\n    hooks: [{id: check-yaml}]\n-   repo: meta\n    hooks: [{id: identity}]\n",
        ];
        for text in good {
            assert_eq!(ok(load_config_text(text, PC, E)).1, NONE);
            assert!(check(&schema, &yaml_json(text), &schema), "{text}");
        }
        let bad = [
            "repos:\n-   repo: local\n    rev: v1\n    hooks: []\n",
            "repos:\n-   repo: https://x/y\n    hooks: []\n",
            "repos:\n-   repo: meta\n    hooks: [{id: identity, entry: x}]\n",
            "repos:\n-   repo: builtin\n    hooks: [{id: x, entry: y}]\n",
            "repos:\n-   repo: local\n    hooks: [{id: a, name: a, entry: a, language: system, groups: ['@x']}]\n",
            "fast_path: always\nrepos: []\n",
            "default_language_version: {pony: x}\nrepos: []\n",
            "repos: []\nskip: [{branch: main}]\n",
            "default_stages: [bogus]\nrepos: []\n",
            "files: []\n",
        ];
        for text in bad {
            assert!(!check(&schema, &yaml_json(text), &schema), "{text}");
        }
    }

    #[test]
    fn sample_config_sha256_matches_pre_commit() {
        let digest = sha2::Sha256::digest(snp_config::SAMPLE_CONFIG.as_bytes());
        let hex = digest.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            write!(s, "{b:02x}").expect("write");
            s
        });
        assert_eq!(
            hex,
            "8d958ac88a46c7465376a9d8b7dfc0a23565ab522249c31b25a95060ec1d887e"
        );
        let (cfg, w) = ok(load_config_text(snp_config::SAMPLE_CONFIG, PC, E));
        assert_eq!(w, NONE);
        assert_eq!(cfg.repos.len(), 1);
    }

    #[test]
    fn effective_dump_is_pre_commits_yaml_dump() {
        let (cfg, _) = ok(load_config_text(
            "repos:\n-   repo: local\n    hooks: [{id: a, name: a, entry: a, language: system}]\n",
            PC,
            E,
        ));
        let dump = snp_config::effective_dump(&cfg, false);
        assert!(
            dump.starts_with(
                "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n"
            ),
            "{dump}"
        );
        assert!(
            dump.contains("default_install_hook_types:\n- pre-commit\n"),
            "{dump}"
        );
        assert!(
            dump.ends_with("fail_fast: false\nminimum_pre_commit_version: '0'\n")
                || dump.contains("exclude: ^$\n"),
            "{dump}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&snp_config::effective_dump(&cfg, true)).expect("json");
        assert_eq!(json["default_stages"].as_array().map(Vec::len), Some(11));
        assert_eq!(json["default_language_version"]["python"], "default");
    }

    #[test]
    fn validate_file_prints_cfgv_blocks_and_strict_reports_hook_keys() {
        let (_dir, path) = file_in_tempdir("c.yaml", Some(b"repos:\n-   repo: local\n    hooks: [{id: a, name: a, entry: a, language: system, foo: 1}]\n"));
        let l = with_opts(PC, E, |o| {
            Ok::<_, snp_config::ConfigError>(validate_file(
                FileKind::Config,
                path.as_os_str(),
                o,
                true,
            ))
        });
        assert_eq!(l.result.expect("ok"), None);
        assert_eq!(l.warnings, ["Unexpected key(s) present on local => a: foo"]);
        let l = with_opts(PC, E, |o| {
            Ok::<_, snp_config::ConfigError>(validate_file(
                FileKind::Config,
                path.as_os_str(),
                o,
                false,
            ))
        });
        assert_eq!(l.warnings, NONE);
        let l = with_opts(PC, E, |o| {
            Ok::<_, snp_config::ConfigError>(validate_file(
                FileKind::Config,
                OsStr::new("does-not-exist"),
                o,
                false,
            ))
        });
        assert_eq!(
            l.result.expect("ok").as_deref(),
            Some("\n=====> does-not-exist is not a file")
        );
        let (_dir, path) = file_in_tempdir("m.yaml", Some(b"{\n"));
        let l = with_opts(PC, E, |o| {
            Ok::<_, snp_config::ConfigError>(validate_file(
                FileKind::Manifest,
                path.as_os_str(),
                o,
                false,
            ))
        });
        let text = l.result.expect("ok").expect("invalid");
        assert!(
            text.starts_with(&format!(
                "\n==> File {}\n=====> while parsing a flow node",
                path.display()
            )),
            "{text}"
        );
    }

    #[test]
    fn hook_def_defaults_match_hook_py() {
        let (cfg, _) = ok(load_config_text(
            "repos:\n-   repo: local\n    hooks: [{id: a, name: a, entry: a, language: system}]\n",
            PC,
            E,
        ));
        let h = &local_hooks(&cfg)[0];
        assert_eq!(
            (
                &*h.files,
                &*h.exclude,
                &*h.language_version,
                &*h.minimum_pre_commit_version
            ),
            ("", "^$", "default", "0")
        );
        assert_eq!(h.types.names().collect::<Vec<_>>(), ["file"]);
        assert!(h.pass_filenames && !h.always_run && !h.verbose && !h.stages_set);
        let sink = CollectWarnings::new();
        let merged = finalize_local(h, &cfg, "local", None, &sink).expect("merge");
        assert_eq!(merged.stages.iter().count(), 11);
        assert!(merged.stages.contains(Stage::Manual));
        assert!(!merged.stages_set);
    }

    #[test]
    fn manifest_get_returns_the_last_duplicate() {
        let (man, _) = ok(load_manifest_text(
            "- {id: a, name: first, entry: x, language: system}\n- {id: a, name: second, entry: x, language: system}\n",
            PC,
        ));
        assert_eq!(man.hooks.len(), 2);
        assert_eq!(man.get("a").map(|h| &*h.name), Some("second"));
        assert!(man.get("b").is_none());
    }

    #[test]
    fn config_error_failure_envelope() {
        let l = load_config_text("repos: 5\n", PC, E);
        let e = l.result.expect_err("invalid");
        assert_eq!(e.py_type(), "InvalidConfigError");
        assert!(e.uncaught().is_none());
        let f = snp_core::failure::Failure::from(e.clone());
        let snp_core::failure::Failure::Exc { kind, exc } = f else {
            panic!("exc")
        };
        assert_eq!(kind, snp_core::failure::ExcKind::Fatal);
        assert_eq!(exc.ty, "InvalidConfigError");
        assert_eq!(exc.text, e.rendered.as_bytes());
    }
}
