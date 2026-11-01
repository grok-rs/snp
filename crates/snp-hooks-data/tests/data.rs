//! Acceptance tests of the generated pre-commit-hooks data (WBS WP-17; 09 §5.1–§5.4, §6.2–§6.3, §7.1).
//!
//! Owner: WP-17.

#[cfg(test)]
mod data {
    use snp_hooks_data::{
        BUILTIN_MANIFEST_YAML, Epoch, GENERATED_FROM, Gate, HookClass, import_closure,
        is_pre_commit_hooks_url, module_for_entry, parity_gate, profile_for_rev, verified_blob,
    };

    /// 09 §6.2: the match table (plus the normalisation rules it states).
    #[test]
    fn url_identity_matches() {
        for url in [
            "https://github.com/pre-commit/pre-commit-hooks",
            "https://github.com/pre-commit/pre-commit-hooks.git",
            "https://github.com/pre-commit/pre-commit-hooks/",
            "https://github.com/pre-commit/pre-commit-hooks.git/",
            "git@github.com:pre-commit/pre-commit-hooks.git",
            "git@github.com:pre-commit/pre-commit-hooks",
            "git@github.com:/pre-commit/pre-commit-hooks.git",
            "github.com:pre-commit/pre-commit-hooks",
            "ssh://git@github.com/pre-commit/pre-commit-hooks",
            "ssh://git@github.com:22/pre-commit/pre-commit-hooks.git",
            "git+ssh://git@github.com/pre-commit/pre-commit-hooks",
            "git://github.com/pre-commit/pre-commit-hooks",
            "http://github.com/pre-commit/pre-commit-hooks",
            "https://GitHub.com/Pre-Commit/Pre-Commit-Hooks",
            "HTTPS://WWW.GITHUB.COM/PRE-COMMIT/PRE-COMMIT-HOOKS.GIT",
            "https://www.github.com/pre-commit/pre-commit-hooks",
            "https://github.com:443/pre-commit/pre-commit-hooks",
            "https://x-access-token:ghp_secret@github.com/pre-commit/pre-commit-hooks",
            "https://user@github.com/pre-commit/pre-commit-hooks.Git",
        ] {
            assert!(is_pre_commit_hooks_url(url), "{url}");
        }
    }

    /// 09 §6.2: non-matches go to Tier B (forks, mirrors, local paths, sub-pages, wrong ports, query/fragment).
    #[test]
    fn url_identity_non_matches() {
        for url in [
            "local",
            "meta",
            "builtin",
            "",
            "https://github.com/pre-commit/pre-commit-hooks/tree/main",
            "https://github.com/someone/pre-commit-hooks",
            "https://github.com/pre-commit/pre-commit-hooks-fork",
            "https://gitlab.com/pre-commit/pre-commit-hooks",
            "https://gitea.example.com/pre-commit/pre-commit-hooks",
            "https://github.com.evil.com/pre-commit/pre-commit-hooks",
            "https://evilgithub.com/pre-commit/pre-commit-hooks",
            "https://api.github.com/pre-commit/pre-commit-hooks",
            "file:///home/u/pre-commit-hooks",
            "/home/u/pre-commit/pre-commit-hooks",
            "../pre-commit-hooks",
            "pre-commit/pre-commit-hooks",
            "github.com/pre-commit/pre-commit-hooks",
            "https://github.com/pre-commit/pre-commit-hooks?ref=main",
            "https://github.com/pre-commit/pre-commit-hooks#main",
            "https://github.com:8443/pre-commit/pre-commit-hooks",
            "https://github.com:22/pre-commit/pre-commit-hooks",
            "ssh://git@github.com:443/pre-commit/pre-commit-hooks",
            "git://github.com:9418/pre-commit/pre-commit-hooks",
            "ftp://github.com/pre-commit/pre-commit-hooks",
            "https://github.com//pre-commit/pre-commit-hooks",
            "https://github.com/pre-commit/pre-commit-hooks//",
            "https://github.com/pre-commit/pre-commit-hooks.git.git",
            "https://github.com/pre-commit/pre-commit-hooks/.git",
            "https://github.com/pre-commit//pre-commit-hooks",
            "https://github.com/pre-commit",
            "https://github.com",
            "git@github.com:pre-commit/pre-commit-hooks/extra",
            "C:\\src\\pre-commit\\pre-commit-hooks",
            "https://github.com/pre%2Dcommit/pre-commit-hooks",
            "https://gıthub.com/pre-commit/pre-commit-hooks",
        ] {
            assert!(!is_pre_commit_hooks_url(url), "{url}");
        }
    }

