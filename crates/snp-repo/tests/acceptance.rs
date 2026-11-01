//! WP-31 acceptance (WBS "### WP-31"): fast-path decisions and transcripts, prefetch parity, deps-variant isolation,
//! the 04b §5.1 traversal reproduction and relative local repositories — against the real store and git, offline.
//!
//! Owner: WP-31.

#[cfg(test)]
mod support;

#[cfg(test)]
mod tests {
    use crate::support;
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::path::Path;
    use std::sync::Arc;

    use crate::support::{FakePreflight, H, exc, head, initializing, installing};
    use snp_config::{ExtMode, LoadOpts};
    use snp_core::lang::LanguageId;
    use snp_git::testing::{Response, ScriptedBackend};
    use snp_lang::harness::Reply;
    use snp_repo::{ExecKind, Resolve, ResolvedHook};
    use snp_testkit::Fixture;

    /// The upstream URL (never contacted: Tier A needs no clone; delegated clones are redirected with `insteadOf`).
    const PCH: &str = "https://github.com/pre-commit/pre-commit-hooks";

    fn pch_cfg(rev: &str, hooks: &str) -> String {
        format!("repos:\n-   repo: {PCH}\n    rev: {rev}\n    hooks:\n{hooks}")
    }

    /// A local stand-in for the upstream repository at `v6.0.0`, and the global `insteadOf` that redirects clones of
    /// the upstream URL to it (the owned-repo env keeps `HOME`, so git reads the test's `~/.gitconfig`).
    fn redirect_upstream(h: &H) -> std::path::PathBuf {
        redirect_upstream_at(h, "v6.0.0")
    }

    /// [`redirect_upstream`] with the manifest of `rev`, tagged `rev`.
    fn redirect_upstream_at(h: &H, rev: &str) -> std::path::PathBuf {
        let profile = snp_hooks_data::profile_for_rev(rev).unwrap();
        let dir =
            h.t.git_repo("pch-mirror")
                .file(".pre-commit-hooks.yaml", profile.manifest_yaml.as_bytes())
                .stage_all()
                .commit(rev)
                .tag(rev)
                .build();
        redirect_to(h, &format!("file://{}", dir.display()));
        dir
    }

    /// A global `insteadOf` sending clones of the upstream URL to `url`.
    fn redirect_to(h: &H, url: &str) {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(h.t.root().join("home/.gitconfig"))
            .unwrap();
        write!(f, "[url \"{url}\"]\n\tinsteadOf = {PCH}\n").unwrap();
    }

    /// The deprecated-stage warning of the upstream repository at a pre-v5 tag.
    fn old_stages_warning() -> String {
        format!(
            "[WARNING] repo `{PCH}` uses deprecated stage names (commit, push) which will be removed in a future \
             version.  Hint: often `pre-commit autoupdate --repo {PCH}` will fix this.  if it does not -- consider \
             reporting an issue to that repo.\n"
        )
    }

    fn repos_v1_entries(h: &H) -> usize {
        std::fs::read_dir(h.store.location().path.join("repos-v1")).map_or(0, |d| {
            d.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count()
        })
    }

