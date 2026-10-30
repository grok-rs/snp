//! The catalog rows WP-21 owns (`tests/catalog/index.tsv`): T10-CFG-01…29 and T10-MIG-01…09 (research 10 §5.1,
//! §5.12), X13-CFG-01…13 and X13-SM-03/10 (research 13 X.4.1, X.5). Each test is named after its row; expected
//! texts are pre-commit 4.6.2's (and, for registered divergences, snp's documented text). Both brands are checked
//! where a text names the tool.
//!
//! Owner: WP-21.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod catalog {
    use std::ffi::OsStr;

    use snp_config::migrate::{Outcome, migrate_config};
    use snp_config::{ExtMode, RepoConfig};
    use snp_core::brand::Brand;
    use snp_core::failure::Failure;
    use snp_core::stage::Stage;

    use crate::support::{
        BRANDS, brand_text, err, hook_ctx, load_config_bytes, load_config_text, load_manifest_text,
        ok,
    };

    const E: ExtMode = ExtMode::Enabled;
    /// No warnings.
    const NONE: [String; 0] = [];
    const PC: Brand = Brand::PreCommit;

    const LOCAL_HOOK: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: do_not_commit\n        name: Block if \"DO NOT COMMIT\" is found\n        entry: DO NOT COMMIT\n        language: pygrep\n        files: ^(.*)$\n";

    const STAGE_CHOICES: &str = "commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg";
    const LANG_CHOICES: &str = "conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script";

    fn local(extra: &str) -> String {
        let mut s = String::from(
            "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n",
        );
        for line in extra.lines() {
            s.push_str("        ");
            s.push_str(line);
            s.push('\n');
        }
        s
    }

    fn valid(text: &str) -> Vec<String> {
        ok(load_config_text(text, PC, E)).1
    }

    fn invalid(text: &str) -> String {
        err(&load_config_text(text, PC, E))
    }

    #[test]
    fn t10_cfg_01_type_tag_check() {
        for brand in BRANDS {
            for tag in ["definitely-not-a-tag", "fiel"] {
                let e = err(&load_config_text(
                    &local(&format!("types: [{tag}]")),
                    brand,
                    E,
                ));
                assert_eq!(
                    e,
                    brand_text(
                        brand,
                        &format!(
                            "{}==> At key: types\n==> At index 0\n=====> Type tag '{tag}' is not recognized.  Try upgrading identify and pre-commit?",
                            hook_ctx("'local'", "'a'")
                        )
                    )
                );
            }
        }
        valid(&local("types: [file]"));
    }

    #[test]
    fn t10_cfg_02_remote_hook_with_files_and_args_is_valid() {
        let base = "repos:\n-   repo: git@github.com:pre-commit/pre-commit-hooks\n    rev: cd74dc150c142c3be70b24eaf0b02cae9d235f37\n    hooks:\n    -   id: pyflakes\n        files: '\\.py$'\n";
        assert_eq!(valid(base), NONE);
        assert_eq!(
            valid(&format!("{base}        args: [foo, bar, baz]\n")),
            NONE
        );
    }

    #[test]
    fn t10_cfg_03_hook_exclude_must_be_a_string() {
        let e = invalid(
            "repos:\n-   repo: git@github.com:pre-commit/pre-commit-hooks\n    rev: cd74dc150c142c3be70b24eaf0b02cae9d235f37\n    hooks:\n    -   id: pyflakes\n        exclude: 0\n",
        );
        assert!(
            e.ends_with(
                "==> At Hook(id='pyflakes')\n==> At key: exclude\n=====> Expected string got int"
            ),
            "{e}"
        );
    }

    #[test]
    fn t10_cfg_04_local_repo_with_rev_fails() {
        let e = invalid(&LOCAL_HOOK.replace("repo: local\n", "repo: local\n    rev: foo\n"));
        assert!(e.ends_with("==> At Repository(repo='local')\n=====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'foo'"), "{e}");
    }

    #[test]
    fn t10_cfg_05_sample_local_config_is_valid() {
        let (cfg, w) = ok(load_config_text(LOCAL_HOOK, PC, E));
        assert_eq!(w, NONE);
        let RepoConfig::Local { hooks } = &cfg.repos[0] else {
            panic!("local")
        };
        assert_eq!(&*hooks[0].id, "do_not_commit");
        assert_eq!(hooks[0].language.as_str(), "pygrep");
    }

    #[test]
    fn t10_cfg_06_config_hook_schema_has_no_defaults() {
        // #227: a config hook keeps only the keys written; merging must not clobber manifest values.
        let (cfg, _) = ok(load_config_text(
            "repos:\n-   repo: https://x/y\n    rev: v1.0\n    hooks: [{id: fake-hook}]\n",
            PC,
            E,
        ));
        let RepoConfig::Remote { hooks, .. } = &cfg.repos[0] else {
            panic!("remote")
        };
        assert_eq!(hooks[0].raw.len(), 1);
    }

    #[test]
    fn t10_cfg_07_ci_map_allowed_without_warning() {
        assert_eq!(
            valid("ci: {skip: [foo]}\nrepos: [{repo: meta, hooks: [{id: identity}]}]\n"),
            NONE
        );
        assert_eq!(
            invalid("ci: invalid\nrepos: []\n"),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: ci\n=====> Expected dict got str"
        );
    }

    #[test]
    fn t10_cfg_08_mutable_rev_warning() {
        let cfg = |rev: &str| {
            format!(
                "repos:\n-   repo: https://gitlab.com/pycqa/flake8\n    rev: {rev}\n    hooks: [{{id: flake8}}]\n"
            )
        };
        for rev in [
            "v0.12.4",
            "b27f281",
            "b27f281eb9398fc8504415d7fbdabf119ea8c5e1",
            "19.10b0",
            "4.3.21-2",
        ] {
            assert!(valid(&cfg(rev)).is_empty(), "{rev}");
        }
        for brand in BRANDS {
            for rev in ["''", "HEAD", "stable", "master", "some_branch_name"] {
                let (_, w) = ok(load_config_text(&cfg(rev), brand, E));
                assert_eq!(
                    w,
                    [brand_text(
                        brand,
                        "The 'rev' field of repo 'https://gitlab.com/pycqa/flake8' appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this."
                    )],
                    "{rev}"
                );
            }
        }
    }

    #[test]
    fn t10_cfg_09_meta_repo_with_rev_fails() {
        let e = invalid("repos:\n-   repo: meta\n    rev: 3.7.7\n    hooks: [{id: identity}]\n");
        assert!(e.ends_with("found rev: '3.7.7'"), "{e}");
    }

    #[test]
    fn t10_cfg_10_files_none_is_not_a_string() {
        let e = invalid(&local("files: ~"));
        assert!(
            e.ends_with("\n==> At key: files\n=====> Expected string got NoneType"),
            "{e}"
        );
        let e = invalid("files:\nrepos: []\n");
        assert_eq!(
            e,
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: files\n=====> Expected string got NoneType"
        );
    }

    #[test]
    fn t10_cfg_11_sensible_regex_at_hook() {
        let cases = [
            (
                "dir/*.py",
                "The 'files' field in hook 'flake8' is a regex, not a glob -- matching '/*' probably isn't what you want here",
            ),
            (
                r"dir[\/].*\.py",
                r"pre-commit normalizes slashes in the 'files' field in hook 'flake8' to forward slashes, so you can use / instead of [\/]",
            ),
            (
                r"dir[/\\].*\.py",
                r"pre-commit normalizes slashes in the 'files' field in hook 'flake8' to forward slashes, so you can use / instead of [/\\]",
            ),
            (
                r"dir[\\/].*\.py",
                r"pre-commit normalizes slashes in the 'files' field in hook 'flake8' to forward slashes, so you can use / instead of [\\/]",
            ),
        ];
        for brand in BRANDS {
            for (re, warning) in cases {
                let text = format!(
                    "repos:\n-   repo: https://gitlab.com/pycqa/flake8\n    rev: v1.0.0\n    hooks:\n    -   id: flake8\n        files: '{re}'\n"
                );
                assert_eq!(
                    ok(load_config_text(&text, brand, E)).1,
                    [brand_text(brand, warning)]
                );
            }
        }
        let w = valid(&LOCAL_HOOK.replace("files: ^(.*)$", "files: dir/*.py"));
        assert_eq!(
            w,
            [
                "The 'files' field in hook 'do_not_commit' is a regex, not a glob -- matching '/*' probably isn't what you want here"
            ]
        );
        let w = valid("repos:\n-   repo: meta\n    hooks: [{id: identity, exclude: dir/*.py}]\n");
        assert_eq!(
            w,
            [
                "The 'exclude' field in hook 'identity' is a regex, not a glob -- matching '/*' probably isn't what you want here"
            ]
        );
    }

    #[test]
    fn t10_cfg_12_sensible_regex_at_top_level() {
        let cases = [
            (
                "dir/*.py",
                "The top-level 'files' field is a regex, not a glob -- matching '/*' probably isn't what you want here",
            ),
            (
                r"dir[\/].*\.py",
                r"pre-commit normalizes the slashes in the top-level 'files' field to forward slashes, so you can use / instead of [\/]",
            ),
            (
                r"dir[/\\].*\.py",
                r"pre-commit normalizes the slashes in the top-level 'files' field to forward slashes, so you can use / instead of [/\\]",
            ),
            (
                r"dir[\\/].*\.py",
                r"pre-commit normalizes the slashes in the top-level 'files' field to forward slashes, so you can use / instead of [\\/]",
            ),
        ];
        for brand in BRANDS {
            for (re, warning) in cases {
                let w = ok(load_config_text(
                    &format!("files: '{re}'\nrepos: []\n"),
                    brand,
                    E,
                ))
                .1;
                assert_eq!(w, [brand_text(brand, warning)]);
            }
        }
    }

    #[test]
    fn t10_cfg_13_invalid_local_stage_error() {
        let e = invalid(&format!("{LOCAL_HOOK}        stages: [invalid]\n"));
        assert_eq!(
            e,
            format!(
                "{}==> At key: stages\n==> At index 0\n=====> Expected one of {STAGE_CHOICES} but got: 'invalid'",
                hook_ctx("'local'", "'do_not_commit'")
            )
        );
    }

    #[test]
    fn t10_cfg_14_deprecated_stage_warnings() {
        for brand in BRANDS {
            let w = ok(load_config_text(
                &format!("{LOCAL_HOOK}        stages: [commit, push]\n"),
                brand,
                E,
            ))
            .1;
            assert_eq!(
                w,
                [brand_text(
                    brand,
                    "hook id `do_not_commit` uses deprecated stage names (commit, push) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this."
                )]
            );
            let w = ok(load_config_text(
                "default_stages: [commit, push]\nrepos: []\n",
                brand,
                E,
            ))
            .1;
            assert_eq!(
                w,
                [brand_text(
                    brand,
                    "top-level `default_stages` uses deprecated stage names (commit, push) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this."
                )]
            );
        }
        assert_eq!(
            valid(&format!(
                "{LOCAL_HOOK}        stages: [pre-commit, pre-push]\n"
            )),
            NONE
        );
        assert_eq!(
            valid("default_stages: [pre-commit, pre-push]\nrepos: []\n"),
            NONE
        );
    }

    #[test]
    fn t10_cfg_15_system_and_script_are_translated() {
        let two = format!(
            "{}{}",
            LOCAL_HOOK.replace("pygrep", "system"),
            LOCAL_HOOK
                .replace("repos:\n", "")
                .replace("pygrep", "script")
        );
        let (cfg, _) = ok(load_config_text(&two, PC, E));
        let langs: Vec<&str> = cfg
            .repos
            .iter()
            .map(|r| match r {
                RepoConfig::Local { hooks } => hooks[0].language.as_str(),
                _ => panic!("local"),
            })
            .collect();
        assert_eq!(langs, ["unsupported", "unsupported_script"]);
        let e = invalid(&LOCAL_HOOK.replace("        language: pygrep\n", ""));
        assert!(
            e.ends_with("==> At Hook(id='do_not_commit')\n=====> Missing required key: language"),
            "{e}"
        );
    }

    #[test]
    fn t10_cfg_16_valid_manifests() {
        for m in [
            "- {id: a, name: b, entry: c, language: python, files: '\\.py$'}\n",
            "- {id: a, name: b, entry: c, language: python, language_version: python3.4, files: '\\.py$'}\n",
            "- {id: a, name: b, entry: c, language: python, files: '', always_run: true}\n",
        ] {
            let (man, w) = ok(load_manifest_text(m, PC));
            assert_eq!(w, NONE);
            assert_eq!(&*man.hooks[0].id, "a");
        }
    }

    #[test]
    fn t10_cfg_17_invalid_meta_hooks() {
        let meta = |h: &str| format!("repos:\n-   repo: meta\n    hooks: [{h}]\n");
        assert!(invalid(&meta("{id: i-dont-exist}")).ends_with("==> At key: id\n=====> Expected one of check-hooks-apply, check-useless-excludes, identity but got: 'i-dont-exist'"));
        assert!(
            invalid(&meta("{id: identity, language: python}")).ends_with(
                "==> At key: language\n=====> Expected one of unsupported but got: 'python'"
            )
        );
        assert!(
            invalid(&meta("{id: identity, name: False}"))
                .ends_with("==> At key: name\n=====> Expected string got bool")
        );
        assert!(
            invalid(&meta("{id: identity, entry: echo hi}"))
                .ends_with("==> At Hook(id='identity')\n=====> 'entry' cannot be overridden")
        );
    }

    #[test]
    fn t10_cfg_18_meta_check_hooks_apply_defaults() {
        let (cfg, _) = ok(load_config_text(
            "repos:\n-   repo: meta\n    hooks: [{id: check-hooks-apply}]\n",
            PC,
            E,
        ));
        let RepoConfig::Meta { hooks } = &cfg.repos[0] else {
            panic!("meta")
        };
        let h = &hooks[0];
        assert_eq!(&*h.files, r"^\.pre\-commit\-config\.yaml$");
        assert_eq!(&*h.name, "Check hooks apply to the repository");
        let rx = snp_pyre::PyRegex::compile(&h.files, snp_pyre::Options::CONFIG).expect("regex");
        assert!(rx.search(".pre-commit-config.yaml").expect("search"));
        assert!(!rx.search("foo/.pre-commit-config.yaml").expect("search"));
    }

    #[test]
    fn t10_cfg_19_invalid_default_language_version() {
        let e = invalid("default_language_version: {pony: '1.0'}\nrepos: []\n");
        assert_eq!(
            e,
            format!(
                "\n==> File cfg.yaml\n==> At Config()\n==> At key: default_language_version\n==> At DefaultLanguageVersion()\n=====> Additional keys found: pony.  Only these keys are allowed: {LANG_CHOICES}"
            )
        );
        let e = invalid("default_language_version: {python: 3}\nrepos: []\n");
        assert!(e.ends_with("==> At DefaultLanguageVersion()\n==> At key: python\n=====> Expected string got int"), "{e}");
    }

    #[test]
    fn t10_cfg_20_parse_version_ordering() {
        // `parse_version('0.1') > parse_version('0.0')`, `'2.1' >= '2'`: observable through the min-version check.
        assert_eq!(
            valid("minimum_pre_commit_version: '4.6'\nrepos: []\n"),
            NONE
        );
        assert_eq!(
            valid("minimum_pre_commit_version: '4.6.2'\nrepos: []\n"),
            NONE
        );
        assert!(
            invalid("minimum_pre_commit_version: '4.6.2.0'\nrepos: []\n")
                .contains("pre-commit version 4.6.2.0 is required")
        );
        assert!(
            invalid("minimum_pre_commit_version: '4.7'\nrepos: []\n")
                .contains("pre-commit version 4.7 is required")
        );
    }

    #[test]
    fn t10_cfg_21_minimum_pre_commit_version_checked_first() {
        for brand in BRANDS {
            let want = brand_text(
                brand,
                "\n==> File cfg.yaml\n==> At Config()\n==> At key: minimum_pre_commit_version\n=====> pre-commit version 999 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`.",
            );
            assert_eq!(
                err(&load_config_text(
                    "minimum_pre_commit_version: '999'\nrepos: []\n",
                    brand,
                    E
                )),
                want
            );
            assert_eq!(
                err(&load_config_text(
                    "repos: 5\nminimum_pre_commit_version: '999'\n",
                    brand,
                    E
                )),
                want
            );
            let hook = err(&load_config_text(
                &format!("{LOCAL_HOOK}        minimum_pre_commit_version: '999'\n"),
                brand,
                E,
            ));
            assert_eq!(
                hook,
                brand_text(
                    brand,
                    &format!(
                        "{}==> At key: minimum_pre_commit_version\n=====> pre-commit version 999 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`.",
                        hook_ctx("'local'", "'do_not_commit'")
                    )
                )
            );
        }
        assert_eq!(valid("minimum_pre_commit_version: '0'\nrepos: []\n"), NONE);
    }

    #[test]
    fn t10_cfg_22_warn_additional_keys() {
        assert_eq!(
            valid("repos: []\nfoo: 1\n"),
            ["Unexpected key(s) present at root: foo"]
        );
        assert_eq!(
            valid(
                "repos:\n-   repo: https://gitlab.com/pycqa/flake8\n    rev: 3.7.7\n    hooks: [{id: flake8}]\n    args: [foo]\n"
            ),
            ["Unexpected key(s) present on https://gitlab.com/pycqa/flake8: args"]
        );
        // Hook-level unknown keys are reported when the hooks are built (repository_test.py:201-215).
        let (cfg, w) = ok(load_config_text(
            &local("hello: 1\nfoo: 2").replace("id: a", "id: too-much"),
            PC,
            E,
        ));
        assert_eq!(w, NONE);
        let RepoConfig::Local { hooks } = &cfg.repos[0] else {
            panic!("local")
        };
        let sink = snp_core::output::CollectWarnings::new();
        snp_config::finalize_local(&hooks[0], &cfg, "local", None, &sink).expect("merge");
        assert_eq!(
            sink.take(),
            ["Unexpected key(s) present on local => too-much: foo, hello"]
        );
    }

    #[test]
    fn t10_cfg_23_stages_migration_keeps_order() {
        let (cfg, _) = ok(load_config_text(
            "default_stages: [commit-msg, push, commit, merge-commit]\nrepos: []\n",
            PC,
            E,
        ));
        let raw = cfg.raw.as_map().expect("map");
        assert_eq!(
            raw[&cfgv::Value::from("default_stages")],
            cfgv::Value::List(
                ["commit-msg", "pre-push", "pre-commit", "pre-merge-commit"]
                    .map(cfgv::Value::from)
                    .to_vec()
            )
        );
        let names: Vec<&str> = cfg.default_stages.iter().map(Stage::as_str).collect();
        assert_eq!(
            names,
            ["commit-msg", "pre-commit", "pre-merge-commit", "pre-push"]
        );
        let (man, _) = ok(load_manifest_text(
            "- {id: fake-hook, name: fake-hook, entry: fake-hook, language: system, stages: [commit-msg, push, commit, merge-commit]}\n",
            PC,
        ));
        assert_eq!(man.hooks[0].stages.iter().count(), 4);
        let (cfg, _) = ok(load_config_text(
            "repos:\n-   repo: https://x/y\n    rev: v1.0\n    hooks: [{id: fake-hook}, {id: h2, stages: [commit-msg, push, commit, merge-commit]}]\n",
            PC,
            E,
        ));
        let RepoConfig::Remote { hooks, .. } = &cfg.repos[0] else {
            panic!("remote")
        };
        assert!(!hooks[0].raw.contains_key(&cfgv::Value::from("stages")));
        assert_eq!(
            hooks[1].raw[&cfgv::Value::from("stages")],
            cfgv::Value::List(
                ["commit-msg", "pre-push", "pre-commit", "pre-merge-commit"]
                    .map(cfgv::Value::from)
                    .to_vec()
            )
        );
    }

    #[test]
    fn t10_cfg_24_manifest_map_forward_compat() {
        for brand in BRANDS {
            let l = crate::support::with_opts(brand, E, |o| {
                snp_config::load_manifest_bytes(
                    b"hooks: {}",
                    OsStr::new("/r/.pre-commit-hooks.yaml"),
                    o,
                )
            });
            let e = l.result.expect_err("map manifest");
            assert!(e.manifest);
            assert_eq!(
                e.rendered,
                brand_text(
                    brand,
                    "\n==> File /r/.pre-commit-hooks.yaml\n=====> \n=====> pre-commit version 5 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`."
                )
            );
        }
    }

    #[test]
    fn t10_cfg_25_yaml_11_booleans() {
        let (cfg, _) = ok(load_config_text(
            &local("always_run: yes\npass_filenames: off"),
            PC,
            E,
        ));
        let RepoConfig::Local { hooks } = &cfg.repos[0] else {
            panic!("local")
        };
        assert!(hooks[0].always_run && !hooks[0].pass_filenames);
    }

    #[test]
    fn t10_cfg_26_float_rev() {
        assert_eq!(
            invalid(
                "repos:\n-   repo: https://example.com/r\n    rev: 1.10\n    hooks: [{id: h}]\n"
            ),
            "\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='https://example.com/r')\n==> At key: rev\n=====> Expected string got float"
        );
    }

    #[test]
    fn t10_cfg_27_duplicate_top_level_key() {
        assert_eq!(valid("repos: []\nrepos: []\n"), NONE);
    }

    #[test]
    fn t10_cfg_28_anchors_and_merge_keys() {
        let w = valid(
            "x-common: &common\n    language: system\n    entry: echo\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        <<: *common\n",
        );
        assert_eq!(w, ["Unexpected key(s) present at root: x-common"]);
    }

    #[test]
    fn t10_cfg_29_missing_name_in_local_hook() {
        assert_eq!(
            invalid(
                "repos:\n-   repo: local\n    hooks:\n    -   id: x\n        entry: x\n        language: system\n"
            ),
            format!(
                "{}=====> Missing required key: name",
                hook_ctx("'local'", "'x'")
            )
        );
    }

    // --- T10-MIG (10 §5.12) --------------------------------------------------------------------------------------

    fn migrated(src: &str) -> Option<String> {
        match migrate_config(src.as_bytes(), OsStr::new(".pre-commit-config.yaml"))
            .expect("migrate")
        {
            Outcome::Migrated(b) => Some(String::from_utf8(b).expect("utf8")),
            Outcome::AlreadyMigrated => None,
            _ => panic!("outcome"),
        }
    }

    const LIST_ROOT: &str = "-   repo: local\n    hooks:\n    -   id: foo\n        name: foo\n        entry: ./bin/foo.sh\n        language: script\n";

    #[test]
    fn t10_mig_01_top_level_list() {
        assert_eq!(
            migrated(LIST_ROOT).as_deref(),
            Some(format!("repos:\n{LIST_ROOT}").as_str())
        );
        assert_eq!(
            snp_config::migrate::MIGRATED,
            "Configuration has been migrated."
        );
    }

    #[test]
    fn t10_mig_02_header_lines_kept() {
        let src = format!("# comment\n\n---\n{LIST_ROOT}");
        assert_eq!(
            migrated(&src),
            Some(format!("# comment\n\n---\nrepos:\n{LIST_ROOT}"))
        );
    }

    #[test]
    fn t10_mig_03_flow_list_indented() {
        let src = "[{\n    repo: local,\n    hooks: [{\n        id: foo, name: foo, entry: ./bin/foo.sh,\n        language: script,\n    }]\n}]";
        assert_eq!(
            migrated(src).as_deref(),
            Some(
                "repos:\n    [{\n        repo: local,\n        hooks: [{\n            id: foo, name: foo, entry: ./bin/foo.sh,\n            language: script,\n        }]\n    }]"
            )
        );
    }

    #[test]
    fn t10_mig_04_already_migrated() {
        assert_eq!(migrated(&format!("repos:\n{LIST_ROOT}")), None);
        assert_eq!(
            snp_config::migrate::ALREADY_MIGRATED,
            "Configuration is already migrated."
        );
    }

    #[test]
    fn t10_mig_05_sha_to_rev() {
        let src = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    sha: v1.2.0\n    hooks: []\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    sha: v1.2.0\n    hooks: []\n";
        assert_eq!(migrated(src), Some(src.replace("sha:", "rev:")));
        let json = "{\"repos\": [{\n    \"repo\": \"https://github.com/pre-commit/pre-commit-hooks\",\n    \"sha\": \"v1.2.0\",\n    \"hooks\": []\n}]}\n";
        assert_eq!(migrated(json), Some(json.replace("\"sha\"", "\"rev\"")));
    }

    #[test]
    fn t10_mig_06_python_venv() {
        let src = "repos:\n-   repo: local\n    hooks:\n    -   id: example\n        name: example\n        entry: example\n        language: python_venv\n    -   id: example\n        name: example\n        entry: example\n        language: system\n";
        assert_eq!(migrated(src), Some(src.replace("python_venv", "python")));
        let quoted = "repos:\n-   repo: local\n    hooks:\n    -   id: example\n        name: example\n        entry: example\n        language: \"python_venv\"\n";
        assert_eq!(
            migrated(quoted),
            Some(quoted.replace("python_venv", "python"))
        );
    }

    #[test]
    fn t10_mig_07_legacy_stages() {
        assert_eq!(
            migrated("default_stages: [commit, push, merge-commit, commit-msg]\nrepos: []\n")
                .as_deref(),
            Some(
                "default_stages: [pre-commit, pre-push, pre-merge-commit, commit-msg]\nrepos: []\n"
            )
        );
        let src = "repos:\n-   repo: local\n    hooks:\n    -   id: example\n        name: example\n        entry: example\n        language: system\n        stages: [\"commit\", \"push\", \"merge-commit\", \"commit-msg\"]\n";
        assert_eq!(
            migrated(src),
            Some(src.replace(
                "[\"commit\", \"push\", \"merge-commit\"",
                "[\"pre-commit\", \"pre-push\", \"pre-merge-commit\""
            ))
        );
    }

    #[test]
    fn t10_mig_08_invalid_yaml() {
        let e = migrate_config(b"[", OsStr::new(".pre-commit-config.yaml")).expect_err("invalid");
        assert!(
            e.rendered
                .starts_with("\n==> File .pre-commit-config.yaml\n=====> "),
            "{}",
            e.rendered
        );
        assert_eq!(Failure::from(e).exit_code(), 1);
    }

    #[test]
    fn t10_mig_09_matchers_yield_scalars_only() {
        // `- name: [not, foo]`-style shapes: a `sha` key holding a collection, a `language` that is a list and
        // non-map / non-seq roots are never rewritten.
        assert_eq!(
            migrated("repos:\n-   repo: r\n    hooks: []\n    rev: v1\n    x: {sha: [a]}\n"),
            None
        );
        assert_eq!(
            migrated("repos:\n-   repo: local\n    hooks:\n    -   language: [python_venv]\n"),
            None
        );
        assert_eq!(migrated("foo\n"), None);
        assert_eq!(migrated(""), None);
        assert_eq!(migrated("repos: 5\n"), None);
        assert_eq!(migrated("default_stages: {commit: 1}\nrepos: []\n"), None);
    }

    // --- X13-CFG (13 X.4.1) ---------------------------------------------------------------------------------------

    #[test]
    fn x13_cfg_01_min_version_int_error_is_a_validation_error() {
        for (lit, bad) in [("abc", "abc"), ("", ""), ("4.", ""), ("4.6.3rc1", "3rc1")] {
            let l = load_config_text(
                &format!("minimum_pre_commit_version: \"{lit}\"\nrepos: []\n"),
                PC,
                E,
            );
            assert_eq!(
                err(&l),
                format!(
                    "\n==> File cfg.yaml\n==> At Config()\n==> At key: minimum_pre_commit_version\n=====> invalid literal for int() with base 10: '{bad}'"
                )
            );
            assert_eq!(Failure::from(l.result.expect_err("err")).exit_code(), 1);
        }
        let e = invalid(&local("minimum_pre_commit_version: abc"));
        assert!(e.ends_with("==> At Hook(id='a')\n==> At key: minimum_pre_commit_version\n=====> invalid literal for int() with base 10: 'abc'"));
    }

    #[test]
    fn x13_cfg_02_non_string_root_key_warns() {
        assert_eq!(
            valid("repos: []\n1: x\n"),
            ["Unexpected key(s) present at root: 1"]
        );
    }

    #[test]
    fn x13_cfg_03_mixed_root_keys_warn() {
        assert_eq!(
            valid("repos: []\n1: x\nfoo: y\n"),
            ["Unexpected key(s) present at root: 1, foo"]
        );
    }

    #[test]
    fn x13_cfg_04_unhashable_values_are_one_of_errors() {
        assert!(invalid(&local("stages: [[a]]")).ends_with(&format!("==> At key: stages\n==> At index 0\n=====> Expected one of {STAGE_CHOICES} but got: ['a']")));
        assert!(
            invalid(&local("").replace("language: system", "language: [x]")).ends_with(&format!(
                "==> At key: language\n=====> Expected one of {LANG_CHOICES} but got: ['x']"
            ))
        );
        for brand in BRANDS {
            let e = err(&load_config_text(&local("types: [[a]]"), brand, E));
            assert!(e.ends_with(&brand_text(brand, "==> At index 0\n=====> Type tag ['a'] is not recognized.  Try upgrading identify and pre-commit?")), "{e}");
        }
    }

    #[test]
    fn x13_cfg_05_huge_repeat_bound_is_not_a_valid_regex() {
        for pat in [
            "a{4294967295}",
            "a{4294967296}",
            "a{0,4294967295}",
            "a{99999999999,}",
        ] {
            assert_eq!(
                invalid(&format!("files: '{pat}'\nrepos: []\n")),
                format!(
                    "\n==> File cfg.yaml\n==> At Config()\n==> At key: files\n=====> '{pat}' is not a valid python regex"
                )
            );
            assert!(
                invalid(&local(&format!("exclude: '{pat}'")))
                    .ends_with(&format!("=====> '{pat}' is not a valid python regex"))
            );
        }
        assert_eq!(valid("files: 'a{4294967294}'\nrepos: []\n"), NONE);
    }

    #[test]
    fn x13_cfg_06_deep_nesting_is_not_a_valid_regex() {
        let deep = |n: usize| {
            let mut s: String = std::iter::repeat_n('(', n).collect();
            s.extend(std::iter::repeat_n(')', n));
            s
        };
        assert_eq!(valid(&format!("files: '{}'\nrepos: []\n", deep(484))), NONE);
        assert!(
            invalid(&format!("files: '{}'\nrepos: []\n", deep(485)))
                .ends_with("' is not a valid python regex")
        );
        assert!(
            invalid(&format!("files: '{}'\nrepos: []\n", deep(490)))
                .ends_with("' is not a valid python regex")
        );
    }

    #[test]
    fn x13_cfg_07_conflicting_global_flags() {
        for pat in ["(?a)(?u)x", "(?u)(?a)x", "(?au)x"] {
            assert!(
                invalid(&format!("exclude: '{pat}'\nrepos: []\n"))
                    .ends_with(&format!("=====> '{pat}' is not a valid python regex"))
            );
        }
        assert_eq!(valid("exclude: '(?u)(?a:x)'\nrepos: []\n"), NONE);
    }

    #[test]
    fn x13_cfg_08_future_warning_patterns_are_accepted() {
        // PYTHONWARNINGS=error turns the FutureWarning into a crash in pre-commit; snp never reads it.
        for pat in ["[[]", "[[:alpha:]]", "[a&&b]", "[a||b]", "[a~~b]"] {
            let l = load_config_text(&format!("files: '{pat}'\nrepos: []\n"), PC, E);
            assert!(l.result.is_ok(), "{pat}");
            assert_eq!(l.diag.len(), 1, "{pat}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn x13_cfg_09_unreadable_config_is_an_uncaught_permission_error() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) =
            crate::support::file_in_tempdir(".pre-commit-config.yaml", Some(b"repos: []\n"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read(&path).is_ok() {
            return; // running as root: permissions are not enforced
        }
        let src = snp_config::ConfigSource {
            path: path.clone(),
            display: ".pre-commit-config.yaml".into(),
            project_root: path.parent().expect("dir").to_path_buf(),
        };
        let l = crate::support::with_opts(PC, E, |o| snp_config::load_config(&src, o));
        let e = l.result.expect_err("unreadable");
        assert_eq!(
            e.rendered,
            "PermissionError: [Errno 13] Permission denied: '.pre-commit-config.yaml'"
        );
        assert_eq!(
            e.uncaught(),
            Some((
                "PermissionError",
                "[Errno 13] Permission denied: '.pre-commit-config.yaml'"
            ))
        );
        let f = Failure::from(e);
        assert_eq!(f.exit_code(), 3);
        let text = snp_config::validate::validate_file(
            snp_config::validate::FileKind::Config,
            path.as_os_str(),
            &snp_config::LoadOpts {
                brand: PC,
                ext: E,
                regexes: &snp_pyre::RegexCache::new(),
                warn: &snp_core::output::CollectWarnings::new(),
                diag: &snp_core::output::CollectWarnings::new(),
            },
            false,
        )
        .expect("error");
        assert!(text.starts_with("PermissionError: [Errno 13] Permission denied: "));
    }

    #[test]
    fn x13_cfg_10_migrate_missing_config() {
        let dir = tempfile::tempdir().expect("tmp");
        let e = snp_config::migrate::read_config(
            &dir.path().join(".pre-commit-config.yaml"),
            OsStr::new(".pre-commit-config.yaml"),
        )
        .expect_err("missing");
        assert_eq!(e.rendered, "\n=====> .pre-commit-config.yaml is not a file");
        assert_eq!(Failure::from(e).exit_code(), 1);
    }

    #[test]
    fn x13_cfg_11_migrate_non_utf8_config() {
        let e = migrate_config(
            b"repos: []\n# caf\xe9 au lait\n",
            OsStr::new(".pre-commit-config.yaml"),
        )
        .expect_err("bad utf8");
        assert_eq!(
            e.rendered,
            "\n==> File .pre-commit-config.yaml\n=====> 'utf-8' codec can't decode byte 0xe9 in position 15: invalid continuation byte"
        );
        assert_eq!(Failure::from(e).exit_code(), 1);
    }

    #[test]
    fn x13_cfg_12_migrate_invalid_fallback() {
        let e = migrate_config(
            b"%YAML 1.1\n---\n-   repo: local\n    hooks: []\n",
            OsStr::new(".pre-commit-config.yaml"),
        )
        .expect_err("fallback");
        assert_eq!(
            e.rendered,
            "\n==> File .pre-commit-config.yaml\n=====> while scanning for the next token\nfound character that cannot start any token\n  in \"<unicode string>\", line 2, column 5"
        );
    }

    #[test]
    fn x13_cfg_13_manifest_crash_rows_are_manifest_errors() {
        let l = load_manifest_text(
            "- {id: a, name: a, entry: a, language: system, files: 'a{4294967296}'}\n",
            PC,
        );
        let e = l.result.expect_err("overflow");
        assert!(e.manifest);
        assert_eq!(
            e.rendered,
            "\n==> File m.yaml\n==> At Hook(id='a')\n==> At key: files\n=====> 'a{4294967296}' is not a valid python regex"
        );
        let f = Failure::from(e);
        assert_eq!(f.exit_code(), 1);
        let Failure::Exc { exc, .. } = f else {
            panic!("exc")
        };
        assert_eq!(exc.ty, "InvalidManifestError");
        let e = load_manifest_text("- {id: a, name: a, entry: a, language: [x]}\n", PC)
            .result
            .expect_err("list");
        assert!(e.rendered.ends_with("but got: ['x']"));
        let e = load_manifest_text(
            "- {id: a, name: a, entry: a, language: system, minimum_pre_commit_version: x}\n",
            PC,
        )
        .result
        .expect_err("int");
        assert!(
            e.rendered
                .ends_with("=====> invalid literal for int() with base 10: 'x'")
        );
    }

    // --- X13-SM (13 X.5) ------------------------------------------------------------------------------------------

    #[test]
    fn x13_sm_03_future_warning_stderr_line() {
        let l = load_config_text(
            "files: '[[]'\nrepos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: a\n        entry: a\n        language: system\n        files: '[[]'\n        exclude: '[a&&b]'\n",
            PC,
            E,
        );
        assert!(l.result.is_ok());
        assert!(l.warnings.is_empty(), "stdout untouched: {:?}", l.warnings);
        assert_eq!(
            l.diag,
            [
                "[WARNING] regex '[[]': Possible nested set at position 1 (Python FutureWarning)",
                "[WARNING] regex '[a&&b]': Possible set intersection at position 2 (Python FutureWarning)",
            ]
        );
        let l = load_manifest_text(
            "- {id: a, name: a, entry: a, language: system, exclude: '[a||b]'}\n",
            PC,
        );
        assert_eq!(
            l.diag,
            ["[WARNING] regex '[a||b]': Possible set union at position 2 (Python FutureWarning)"]
        );
    }

    #[test]
    fn x13_sm_10_pythonwarnings_has_no_effect() {
        // snp-config never reads the environment: the line is the same whatever PYTHONWARNINGS says, and the
        // pattern keeps Python's literal-set semantics.
        let text = "files: '[[]'\nrepos: []\n";
        let a = load_config_bytes(text.as_bytes(), "cfg.yaml", PC, E);
        let b = load_config_bytes(text.as_bytes(), "cfg.yaml", Brand::Snp, ExtMode::Strict);
        assert_eq!(a.diag, b.diag);
        assert_eq!(a.diag.len(), 1);
        let rx = snp_pyre::PyRegex::compile("[[]", snp_pyre::Options::CONFIG).expect("compile");
        assert!(rx.search("a[b").expect("search"));
    }
}