    /// 09 §5.1: every tag and its full commit resolve to the same profile; nothing else does.
    #[test]
    fn tier_a_profiles() {
        let table = [
            ("v3.0.0", "66250ba9bf5ae48f660b7648d9feca8ed0b18e58"),
            ("v3.0.1", "0e2c0f75028c039052b637c17173833aaf5be97a"),
            ("v3.1.0", "ebc15addedad713c86ef18ae9632c88e187dd0af"),
            ("v3.2.0", "e1668fe86af3810fbca72b8653fe478e66a0afdc"),
            ("v3.3.0", "9136088a246768144165fcc3ecc3d31bb686920a"),
            ("v3.4.0", "6e2418c5521b7d606e72914dced3253f9ace1205"),
            ("v4.0.0", "9cc62e1a631dd72a054fb2dccda6fdebd99aacab"),
            ("v4.0.1", "38b88246ccc552bffaaf54259d064beeee434539"),
            ("v4.1.0", "8fe62d14e0b4d7d845a7022c5c2c3ae41bdd3f26"),
            ("v4.2.0", "db7346d375eda68a0174f2c057dd97f2fbffe030"),
            ("v4.3.0", "3298ddab3c13dd77d6ce1fc0baf97691430d84b0"),
            ("v4.4.0", "f71fa2c1f9cf5cb705f73dffe4b21f7c61470ba9"),
            ("v4.5.0", "c4a0b883114b00d8d76b479c820ce7950211c99b"),
            ("v4.6.0", "2c9f875913ee60ca25ce70243dc24d5b6415598c"),
            ("v5.0.0", "cef0300fd0fc4d2a87a85fa2093c6b283ea36f4b"),
            ("v6.0.0", "3e8a8703264a2f4a69428a0aa4dcb512790b2c8c"),
        ];
        for (tag, commit) in table {
            let by_tag = profile_for_rev(tag).unwrap();
            let by_commit = profile_for_rev(commit).unwrap();
            assert!(std::ptr::eq(by_tag, by_commit), "{tag}");
            assert_eq!((by_tag.tag, by_tag.commit), (tag, commit));
            assert!(by_tag.manifest_yaml.starts_with("-   id: "), "{tag}");
            assert_ne!(by_tag.hooks.len(), 0);
        }
        for rev in [
            "v6.0",
            "6.0.0",
            "V6.0.0",
            "v2.5.0",
            "v6.0.1",
            "main",
            "master",
            "3e8a870",
            "3E8A8703264A2F4A69428A0AA4DCB512790B2C8C",
            "3e8a8703264a2f4a69428a0aa4dcb512790b2c8",
            "3e8a8703264a2f4a69428a0aa4dcb512790b2c8c0",
            "3e8a8703264a2f4a69428a0aa4dcb512790b2c8d",
            "",
            " v6.0.0",
        ] {
            assert!(profile_for_rev(rev).is_none(), "{rev:?}");
        }
    }

    fn class_of(tag: &str, id: &str) -> (HookClass, Epoch) {
        let p = profile_for_rev(tag).unwrap();
        p.hooks
            .iter()
            .find(|h| h.0 == id)
            .map_or_else(|| panic!("{tag} {id}"), |&(_, c, e)| (c, e))
    }