    #[test]
    fn tier_a_cold_and_warm_transcripts_without_a_clone() {
        let h = H::new();
        let cfg = h.config(&pch_cfg(
            "v6.0.0",
            "    -   id: trailing-whitespace\n    -   id: end-of-file-fixer\n    -   id: check-json\n",
        ));
        let hooks = h.resolve_and_install(&cfg).unwrap();
        // pre-commit 4.6.2: one Initializing line (the clone) and one triplet (the shared python env).
        let out = h.out.take();
        assert_eq!(out, format!("{}{}", initializing(PCH), installing(PCH)));
        // The resolution/install part of the oracle's cold transcript (the hook lines are the run's, WP-32).
        let golden: String = include_str!("goldens/pc462-py312-pch-v6-cold.txt")
            .split_inclusive('\n')
            .filter(|l| l.starts_with("[INFO] "))
            .collect();
        assert_eq!(out, golden, "pc462-py312 cold transcript");
        assert!(
            include_str!("goldens/pc462-py312-pch-v6-warm.txt")
                .lines()
                .all(|l| !l.starts_with('[')),
            "the oracle's warm run prints no resolution line"
        );
        assert_eq!(repos_v1_entries(&h), 0, "Tier A never clones");
        // Nothing is installed: the only commands are the interpreter probes of `get_default_version` (pre-commit
        // resolves `language_version` for every hook, and the virtual install key carries it).
        let calls = h.runner.calls();
        assert!(
            calls
                .iter()
                .all(|c| c.argv.get(1).is_some_and(|a| a == "-S")),
            "nothing is installed: {calls:?}"
        );
        for hk in &hooks {
            let ExecKind::Native { id, .. } = &hk.exec else {
                panic!("{:?}", hk.exec)
            };
            assert_eq!(**id, *hk.def.id);
            assert!(hk.install_key.is_none());
            assert_eq!(
                hk.virtual_install.as_ref().unwrap().lang,
                LanguageId::Python
            );
        }
        assert_eq!(&*hooks[0].def.name, "trim trailing whitespace");
        // Warm: byte-identical to pre-commit's warm run (nothing).
        h.resolve_and_install(&cfg).unwrap();
        assert_eq!(h.out.take(), "");
    }

    #[test]
    fn tier_a_old_tag_warns_deprecated_stages_once() {
        let h = H::new();
        let cfg = h.config(&pch_cfg("v4.4.0", "    -   id: trailing-whitespace\n"));
        h.resolve(&cfg).unwrap();
        assert_eq!(
            h.out.take(),
            format!("{}{}", initializing(PCH), old_stages_warning())
        );
        h.resolve(&cfg).unwrap();
        assert_eq!(h.out.take(), "");
    }

    /// A delegated Tier A hook: pre-commit's order is Initializing → clone → warning → db row; a failing clone
    /// prints no warning and records nothing, so the next run starts over with its Initializing line.
    #[test]
    fn tier_a_delegated_clone_failure_records_nothing() {
        let h = H::new();
        redirect_to(&h, "file:///nonexistent/pch");
        let cfg = h.config(&pch_cfg(
            "v4.4.0",
            "    -   id: check-yaml\n        entry: check-yaml --unsafe\n",
        ));
        for _ in 0..2 {
            let (ty, _) = exc(&h.resolve(&cfg).unwrap_err());
            assert_eq!(ty, "CalledProcessError");
            assert_eq!(
                h.out.take(),
                initializing(PCH),
                "no warning, nothing recorded"
            );
        }
        assert_eq!(repos_v1_entries(&h), 0);
    }

    #[test]
    fn tier_a_delegated_clone_prints_initializing_then_the_warning() {
        let h = H::new();
        redirect_upstream_at(&h, "v4.4.0");
        let cfg = h.config(&pch_cfg(
            "v4.4.0",
            "    -   id: trailing-whitespace\n    -   id: check-yaml\n        entry: check-yaml --unsafe\n",
        ));
        let hooks = h.resolve(&cfg).unwrap();
        assert_eq!(
            h.out.take(),
            format!("{}{}", initializing(PCH), old_stages_warning())
        );
        assert!(matches!(hooks[0].exec, ExecKind::Native { .. }));
        assert!(matches!(hooks[1].exec, ExecKind::Language));
        assert_eq!(repos_v1_entries(&h), 1);
        h.resolve(&cfg).unwrap();
        assert_eq!(h.out.take(), "");
    }

    /// ARCHITECTURE §gc: a hook moving from native to delegated keeps pre-commit's transcript — its one environment
    /// was installed on the first run, so the second prints nothing. (The harness has no python, so the real install
    /// then fails; the point is that it starts without a second triplet. The reverse direction and the scheduler
    /// rules are unit-tested in `install.rs`.)
    #[test]
    fn native_then_delegated_prints_no_second_triplet() {
        let h = H::new();
        redirect_upstream(&h);
        let native = h.config(&pch_cfg("v6.0.0", "    -   id: check-json\n"));
        h.resolve_and_install(&native).unwrap();
        assert_eq!(
            h.out.take(),
            format!("{}{}", initializing(PCH), installing(PCH))
        );
        let delegated = h.config(&pch_cfg(
            "v6.0.0",
            "    -   id: check-json\n        entry: check-json\n",
        ));
        let (ty, text) = exc(&h.resolve_and_install(&delegated).unwrap_err());
        assert_eq!(
            (ty.as_str(), text.as_str()),
            (
                "ValueError",
                "failed to find interpreter for python_spec='default'"
            )
        );
        assert_eq!(
            h.out.take(),
            "",
            "pre-commit prints nothing on the second run"
        );
        h.resolve_and_install(&native).unwrap();
        assert_eq!(h.out.take(), "");
    }