    /// 09 §1 classes and 09 §5.2 epochs (as refined in the generated `Epoch` table) at the boundaries.
    #[test]
    fn tier_a_classes_and_epochs() {
        use HookClass::{Delegate, FailLanguage, Native, NativeWithCaveats, Stub};
        let v6 = profile_for_rev("v6.0.0").unwrap();
        assert_eq!(v6.hooks.len(), 34, "09 §0: 34 hook ids in v6.0.0");
        let count = |c: HookClass| v6.hooks.iter().filter(|h| h.1 == c).count();
        // 09 §0: 23 NATIVE (2 of them removed stubs), 4 caveats, 5 DELEGATE, 2 fail.
        assert_eq!((count(Native), count(Stub)), (21, 2));
        assert_eq!(
            (
                count(NativeWithCaveats),
                count(Delegate),
                count(FailLanguage)
            ),
            (4, 5, 2)
        );
        for (tag, id, want) in [
            ("v3.0.0", "check-json", (Native, Epoch(1))),
            ("v3.4.0", "check-json", (Native, Epoch(1))),
            ("v4.0.0", "check-json", (Native, Epoch(2))),
            ("v4.0.1", "check-merge-conflict", (Native, Epoch(1))),
            ("v4.1.0", "check-merge-conflict", (Native, Epoch(2))),
            ("v4.2.0", "check-merge-conflict", (Native, Epoch(3))),
            ("v6.0.0", "check-merge-conflict", (Native, Epoch(3))),
            ("v4.2.0", "check-toml", (Delegate, Epoch(0))),
            ("v4.3.0", "check-toml", (NativeWithCaveats, Epoch(1))),
            ("v3.4.0", "pretty-format-json", (Delegate, Epoch(0))),
            ("v4.6.0", "pretty-format-json", (Native, Epoch(1))),
            ("v5.0.0", "pretty-format-json", (Native, Epoch(2))),
            ("v3.0.1", "requirements-txt-fixer", (Delegate, Epoch(0))),
            ("v3.1.0", "requirements-txt-fixer", (Native, Epoch(1))),
            ("v4.6.0", "requirements-txt-fixer", (Native, Epoch(3))),
            ("v5.0.0", "requirements-txt-fixer", (Native, Epoch(4))),
            ("v4.0.1", "detect-private-key", (Native, Epoch(1))),
            ("v4.1.0", "detect-private-key", (Native, Epoch(2))),
            ("v3.4.0", "no-commit-to-branch", (Native, Epoch(1))),
            ("v4.0.0", "no-commit-to-branch", (Native, Epoch(2))),
            ("v3.2.0", "check-added-large-files", (Native, Epoch(1))),
            ("v3.3.0", "check-added-large-files", (Native, Epoch(2))),
            ("v4.1.0", "check-added-large-files", (Native, Epoch(3))),
            ("v5.0.0", "check-byte-order-marker", (Native, Epoch(1))),
            ("v6.0.0", "check-byte-order-marker", (Stub, Epoch(2))),
            ("v5.0.0", "fix-encoding-pragma", (Native, Epoch(2))),
            ("v6.0.0", "fix-encoding-pragma", (Stub, Epoch(2))),
            ("v3.0.0", "autopep8-wrapper", (Stub, Epoch(1))),
            ("v3.4.0", "flake8", (Stub, Epoch(2))),
            (
                "v4.0.0",
                "check-shebang-scripts-are-executable",
                (Delegate, Epoch(0)),
            ),
            (
                "v4.0.1",
                "check-shebang-scripts-are-executable",
                (Native, Epoch(1)),
            ),
            (
                "v6.0.0",
                "check-shebang-scripts-are-executable",
                (Native, Epoch(3)),
            ),
            (
                "v6.0.0",
                "check-illegal-windows-names",
                (FailLanguage, Epoch(1)),
            ),
            ("v4.4.0", "forbid-submodules", (FailLanguage, Epoch(1))),
            ("v6.0.0", "check-ast", (Delegate, Epoch(0))),
            ("v6.0.0", "debug-statements", (Delegate, Epoch(0))),
            ("v6.0.0", "check-yaml", (NativeWithCaveats, Epoch(2))),
            ("v6.0.0", "trailing-whitespace", (Native, Epoch(1))),
        ] {
            assert_eq!(class_of(tag, id), want, "{tag} {id}");
        }
        // Hooks that appear later are absent before (09 §5.3).
        assert!(
            profile_for_rev("v3.2.0")
                .unwrap()
                .hooks
                .iter()
                .all(|h| h.0 != "fix-byte-order-marker")
        );
        assert!(
            profile_for_rev("v4.3.0")
                .unwrap()
                .hooks
                .iter()
                .all(|h| h.0 != "forbid-submodules")
        );
        assert!(
            profile_for_rev("v4.6.0")
                .unwrap()
                .hooks
                .iter()
                .all(|h| h.0 != "check-illegal-windows-names")
        );
        assert!(
            profile_for_rev("v4.0.0")
                .unwrap()
                .hooks
                .iter()
                .all(|h| h.0 != "flake8")
        );
    }

    /// 09 §5.3: display names differ per tag, so Tier A embeds every manifest verbatim.
    #[test]
    fn tier_a_manifests_are_verbatim() {
        assert!(
            profile_for_rev("v4.0.1")
                .unwrap()
                .manifest_yaml
                .contains("name: Trim Trailing Whitespace\n")
        );
        assert!(
            profile_for_rev("v4.1.0")
                .unwrap()
                .manifest_yaml
                .contains("name: trim trailing whitespace\n")
        );
        assert!(
            profile_for_rev("v6.0.0")
                .unwrap()
                .manifest_yaml
                .contains("name: check-byte-order-marker (removed)\n")
        );
        assert!(
            profile_for_rev("v5.0.0")
                .unwrap()
                .manifest_yaml
                .contains("stages: [pre-commit, pre-push, manual]\n")
        );
        assert!(
            profile_for_rev("v4.6.0")
                .unwrap()
                .manifest_yaml
                .contains("stages: [commit, push, manual]\n")
        );
    }

    /// 09 §5.4: the v6.0.0 seed blobs map to their modules; DELEGATE epochs map to `Epoch(0)`; others are unknown.
    #[test]
    fn tier_b_blobs() {
        for (blob, module, epoch) in [
            ("d6c90ae02f7b07f10232fb6d00fb6d2bf9e8784f", "util", 1),
            (
                "e67416238d28bd20be4a0d0406a6ca87ded486da",
                "check_added_large_files",
                3,
            ),
            ("612111c531d9a1413cdc4d62def2a8fb915092c2", "check_json", 2),
            (
                "54a083ee5e45af91a43f23f642ccece4dda7c0d4",
                "check_merge_conflict",
                3,
            ),
            (
                "937425b0843e5f42a2d4a8dfc751470e1f6850c4",
                "check_shebang_scripts_are_executable",
                3,
            ),
            ("2105b072e99cca264ace2884dd593dc1c1ab078d", "check_toml", 1),
            ("c94ea7168082d4e6f4e85464d48fdaa84a2fbe30", "check_yaml", 2),
            (
                "4435d6a61ddc03b7f7c9b6c22150029df7b55197",
                "file_contents_sorter",
                5,
            ),
            ("fb2b6d98c6c0823b7b03b69dfa52ee23b8bb6088", "removed", 2),
            (
                "8ce8ec643a233bffb209e3eb2c384314adb23a9a",
                "requirements_txt_fixer",
                4,
            ),
            (
                "dab8b14a83b50a0d6e327d95aa84e1e4fcc9a72e",
                "trailing_whitespace_fixer",
                1,
            ),
            ("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", "__init__", 1),
        ] {
            assert_eq!(verified_blob(blob), Some((module, Epoch(epoch))), "{blob}");
        }
        // v3.0.0–v4.2.0 `check_toml.py` (the `toml` package) is a DELEGATE epoch.
        assert_eq!(
            verified_blob("88f70865cd4014552dedb2bcbcf7561f638be45f"),
            Some(("check_toml", Epoch(0)))
        );
        assert_eq!(
            verified_blob("9710b2d67ef58782993c4ab54a83d1e4bdd1e233"),
            Some(("removed", Epoch(1)))
        );
        for blob in [
            "0000000000000000000000000000000000000000",
            "D6C90AE02F7B07F10232FB6D00FB6D2BF9E8784F",
            "d6c90ae",
            "a98a1e574bb3b8d2255f7215a97d6b15e1210a2e", // the manifest blob is not a module
            "",
        ] {
            assert_eq!(verified_blob(blob), None, "{blob}");
        }
    }