    /// `_new_repo` checks again under the lock: a run that waited for another process's clone prints only the
    /// `Locking` line (once per process), never a second Initializing line — for a clone and for a Tier A
    /// repository that is never cloned.
    #[test]
    fn a_run_that_waited_for_another_processs_entry_prints_no_initializing_line() {
        use snp_store::{MarkerKey, RepoId, Store};
        let h = H::new();
        let (dir, sha) = h.hook_repo(
            "waited-hooks",
            &[(
                ".pre-commit-hooks.yaml",
                &support::manifest_one("w", "w", "system", ""),
                false,
            )],
        );
        let url = dir.to_string_lossy().into_owned();
        let tier_a = h.config(&pch_cfg("v6.0.0", "    -   id: trailing-whitespace\n"));
        h.store.ensure_created(h.brand, &h.printer).unwrap();
        for (repo, rev, cfg) in [
            (
                url.clone(),
                sha.clone(),
                h.config(&format!(
                    "repos:\n-   repo: {url}\n    rev: {sha}\n    hooks:\n    -   id: w\n"
                )),
            ),
            (PCH.to_owned(), "v6.0.0".to_owned(), tier_a),
        ] {
            let id = RepoId::from_config(&repo, &h.project);
            let key = Store::remote_key(&id, &rev, &[]);
            // the marker another process writes is keyed like its clone (ADR-082 item 4)
            let marker = snp_store::marker_repo_name(&id, &repo, &[]);
            let held = h
                .store
                .repo_lock(&key, &mut || {}, &snp_core::cancel::CancelToken::new())
                .unwrap();
            let loc = h.store.location().clone();
            let other = std::thread::Builder::new()
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    // The other process's `_new_repo` inserted its row before releasing the lock.
                    Store::new(loc).set_marker(&MarkerKey::Initialized {
                        db_repo_name: Arc::from(marker.as_str()),
                        rev: Arc::from(rev.as_str()),
                    });
                    drop(held);
                })
                .unwrap();
            h.resolve(&cfg).unwrap();
            other.join().unwrap();
            let out = h.out.take();
            assert!(
                out.is_empty() || out == "[INFO] Locking pre-commit directory\n",
                "{out:?}"
            );
        }
    }

    #[test]
    fn tier_a_frozen_commit_and_unknown_tag() {
        let h = H::new();
        let commit = snp_hooks_data::profile_for_rev("v6.0.0").unwrap().commit;
        let cfg = h.config(&pch_cfg(commit, "    -   id: check-json\n"));
        let hooks = h.resolve(&cfg).unwrap();
        assert!(matches!(hooks[0].exec, ExecKind::Native { .. }));
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        assert_eq!(&*rows[0].1, "native");
        assert!(
            rows[0].2.contains("pre-commit-hooks v6.0.0, tier A"),
            "{rows:?}"
        );
        // An unknown tag is not Tier A: it would be cloned (the table says so without cloning).
        let cfg = h.config(&pch_cfg("v99.0.0", "    -   id: check-json\n"));
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        assert_eq!(&*rows[0].1, "undecided");
        assert_eq!(repos_v1_entries(&h), 0);
    }

    #[test]
    fn tier_a_hook_whose_preflight_fails_is_cloned_in_all_hooks_and_installed_later() {
        let mut h = H::new();
        h.preflight = FakePreflight {
            refuse: vec!["check-json"],
        };
        let mirror = redirect_upstream(&h);
        let cfg = h.config(&pch_cfg(
            "v6.0.0",
            "    -   id: trailing-whitespace\n    -   id: check-json\n",
        ));
        let hooks = h.resolve(&cfg).unwrap();
        // The Initializing line came from the marker; the clone prints nothing extra.
        assert_eq!(h.out.take(), initializing(PCH));
        assert_eq!(repos_v1_entries(&h), 1);
        assert!(matches!(hooks[0].exec, ExecKind::Native { .. }));
        assert!(matches!(hooks[1].exec, ExecKind::Language));
        let key = hooks[1].install_key.clone().unwrap();
        assert_eq!(key.lang, LanguageId::Python);
        assert_eq!(
            hooks[0].virtual_install.as_ref(),
            Some(&key),
            "same env as pre-commit's"
        );
        assert_eq!(head(&h.env, &hooks[1].prefix), head(&h.env, &mirror));
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        assert_eq!(&*rows[1].1, "delegate");
        assert_eq!(
            rows[1].2,
            "unsupported argument: check-json: scripted refusal"
        );
    }

    #[test]
    fn g4_overrides_from_the_config_and_from_an_overlay_patch() {
        let h = H::new();
        let cfg = h.config(&pch_cfg(
            "v6.0.0",
            "    -   id: trailing-whitespace\n        args: [--markdown-linebreak-ext=md]\n    -   id: check-json\n        \
             language: python\n    -   id: end-of-file-fixer\n        additional_dependencies: []\n    -   id: \
             check-yaml\n        entry: check-yaml --unsafe\n",
        ));
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        let decisions: Vec<(&str, &str)> = rows.iter().map(|r| (&*r.1, r.2.as_str())).collect();
        assert_eq!(decisions[0].0, "native");
        assert_eq!(decisions[1], ("delegate", "user override (`language`)"));
        assert_eq!(
            decisions[2].0, "native",
            "an empty additional_dependencies is no override"
        );
        assert_eq!(decisions[3], ("delegate", "user override (`entry`)"));

        let ov_path = h.project.join(".pre-commit-config.local.yaml");
        std::fs::write(
            &ov_path,
            "hooks:\n    trailing-whitespace:\n        entry: '{entry} --x'\n    unknown-hook:\n        args: [a]\n",
        )
        .unwrap();
        let overlay = snp_config::overlay::load(
            &ov_path,
            &LoadOpts {
                brand: h.brand,
                ext: ExtMode::Enabled,
                regexes: &h.regexes,
                warn: &h.printer,
                diag: &h.printer,
            },
        )
        .unwrap();
        let cfg = h.config(&pch_cfg("v6.0.0", "    -   id: trailing-whitespace\n"));
        let rows = h.with_ctx(&cfg, Some(&overlay), |cx| {
            snp_repo::decision_table(&cfg, cx)
        });
        assert_eq!(
            (&*rows[0].1, rows[0].2.as_str()),
            ("delegate", "user override (`entry`)")
        );
        // Through all_hooks the patched entry is the merged one; the unmatched overlay key is reported on stderr.
        let mirror = redirect_upstream(&h);
        let hooks = h
            .with_ctx(&cfg, Some(&overlay), |cx| h.resolver.all_hooks(cx))
            .unwrap();
        assert_eq!(&*hooks[0].def.entry, "trailing-whitespace-fixer --x");
        assert!(matches!(hooks[0].exec, ExecKind::Language));
        assert_eq!(head(&h.env, &hooks[0].prefix), head(&h.env, &mirror));
        assert_eq!(
            h.err.take(),
            "snp: local config overrides unknown hook 'unknown-hook'\n"
        );
    }

    #[test]
    fn g1_disabled_by_the_policy_and_by_exclude() {
        let mut h = H::new();
        h.fast_path.exclude = vec![Arc::from("check-json")];
        let cfg = h.config(&pch_cfg(
            "v6.0.0",
            "    -   id: trailing-whitespace\n    -   id: check-json\n",
        ));
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        assert_eq!(&*rows[0].1, "native");
        assert_eq!(
            (&*rows[1].1, rows[1].2.as_str()),
            ("delegate", "disabled by user")
        );
        let cfg = h.config(&format!(
            "fast_path: 'off'\n{}",
            pch_cfg("v6.0.0", "    -   id: trailing-whitespace\n")
        ));
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        assert_eq!(
            &*rows[0].1, "undecided",
            "off: the repository is cloned like any other"
        );
    }

    /// A fork with the upstream `check_json.py` (v6.0.0 blob) and its manifest entry.
    fn fork(h: &H, name: &str, module: &[u8]) -> (String, String) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pch-fork");
        let manifest = b"-   id: check-json\n    name: check json\n    entry: check-json\n    language: python\n    \
    types: [json]\n";
        let setup_cfg =
            b"[metadata]\nname = pre_commit_hooks\n\n[options.entry_points]\nconsole_scripts =\n    \
    check-json = pre_commit_hooks.check_json:main\n";
        let _license = std::fs::read(fixture.join("LICENSE.pre-commit-hooks")).unwrap();
        let (dir, sha) = h.hook_repo(
            name,
            &[
                (".pre-commit-hooks.yaml", manifest, false),
                ("setup.cfg", setup_cfg, false),
                ("pre_commit_hooks/__init__.py", b"", false),
                ("pre_commit_hooks/check_json.py", module, false),
            ],
        );
        (dir.to_string_lossy().into_owned(), sha)
    }

    fn upstream_check_json() -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pch-fork/pre_commit_hooks/check_json.py"),
        )
        .unwrap()
    }

    #[test]
    fn tier_b_identical_fork_is_native_and_a_modified_one_delegates() {
        let h = H::new();
        let (same, same_rev) = fork(&h, "fork-same", &upstream_check_json());
        let mut modified = upstream_check_json();
        modified.extend_from_slice(b"# local change\n");
        let (changed, changed_rev) = fork(&h, "fork-changed", &modified);
        let yaml = format!(
            "repos:\n-   repo: {same}\n    rev: {same_rev}\n    hooks:\n    -   id: check-json\n-   repo: {changed}\n    \
             rev: {changed_rev}\n    hooks:\n    -   id: check-json\n"
        );
        let cfg = h.config(&yaml);
        let hooks = h.resolve(&cfg).unwrap();
        assert_eq!(
            h.out.take(),
            format!("{}{}", initializing(&same), initializing(&changed))
        );
        let ExecKind::Native { id, epoch, .. } = &hooks[0].exec else {
            panic!("{:?}", hooks[0].exec)
        };
        assert_eq!(&**id, "check-json");
        assert!(epoch.0 >= 1);
        assert!(hooks[0].install_key.is_none());
        assert!(hooks[0].virtual_install.is_some());
        assert!(matches!(hooks[1].exec, ExecKind::Language));
        assert_eq!(
            hooks[1].install_key.as_ref().unwrap().lang,
            LanguageId::Python
        );
        let rows = h.with_ctx(&cfg, None, |cx| snp_repo::decision_table(&cfg, cx));
        assert!(rows[0].2.contains("tier B"), "{rows:?}");
        assert_eq!(
            rows[1].2,
            "unknown implementation (implementation differs from verified upstream blobs (`check_json`))"
        );
    }

    /// Two remote repositories, the second missing, with a recording backend; returns the error text and the calls.
    fn missing_second(
        h_env: impl FnOnce(snp_core::env::Env) -> snp_core::env::Env,
    ) -> (String, String, Vec<String>) {
        let backend = Arc::new(ScriptedBackend::new());
        let dyn_backend: Arc<dyn snp_git::GitBackend> = Arc::<ScriptedBackend>::clone(&backend);
        let h = H::with(h_env).backend(dyn_backend);
        let good = h.t.fixture_repo(Fixture::Resource("script_hooks_repo"));
        let missing =
            h.t.root()
                .join("private/repo")
                .to_string_lossy()
                .into_owned();
        let yaml = format!(
            "repos:\n-   repo: {}\n    rev: {}\n    hooks:\n    -   id: bash_hook\n-   repo: {missing}\n    rev: v1\n    \
             hooks:\n    -   id: x\n",
            good.display(),
            head(&h.env, &good)
        );
        let cfg = h.config(&yaml);
        let err = h.resolve(&cfg).unwrap_err();
        let (_, text) = exc(&err);
        let calls = backend
            .calls()
            .iter()
            .map(|c| {
                c.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        (h.out.take(), text.replace(&missing, "<missing>"), calls)
    }

    #[test]
    fn x13_git_01_prefetch_failures_are_redone_in_the_foreground_with_pre_commit_text() {
        let (out_off, text_off, _) = missing_second(|e| {
            e.patched(&snp_core::env::EnvPatch::default().set("SNP_PREFETCH", "0"))
        });
        for (k, v) in [("LANG", "de_DE.UTF-8"), ("CI", "1")] {
            let (out, text, calls) =
                missing_second(|e| e.patched(&snp_core::env::EnvPatch::default().set(k, v)));
            assert_eq!(text, text_off, "{k}={v}");
            assert_eq!(out.lines().count(), 2, "{out}");
            assert!(out.ends_with("private/repo.\n"), "{out}");
            assert_eq!(out_off.lines().count(), 2);
            // The background attempt and the foreground redo both ran the fallback fetch.
            let fallbacks = calls
                .iter()
                .filter(|c| c.ends_with("fetch origin --tags"))
                .count();
            assert_eq!(fallbacks, 2, "{calls:#?}");
        }
        assert!(
            text_off.contains("'fetch', 'origin', '--tags')"),
            "{text_off}"
        );
    }

    #[test]
    fn ssh_urls_with_git_ssh_command_are_never_prefetched() {
        let backend = Arc::new(ScriptedBackend::new());
        backend.on(
            "*fetch origin*",
            Response::Exit {
                code: 128,
                stdout: Vec::new(),
                stderr: b"fatal: Could not read from remote repository.\n".to_vec(),
            },
        );
        let dyn_backend: Arc<dyn snp_git::GitBackend> = Arc::<ScriptedBackend>::clone(&backend);
        let h = H::with(|e| {
            e.patched(&snp_core::env::EnvPatch::default().set("GIT_SSH_COMMAND", "false"))
        })
        .backend(dyn_backend);
        let cfg = h.config(
            "repos:\n-   repo: git@example.invalid:o/a\n    rev: v1\n    hooks:\n    -   id: x\n-   repo: \
             ssh://git@example.invalid/o/b\n    rev: v1\n    hooks:\n    -   id: y\n",
        );
        assert!(h.resolve(&cfg).is_err());
        let inits = backend
            .calls()
            .iter()
            .filter(|c| c.iter().any(|a| a == "init"))
            .count();
        assert_eq!(
            inits, 1,
            "only the foreground clone of the first repository"
        );
        assert_eq!(h.out.take(), initializing("git@example.invalid:o/a"));
    }

    #[test]
    fn parallel_prefetch_keeps_initializing_lines_in_config_order() {
        let h = H::new();
        let mut repos = Vec::new();
        for i in 0..4 {
            let (dir, rev) = h.hook_repo(
                &format!("par-{i}"),
                &[(
                    ".pre-commit-hooks.yaml",
                    &support::manifest_one("h", "h", "system", ""),
                    false,
                )],
            );
            repos.push((dir.to_string_lossy().into_owned(), rev));
        }
        let mut yaml = String::from("repos:\n");
        for (r, rev) in &repos {
            writeln!(
                yaml,
                "-   repo: {r}\n    rev: {rev}\n    hooks:\n    -   id: h"
            )
            .unwrap();
        }
        let cfg = h.config(&yaml);
        let hooks = h.resolve(&cfg).unwrap();
        let want: String = repos.iter().map(|(r, _)| initializing(r)).collect();
        assert_eq!(h.out.take(), want);
        for (hk, (_, rev)) in hooks.iter().zip(&repos) {
            assert_eq!(&head(&h.env, &hk.prefix), rev);
        }
        // No prefetch directory is left behind.
        assert_eq!(
            std::fs::read_dir(h.store.scratch_dir()).map_or(0, Iterator::count),
            0
        );
    }

    #[test]
    fn deps_variants_are_isolated() {
        let h = H::new();
        // `cpan -T . <deps>` leaves build residue in the prefix it runs in.
        h.runner.reply(
            "cpan",
            Reply::ok("").sticky().with_effect(|r| {
                std::fs::write(r.cwd.join("Makefile-residue"), r.argv.len().to_string())
            }),
        );
        let (repo, rev) = h.hook_repo(
            "perl-variants",
            &[(
                ".pre-commit-hooks.yaml",
                &support::manifest_one("p", "p", "perl", ""),
                false,
            )],
        );
        let repo = repo.to_string_lossy().into_owned();
        let cfg = h.config(&format!(
            "repos:\n-   repo: {repo}\n    rev: {rev}\n    hooks:\n    -   id: p\n        additional_dependencies: [A]\n    \
             -   id: p\n        additional_dependencies: [B, C]\n    -   id: p\n"
        ));
        let hooks = h.resolve_and_install(&cfg).unwrap();
        let prefixes: Vec<&Path> = hooks.iter().map(|hk| hk.prefix.as_path()).collect();
        assert_ne!(prefixes[0], prefixes[1]);
        assert_ne!(prefixes[0], prefixes[2]);
        assert_eq!(
            std::fs::read_to_string(prefixes[0].join("Makefile-residue")).unwrap(),
            "4"
        );
        assert_eq!(
            std::fs::read_to_string(prefixes[1].join("Makefile-residue")).unwrap(),
            "5"
        );
        assert_eq!(
            std::fs::read_to_string(prefixes[2].join("Makefile-residue")).unwrap(),
            "3"
        );
        // The base clone is untouched by the variants' installs except for its own.
        assert_eq!(
            h.out.take(),
            format!(
                "{}{}{}{}{}{}",
                initializing(&repo),
                initializing(&format!("{repo}:A")),
                initializing(&format!("{repo}:B,C")),
                installing(&repo),
                installing(&repo),
                installing(&repo),
            )
        );
    }

    #[test]
    fn traversal_reproduction_04b_5_1_deletes_nothing() {
        let h = H::new();
        let store_parent = h.store.location().path.parent().unwrap().to_path_buf();
        let victim = store_parent.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("keep"), b"precious").unwrap();
        for version in ["/../../../victim", "../../../../victim", "../../victim"] {
            let cfg = h.config(&format!(
                "repos:\n-   repo: local\n    hooks:\n    -   id: t\n        name: t\n        entry: t\n        language: \
                 perl\n        language_version: '{version}'\n"
            ));
            let err = h.resolve_and_install(&cfg).unwrap_err();
            let (ty, text) = exc(&err);
            assert_eq!(ty, "AssertionError");
            assert!(
                text.contains(&format!("you selected `language_version: {version}`")),
                "{text}"
            );
            assert_eq!(std::fs::read(victim.join("keep")).unwrap(), b"precious");
        }
        // A language that accepts any version (python) gets a hashed, contained env dir.
        let cfg = h.config(
            "repos:\n-   repo: local\n    hooks:\n    -   id: t\n        name: t\n        entry: t\n        language: \
             python\n        language_version: '../../../../victim'\n",
        );
        let hooks = h.resolve(&cfg).unwrap();
        let repo = hooks[0].repo.as_ref().unwrap();
        let env_dir = h
            .store
            .env_dir(repo, LanguageId::Python, &hooks[0].language_version);
        assert_eq!(env_dir.parent().unwrap(), repo.envs);
        assert!(
            env_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("py_env-h")
        );
    }

    #[test]
    fn a_relative_local_repo_resolves_against_the_project_root() {
        let h = H::new();
        let (dir, rev) = h.hook_repo(
            "rel-hooks",
            &[(
                ".pre-commit-hooks.yaml",
                &support::manifest_one("r", "r", "system", ""),
                false,
            )],
        );
        assert_eq!(dir.parent(), h.project.parent());
        let cfg = h.config(&format!(
            "repos:\n-   repo: ../rel-hooks\n    rev: {rev}\n    hooks:\n    -   id: r\n"
        ));
        let hooks: Vec<ResolvedHook> = h.resolve(&cfg).unwrap();
        assert_eq!(h.out.take(), initializing("../rel-hooks"));
        assert_eq!(head(&h.env, &hooks[0].prefix), rev);
        // The key is the canonical path: an absolute spelling of the same directory reuses the clone.
        let cfg = h.config(&format!(
            "repos:\n-   repo: {}\n    rev: {rev}\n    hooks:\n    -   id: r\n",
            dir.display()
        ));
        let again = h.resolve(&cfg).unwrap();
        assert_eq!(again[0].prefix, hooks[0].prefix);
    }
}