    /// `setup.cfg` console scripts (09 §1 examples) and import closures (09 §5.4).
    #[test]
    fn console_scripts_and_closures() {
        for (entry, module) in [
            ("trailing-whitespace-fixer", "trailing_whitespace_fixer"),
            ("debug-statement-hook", "debug_statement_hook"),
            ("double-quote-string-fixer", "string_fixer"),
            ("name-tests-test", "tests_should_end_in_test"),
            ("pre-commit-hooks-removed", "removed"),
            ("check-byte-order-marker", "check_byte_order_marker"),
            (
                "check-shebang-scripts-are-executable",
                "check_shebang_scripts_are_executable",
            ),
        ] {
            assert_eq!(module_for_entry(entry), Some(module), "{entry}");
        }
        assert_eq!(module_for_entry("trailing-whitespace"), None);
        assert_eq!(module_for_entry(""), None);
        assert_eq!(
            import_closure("check_added_large_files"),
            ["__init__", "util"]
        );
        assert_eq!(import_closure("check_case_conflict"), ["__init__", "util"]);
        assert_eq!(
            import_closure("check_shebang_scripts_are_executable"),
            ["__init__", "check_executables_have_shebangs", "util"]
        );
        assert_eq!(import_closure("check_json"), ["__init__"]);
        assert_eq!(import_closure("util"), ["__init__"]);
        assert_eq!(import_closure("__init__").len(), 0);
        assert_eq!(import_closure("nope").len(), 0);
    }

    /// ADR-031 gates: Exact for NATIVE ids; absent ids delegate. The caveat ids (check-yaml, check-toml, check-xml,
    /// detect-aws-credentials) follow their corpus comparison, which snp-hooks' `tests/caveats`
    /// (`caveat_corpus_gates`) checks against this table.
    #[test]
    fn parity_gates() {
        for id in [
            "trailing-whitespace",
            "check-json",
            "pretty-format-json",
            "check-byte-order-marker",
            "flake8",
        ] {
            assert_eq!(parity_gate(id), Gate::Exact, "{id}");
        }
        for id in [
            "check-ast",
            "debug-statements",
            "check-illegal-windows-names",
            "forbid-submodules",
            "unknown",
        ] {
            assert_eq!(parity_gate(id), Gate::Delegate, "{id}");
        }
    }

    /// 09 §7.1: the builtin manifest is the v6.0.0 manifest restricted to the ids snp serves natively.
    #[test]
    fn builtin_manifest() {
        let ids: Vec<&str> = BUILTIN_MANIFEST_YAML
            .lines()
            .filter_map(|l| l.strip_prefix("-   id: "))
            .collect();
        assert_eq!(ids.len(), 27);
        for gone in [
            "check-ast",
            "check-builtin-literals",
            "check-docstring-first",
            "debug-statements",
            "double-quote-string-fixer",
            "check-byte-order-marker",
            "fix-encoding-pragma",
        ] {
            assert!(!ids.contains(&gone), "{gone}");
        }
        for kept in [
            "trailing-whitespace",
            "check-yaml",
            "check-illegal-windows-names",
            "forbid-submodules",
        ] {
            assert!(ids.contains(&kept), "{kept}");
        }
        // Every block is copied verbatim from v6.0.0.
        let v6 = profile_for_rev("v6.0.0").unwrap().manifest_yaml;
        let mut blocks = BUILTIN_MANIFEST_YAML
            .split("-   id: ")
            .filter(|b| !b.is_empty());
        assert!(blocks.all(|b| v6.contains(&format!("-   id: {b}"))));
        assert!(BUILTIN_MANIFEST_YAML.starts_with("-   id: check-added-large-files\n"));
    }

    #[test]
    fn provenance() {
        assert_eq!(
            GENERATED_FROM,
            "pre-commit-hooks v3.0.0..v6.0.0 @ 3e8a8703264a2f4a69428a0aa4dcb512790b2c8c"
        );
    }
}
